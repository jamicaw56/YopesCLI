use super::*;

pub(super) fn draw_header(frame: &mut Frame, app: &mut App, area: Rect) {
    let inner = padded(area, 2);
    // The top row is the provider line alone: the window title already says
    // yopes, and a brand mark parked in the corner read as a stray footer.
    // The mode, the permission badge and the context load sit in the footer.
    let ttft = app
        .session
        .messages
        .iter()
        .rev()
        .find_map(|message| message.usage)
        .map(|usage| usage.ttft_ms);
    let bars = signal_bars(ttft);
    // The meter keeps its own width out of the header's budget, so a narrow
    // terminal drops the provider name before the bars collide with the spend.
    let bars_w = if bars.is_empty() {
        0
    } else {
        spans_width(&bars) + 1
    };
    let right = header_right(
        app.backend.provider_label(),
        app.backend.model_display(),
        &app.spent_cost(),
        usize::from(inner.width).saturating_sub(bars_w),
    );
    if !right.is_empty() || !bars.is_empty() {
        let mut spans = Vec::with_capacity(3);
        if !right.is_empty() {
            spans.push(Span::raw(right));
        }
        if !bars.is_empty() {
            if !spans.is_empty() {
                spans.push(Span::raw(" "));
            }
            spans.extend(bars);
        }
        frame.render_widget(
            Paragraph::new(Line::from(spans))
                .alignment(Alignment::Right)
                .style(Style::default().fg(muted())),
            inner,
        );
    }
}

/// The permission badge: the word and the color the footer shows for the
/// current `/set tools` mode.
fn permission_badge(mode: ToolsMode) -> (&'static str, Style) {
    match mode {
        ToolsMode::On => ("[ON]", Style::default().fg(dim())),
        ToolsMode::Ask => ("[ASK]", Style::default().fg(dim())),
        ToolsMode::Off => ("[OFF]", Style::default().fg(dim())),
        ToolsMode::Auto => (
            "[YOLO]",
            Style::default().fg(warning()).add_modifier(Modifier::BOLD),
        ),
        ToolsMode::Dangerous => (
            "[DANGER]",
            Style::default().fg(error()).add_modifier(Modifier::BOLD),
        ),
    }
}

/// Four ascending bars, the way a signal meter reads: the faster the last
/// reply came back, the more of them light up. No sample yet, or a zero
/// timing, draws nothing, so the header never shows a meter it cannot back.
fn signal_bars(ttft_ms: Option<u64>) -> Vec<Span<'static>> {
    const BARS: [&str; 4] = ["▁", "▃", "▅", "█"];
    let Some(ms) = ttft_ms.filter(|ms| *ms > 0) else {
        return Vec::new();
    };
    let (lit, color) = match ms {
        0..=400 => (4, added()),
        401..=900 => (3, warning()),
        901..=2_000 => (2, error()),
        _ => (1, blend(error(), dim(), 60)),
    };
    let mut spans = Vec::with_capacity(BARS.len() * 2);
    for (index, bar) in BARS.iter().enumerate() {
        if index > 0 {
            // A cell of air between bars, the way a signal meter draws it.
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            *bar,
            Style::default().fg(if index < lit { color } else { dim() }),
        ));
    }
    spans
}

/// The header's right side: provider, model and spend.
///
/// The context load lives at the bottom right, next to the message count, so
/// the header's right side stays short. A narrow terminal drops the least
/// useful segments from the front, the model name first.
fn header_right(provider: &str, model: &str, cost: &str, budget: usize) -> String {
    let mut segments: Vec<String> = Vec::new();
    if !provider.is_empty() {
        segments.push(provider.to_owned());
    }
    if !model.is_empty() {
        segments.push(model.to_owned());
    }
    // A provider that bills nothing it can price sends an empty cost: the
    // segment is dropped, so no dash placeholder reaches the row.
    if !cost.is_empty() {
        segments.push(cost.to_owned());
    }
    let joined = |segments: &[String]| segments.join(" · ");
    while segments.len() > 2 && UnicodeWidthStr::width(joined(&segments).as_str()) > budget {
        segments.remove(0);
    }
    joined(&segments)
}

/// The context load as `ctx used/window`, for the footer's right side.
fn context_label(context: u64, window: u64) -> String {
    if context == 0 {
        return String::new();
    }
    format!("ctx {}/{}", format_context(context), format_context(window))
}

pub(super) fn draw_separator(frame: &mut Frame, area: Rect, step: usize) {
    let inner = padded(area, 2);
    if inner.width == 0 {
        return;
    }
    // The rule above the composer: a glow head travels across it every couple
    // of seconds and the cells near the head lean toward the accent, while a
    // scatter of dashes reshuffles each frame so the line never reads as a
    // static border.
    let (accent2, dim) = {
        let theme = crate::theme::current();
        (theme.accent2, theme.dim)
    };
    let width = usize::from(inner.width).max(1);
    let head = (step / 2) % width;
    let mut spans = Vec::with_capacity(width);
    for column in 0..width {
        // Circular distance, so the glow wraps instead of pooling at an edge.
        let distance = column.abs_diff(head).min(width - column.abs_diff(head));
        let glow = 100usize.saturating_sub(distance * 100 / 10);
        let glyph = if scramble(step as u64 * 7 + column as u64) % 19 == 0 {
            "┄"
        } else {
            "─"
        };
        spans.push(Span::styled(
            glyph,
            Style::default().fg(blend(dim, accent2, glow)),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), inner);
}

/// A cheap deterministic-scramble: same seed, same number, different seed,
/// scattered. No rng dependency for a decorative shimmer.
fn scramble(seed: u64) -> u64 {
    let mut value = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
    value ^= value >> 33;
    value.wrapping_mul(0xff51_afd7_ed55_8ccd) >> 32
}

/// The color `blend%` of the way from `from` to `to`, per channel.
pub(super) fn blend(from: Color, to: Color, blend: usize) -> Color {
    let (Color::Rgb(fr, fg, fb), Color::Rgb(tr, tg, tb)) = (from, to) else {
        return to;
    };
    let mix = |f: u8, t: u8| {
        // Widened math: the target channel can sit below the source, so the
        // interpolation runs in i32 before clamping back into a byte.
        let f = i32::from(f);
        let t = i32::from(t);
        (f + (t - f) * blend as i32 / 100).clamp(0, 255) as u8
    };
    Color::Rgb(mix(fr, tr), mix(fg, tg), mix(fb, tb))
}

/// The composer's rounded frame: a top edge fading from the secondary accent
/// into dim, a bottom edge fading the other way, and side rails at the
/// midpoint. The input reads as its own card instead of a strip cut off by a
/// rule, and the fade keeps the corners from shouting.
fn draw_composer_box(frame: &mut Frame, area: Rect) {
    if area.width < 6 || area.height < 3 {
        return;
    }
    let (accent, dim) = {
        let theme = crate::theme::current();
        (theme.accent2, theme.dim)
    };
    let width = usize::from(area.width);
    let span_w = width - 2;
    let third = span_w / 3;
    // One edge, in three faded runs: the same thirds the separator band uses.
    let mut edge = |corners: (&'static str, &'static str), (from, to): (Color, Color), row: u16| {
        let mut spans = vec![Span::styled(corners.0, Style::default().fg(from))];
        for (step, count) in [third, third, span_w - 2 * third].into_iter().enumerate() {
            if count == 0 {
                continue;
            }
            spans.push(Span::styled(
                "─".repeat(count),
                Style::default().fg(blend(from, to, step * 50)),
            ));
        }
        spans.push(Span::styled(corners.1, Style::default().fg(to)));
        frame.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect {
                y: row,
                height: 1,
                ..area
            },
        );
    };
    edge(("╭", "╮"), (accent, dim), area.y);
    edge(
        ("╰", "╯"),
        (dim, accent),
        area.y.saturating_add(area.height - 1),
    );
    let rail = Style::default().fg(blend(accent, dim, 50));
    let buffer = frame.buffer_mut();
    for row in area.y.saturating_add(1)..area.y.saturating_add(area.height - 1) {
        buffer.set_string(area.x, row, "│", rail);
        buffer.set_string(area.x + area.width - 1, row, "│", rail);
    }
}

pub(super) fn composer_height(app: &App, width: u16, height: u16) -> u16 {
    let content_width = width.saturating_sub(6).max(1);
    let lines = wrapped_line_count(&app.input.with_cursor(), content_width);
    // A short terminal keeps its transcript rows: the composer shrinks first.
    let max = (height.saturating_sub(6) / 2).clamp(1, 6);
    (lines as u16).clamp(1, max)
}

pub(super) fn draw_composer(frame: &mut Frame, app: &mut App, area: Rect) {
    // The bordered card owns the outer rows and columns; the prompt sits inside
    // it, still two columns in from the edge.
    draw_composer_box(frame, area);
    let inner = Rect {
        y: area.y.saturating_add(1),
        height: area.height.saturating_sub(2),
        ..padded(area, 2)
    };
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(2), Constraint::Min(1)])
        .split(inner);
    // The prompt flashes briefly after a send: input registered, turn running.
    let pulse = match app.send_pulse {
        Some(mark) => app.spinner.saturating_sub(mark) < 5,
        None => false,
    };
    let (prompt_marker, prompt_style) = if app.tools_mode == ToolsMode::Auto {
        (
            // A custom armed-mode marker: reads as "aimed at the target", and
            // avoids the lightning bolt every other terminal tool uses.
            "◉",
            Style::default().fg(warning()).add_modifier(Modifier::BOLD),
        )
    } else if pulse {
        (
            "›",
            Style::default()
                .fg(crate::theme::current().accent2)
                .add_modifier(Modifier::BOLD),
        )
    } else if app.is_busy() {
        ("›", Style::default().fg(muted()))
    } else {
        (
            "›",
            Style::default()
                .fg(crate::theme::current().accent2)
                .add_modifier(Modifier::BOLD),
        )
    };
    frame.render_widget(
        Paragraph::new(prompt_marker).style(prompt_style),
        columns[0],
    );

    let target = columns[1];
    app.composer_area = Some(target);
    if target.width == 0 || target.height == 0 {
        return;
    }
    // A cursor after a trailing newline sits on a fresh row the wrap count
    // folds into the last one; keep that row inside the viewport.
    let mut before = wrapped_line_count(app.input.before_cursor(), target.width).saturating_sub(1);
    if app.input.before_cursor().ends_with('\n') {
        before += 1;
    }
    let height = usize::from(target.height);
    if before < app.input_scroll {
        app.input_scroll = before;
    } else if before >= app.input_scroll.saturating_add(height) {
        app.input_scroll = before.saturating_add(1).saturating_sub(height);
    }
    frame.render_widget(
        Paragraph::new(render_composer_text(
            &app.input,
            typing_caret(app.spinner, app.typing_tick()),
        ))
        .wrap(Wrap { trim: false })
        .scroll((app.input_scroll.min(u16::MAX as usize) as u16, 0)),
        target,
    );
}

/// The composer caret: a steady bar at rest, and a bright bar that throbs and
/// shifts width for a few ticks after each keystroke, so typing reads as
/// motion without the whole composer repainting.
fn typing_caret(step: usize, elapsed: Option<usize>) -> Span<'static> {
    let accent = crate::theme::current().accent2;
    let Some(elapsed) = elapsed else {
        return Span::styled("▌", Style::default().fg(accent));
    };
    let full = crate::app::App::TYPING_TICKS;
    let glow = (full - elapsed) * 100 / full;
    let color = blend(accent, crate::theme::current().text, glow);
    let glyph = if step % 2 == 0 { "▌" } else { "▍" };
    Span::styled(glyph, Style::default().fg(color))
}

fn render_composer_text(input: &Input, caret: Span<'static>) -> Text<'static> {
    let sel = input.selection();
    let sel_style = Style::default().add_modifier(Modifier::REVERSED);
    let norm_style = Style::default().fg(text());
    let style_at = |start: usize| match &sel {
        Some(range) if start >= range.start && start < range.end => sel_style,
        _ => norm_style,
    };

    enum Seg {
        // One styled byte range, emitted as a single span.
        Run(usize, usize, Style),
        Cursor,
        Break,
    }

    // Slice-based runs: one span per styled range instead of a chunk per char.
    let mut segs: Vec<Seg> = Vec::new();
    let mut run: Option<(usize, Style)> = None;
    for (offset, ch) in input.value.char_indices() {
        if offset == input.cursor {
            if let Some((start, style)) = run.take() {
                segs.push(Seg::Run(start, offset, style));
            }
            segs.push(Seg::Cursor);
        }
        if ch == '\n' {
            if let Some((start, style)) = run.take() {
                segs.push(Seg::Run(start, offset, style));
            }
            segs.push(Seg::Break);
            continue;
        }
        let style = style_at(offset);
        match run {
            Some((_, run_style)) if run_style == style => {}
            Some((start, run_style)) => {
                segs.push(Seg::Run(start, offset, run_style));
                run = Some((offset, style));
            }
            None => run = Some((offset, style)),
        }
    }
    if let Some((start, style)) = run {
        segs.push(Seg::Run(start, input.value.len(), style));
    }
    if input.cursor == input.value.len() {
        segs.push(Seg::Cursor);
    }

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut spans: Vec<Span<'static>> = Vec::new();
    for seg in segs {
        match seg {
            Seg::Run(start, end, style) => {
                spans.push(Span::styled(input.value[start..end].to_owned(), style));
            }
            Seg::Cursor => spans.push(caret.clone()),
            Seg::Break => lines.push(Line::from(std::mem::take(&mut spans))),
        }
    }
    lines.push(Line::from(spans));

    Text::from(lines)
}

/// The native progress panel: the session's task checklist above the composer.
///
/// The window follows the first unfinished task, so a run that completes a row
/// scrolls the checklist by itself; the mouse wheel sets an explicit offset.
pub(super) fn draw_progress(frame: &mut Frame, app: &mut App, area: Rect) {
    if area.width < 20 {
        return;
    }
    app.progress_row = Some(area.y);
    app.progress_area = Some((area.x, area.y, area.width, area.height));
    let inner = padded(area, 2);
    let marker = if app.progress_collapsed { "▸" } else { "▾" };
    let total = app.session.tasks.len();
    let done = app.session.tasks.iter().filter(|task| task.done).count();

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut scrolled = false;
    if !app.progress_collapsed {
        if area.height < 3 {
            return;
        }
        let visible = usize::from(inner.height).saturating_sub(1).max(1);
        let active = app
            .session
            .tasks
            .iter()
            .position(|task| !task.done)
            .unwrap_or(total.saturating_sub(1));
        let maximum = total.saturating_sub(visible);
        let start = match app.progress_scroll {
            Some(offset) => offset.min(maximum),
            None => active
                .saturating_add(1)
                .saturating_sub(visible)
                .min(maximum),
        };
        app.progress_start = start;
        scrolled = maximum > 0;
        for (index, task) in app
            .session
            .tasks
            .iter()
            .enumerate()
            .skip(start)
            .take(visible)
        {
            let (mark, mark_color) = if task.done {
                ("✓", added())
            } else if index == active {
                ("▶", accent2())
            } else {
                ("○", accent())
            };
            lines.push(Line::from(vec![
                Span::styled(format!("{mark} "), Style::default().fg(mark_color)),
                Span::styled(
                    task.text.clone(),
                    Style::default().fg(if task.done { muted() } else { text() }),
                ),
            ]));
        }
    }

    // The title row carries a tiny progress meter: done and open boxes side
    // by side, so the ratio reads at a glance before the words.
    let meter = {
        let done_boxes = "■".repeat(done.min(6));
        let open_boxes = "□".repeat(total.min(6).saturating_sub(done));
        let mut spans = Vec::new();
        if !done_boxes.is_empty() {
            spans.push(Span::styled(done_boxes, Style::default().fg(added())));
        }
        if !open_boxes.is_empty() {
            spans.push(Span::styled(open_boxes, Style::default().fg(dim())));
        }
        if total > 6 {
            spans.push(Span::styled(
                format!(" +{}", total - 6),
                Style::default().fg(muted()),
            ));
        }
        spans
    };
    let mut title = vec![
        Span::styled(format!("{marker} "), Style::default().fg(dim())),
        Span::styled(
            "Progress ",
            Style::default().fg(accent()).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{done}/{total}"),
            Style::default().fg(text()).add_modifier(Modifier::BOLD),
        ),
    ];
    if !meter.is_empty() {
        title.push(Span::styled("  ", Style::default()));
        title.extend(meter);
    }
    title.push(Span::styled(
        if scrolled {
            " · wheel to scroll · click to fold"
        } else {
            " · click to fold"
        },
        Style::default().fg(dim()),
    ));
    lines.insert(0, Line::from(title));
    frame.render_widget(Paragraph::new(lines), inner);
}

/// The session tabs, on their own row under the header.
///
/// The row belongs to the tabs alone: the permission badge and the context
/// load stay in the footer, so a bar full of sessions never squeezes them.
pub(super) fn draw_tab_bar(frame: &mut Frame, app: &mut App, area: Rect) {
    let inner = padded(area, 2);
    if inner.width < 12 {
        return;
    }
    let sep_w: u16 = 3; // " │ " between two tabs
    let close_w: u16 = 2; // " ×" after each tab label
    let plus_w: u16 = 3; // " +" opens a tab

    let tab_count = app.tab_count();
    let markers: Vec<String> = (0..tab_count)
        .map(|index| {
            if app.tab_streaming(index) {
                format!("{LIVE_DOT} ")
            } else if app.tab_busy(index) {
                "! ".to_owned()
            } else {
                String::new()
            }
        })
        .collect();
    let labels: Vec<String> = (0..tab_count)
        .map(|index| format!("{} {}", index + 1, app.tab_title(index)))
        .collect();
    let marker_w = |index: usize| UnicodeWidthStr::width(markers[index].as_str()) as u16;
    let tab_w = |index: usize, label: &str| -> u16 {
        marker_w(index) + UnicodeWidthStr::width(label) as u16 + close_w
    };
    let seps = sep_w * tab_count.saturating_sub(1) as u16;
    let full: u16 = (0..tab_count)
        .map(|index| tab_w(index, &labels[index]))
        .sum::<u16>()
        + seps;
    let smallest: u16 = (0..tab_count).map(|index| tab_w(index, "9")).sum::<u16>() + seps;

    // The labels shrink to the tab numbers before any tab drops from the row.
    // The active tab earns twice the share, so its title stays readable.
    let active = app.tab_index();
    let mut labels = labels;
    let budget = inner.width.saturating_sub(plus_w);
    if full > budget {
        let shareable = budget.saturating_sub(smallest);
        let per_tab = if tab_count > 0 {
            usize::from(shareable / tab_count as u16)
        } else {
            0
        };
        for (index, label) in labels.iter_mut().enumerate() {
            let cells = if index == active {
                per_tab.saturating_mul(2)
            } else {
                per_tab
            };
            *label = truncate(label, cells.max(1));
        }
    }

    // Fit the tabs left to right, dropping the last ones when even the shrunk
    // labels do not fit: a `+N` count names what went. The widths add up to the
    // row, so the hit-boxes below land on real screen columns.
    let mut shown = tab_count;
    let mut used = 0u16;
    for (index, label) in labels.iter().enumerate() {
        let sep = if index > 0 { sep_w } else { 0 };
        let w = sep + tab_w(index, label);
        if used + w > budget {
            shown = index;
            break;
        }
        used += w;
    }
    let overflow = tab_count - shown;

    // Session tabs: a turning ring while a tab streams, `!` when it wants a
    // decision. Click or Alt+1..9 to switch; the active one is accent + bold.
    app.tab_hits.clear();
    app.tab_close_hits.clear();
    app.tab_new_hit = None;
    let row = inner.y;
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut cursor = inner.x;
    for index in 0..shown {
        if index > 0 {
            spans.push(Span::styled(" │ ", Style::default().fg(dim())));
            cursor += sep_w;
        }
        if !markers[index].is_empty() {
            let marker_style = if app.tab_busy(index) && !app.tab_streaming(index) {
                Style::default().fg(warning()).add_modifier(Modifier::BOLD)
            } else if app.tab_streaming(index) {
                // A streaming tab breathes instead of spinning: the tab row
                // never reflows while requests run in the background.
                breathe(app.spinner, dim(), accent())
            } else {
                Style::default().fg(accent())
            };
            spans.push(Span::styled(markers[index].clone(), marker_style));
            cursor += marker_w(index);
        }
        let style = if index == active {
            Style::default().fg(accent()).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(muted())
        };
        let label_w = UnicodeWidthStr::width(labels[index].as_str()) as u16;
        spans.push(Span::styled(labels[index].clone(), style));
        app.tab_hits.push((cursor, cursor + label_w, row, index));
        cursor += label_w;
        // `×` after each label: mouse users close that tab without switching.
        spans.push(Span::styled(" ×", Style::default().fg(dim())));
        app.tab_close_hits
            .push((cursor, cursor + close_w, row, index));
        cursor += close_w;
    }
    if shown == tab_count {
        spans.push(Span::styled(
            " +",
            Style::default().fg(accent()).add_modifier(Modifier::BOLD),
        ));
        let start = cursor;
        app.tab_new_hit = Some((start + 1, start + plus_w, row));
    } else {
        // Hidden tabs are never silent: the count says what did not fit, and
        // Alt+1..9 still reaches every one of them.
        spans.push(Span::styled(
            format!(" +{overflow}"),
            Style::default().fg(dim()),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), inner);
}

/// The billed tokens as `↑12.3k ↓4.5k`, for the footer beside the context
/// load. Empty until a provider reports usage, so no estimate poses as a
/// bill.
fn token_label(spent: Option<(u64, u64)>) -> String {
    match spent {
        Some((prompt, completion)) if prompt + completion > 0 => {
            format!("↑{} ↓{}", format_tokens(prompt), format_tokens(completion))
        }
        _ => String::new(),
    }
}

pub(super) fn draw_footer(frame: &mut Frame, app: &mut App, area: Rect) {
    let inner = padded(area, 2);
    let context = app.context_tokens();
    let window = app
        .model_context_window()
        .map(|w| w.max(context))
        .unwrap_or_else(|| DEFAULT_CONTEXT_WINDOW.max(context));
    let ctx = context_label(context, window);
    let tokens = token_label(app.display_tokens());
    let (badge, badge_style) = permission_badge(app.tools_mode);
    let badge_w = UnicodeWidthStr::width(badge) as u16;
    let gap: u16 = 3; // " · " between the three groups
    let ctx_w = UnicodeWidthStr::width(ctx.as_str()) as u16;
    let ctx_cost = if ctx.is_empty() { 0 } else { gap + ctx_w };
    let tokens_w = UnicodeWidthStr::width(tokens.as_str()) as u16;
    let tokens_cost = if tokens.is_empty() { 0 } else { gap + tokens_w };
    let mode = format!("{} {}", app.mode.icon(), app.mode.label());
    let mode_w = UnicodeWidthStr::width(mode.as_str()) as u16;
    // A goal carries its progress beside the permission badge: the checklist
    // from todo_write is the only honest measure of how close the goal is.
    let goal = app.goal_progress_label();
    let goal_cost = goal
        .as_ref()
        .map(|label| gap + UnicodeWidthStr::width(label.as_str()) as u16)
        .unwrap_or(0);
    // The mode and the badge never drop, and the token arrows outrank the goal
    // and the context load: the numbers the user is watching move first, and
    // the narrow-terminal squeeze takes the goal, then the context load.
    let fixed = mode_w + gap + badge_w;
    let keep_tokens = tokens_cost > 0 && fixed + tokens_cost <= inner.width;
    let keep_ctx =
        ctx_cost > 0 && fixed + if keep_tokens { tokens_cost } else { 0 } + ctx_cost <= inner.width;
    let keep_goal = goal_cost > 0
        && fixed
            + if keep_tokens { tokens_cost } else { 0 }
            + if keep_ctx { ctx_cost } else { 0 }
            + goal_cost
            <= inner.width;
    let right_width = (fixed
        + if keep_goal { goal_cost } else { 0 }
        + if keep_tokens { tokens_cost } else { 0 }
        + if keep_ctx { ctx_cost } else { 0 })
    .min(inner.width);
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(right_width)])
        .split(inner);

    let mut spans = vec![
        Span::styled(
            mode,
            Style::default()
                .fg(app.mode.color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · ", Style::default().fg(dim())),
        Span::styled(badge, badge_style),
    ];
    if keep_goal && let Some(goal) = goal {
        spans.push(Span::styled(" · ", Style::default().fg(dim())));
        spans.push(Span::styled(goal, Style::default().fg(accent2())));
    }
    if keep_tokens {
        spans.push(Span::styled(" · ", Style::default().fg(dim())));
        spans.push(Span::styled(tokens, Style::default().fg(muted())));
    }
    if keep_ctx {
        spans.push(Span::styled(" · ", Style::default().fg(dim())));
        spans.push(Span::styled(ctx, Style::default().fg(muted())));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), columns[1]);

    // The live row animates by light, not by a spinning glyph: the status
    // word is swept and the dot beside it breathes, so nothing on this row
    // reflows while a turn runs.
    let step = app.spinner;
    let mut live: Vec<Span<'static>> = Vec::new();
    if app.approval.is_some() {
        live.push(Span::styled(
            "approval needed · enter run · a always allow · shift+tab yolo · esc deny · v diff",
            Style::default().fg(accent()),
        ));
    } else if let Some(retry) = &app.retry {
        // A retry in flight: the wait counts down on the ticker, so a slow
        // provider neither looks like a hang nor a frozen number.
        live.extend(live_label("retrying", step, error(), warning()));
        live.push(Span::styled(
            format!(
                " in {}s · attempt {} · {}",
                retry.remaining_secs(),
                retry.attempt,
                retry.reason
            ),
            Style::default().fg(error()),
        ));
    } else if let Some(running) = &app.running_tool {
        let elapsed = app.running_tool_elapsed().unwrap_or_default();
        live.extend(live_label("running", step, accent2(), accent()));
        live.push(Span::styled(
            format!(" {} · {elapsed:.1}s", running.call.function.name),
            Style::default().fg(accent2()),
        ));
    } else if let Some(elapsed) = app.elapsed_seconds() {
        // Compaction is a request of its own: say so instead of "thinking".
        if app.compaction_active() {
            live.extend(live_label("compacting context", step, warning(), accent2()));
            live.push(Span::styled(
                format!(" · {elapsed:.1}s · esc stop"),
                Style::default().fg(warning()),
            ));
        } else if let Some(notice) = &app.notice {
            // A command typed mid-turn confirms itself here: the "thinking"
            // line would otherwise bury its notice for as long as the stream
            // runs, and the notice expires unseen.
            let color = match notice.kind {
                NoticeKind::Info => muted(),
                NoticeKind::Error => error(),
            };
            live.push(Span::styled(LIVE_DOT, breathe(step, dim(), color)));
            live.push(Span::styled(
                format!(" {} · {elapsed:.1}s · esc stop", notice.message),
                Style::default().fg(color),
            ));
        } else {
            let progress = match app.stream_tool_progress() {
                Some((name, detail, bytes)) => {
                    let size = format_bytes(bytes);
                    if detail.is_empty() {
                        format!("{name} · {size}")
                    } else {
                        format!("{name} {detail} · {size}")
                    }
                }
                None => String::new(),
            };
            if progress.is_empty() {
                live.extend(live_label("thinking", step, muted(), accent2()));
            } else {
                live.push(Span::styled(LIVE_DOT, breathe(step, dim(), accent())));
                live.extend(shimmer(&progress, step, accent2(), text_idle()));
            }
            let mut tail = format!(" · {elapsed:.1}s");
            if let Some((_, out_tokens)) = app.stream_tokens() {
                if out_tokens > 0 {
                    // The footer's ↑↓ arrows already carry the token counts;
                    // this row only adds the pace they arrive at.
                    let rate = out_tokens as f64 / elapsed.max(0.2);
                    tail.push_str(&format!(" · {rate:.0} t/s"));
                }
            }
            tail.push_str(" · esc stop");
            live.push(Span::styled(tail, Style::default().fg(accent2())));
        }
    } else if app.fetching_models {
        live.extend(live_label("fetching models", step, muted(), accent()));
        live.push(Span::styled(" · esc stop", Style::default().fg(accent())));
    } else if app.generating_image {
        // An image model takes a minute: say what is being waited on instead
        // of sitting on an idle row. There is no interrupt to offer: the
        // request is a detached task that ends when the endpoint answers.
        live.extend(live_label("generating image", step, muted(), accent()));
    } else if let Some(notice) = &app.notice {
        live.push(Span::styled(
            notice.message.clone(),
            Style::default().fg(match notice.kind {
                NoticeKind::Info => muted(),
                NoticeKind::Error => error(),
            }),
        ));
    }
    // An idle row stays empty: the mode dot and the permission badge already
    // sit in the header and the footer, and the shortcut hints live in F1
    // help. A notice or an in-flight turn still speaks here.
    frame.render_widget(Paragraph::new(Line::from(live)), columns[0]);
}

/// The color a swept status word peaks at: plain text where the theme has an
/// RGB one, the secondary accent for the themes that leave `text` to the
/// terminal, so the sweep always has two RGB ends to blend between.
fn text_idle() -> Color {
    match crate::theme::current().text {
        Color::Reset => accent2(),
        text => text,
    }
}

#[cfg(test)]
mod tests {
    use super::{context_label, header_right, token_label};
    use ratatui::style::Color;
    use ratatui::text::Span;

    /// The composer caret animates only while a keystroke is fresh: a glowing
    /// narrower bar that settles back into the steady caret.
    #[test]
    fn the_composer_caret_throbs_after_a_keystroke() {
        let rest = super::typing_caret(0, None);
        assert_eq!(rest.content, "▌");

        let live = super::typing_caret(0, Some(0));
        assert_eq!(live.content, "▌");
        assert_ne!(live.style.fg, Some(crate::theme::current().accent2));

        // Alternate ticks swap the glyph, so the caret pulses in width too.
        assert_eq!(super::typing_caret(1, Some(1)).content, "▍");

        let mut app = crate::app::test_app();
        app.typing_pulse = Some(app.spinner);
        assert_eq!(app.typing_tick(), Some(0));
        for _ in 0..crate::app::App::TYPING_TICKS {
            app.spinner = app.spinner.wrapping_add(1);
        }
        assert_eq!(app.typing_tick(), None, "the caret never settles");
    }

    #[test]
    fn the_token_label_names_both_directions() {
        assert_eq!(token_label(Some((12_300, 4_500))), "↑12.3k ↓4.5k");
        // Nothing billed yet, or a provider that reports no usage: no row.
        assert_eq!(token_label(None), "");
        assert_eq!(token_label(Some((0, 0))), "");
    }

    #[test]
    fn header_right_shows_provider_model_and_spend() {
        // Full width: everything shows.
        let wide = header_right("anthropic", "claude-sonnet-4", "$0.12", 200);
        assert_eq!(wide, "anthropic · claude-sonnet-4 · $0.12");
        // Narrow: the provider name drops first, the model and spend survive.
        let narrow = header_right("anthropic", "claude-sonnet-4", "$0.12", 20);
        assert_eq!(narrow, "claude-sonnet-4 · $0.12");
        // A provider with no price drops the spend segment: no dangling
        // separator where it used to sit.
        assert_eq!(header_right("p", "m", "", 200), "p · m");
    }

    #[test]
    fn the_signal_bars_track_the_first_token_wait() {
        assert!(super::signal_bars(None).is_empty());
        // A zero timing means nothing was timed, not a perfect connection.
        assert!(super::signal_bars(Some(0)).is_empty());

        // The meter reads left to right, shortest bar first: the number of
        // lit bars is what carries the speed.
        let bars = |ms| super::signal_bars(Some(ms));
        let lit = |spans: &[Span<'_>], color: Color| {
            spans
                .iter()
                .filter(|span| span.style.fg == Some(color))
                .count()
        };
        let text = |spans: &[Span<'_>]| {
            spans
                .iter()
                .map(|span| span.content.to_string())
                .collect::<String>()
        };
        assert_eq!(text(&bars(120)), "▁ ▃ ▅ █");
        assert_eq!(lit(&bars(120), crate::ui::added()), 4);
        assert_eq!(lit(&bars(700), crate::ui::warning()), 3);
        assert_eq!(lit(&bars(1_500), crate::ui::error()), 2);
        assert_eq!(lit(&bars(9_000), crate::ui::dim()), 3);
        // The worst level is a darker red, still short of the plain error red.
        assert!(
            bars(9_000)
                .iter()
                .all(|span| span.style.fg != Some(crate::ui::error()))
        );
    }

    #[test]
    fn the_footer_names_the_context_load_and_its_window() {
        assert_eq!(context_label(12_800, 128_000), "ctx 12.8K/128K");
        // A million-token window reads as `1M`, not five digits.
        assert_eq!(context_label(383_000, 1_000_000), "ctx 383K/1M");
        // An empty transcript shows no context row at all.
        assert_eq!(context_label(0, 128_000), "");
        // A malformed window of 0 still renders instead of panicking.
        assert_eq!(context_label(500, 0), "ctx 500/0");
    }

    /// Nine tabs with long titles must all stay visible on a terminal that
    /// cannot fit them at full width: the labels shrink to the tab numbers,
    /// no tab poofs.
    #[test]
    fn tabs_shrink_instead_of_poofing_on_a_small_terminal() {
        use ratatui::{Terminal, backend::TestBackend};

        let mut app = crate::app::test_app();
        for _ in 0..8 {
            app.open_tab();
        }
        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let row = screen_row(&terminal, 1);
        for index in 1..=9u16 {
            assert!(
                row.contains(&index.to_string()),
                "tab {index} is missing from the tab row: {row}"
            );
        }
        // Every tab keeps a click hit-box, so switching still reaches all of them.
        assert_eq!(app.tab_hits.len(), 9);
    }

    /// The context load closes the footer row: with messages in the session it
    /// sits after the permission badge, and no tab shares that row.
    #[test]
    fn the_context_load_closes_the_footer_row() {
        use crate::session::{Message, Role};
        use ratatui::{Terminal, backend::TestBackend};

        let mut app = crate::app::test_app();
        app.open_tab();
        // A few hundred characters estimate to a non-zero context load, and
        // the new tab is the active one, so the message goes in after it.
        app.session
            .messages
            .push(Message::new(Role::User, "hi".repeat(200)));
        let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let footer = screen_row(&terminal, 22);
        assert!(
            footer.contains("ctx "),
            "the context load is missing from the footer: {footer}"
        );
        // Order: badge, then the context load. The tabs moved to their own row.
        let badge = footer.find("[ON]").expect("badge");
        let ctx = footer.find("ctx ").expect("context load");
        assert!(badge < ctx, "wrong order: {footer}");
        assert!(
            !footer.contains(" │ "),
            "tabs still in the footer: {footer}"
        );
    }

    #[test]
    fn the_session_shortcut_row_sits_directly_above_the_footer() {
        use ratatui::{Terminal, backend::TestBackend};

        let mut app = crate::app::test_app();
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let footer = screen_row(&terminal, 22);
        assert!(!screen_row(&terminal, 23).contains("Alt+H sessions"));
        assert!(footer.contains("build"), "{footer}");
        assert!(footer.contains("[ON]"), "{footer}");
    }

    #[test]
    fn landing_hints_and_the_centered_session_browser_are_visible() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        use ratatui::{Terminal, backend::TestBackend};

        let mut app = crate::app::test_app();
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let landing = (0..30)
            .map(|row| screen_row(&terminal, row))
            .collect::<Vec<_>>()
            .join("\n");
        for hint in [
            "quick keys",
            "Alt+Enter",
            "Shift+Tab",
            "Ctrl+N",
            "Ctrl+G",
            "all shortcuts",
        ] {
            assert!(landing.contains(hint), "missing {hint}: {landing}");
        }

        app.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::ALT));
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let title_row = (0..30)
            .find(|row| screen_row(&terminal, *row).contains("sessions · Alt+H"))
            .expect("centered session browser title");
        assert!(
            (12..=15).contains(&title_row),
            "session browser is not centered: row {title_row}"
        );
    }

    fn screen_row(terminal: &ratatui::Terminal<ratatui::backend::TestBackend>, row: u16) -> String {
        let width = terminal.backend().buffer().area.width;
        (0..width)
            .map(|column| {
                terminal.backend().buffer()[(column, row)]
                    .symbol()
                    .to_string()
            })
            .collect()
    }

    /// The composer card: rounded corners on the outer rows, side rails, and
    /// the prompt still drawn inside the frame.
    #[test]
    fn the_composer_sits_in_a_bordered_card() {
        use ratatui::{Terminal, backend::TestBackend};

        let mut app = crate::app::test_app();
        app.input.set("hello");
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let rows = (0..24)
            .map(|row| screen_row(&terminal, row))
            .collect::<Vec<_>>();
        let card = rows
            .iter()
            .position(|row| row.starts_with("╭") && row.ends_with("╮"))
            .unwrap_or_else(|| panic!("no card top: {rows:#?}"));
        assert!(rows[card + 1].contains("hello"), "{}", rows[card + 1]);
        assert!(rows[card + 1].starts_with('│') && rows[card + 1].ends_with('│'));
        assert!(
            rows[card + 2].starts_with("╰") && rows[card + 2].ends_with("╯"),
            "{}",
            rows[card + 2]
        );
        // The click map covers the framed text rows, not the border rows.
        let area = app.composer_area.expect("composer area");
        assert_eq!(u16::from(area.y), card as u16 + 1);
    }

    #[test]
    fn the_backdrop_paints_empty_cells_without_hiding_text() {
        use ratatui::{Terminal, backend::TestBackend, style::Color};

        let mut app = crate::app::test_app();
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let cell = &terminal.backend().buffer()[(0, 0)];
        assert_ne!(cell.bg, Color::Reset, "backdrop was not drawn");
        let landing = (0..24)
            .map(|row| screen_row(&terminal, row))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(landing.contains("quick keys"), "{landing}");
    }

    /// A checklist longer than the panel follows the first unfinished task,
    /// and an explicit wheel offset scrolls it back up.
    #[test]
    fn the_progress_panel_follows_the_current_task_and_scrolls() {
        use crate::session::TaskState;
        use ratatui::{Terminal, backend::TestBackend};

        let mut app = crate::app::test_app();
        app.session.tasks = (0..10)
            .map(|index| TaskState {
                text: format!("task-{index}"),
                done: index < 5,
            })
            .collect();
        let mut terminal = Terminal::new(TestBackend::new(60, 5)).unwrap();
        let text = |terminal: &Terminal<TestBackend>| {
            (0..5)
                .map(|row| screen_row(terminal, row))
                .collect::<Vec<_>>()
                .join("\n")
        };
        terminal
            .draw(|frame| super::draw_progress(frame, &mut app, frame.area()))
            .unwrap();
        let followed = text(&terminal);
        assert!(
            followed.contains("task-5"),
            "current task hidden: {followed}"
        );
        assert!(
            !followed.contains("task-0"),
            "finished work crowds out the current task: {followed}"
        );
        assert!(followed.contains("Progress 5/10"), "{followed}");

        app.progress_scroll = Some(0);
        terminal
            .draw(|frame| super::draw_progress(frame, &mut app, frame.area()))
            .unwrap();
        let scrolled = text(&terminal);
        assert!(
            scrolled.contains("task-0"),
            "the wheel cannot scroll up: {scrolled}"
        );
        assert!(scrolled.contains("wheel to scroll"), "{scrolled}");
    }

    /// The header keeps only the mode dot and the provider line: the
    /// permission badge lives in the footer, the tabs on their own row.
    #[test]
    fn the_header_holds_no_badge_and_no_tabs() {
        use ratatui::{Terminal, backend::TestBackend};

        let mut app = crate::app::test_app();
        app.open_tab();
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let header = screen_row(&terminal, 0);
        let tabs = screen_row(&terminal, 1);
        let footer = screen_row(&terminal, 22);
        assert!(!header.contains("[ON]"), "badge in header: {header}");
        assert!(!header.contains(" │ "), "tab separator in header: {header}");
        assert!(!header.contains("build"), "mode in header: {header}");
        assert!(tabs.contains(" │ "), "no tabs on the tab row: {tabs}");
        assert!(footer.contains("build"), "no mode in footer: {footer}");
        assert!(footer.contains("[ON]"), "no badge in footer: {footer}");
        assert!(
            !footer.contains(" │ "),
            "tabs still in the footer: {footer}"
        );
    }
}
