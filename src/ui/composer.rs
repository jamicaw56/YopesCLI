use super::*;

pub(super) fn draw_header(frame: &mut Frame, app: &mut App, area: Rect) {
    let inner = padded(area, 2);
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(9), Constraint::Min(1)])
        .split(inner);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("◆", Style::default().fg(accent2())),
            Span::styled(
                " yopes",
                Style::default().fg(accent2()).add_modifier(Modifier::BOLD),
            ),
        ])),
        columns[0],
    );

    // The header carries the passive provider line. The mode, the permission
    // badge and the context load sit together in the footer.
    let right = header_right(
        app.backend.provider_label(),
        app.backend.model(),
        &app.spent_cost(),
        columns[1].width as usize,
    );
    if !right.is_empty() {
        frame.render_widget(
            Paragraph::new(right)
                .alignment(Alignment::Right)
                .style(Style::default().fg(muted())),
            columns[1],
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
    format!("ctx {}/{}", format_tokens(context), format_tokens(window))
}

pub(super) fn draw_separator(frame: &mut Frame, area: Rect) {
    let inner = padded(area, 2);
    if inner.width == 0 {
        return;
    }
    // The divider fades from the secondary accent into dim across its width,
    // so the composer edge reads as one soft band instead of a hard rule.
    let (accent2, dim) = {
        let theme = crate::theme::current();
        (theme.accent2, theme.dim)
    };
    let left = blend(accent2, dim, 0);
    let mid = blend(accent2, dim, 50);
    let right = dim;
    let width = usize::from(inner.width);
    let mut spans = Vec::with_capacity(3);
    let third = width / 3;
    for (index, (color, count)) in [(left, third), (mid, third), (right, width - 2 * third)]
        .into_iter()
        .enumerate()
    {
        if index == 2 && count == 0 {
            // A narrow terminal splits into thirds with nothing left over.
            continue;
        }
        spans.push(Span::styled("─".repeat(count), Style::default().fg(color)));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), inner);
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

pub(super) fn composer_height(app: &App, width: u16, height: u16) -> u16 {
    let content_width = width.saturating_sub(6).max(1);
    let lines = wrapped_line_count(&app.input.with_cursor(), content_width);
    // A short terminal keeps its transcript rows: the composer shrinks first.
    let max = (height.saturating_sub(6) / 2).clamp(1, 6);
    (lines as u16).clamp(1, max)
}

pub(super) fn draw_composer(frame: &mut Frame, app: &mut App, area: Rect) {
    let inner = padded(area, 2);
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
        Paragraph::new(render_composer_text(&app.input))
            .wrap(Wrap { trim: false })
            .scroll((app.input_scroll.min(u16::MAX as usize) as u16, 0)),
        target,
    );
}

fn render_composer_text(input: &Input) -> Text<'static> {
    let sel = input.selection();
    let sel_style = Style::default().add_modifier(Modifier::REVERSED);
    let norm_style = Style::default().fg(text());
    let cursor_style = Style::default().fg(crate::theme::current().accent2);
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
            Seg::Cursor => spans.push(Span::styled("▌", cursor_style)),
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
                format!("{} ", spinner(app.spinner))
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

    let (text, color) = if app.approval.is_some() {
        (
            "approval needed · enter run · a always allow · shift+tab yolo · esc deny · v diff"
                .to_owned(),
            accent(),
        )
    } else if let Some(retry) = &app.retry {
        // A retry in flight: the wait is shown so a slow provider does not
        // look like a hang.
        (
            format!(
                "retrying in {}s · attempt {} · {}",
                retry.delay_secs, retry.attempt, retry.reason
            ),
            error(),
        )
    } else if let Some(running) = &app.running_tool {
        let elapsed = app.running_tool_elapsed().unwrap_or_default();
        (
            format!(
                "{} running {} · {elapsed:.1}s",
                spinner(app.spinner),
                running.call.function.name
            ),
            accent2(),
        )
    } else if let Some(elapsed) = app.elapsed_seconds() {
        // Compaction is a request of its own: say so instead of "thinking".
        if app.compaction_active() {
            (
                format!(
                    "{} compacting context · {elapsed:.1}s · esc stop",
                    spinner(app.spinner)
                ),
                warning(),
            )
        } else if let Some(notice) = &app.notice {
            // A command typed mid-turn confirms itself here: the "thinking"
            // line would otherwise bury its notice for as long as the stream
            // runs, and the notice expires unseen.
            (
                format!(
                    "{} {} · {elapsed:.1}s · esc stop",
                    spinner(app.spinner),
                    notice.message
                ),
                match notice.kind {
                    NoticeKind::Info => muted(),
                    NoticeKind::Error => error(),
                },
            )
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
                None => "thinking".to_owned(),
            };
            let mut text = format!("{} {progress} · {elapsed:.1}s", spinner(app.spinner));
            if let Some((_, out_tokens)) = app.stream_tokens() {
                if out_tokens > 0 {
                    // The footer's ↑↓ arrows already carry the token counts;
                    // this row only adds the pace they arrive at.
                    let rate = out_tokens as f64 / elapsed.max(0.2);
                    text.push_str(&format!(" · {rate:.0} t/s"));
                }
            }
            text.push_str(" · esc stop");
            (text, accent2())
        }
    } else if app.fetching_models {
        (
            format!("{} fetching models · esc stop", spinner(app.spinner)),
            accent(),
        )
    } else if let Some(notice) = &app.notice {
        (
            notice.message.clone(),
            match notice.kind {
                NoticeKind::Info => muted(),
                NoticeKind::Error => error(),
            },
        )
    } else {
        // The idle row stays quiet: the mode dot and the permission badge
        // already sit in the header and the footer, and the shortcut hints
        // live in F1 help. A notice or an in-flight turn still speaks here.
        (String::new(), muted())
    };
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(color)),
        columns[0],
    );
}

#[cfg(test)]
mod tests {
    use super::{context_label, header_right, token_label};

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
    fn the_footer_names_the_context_load_and_its_window() {
        assert_eq!(context_label(12_800, 128_000), "ctx 12.8k/128.0k");
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
