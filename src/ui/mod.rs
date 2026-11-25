use std::{path::PathBuf, time::SystemTime};

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Clear, Padding, Paragraph, Wrap},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    app::{
        App, COMMANDS, ChangeReport, ConnectStage, ConnectState, DEFAULT_CONTEXT_WINDOW, Input,
        ListPicker, ListRow, ModelRow, NoticeKind, SETTING_ROWS, Suggestion, ToolApproval,
        ToolsMode, format_bytes, format_tokens, list_row_of,
    },
    diff::{ChangeKind, DiffLine, FileDiff},
    session::{Message, Role, Usage},
    tools,
};

mod agents;
mod approval;
mod changelog;
mod composer;
mod connect;
mod help;
mod markdown;
mod pickers;
mod settings;
mod transcript;

use self::{
    agents::*, approval::*, changelog::*, composer::*, connect::*, help::*, pickers::*,
    settings::*, transcript::*,
};

/// How a `read_file` result surfaces in the transcript: quiet. The header and
/// range lines render instead of the contents; a click still expands it.
fn read_result_summary(result: &str) -> Option<(String, Vec<String>)> {
    let first = result.lines().next()?;
    let open = first.rfind(" (")?;
    let path = first[..open].trim();
    if path.is_empty() {
        return None;
    }
    let count = first.get(open + 2..)?.strip_suffix(" lines)")?;
    count.trim().parse::<usize>().ok()?;
    let mut summary = vec![first.trim().to_owned()];
    if let Some(range) = result
        .lines()
        .nth(1)
        .filter(|line| line.starts_with("lines "))
    {
        summary.push(range.trim().to_owned());
    }
    Some((path.to_owned(), summary))
}

/// Tools whose call and result stay out of the transcript: their effect is
/// already visible in the Progress panel.
fn is_hidden_tool(name: &str) -> bool {
    matches!(name, "todo_write")
}

/// Colors come from the active theme; see `crate::theme`.
pub(crate) fn muted() -> Color {
    crate::theme::current().muted
}
pub(crate) fn accent() -> Color {
    crate::theme::current().accent
}
pub(crate) fn error() -> Color {
    crate::theme::current().error
}
pub(crate) fn added() -> Color {
    crate::theme::current().added
}
pub(crate) fn removed() -> Color {
    crate::theme::current().removed
}
pub(crate) fn warning() -> Color {
    crate::theme::current().warning
}
pub(crate) fn text() -> Color {
    crate::theme::current().text
}
pub(crate) fn code() -> Color {
    crate::theme::current().code
}
pub(crate) fn dim() -> Color {
    crate::theme::current().dim
}
pub(crate) fn accent2() -> Color {
    crate::theme::current().accent2
}

/// A quiet ambient backdrop: a vertical wash in the active theme plus a
/// sparse dot grid. Widgets draw over it, so empty cells stop looking like a
/// flat void without competing with the transcript.
///
/// `phase` is the app's 100ms tick: it slides the rain and snow of `auto`.
fn draw_backdrop(frame: &mut Frame, area: Rect, phase: usize) {
    let theme = crate::theme::current();
    let light = match theme.dim {
        Color::Rgb(r, g, b) => u32::from(r) + u32::from(g) + u32::from(b) > 420,
        _ => false,
    };
    let base = if light {
        Color::Rgb(247, 248, 251)
    } else {
        Color::Rgb(9, 11, 19)
    };
    // `auto` paints the sky for the local hour: a top-to-horizon gradient
    // (night, dawn, noon, sunset...) with stars that fade in after dusk, and,
    // once `/set weather` saved a place, the weather in front of it: a sun
    // glow on a clear day, falling rain or snow, gray cloud and fog.
    let sky = crate::theme::sky();
    let weather = sky
        .as_ref()
        .map_or(crate::weather::Weather::Unknown, |sky| sky.weather);
    let stars = sky.as_ref().map_or(0, |sky| sky.stars);
    let wash = blend(theme.accent2, base, 90);
    let haze = blend(theme.accent, base, 94);
    let dot = blend(theme.accent2, base, 78);
    let rows = usize::from(area.height);
    let columns = usize::from(area.width);
    // The sun's disc in cell units: center column and row, tint, strength,
    // and glow radius. A cell is about twice as tall as wide, so row
    // distances count double.
    let sun = sky.as_ref().and_then(|sky| sky.sun.as_ref()).map(|sun| {
        let radius = (columns as f32 * 0.16).clamp(6.0, 22.0);
        (
            sun.x * columns as f32,
            sun.y * rows as f32,
            sun.color,
            sun.strength as f32,
            radius,
        )
    });
    let mut lines = Vec::with_capacity(rows);
    for y in 0..rows {
        let position = y * 100 / rows.max(1);
        let color = match &sky {
            Some(sky) => blend(
                theme.accent2,
                blend(
                    sky.top,
                    sky.horizon,
                    y * 100 / rows.saturating_sub(1).max(1),
                ),
                95,
            ),
            None if position < 40 => wash,
            None if position < 75 => haze,
            None => base,
        };
        let star_color = blend(Color::Rgb(205, 214, 255), color, 55);
        let drop_color = blend(
            if weather == crate::weather::Weather::Snow {
                Color::Rgb(220, 228, 244)
            } else {
                Color::Rgb(140, 170, 214)
            },
            color,
            45,
        );
        // The row's color under the sun: unchanged away from it, tinted
        // toward the sun by a falling-off glow, with a bright core.
        let ground = |x: usize| -> Color {
            let Some((cx, cy, tint, strength, radius)) = sun else {
                return color;
            };
            let (dx, dy) = (x as f32 - cx, (y as f32 - cy) * 2.0);
            let distance = (dx * dx + dy * dy).sqrt();
            if distance >= radius {
                color
            } else if distance < 1.5 {
                blend(color, Color::Rgb(255, 226, 150), (strength * 1.4) as usize)
            } else {
                let falloff = 1.0 - distance / radius;
                blend(color, tint, (strength * falloff * falloff) as usize)
            }
        };
        // What one cell shows: a drop, a star, a grid dot, or nothing.
        let cell = |x: usize| -> (&'static str, Style) {
            let bg = ground(x);
            if let Some(symbol) = crate::weather::drop_at(weather, x, y, phase) {
                return (symbol, Style::default().fg(drop_color).bg(bg));
            }
            if sky.is_some() {
                let mut h =
                    (x as u32).wrapping_mul(0x9E37_79B1) ^ (y as u32).wrapping_mul(0x85EB_CA6B);
                h ^= h >> 15;
                h = h.wrapping_mul(0x2C1B_3C6D);
                h ^= h >> 13;
                if position < 70 && (h % 1000) as usize * 4 < stars {
                    let symbol = if h % 7 == 0 { "*" } else { "·" };
                    return (symbol, Style::default().fg(star_color).bg(bg));
                }
            }
            if y % 3 == 0 && x % 8 == 4 {
                ("·", Style::default().fg(dot).bg(bg))
            } else {
                (" ", Style::default().bg(bg))
            }
        };
        // Neighbours with one style share a span, so a quiet row stays a
        // handful of spans instead of one per cell.
        let mut spans = Vec::new();
        let mut run = String::new();
        let mut run_style = Style::default();
        for x in 0..columns {
            let (symbol, style) = cell(x);
            if style != run_style && !run.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut run), run_style));
            }
            run_style = style;
            run.push_str(symbol);
        }
        if !run.is_empty() {
            spans.push(Span::styled(run, run_style));
        }
        lines.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

pub(super) fn spans_width(spans: &[Span<'_>]) -> usize {
    spans
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
        .sum()
}

/// Rows of command text shown in the approval prompt before it stops.
const MAX_APPROVAL_LINES: usize = 4;
/// Tool output rows shown before the rest is collapsed.
const TOOL_OUTPUT_PREVIEW_LINES: usize = 7;
/// Rows of the command palette shown before it scrolls: the list never eats
/// more than this many transcript rows.
const MAX_PALETTE_ROWS: usize = 10;
/// The spinner frames: a braille ring whose gap turns, the terminal stand-in
/// for a ring with an arc sweeping around it. Eight frames, one per tick.
const SPINNER: [&str; 8] = ["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"];

/// The spinner glyph for `step`: the footer, the session tabs, the sidebar and
/// the transcript all turn off this one clock.
pub(super) fn spinner(step: usize) -> &'static str {
    SPINNER[step % SPINNER.len()]
}

pub(super) fn responsive_palette_width(area_width: u16) -> u16 {
    (area_width.saturating_sub(4) * 85 / 100)
        .clamp(60, 110)
        .min(area_width.saturating_sub(2))
}

/// The workspace path with the user's home directory shortened to `~`.
fn workspace_label(path: &std::path::Path) -> String {
    let home = directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf());
    if let Some(home) = home {
        if let Ok(rest) = path.strip_prefix(&home) {
            return if rest.as_os_str().is_empty() {
                "~".to_owned()
            } else {
                format!("~/{}", rest.display())
            };
        }
    }
    path.display().to_string()
}

/// The streaming caret alternates on a ~300ms cadence driven by the 100ms
/// ticker, so a quiet stretch of tokens still reads as alive.
fn caret(spinner: usize) -> &'static str {
    if (spinner / 3) % 2 == 0 { "▌" } else { "" }
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    draw_backdrop(frame, area, app.spinner);
    if area.width < 28 || area.height < 8 {
        app.output_rows.clear();
        frame.render_widget(
            Paragraph::new("Terminal too small")
                .alignment(Alignment::Center)
                .style(Style::default().fg(muted())),
            area,
        );
        return;
    }

    let picker = app.model_picker.clone();
    let session_picker = app.session_picker.clone();
    let revert_picker = app.revert_picker.clone();
    let agents_picker = app.agents_picker.clone();
    // The Progress panel steps aside once every task is done: a finished
    // checklist means finished work, not a panel parked over the composer.
    let tasks = app.session.tasks.iter().any(|task| !task.done);
    let palette = if app.show_help
        || app.changelog.is_some()
        || app.connect_open()
        || app.approval.is_some()
        || picker.is_some()
        || session_picker.is_some()
        || revert_picker.is_some()
        || agents_picker.is_some()
        || app.settings_open
        || app.subagent_view.is_some()
    {
        Vec::new()
    } else {
        app.palette_matches()
    };
    // A long match list scrolls inside the bordered popup instead of taking
    // the transcript's rows.
    let palette_height = if palette.is_empty() {
        0
    } else {
        (palette.len().clamp(1, MAX_PALETTE_ROWS) as u16).saturating_add(2)
    };
    let picker_height = if picker.is_some() {
        // A long list scrolls inside the bordered popup instead of growing.
        (app.model_picker_count().clamp(1, 12) as u16).saturating_add(2)
    } else {
        0
    };
    let revert_height = if revert_picker.is_some() {
        (app.revert_picker_count().clamp(1, 12) as u16).saturating_add(2)
    } else {
        0
    };
    let agents_height = if agents_picker.is_some() {
        (app.agents_picker_count().clamp(1, 12) as u16).saturating_add(2)
    } else {
        0
    };
    let popup_height = palette_height + picker_height + revert_height + agents_height;
    let composer_height = composer_height(app, area.width, area.height);
    let progress_height = if tasks {
        if app.progress_collapsed {
            1
        } else {
            (app.session.tasks.len().min(6) as u16) + 2
        }
    } else {
        app.progress_row = None;
        0
    };

    let mut constraints = vec![Constraint::Length(1)];
    // The session tabs get their own row under the header, so a full set of
    // them never squeezes the footer's badge and context load.
    constraints.push(Constraint::Length(1));
    constraints.push(Constraint::Min(2));
    if popup_height > 0 {
        constraints.push(Constraint::Length(popup_height));
    }
    if progress_height > 0 {
        constraints.push(Constraint::Length(progress_height));
    }
    constraints.push(Constraint::Length(1));
    constraints.push(Constraint::Length(composer_height));
    constraints.push(Constraint::Length(1));
    constraints.push(Constraint::Length(1));

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    draw_header(frame, app, chunks[0]);
    let mut index = 1;
    draw_tab_bar(frame, app, chunks[index]);
    index += 1;
    // The sidebar of subagent runs takes the transcript's right edge while
    // anything still streams, and steps aside once every run has finished —
    // a finished run means finished work, not a panel parked over the
    // transcript. `/agents` or the viewer still reach every run afterwards.
    // Narrower terminals keep the sidebar: its rows and badge truncate to
    // the box instead of the panel vanishing.
    let show_sidebar = app.any_subagent_running() && area.width >= SIDEBAR_MIN_WIDTH;
    let transcript_area = if show_sidebar {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(20), Constraint::Length(SIDEBAR_WIDTH)])
            .split(chunks[index]);
        draw_agents_sidebar(frame, app, columns[1]);
        columns[0]
    } else {
        app.sidebar_hits.clear();
        app.sidebar_area = None;
        chunks[index]
    };
    draw_transcript(frame, app, transcript_area);

    index += 1;
    if popup_height > 0 {
        if let Some(revert_picker) = &revert_picker {
            draw_revert_picker(frame, app, revert_picker, chunks[index]);
        } else if let Some(agents_picker) = &agents_picker {
            draw_agents_picker(frame, app, agents_picker, chunks[index]);
        } else if let Some(picker) = &picker {
            draw_model_picker(frame, app, picker, chunks[index]);
        } else {
            draw_palette(frame, app, &palette, chunks[index]);
        }
        index += 1;
    }
    if progress_height > 0 {
        draw_progress(frame, app, chunks[index]);
        index += 1;
    } else if popup_height == 0 {
        draw_separator(frame, chunks[index]);
    }
    // The separator slot is reserved even when an overlay or the panel takes
    // that row instead.
    index += 1;
    draw_composer(frame, app, chunks[index]);
    draw_footer(frame, app, chunks[index + 1]);

    let spinner = app.spinner;
    let sides = app.diff_sides;
    if let Some(approval) = app.approval.as_mut() {
        draw_approval(frame, approval, sides, spinner, area);
    } else if app.changelog.is_some() {
        draw_changelog(frame, app, area);
    } else if app.show_help {
        draw_help(frame, app, area);
    } else if app.settings_open {
        draw_settings(frame, app, area);
    } else if let Some(connect) = app.connect_state() {
        draw_connect(frame, app, connect, spinner, area);
    } else if let Some(session_picker) = &session_picker {
        draw_session_picker(frame, app, session_picker, area);
    } else if app.subagent_view.is_some() {
        draw_agents_viewer(frame, app, area);
    }
}

fn truncate(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(text) <= width {
        return text.to_owned();
    }
    let mut result = String::new();
    let mut used = 0usize;
    for grapheme in text.graphemes(true) {
        let cells = UnicodeWidthStr::width(grapheme);
        if used + cells + 1 > width {
            break;
        }
        result.push_str(grapheme);
        used += cells;
    }
    result.push('…');
    result
}

// ponytail: 4-space tab stops; add configurable tab size if user settings support it.
/// Tabs become spaces and carriage returns are stripped: ratatui has no tab stops
/// and raw `\r` desyncs cursor position in terminal cells.
pub(super) fn normalize_line(line: &str) -> String {
    let mut output = String::with_capacity(line.len());
    let mut column = 0;
    for ch in line.chars() {
        if ch == '\r' {
            continue;
        }
        if ch == '\t' {
            let spaces = 4 - column % 4;
            output.push_str(&" ".repeat(spaces));
            column += spaces;
        } else {
            output.push(ch);
            column += UnicodeWidthChar::width(ch).unwrap_or(0);
        }
    }
    output
}

/// Pad `text` with spaces to `width` display columns, so CJK and emoji
/// align like ASCII does.
#[cfg(test)]
pub(super) fn pad_to(text: &str, width: usize) -> String {
    let display = UnicodeWidthStr::width(text);
    if display >= width {
        text.to_owned()
    } else {
        format!("{text}{}", " ".repeat(width - display))
    }
}

/// Fit `text` to exactly `width` columns: truncates with ellipsis if longer,
/// pads with spaces if shorter.
pub(super) fn fit_to(text: &str, width: usize) -> String {
    let truncated = truncate(text, width);
    let display = UnicodeWidthStr::width(truncated.as_str());
    if display >= width {
        truncated
    } else {
        format!("{truncated}{}", " ".repeat(width - display))
    }
}

/// The muted stats line under a finished reply: tokens out, then how long.
fn usage_line(usage: Usage) -> Line<'static> {
    let mut text = format!("out {}", format_tokens(usage.completion_tokens));
    let seconds = usage.elapsed_ms as f64 / 1000.0;
    if usage.elapsed_ms > 0 {
        text.push_str(&format!(" · {seconds:.1}s"));
    }
    if let Some(rate) = usage.tokens_per_second() {
        text.push_str(&format!(" · {rate:.0} t/s"));
    }
    Line::styled(text, Style::default().fg(muted()))
}

/// Thinking above a reply: every dim line while streaming or expanded, and
/// after the reply a single summary that the expand set can open back up.
/// Lines of thinking shown while streaming; older lines collapse into a
/// summary so long reasoning does not re-wrap the whole transcript per frame.
const STREAMING_REASONING_LINES: usize = 24;

/// Reply characters parsed for markdown while the message streams. The
/// viewport follows the tail while a reply streams, so the earlier text sits
/// above the fold anyway; re-parsing it on every frame is the lag.
const STREAMING_MARKDOWN_CHARS: usize = 8000;

/// The tail of a streaming reply, cut forward to a character boundary.
fn streaming_tail(content: &str) -> &str {
    if content.len() <= STREAMING_MARKDOWN_CHARS {
        return content;
    }
    let mut cut = content.len() - STREAMING_MARKDOWN_CHARS;
    while !content.is_char_boundary(cut) {
        cut += 1;
    }
    &content[cut..]
}

fn reasoning_lines(message: &Message, streaming: bool, expanded: bool) -> Vec<Line<'static>> {
    let lines: Vec<&str> = message
        .reasoning
        .split('\n')
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if lines.is_empty() {
        return Vec::new();
    }
    if streaming || expanded {
        let style = Style::default().fg(muted());
        let hidden = lines.len().saturating_sub(STREAMING_REASONING_LINES);
        let show_hidden = streaming && hidden > 0 && !expanded;
        let start = if show_hidden { hidden } else { 0 };
        let mut output: Vec<Line<'static>> = Vec::new();
        if show_hidden {
            output.push(Line::from(vec![
                Span::styled("│ ", Style::default().fg(muted())),
                Span::styled(format!("… {hidden} earlier lines of thinking"), style),
            ]));
        }
        // A fenced block in the thinking is drawn as a code box instead of raw
        // backticks, so a model that sketches code mid-thought reads as code.
        let mut in_code = false;
        for line in &lines[start..] {
            if let Some(language) = line.trim_start().strip_prefix("```") {
                let opening = !in_code;
                in_code = opening;
                let language = language.trim();
                let head = match (opening, language.is_empty()) {
                    (true, true) => "╭─ code".to_owned(),
                    (true, false) => format!("╭─ {language}"),
                    (false, _) => "╰─".to_owned(),
                };
                output.push(Line::from(vec![
                    Span::styled("│ ", Style::default().fg(muted())),
                    Span::styled(head, Style::default().fg(code())),
                ]));
                continue;
            }
            let mut spans = vec![Span::styled("│ ", Style::default().fg(muted()))];
            if in_code {
                spans.push(Span::styled("│ ", Style::default().fg(dim())));
                spans.push(Span::styled(
                    (*line).to_owned(),
                    Style::default().fg(code()),
                ));
            } else {
                spans.push(Span::styled((*line).to_owned(), style));
            }
            output.push(Line::from(spans));
        }
        return output;
    }
    let text = if lines.len() > 2 {
        format!("{} … {} lines of thinking", lines[0], lines.len())
    } else {
        lines.join(" ")
    };
    vec![Line::from(vec![
        Span::styled("▸ ", Style::default().fg(muted())),
        Span::styled(text, Style::default().fg(muted())),
    ])]
}

fn padded(area: Rect, horizontal: u16) -> Rect {
    Rect {
        x: area.x.saturating_add(horizontal),
        width: area.width.saturating_sub(horizontal.saturating_mul(2)),
        ..area
    }
}

fn wrapped_line_count(text: &str, width: u16) -> usize {
    let width = usize::from(width).max(1);
    text.split('\n').map(|line| wrapped_rows(line, width)).sum()
}

/// Rows ratatui's `WordWrapper { trim: false }` renders for one logical line.
///
/// A port of the width bookkeeping in ratatui 0.29 `process_input`: greedy word
/// wrap, a word longer than the width splits across rows. textwrap disagrees on
/// exactly those cases, and the click map must match what is drawn.
fn wrapped_rows(line: &str, width: usize) -> usize {
    let mut rows = 0usize;
    let mut line_width = 0usize;
    let mut word_width = 0usize;
    let mut whitespace_width = 0usize;
    // Pending whitespace is the contiguous run before the current word, so a
    // byte range stands in for ratatui's VecDeque and skips the heap churn.
    let mut whitespace: (usize, usize) = (0, 0);
    let mut non_whitespace_previous = false;
    let mut line_has_graphemes = false;
    let mut word_has_graphemes = false;

    for (offset, grapheme) in line.grapheme_indices(true) {
        let is_whitespace = grapheme.chars().all(char::is_whitespace);
        let symbol_width = grapheme.width();
        if symbol_width > width {
            continue;
        }

        let word_found = non_whitespace_previous && is_whitespace;
        let untrimmed_overflow =
            !line_has_graphemes && word_width + whitespace_width + symbol_width > width;
        if word_found || untrimmed_overflow {
            line_width += whitespace_width + word_width;
            line_has_graphemes = true;
            whitespace_width = 0;
            word_width = 0;
            word_has_graphemes = false;
            whitespace = (offset, offset);
        }

        let line_full = line_width >= width;
        let pending_word_overflow =
            symbol_width > 0 && line_width + whitespace_width + word_width >= width;
        if line_full || pending_word_overflow {
            rows += 1;
            let mut remaining = width.saturating_sub(line_width);
            line_width = 0;
            line_has_graphemes = false;
            while whitespace.0 < whitespace.1 {
                let head = &line[whitespace.0..whitespace.1];
                let first = head.graphemes(true).next().unwrap();
                if first.width() > remaining {
                    break;
                }
                whitespace_width -= first.width();
                remaining -= first.width();
                whitespace.0 += first.len();
            }
            if is_whitespace && whitespace.0 == whitespace.1 {
                continue;
            }
        }

        if is_whitespace {
            whitespace_width += symbol_width;
            if whitespace.0 == whitespace.1 {
                whitespace.0 = offset;
            }
            whitespace.1 = offset + grapheme.len();
        } else {
            word_width += symbol_width;
            word_has_graphemes = true;
        }
        non_whitespace_previous = !is_whitespace;
    }

    if !line_has_graphemes && !word_has_graphemes && whitespace.0 < whitespace.1 {
        rows += 1;
    }
    line_has_graphemes = line_has_graphemes || whitespace.0 < whitespace.1 || word_has_graphemes;
    if line_has_graphemes {
        rows += 1;
    }
    rows.max(1)
}

#[cfg(test)]
mod tests {
    use super::{frame_lines, pad_to, reasoning_lines, wrapped_line_count};
    use crate::session::{Message, Role};
    use ratatui::{style::Color, text::Line};
    use unicode_width::UnicodeWidthStr;

    fn text_of(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn approvals_render_the_diff_of_a_change() {
        use super::approval_lines;
        use crate::app::ToolApproval;
        use crate::diff::FileDiff;
        use crate::session::ToolCall;

        let call = ToolCall::new(
            "call_1",
            "write_file",
            r#"{"path":"hello.txt","content":"hi"}"#,
        );
        let approval = ToolApproval {
            call,
            note: Some("old_text was not found; the edit will fail.".to_owned()),
            diffs: vec![FileDiff::new("hello.txt", "", "hi\n", true)],
            scroll: 0,
            page_height: 0,
            popup_area: std::cell::Cell::new(ratatui::layout::Rect::default()),
        };
        let lines = approval_lines(&approval, 60, false);
        let text = lines.iter().map(text_of).collect::<Vec<_>>().join("\n");
        assert!(text.contains("hello.txt"), "{text}");
        assert!(text.contains("+1"), "{text}");
        assert!(!text.contains("$"), "{text}");
        assert!(text.contains("old_text was not found"), "{text}");
    }

    #[test]
    fn quiet_read_summaries_skip_the_contents() {
        use super::read_result_summary;
        let (path, summary) = read_result_summary(
            "src/main.rs (1014 lines)\nlines 1-400 of 1014\nfn main() {\n    ...",
        )
        .unwrap();
        assert_eq!(path, "src/main.rs");
        assert_eq!(summary[0], "src/main.rs (1014 lines)");
        assert_eq!(summary[1], "lines 1-400 of 1014");
        assert_eq!(summary.len(), 2);

        // A full read keeps only the header line.
        let (path, summary) =
            read_result_summary("todo.md (12 lines)\n- [x] ready\n- [ ] go").unwrap();
        assert_eq!(path, "todo.md");
        assert_eq!(summary.len(), 1);

        // Directory listings and unplanned shapes stay visible in full.
        assert!(read_result_summary("./\nnote.txt  2 B\nnested/\n").is_none());
        assert!(read_result_summary("src/missing.txt (10 lines) nope").is_none());
    }

    #[test]
    fn streaming_reasoning_shows_recent_lines_with_a_summary() {
        let mut message = Message::new(Role::Assistant, "");
        message.reasoning = (0..40)
            .map(|i| format!("thought {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let lines = reasoning_lines(&message, true, false);
        assert_eq!(lines.len(), 25);
        assert_eq!(text_of(&lines[0]), "│ … 16 earlier lines of thinking");
        assert_eq!(text_of(&lines[1]), "│ thought 16");
        assert_eq!(text_of(&lines[24]), "│ thought 39");
        // Expanding keeps every line.
        assert_eq!(reasoning_lines(&message, true, true).len(), 40);
        assert_eq!(reasoning_lines(&message, false, false).len(), 1);
    }

    #[test]
    fn fenced_code_in_thinking_draws_a_code_box() {
        let mut message = Message::new(Role::Assistant, "");
        message.reasoning = "plan:\n```rust\nlet x = 1;\n```\ndone".to_owned();
        let lines = reasoning_lines(&message, true, false);
        let text = lines.iter().map(text_of).collect::<Vec<_>>().join("\n");
        assert!(text.contains("│ ╭─ rust"), "{text}");
        assert!(text.contains("│ │ let x = 1;"), "{text}");
        assert!(text.contains("│ ╰─"), "{text}");
        assert!(!text.contains("```"), "the fence markers leaked: {text}");
        // The code line carries the code color, the prose stays muted.
        let code_line = lines
            .iter()
            .find(|line| text_of(line).contains("let x = 1;"))
            .expect("code line");
        assert_eq!(
            code_line.spans.last().map(|span| span.style.fg),
            Some(Some(crate::theme::current().code))
        );
    }

    #[test]
    fn every_framed_line_has_the_same_width() {
        let body = vec![
            Line::raw("short"),
            Line::raw("a much longer body line that has to be clipped"),
            Line::raw(""),
        ];
        let lines = frame_lines("main.py".to_owned(), body, 20, Color::Reset);
        assert_eq!(lines.len(), 6);
        for line in &lines[1..] {
            assert_eq!(text_of(line).chars().count(), 24, "{:?}", text_of(line));
        }
        assert!(text_of(&lines[1]).starts_with("╭─ main.py "));
        assert!(text_of(&lines[3]).starts_with("│ a much longer body"));
        assert!(text_of(&lines[5]).starts_with('╰'));
    }

    #[test]
    fn frame_lines_sanitizes_tabs_and_carriage_returns() {
        let body = vec![Line::raw("col\tnext\r"), Line::raw("\t\tindented")];
        let lines = frame_lines("tool".to_owned(), body, 20, Color::Reset);
        for line in &lines[1..] {
            let text = text_of(line);
            assert_eq!(UnicodeWidthStr::width(text.as_str()), 24, "{text:?}");
            assert!(!text.contains('\t'));
            assert!(!text.contains('\r'));
            assert!(text.ends_with('│') || text.ends_with('╮') || text.ends_with('╯'));
        }
    }

    #[test]
    fn frame_lines_preserves_span_styles_for_diff_colors() {
        use ratatui::style::Style;
        use ratatui::text::Span;
        let line = Line::from(vec![
            Span::styled("1 ", Style::default().fg(Color::DarkGray)),
            Span::styled("+ ", Style::default().fg(Color::Green)),
            Span::styled("new line", Style::default().fg(Color::Green)),
        ]);
        let lines = frame_lines("diff".to_owned(), vec![line], 30, Color::Reset);
        assert_eq!(lines.len(), 4);
        let body_row = &lines[2];
        assert_eq!(body_row.spans[1].style.fg, Some(Color::DarkGray));
        assert_eq!(body_row.spans[2].style.fg, Some(Color::Green));
        assert_eq!(body_row.spans[3].style.fg, Some(Color::Green));
    }

    #[test]
    fn a_label_wider_than_the_frame_does_not_break_it() {
        let lines = frame_lines("x".repeat(60), vec![Line::raw("body")], 10, Color::Reset);
        assert_eq!(text_of(&lines[1]).chars().count(), 14);
        assert_eq!(text_of(&lines[2]).chars().count(), 14);
    }

    #[test]
    fn padding_counts_display_columns_not_chars() {
        // A CJK char is two columns; char-count padding would under-pad it.
        assert_eq!(pad_to("a", 4), "a   ");
        assert_eq!(pad_to("汉", 4), "汉  ");
        assert_eq!(pad_to("汉汉", 4), "汉汉");
        assert_eq!(pad_to("too long here", 4), "too long here");
    }

    #[test]
    fn the_frame_fits_every_usable_width() {
        for inner in 6..90 {
            for label in ["tool", "main.py", "yopes", "a-very-long-path/some/file.rs"] {
                let body = vec![
                    Line::raw("short"),
                    Line::raw("a much longer body line that has to be clipped"),
                    Line::raw(""),
                ];
                let lines = frame_lines(label.to_owned(), body, inner, Color::Reset);
                for line in &lines[1..] {
                    assert_eq!(
                        text_of(line).chars().count(),
                        inner + 4,
                        "inner={inner} label={label}",
                    );
                }
            }
        }
    }

    /// The palette is truncated to a fixed window and scrolls to follow the
    /// selection, so a long match list never eats the transcript's rows.
    #[test]
    fn the_command_palette_scrolls_instead_of_growing() {
        use ratatui::{Terminal, backend::TestBackend};

        let mut app = crate::app::test_app();
        app.input.set("/".to_owned());
        let palette = app.palette_matches();
        assert!(palette.len() > 4, "not enough commands to need scrolling");

        // A popup only four rows tall: the window follows the selection
        // instead of drawing past the border.
        let mut terminal = Terminal::new(TestBackend::new(40, 6)).unwrap();
        let text = |terminal: &Terminal<TestBackend>| {
            (0..6)
                .map(|row| {
                    (0..40)
                        .map(|column| {
                            terminal.backend().buffer()[(column, row)]
                                .symbol()
                                .to_string()
                        })
                        .collect::<String>()
                })
                .collect::<Vec<String>>()
                .join("\n")
        };
        let draw_palette = |terminal: &mut Terminal<TestBackend>, app: &crate::app::App| {
            let palette = app.palette_matches();
            terminal
                .draw(|frame| {
                    super::pickers::draw_palette(frame, app, &palette, frame.area());
                })
                .unwrap();
        };
        draw_palette(&mut terminal, &app);
        let first = text(&terminal);
        assert!(first.contains(" commands 1/"), "{first}");
        assert!(first.contains(&palette[0].label), "{first}");

        // The last command scrolls into the window and the first scrolls out.
        for _ in 1..palette.len() {
            app.palette_move(true);
        }
        draw_palette(&mut terminal, &app);
        let last = text(&terminal);
        assert!(
            last.contains(&format!(" commands {0}/{0} ", palette.len())),
            "{last}"
        );
        assert!(
            !last.contains(&palette[0].label),
            "the first row did not scroll away: {last}"
        );
    }

    #[test]
    fn the_workspace_line_shortens_home_to_a_tilde() {
        use super::workspace_label;

        let Some(home) = directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf())
        else {
            return;
        };
        assert_eq!(workspace_label(&home), "~");
        let nested = home.join("Desktop").join("YopesCLI");
        let label = workspace_label(&nested);
        assert!(label.starts_with('~'), "{label}");
        assert!(label.ends_with("YopesCLI"), "{label}");
        assert!(
            !label.contains(home.display().to_string().as_str()),
            "home was not shortened: {label}"
        );
        // A path outside home renders in full.
        let elsewhere = std::path::Path::new("/opt/somewhere");
        assert_eq!(workspace_label(elsewhere), elsewhere.display().to_string());
    }

    #[test]
    fn wrapped_counts_include_words_and_explicit_newlines() {
        assert_eq!(wrapped_line_count("", 10), 1);
        assert_eq!(wrapped_line_count("one\ntwo", 10), 2);
        assert_eq!(wrapped_line_count("aaaaaa aaaaaa aaaaaa", 10), 3);
    }

    #[test]
    fn wrapped_counts_hold_for_long_text() {
        // One 600-char word: it splits across rows, no panic, no truncation.
        let word = "x".repeat(600);
        let single = wrapped_line_count(&word, 60);
        assert_eq!(wrapped_line_count(&word, 60), single, "stable per call");

        // 500+ chars of prose wrap greedily: rows stay within the width.
        let prose = "lorem ipsum ".repeat(50);
        let count = wrapped_line_count(prose.trim(), 60);
        assert!((10..=12).contains(&count), "greedy rows: {count}");

        // Long multi-line text keeps every explicit line plus its wraps.
        let text = format!("{}\n{}\n{}", "a".repeat(300), "", "b".repeat(250));
        let total = wrapped_line_count(&text, 60);
        assert_eq!(
            total,
            wrapped_line_count(&"a".repeat(300), 60) + 1 + wrapped_line_count(&"b".repeat(250), 60),
            "lines add up independently"
        );

        // Wide (CJK) text doubles its column count per char.
        let cjk = "汉".repeat(260);
        assert_eq!(
            wrapped_line_count(&cjk, 60),
            wrapped_line_count(&"x".repeat(520), 60),
            "CJK width matches twice the char count"
        );
    }
}
