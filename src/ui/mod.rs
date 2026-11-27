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
        ToolsMode, format_bytes, format_context, format_rtt, format_tokens, list_row_of,
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
mod glow;
mod help;
mod markdown;
mod mockups;
mod pickers;
mod settings;
mod stats;
mod transcript;

use self::{
    agents::*, approval::*, changelog::*, composer::*, connect::*, glow::*, help::*, pickers::*,
    settings::*, stats::*, transcript::*,
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

/// The backdrop's base color: a near-white wash on a light theme, a near-black
/// one on a dark theme.
fn backdrop_base(theme: &crate::theme::Theme) -> Color {
    let light = match theme.dim {
        Color::Rgb(r, g, b) => u32::from(r) + u32::from(g) + u32::from(b) > 420,
        _ => false,
    };
    if light {
        Color::Rgb(247, 248, 251)
    } else {
        Color::Rgb(9, 11, 19)
    }
}

/// The color of one backdrop row: the sky gradient under `auto`, the theme's
/// vertical wash otherwise.
fn backdrop_row(
    theme: &crate::theme::Theme,
    sky: Option<&crate::theme::Sky>,
    rows: usize,
    y: usize,
) -> Color {
    let base = backdrop_base(theme);
    let position = y * 100 / rows.max(1);
    match sky {
        Some(sky) => blend(
            theme.accent2,
            blend(
                sky.top,
                sky.horizon,
                y * 100 / rows.saturating_sub(1).max(1),
            ),
            95,
        ),
        None if position < 40 => blend(theme.accent2, base, 90),
        None if position < 75 => blend(theme.accent, base, 94),
        None => base,
    }
}

/// The sun's disc in cell units: center column and row, tint, strength, and
/// glow radius. A cell is about twice as tall as wide, so row distances count
/// double.
type SunGlow = Option<(f32, f32, Color, f32, f32)>;

fn sun_glow(sky: Option<&crate::theme::Sky>, rows: usize, columns: usize) -> SunGlow {
    sky.and_then(|sky| sky.sun.as_ref()).map(|sun| {
        let radius = (columns as f32 * 0.16).clamp(6.0, 22.0);
        (
            sun.x * columns as f32,
            sun.y * rows as f32,
            sun.color,
            sun.strength as f32,
            radius,
        )
    })
}

/// The row color under the sun: unchanged away from it, tinted toward the sun
/// by a falling-off glow, with a bright core.
fn ground(color: Color, sun: SunGlow, x: usize, y: usize) -> Color {
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
}

/// What an empty cell shows in front of the wash: rain or snow, a star, a grid
/// dot, or nothing.
fn backdrop_glyph(
    sky: bool,
    weather: crate::weather::Weather,
    stars: usize,
    phase: usize,
    row: Color,
    dot: Color,
    x: usize,
    y: usize,
    position: usize,
) -> Option<(&'static str, Color)> {
    if let Some(symbol) = crate::weather::drop_at(weather, x, y, phase) {
        let ink = blend(
            if weather == crate::weather::Weather::Snow {
                Color::Rgb(220, 228, 244)
            } else {
                Color::Rgb(140, 170, 214)
            },
            row,
            45,
        );
        return Some((symbol, ink));
    }
    if sky {
        let mut h = (x as u32).wrapping_mul(0x9E37_79B1) ^ (y as u32).wrapping_mul(0x85EB_CA6B);
        h ^= h >> 15;
        h = h.wrapping_mul(0x2C1B_3C6D);
        h ^= h >> 13;
        if position < 70 && (h % 1000) as usize * 4 < stars {
            let symbol = if h % 7 == 0 { "*" } else { "·" };
            return Some((symbol, blend(Color::Rgb(205, 214, 255), row, 55)));
        }
    }
    if y % 3 == 0 && x % 8 == 4 {
        return Some(("·", dot));
    }
    None
}

/// A quiet ambient backdrop: a vertical wash in the active theme plus a
/// sparse dot grid. Widgets draw over it, so empty cells stop looking like a
/// flat void without competing with the transcript.
///
/// Only the background is painted here. Its glyphs land in
/// `draw_backdrop_glyphs` after every widget: ratatui patches styles, so a
/// dot's foreground left under a later span without a color tinted that text
/// in the dot's faint color.
fn draw_backdrop(frame: &mut Frame, area: Rect) {
    let theme = crate::theme::current();
    let sky = crate::theme::sky();
    let rows = usize::from(area.height);
    let columns = usize::from(area.width);
    let sun = sun_glow(sky.as_ref(), rows, columns);
    let mut lines = Vec::with_capacity(rows);
    for y in 0..rows {
        let color = backdrop_row(&theme, sky.as_ref(), rows, y);
        // Neighbours with one style share a span, so a quiet row stays a
        // handful of spans instead of one per cell.
        let mut spans = Vec::new();
        let mut run = String::new();
        let mut run_style = Style::default();
        for x in 0..columns {
            let style = Style::default().bg(ground(color, sun, x, y));
            if style != run_style && !run.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut run), run_style));
            }
            run_style = style;
            run.push(' ');
        }
        if !run.is_empty() {
            spans.push(Span::styled(run, run_style));
        }
        lines.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

/// The backdrop's glyphs, painted after every widget so a cell that carries
/// text or a panel keeps its own foreground. A cell is only decorated while it
/// is still empty and still sits on the wash: anything a widget painted is
/// left alone.
///
/// `phase` is the app's 100ms tick: it slides the rain and snow of `auto`.
fn draw_backdrop_glyphs(frame: &mut Frame, area: Rect, phase: usize) {
    let theme = crate::theme::current();
    let sky = crate::theme::sky();
    let weather = sky
        .as_ref()
        .map_or(crate::weather::Weather::Unknown, |sky| sky.weather);
    let stars = sky.as_ref().map_or(0, |sky| sky.stars);
    let dot = blend(theme.accent2, backdrop_base(&theme), 78);
    let rows = usize::from(area.height);
    let columns = usize::from(area.width);
    let sun = sun_glow(sky.as_ref(), rows, columns);
    let buffer = frame.buffer_mut();
    for y in 0..rows {
        let color = backdrop_row(&theme, sky.as_ref(), rows, y);
        let position = y * 100 / rows.max(1);
        for x in 0..columns {
            // A blank cell wedged between glyphs belongs to a text line: the
            // grid stays out of it, so no dot lands between two words.
            let neighbour_text = |dx: isize| {
                let at = x as isize + dx;
                (at >= 0 && at < columns as isize)
                    && buffer
                        .cell((at as u16, y as u16))
                        .is_some_and(|cell| cell.symbol() != " ")
            };
            if neighbour_text(-1) || neighbour_text(1) {
                continue;
            }
            let Some(cell) = buffer.cell_mut((x as u16, y as u16)) else {
                continue;
            };
            // Text, a wide glyph's tail, or a widget's own foreground or
            // background: not ours. A span patching a color leaves the cell
            // marked, even when it painted only blanks.
            if cell.symbol() != " "
                || cell.skip
                || cell.fg != Color::Reset
                || cell.bg != ground(color, sun, x, y)
            {
                continue;
            }
            let Some((symbol, ink)) = backdrop_glyph(
                sky.is_some(),
                weather,
                stars,
                phase,
                color,
                dot,
                x,
                y,
                position,
            ) else {
                continue;
            };
            cell.set_symbol(symbol);
            cell.fg = ink;
        }
    }
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
/// The one live-status glyph: a filled dot whose color breathes. Nothing
/// spins and nothing reflows, so a status row keeps its width while it moves.
pub(super) const LIVE_DOT: &str = "●";

/// A breathing color on the 100ms ticker: from `base` to `bright` and back
/// over about a second.
///
/// This is what replaced the braille spinner: a live row reads as alive from
/// its light, not from a rotating glyph that drags the eye to the left edge
/// of every line it touches.
pub(super) fn breathe(step: usize, base: Color, bright: Color) -> Style {
    let phase = (step % 14) as f32 / 14.0 * std::f32::consts::TAU;
    let wave = (phase.sin() * 0.5 + 0.5) * 100.0;
    Style::default().fg(blend(base, bright, wave as usize))
}

/// A sweep of light crossing `text`, one pass every couple of seconds: each
/// glyph is blended toward `bright` by how close it sits to a head that
/// travels the string.
///
/// The words stay where they are and only their color moves, so a thinking
/// line has momentum without the spinner's churn.
pub(super) fn shimmer(text: &str, step: usize, base: Color, bright: Color) -> Vec<Span<'static>> {
    // Four quiet cells ahead of the text, so the sweep starts off the first
    // glyph instead of lighting it up from nothing.
    let lead = 4usize;
    let length = text.chars().count();
    let period = length + lead + 6;
    let head = (step / 2) % period;
    let mut spans: Vec<Span<'static>> = Vec::with_capacity(length);
    let mut run = String::new();
    let mut run_style = Style::default().fg(base);
    for (index, character) in text.chars().enumerate() {
        let distance = (index + lead).abs_diff(head);
        // A three-cell gradient: full, two thirds, one third, then back to
        // the base color, which keeps the sweep cheap to blend per frame.
        let style = match distance {
            0 => Style::default().fg(bright),
            1 => Style::default().fg(blend(base, bright, 66)),
            2 => Style::default().fg(blend(base, bright, 33)),
            _ => Style::default().fg(base),
        };
        if style != run_style && !run.is_empty() {
            spans.push(Span::styled(std::mem::take(&mut run), run_style));
        }
        run_style = style;
        run.push(character);
    }
    if !run.is_empty() {
        spans.push(Span::styled(run, run_style));
    }
    spans
}

/// A shimmering status prefix: the breathing dot and a swept label.
pub(super) fn live_label(
    label: &str,
    step: usize,
    base: Color,
    bright: Color,
) -> Vec<Span<'static>> {
    let mut spans = vec![
        Span::styled(LIVE_DOT, breathe(step, base, bright)),
        Span::styled(" ", Style::default().fg(base)),
    ];
    spans.extend(shimmer(label, step, base, bright));
    spans
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

/// The live "thinking" row: a breathing rail mark and the swept word.
///
/// This is what a stream with no text yet shows instead of a blinking cursor
/// on its own line: the row says what is happening, and its light says the
/// process is alive.
pub(super) fn thinking_line(step: usize) -> Line<'static> {
    let mut spans = vec![Span::styled("▍", breathe(step, dim(), accent()))];
    spans.push(Span::styled(" ", Style::default().fg(dim())));
    spans.extend(shimmer("thinking", step, muted(), accent2()));
    Line::from(spans)
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    draw_backdrop(frame, area);
    if area.width < 28 || area.height < 8 {
        app.output_rows.clear();
        frame.render_widget(
            Paragraph::new("Terminal too small")
                .alignment(Alignment::Center)
                .style(Style::default().fg(muted())),
            area,
        );
        draw_backdrop_glyphs(frame, area, app.spinner);
        return;
    }

    let picker = app.model_picker.clone();
    let session_picker = app.session_picker.clone();
    let revert_picker = app.revert_picker.clone();
    let agents_picker = app.agents_picker.clone();
    let connect_open = app.connect_open();
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
        || app.stats.is_some()
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
    // The connect dialog is a docked box in the same slot as the palette: it
    // used to be a full-screen window over the session it was changing.
    let connect_height = connect_height(app);
    // Room for the box and its two borders. A short terminal gives the list
    // up to the transcript rather than pushing the composer off the screen.
    let popup_height = palette_height + picker_height + revert_height + agents_height;
    let dock_height = popup_height.max(connect_height);
    let composer_height = composer_height(app, area.width, area.height).saturating_add(2);
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
    if dock_height > 0 {
        constraints.push(Constraint::Length(dock_height));
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
    if connect_open {
        if let Some(connect) = app.connect_state() {
            draw_connect(frame, app, connect, app.spinner, chunks[index]);
        }
        index += 1;
    } else if popup_height > 0 {
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
    } else if dock_height == 0 {
        draw_separator(frame, chunks[index], app.spinner);
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
    } else if app.stats.is_some() {
        draw_stats(frame, app, area);
    } else if app.show_help {
        draw_help(frame, app, area);
    } else if app.settings_open {
        draw_settings(frame, app, area);
    } else if let Some(session_picker) = &session_picker {
        draw_session_picker(frame, app, session_picker, area);
    } else if app.subagent_view.is_some() {
        draw_agents_viewer(frame, app, area);
    }
    draw_backdrop_glyphs(frame, area, app.spinner);
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
    if usage.ttft_ms > 0 {
        text.push_str(&format!(" · rtt {}", format_rtt(usage.ttft_ms)));
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
    use super::{pad_to, reasoning_lines, tree_lines, wrapped_line_count};
    use crate::session::{Message, Role};
    use ratatui::{style::Color, text::Line};

    /// The dot grid is painted after the widgets, so a span that names no
    /// color cannot inherit the dot's faint foreground: every letter keeps its
    /// own color while the grid still shows through the empty cells.
    #[test]
    fn the_dot_grid_never_tints_text() {
        use ratatui::{Terminal, backend::TestBackend};

        let mut app = crate::app::test_app();
        app.session
            .messages
            .push(Message::new(Role::User, "hello world, a line with letters"));
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| super::draw(frame, &mut app)).unwrap();

        let theme = crate::theme::current();
        let dot = super::blend(theme.accent2, super::backdrop_base(&theme), 78);
        let mut glyphs = 0;
        let mut dots = 0;
        for cell in terminal.backend().buffer().content() {
            if cell.symbol() == "·" && cell.fg == dot {
                dots += 1;
            }
            if cell.symbol().chars().any(char::is_alphanumeric) {
                glyphs += 1;
                assert_ne!(
                    cell.fg,
                    dot,
                    "text inherited the dot color: {:?}",
                    cell.symbol()
                );
            }
        }
        assert!(glyphs > 0, "nothing rendered");
        assert!(dots > 0, "the dot grid stopped drawing");
    }

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
    fn tool_output_hangs_under_the_call_row() {
        let body = vec![Line::raw("1 match"), Line::raw("src/theme.rs")];
        let lines = tree_lines(body, Color::Reset, true);
        assert_eq!(lines.len(), 2);
        // The first row is the connector under `→ tool …`, the rest indent
        // under it: no head label, no closing rule, no box.
        assert_eq!(text_of(&lines[0]), "  ╰ 1 match");
        assert_eq!(text_of(&lines[1]), "    src/theme.rs");
        for line in &lines {
            let text = text_of(line);
            assert!(
                !text.contains('│') && !text.contains('╭') && !text.contains('╮'),
                "the tree drew a box: {text}"
            );
        }
    }

    /// Two results from the same turn join into one rail: the first keeps a
    /// joint and a running rail, the last closes it.
    #[test]
    fn a_second_result_keeps_the_connector_rail() {
        let body = vec![Line::raw("out"), Line::raw("more")];
        let lines = tree_lines(body, Color::Reset, false);
        assert_eq!(text_of(&lines[0]), "  ├ out");
        assert_eq!(text_of(&lines[1]), "  │ more");
    }

    #[test]
    fn the_tree_sanitizes_tabs_and_keeps_span_styles() {
        use ratatui::style::Style;
        use ratatui::text::Span;
        let line = Line::from(vec![
            Span::styled("1 ", Style::default().fg(Color::DarkGray)),
            Span::styled("+ ", Style::default().fg(Color::Green)),
            Span::styled("new line", Style::default().fg(Color::Green)),
        ]);
        let lines = tree_lines(vec![Line::raw("col\tnext\r"), line], Color::Reset, true);
        for line in &lines {
            let text = text_of(line);
            assert!(!text.contains('\t'), "{text:?}");
            assert!(!text.contains('\r'), "{text:?}");
        }
        // Spans land past the four-cell marker, colors intact.
        let body_row = &lines[1];
        assert_eq!(body_row.spans[1].style.fg, Some(Color::DarkGray));
        assert_eq!(body_row.spans[2].style.fg, Some(Color::Green));
        assert_eq!(body_row.spans[3].style.fg, Some(Color::Green));
    }

    #[test]
    fn padding_counts_display_columns_not_chars() {
        // A CJK char is two columns; char-count padding would under-pad it.
        assert_eq!(pad_to("a", 4), "a   ");
        assert_eq!(pad_to("汉", 4), "汉  ");
        assert_eq!(pad_to("汉汉", 4), "汉汉");
        assert_eq!(pad_to("too long here", 4), "too long here");
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
