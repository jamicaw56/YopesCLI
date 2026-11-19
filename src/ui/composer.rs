use super::*;

pub(super) fn draw_header(frame: &mut Frame, app: &mut App, area: Rect) {
    let inner = padded(area, 2);
    let context = app.context_tokens();
    let window = app
        .model_context_window()
        .map(|w| w.max(context))
        .unwrap_or_else(|| DEFAULT_CONTEXT_WINDOW.max(context));

    // No product branding in the working view. The permission badge and the
    // session tabs live in the footer's right column.
    let mode_style = Style::default()
        .fg(app.mode.color())
        .add_modifier(Modifier::BOLD);
    let title_spans = vec![
        Span::styled("◆", mode_style),
        Span::styled(format!(" {}", app.mode.label()), mode_style),
    ];
    let title_w = title_spans
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum::<usize>() as u16;
    let right_budget = inner.width.saturating_sub(title_w.saturating_add(2));
    let right = header_right(
        app.backend.provider_label(),
        app.backend.model(),
        &app.spent_cost(),
        context,
        window,
        right_budget as usize,
    );
    let right_width = (UnicodeWidthStr::width(right.as_str()) as u16).min(right_budget);
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(4), Constraint::Length(right_width)])
        .split(inner);
    frame.render_widget(Paragraph::new(Line::from(title_spans)), columns[0]);

    if right_width > 0 {
        frame.render_widget(
            Paragraph::new(right)
                .alignment(Alignment::Right)
                .style(Style::default().fg(muted())),
            columns[1],
        );
    }
}

/// The permission badge for the footer; `On` is the default and shows none.
fn permission_badge(mode: ToolsMode) -> Option<Span<'static>> {
    let bold = |color| Style::default().fg(color).add_modifier(Modifier::BOLD);
    match mode {
        ToolsMode::Auto => Some(Span::styled("[YOLO]", bold(warning()))),
        ToolsMode::Ask => Some(Span::styled("[ASK]", Style::default().fg(dim()))),
        ToolsMode::Off => Some(Span::styled("[OFF]", Style::default().fg(dim()))),
        ToolsMode::Dangerous => Some(Span::styled("[DANGER]", bold(error()))),
        ToolsMode::On => None,
    }
}

/// Lays the session tabs out from column 0 within `avail` cells and returns
/// the spans with their width. Hit-boxes land in `app` relative to column 0;
/// the caller shifts them once the row's screen position is known.
fn tab_row(app: &mut App, avail: u16, row: u16) -> (Vec<Span<'static>>, u16) {
    app.tab_hits.clear();
    app.tab_close_hits.clear();
    app.tab_new_hit = None;
    let mut spans: Vec<Span<'static>> = Vec::new();
    let count = app.tab_count();
    let markers: Vec<String> = (0..count)
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
    let tab_width = |index: usize, label: &str| -> u16 {
        let sep_w: u16 = if index > 0 { 3 } else { 0 };
        let marker_w: u16 = if markers[index].is_empty() { 0 } else { 2 };
        sep_w + marker_w + UnicodeWidthStr::width(label) as u16 + 2 // " ×"
    };
    let active = app.tab_index();
    let labels: Vec<String> = (0..count)
        .map(|index| format!("{} {}", index + 1, app.tab_title(index)))
        .collect();
    let full: u16 = (0..count)
        .map(|index| tab_width(index, &labels[index]))
        .sum();
    // The smallest useful form is the tab number alone.
    let smallest: u16 = (0..count).map(|index| tab_width(index, "9")).sum();
    // The `+` button's columns are reserved up front, so a full row of tabs
    // never overflows into a "+N" count the budget promised to keep.
    let plus_w: u16 = 2; // " +"
    let tabs_full = full.saturating_add(plus_w);
    let mut x: u16 = 0;
    // Full labels first; when they do not fit, every label shrinks to an
    // equal share so the whole stack stays visible. The active tab earns
    // twice the share: its title is the one worth reading on a cramped row.
    let mut budget: Option<usize> = None;
    if tabs_full > avail {
        let shareable = avail.saturating_sub(plus_w);
        if count > 0 && shareable >= smallest {
            budget = Some(usize::from((shareable - smallest) / count as u16) + 1);
        }
    }
    let mut overflow = 0usize;
    for index in 0..count {
        let sep_w: u16 = if index > 0 { 3 } else { 0 };
        let marker = markers[index].clone();
        let marker_w: u16 = if marker.is_empty() { 0 } else { 2 };
        let close_w: u16 = 2; // " ×"
        let label = match budget {
            Some(share) => {
                let cells = if index == active {
                    share.saturating_mul(2)
                } else {
                    share
                };
                truncate(&labels[index], cells)
            }
            None => labels[index].clone(),
        };
        let label_w = UnicodeWidthStr::width(label.as_str()) as u16;
        if x.saturating_add(sep_w + marker_w + label_w + close_w) > avail {
            overflow = count - index;
            break;
        }
        if index > 0 {
            spans.push(Span::styled(" │ ", Style::default().fg(dim())));
            x = x.saturating_add(sep_w);
        }
        if !marker.is_empty() {
            let marker_style = if app.tab_busy(index) && !app.tab_streaming(index) {
                Style::default().fg(warning()).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(accent())
            };
            spans.push(Span::styled(marker, marker_style));
            x = x.saturating_add(marker_w);
        }
        let style = if index == active {
            Style::default().fg(accent()).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(muted())
        };
        spans.push(Span::styled(label, style));
        x = x.saturating_add(label_w);
        let start = x.saturating_sub(label_w + marker_w);
        app.tab_hits.push((start, x, row, index));
        // `×` after each label: mouse users close that tab without switching.
        spans.push(Span::styled(" ×", Style::default().fg(dim())));
        x = x.saturating_add(close_w);
        app.tab_close_hits.push((x.saturating_sub(close_w), x, row, index));
    }
    if overflow > 0 {
        // Hidden tabs are never silent: the marker counts what did not fit,
        // and Alt+1..9 still reaches every one of them.
        let more = format!(" +{overflow}");
        x = x.saturating_add(UnicodeWidthStr::width(more.as_str()) as u16);
        spans.push(Span::styled(more, Style::default().fg(dim())));
    }
    // `+` after the last tab: opens one. A click past the limit is told so.
    if x.saturating_add(plus_w) <= avail {
        spans.push(Span::styled(
            " +",
            Style::default().fg(accent()).add_modifier(Modifier::BOLD),
        ));
        x = x.saturating_add(plus_w);
        app.tab_new_hit = Some((x - 1, x, row));
    }
    (spans, x)
}

/// The header's right side: provider, model, spend and the context load as
/// `ctx used/window`.
///
/// A narrow terminal drops the least useful segments from the front — model
/// name first — so the context load survives instead of being cut mid-string.
fn header_right(
    provider: &str,
    model: &str,
    cost: &str,
    context: u64,
    window: u64,
    budget: usize,
) -> String {
    let mut segments: Vec<String> = Vec::new();
    if !provider.is_empty() {
        segments.push(provider.to_owned());
    }
    if !model.is_empty() {
        segments.push(model.to_owned());
    }
    // A provider that bills nothing it can price sends an empty cost: the
    // segment is dropped, so no `—` placeholder reaches the row.
    if !cost.is_empty() {
        segments.push(cost.to_owned());
    }
    if context > 0 {
        segments.push(format!(
            "ctx {}/{}",
            format_tokens(context),
            format_tokens(window)
        ));
    }
    let joined = |segments: &[String]| segments.join(" · ");
    while segments.len() > 2 && UnicodeWidthStr::width(joined(&segments).as_str()) > budget {
        segments.remove(0);
    }
    joined(&segments)
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
pub(super) fn draw_progress(frame: &mut Frame, app: &mut App, area: Rect) {
    if area.width < 20 {
        return;
    }
    app.progress_row = Some(area.y);
    let inner = padded(area, 2);
    let marker = if app.progress_collapsed { "▸" } else { "▾" };
    let total = app.session.tasks.len();

    let mut lines: Vec<Line<'static>> = Vec::new();
    let done = if app.progress_collapsed {
        app.session.tasks.iter().filter(|task| task.done).count()
    } else {
        if area.height < 3 {
            return;
        }
        // One pass counts completion for every task and renders the visible
        // rows, so the header never costs a second scan. Done tasks take the
        // added color: a checklist that scans as green-first reads its own
        // progress without counting checkboxes.
        let mut counted = 0usize;
        for (index, task) in app.session.tasks.iter().enumerate() {
            counted += usize::from(task.done);
            match index {
                0..=5 => {
                    let (mark, mark_color) = if task.done {
                        ("✓", added())
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
                6 => lines.push(Line::styled(
                    format!("… {} more", total - 6),
                    Style::default().fg(muted()),
                )),
                _ => {}
            }
        }
        counted
    };

    // The title row carries a tiny progress meter: done and open boxes side
    // by side, so the ratio reads at a glance before the words.
    let meter = {
        let done_boxes = "■".repeat(done.min(6));
        let open_boxes = "□".repeat(total.min(6).saturating_sub(done));
        let mut spans = Vec::new();
        if !done_boxes.is_empty() {
            spans.push(Span::styled(
                done_boxes.clone(),
                Style::default().fg(added()),
            ));
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
    title.push(Span::styled(" · click to fold", Style::default().fg(dim())));
    lines.insert(0, Line::from(title));
    frame.render_widget(Paragraph::new(lines), inner);
}

pub(super) fn draw_footer(frame: &mut Frame, app: &mut App, area: Rect) {
    let inner = padded(area, 2);
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
            if let Some((in_tokens, out_tokens)) = app.stream_tokens() {
                text.push_str(&format!(" · in {}", format_tokens(in_tokens)));
                if out_tokens > 0 {
                    let rate = out_tokens as f64 / elapsed.max(0.2);
                    text.push_str(&format!(
                        " · out {} · {rate:.0} t/s",
                        format_tokens(out_tokens)
                    ));
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
        // The idle row stays quiet: the shortcut hints live in F1 help. A
        // notice or an in-flight turn still speaks here.
        (String::new(), muted())
    };
    // Right column: permission badge, then the session tabs. They are the
    // only place either lives, so they always show and the status text on
    // the left is the one that truncates. A busy row still keeps a quarter
    // of the width for the status.
    let badge = permission_badge(app.tools_mode);
    let badge_w = badge
        .as_ref()
        .map_or(0, |span| UnicodeWidthStr::width(span.content.as_ref()) as u16 + 1);
    let reserve_left = if text.is_empty() { 0 } else { inner.width / 4 };
    let tabs_avail = inner
        .width
        .saturating_sub(badge_w)
        .saturating_sub(reserve_left);
    let (tab_spans, tabs_w) = tab_row(app, tabs_avail, area.y);
    let right_w = (badge_w + tabs_w).min(inner.width);
    let right_x = inner.x + inner.width - right_w;
    // Hit-boxes were laid out from column 0: move them onto the screen.
    let shift = right_x + badge_w;
    for hit in app.tab_hits.iter_mut().chain(app.tab_close_hits.iter_mut()) {
        hit.0 += shift;
        hit.1 += shift;
    }
    if let Some(hit) = app.tab_new_hit.as_mut() {
        hit.0 += shift;
        hit.1 += shift;
    }
    let mut right_spans = Vec::with_capacity(tab_spans.len() + 2);
    if let Some(badge) = badge {
        right_spans.push(badge);
        right_spans.push(Span::raw(" "));
    }
    right_spans.extend(tab_spans);

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(right_w)])
        .split(inner);
    // One column of air between the status and the tabs.
    let left = Rect {
        width: columns[0].width.saturating_sub(1),
        ..columns[0]
    };
    frame.render_widget(
        Paragraph::new(truncate(&text, usize::from(left.width))).style(Style::default().fg(color)),
        left,
    );
    frame.render_widget(Paragraph::new(Line::from(right_spans)), columns[1]);
}

#[cfg(test)]
mod tests {
    use super::header_right;

    #[test]
    fn header_right_shows_the_context_load_and_drops_segments_when_narrow() {
        // Full width: everything shows, with the window.
        let wide = header_right(
            "anthropic",
            "claude-sonnet-4",
            "$0.12",
            12_800,
            128_000,
            200,
        );
        assert_eq!(
            wide,
            "anthropic · claude-sonnet-4 · $0.12 · ctx 12.8k/128.0k"
        );
        // Narrow: provider and model drop, the spend and context load survive.
        let narrow = header_right("anthropic", "claude-sonnet-4", "$0.12", 12_800, 128_000, 30);
        assert_eq!(narrow, "$0.12 · ctx 12.8k/128.0k");
        // A malformed window of 0 renders without panicking, and a provider
        // with no price drops the spend segment: no `—` placeholder, and no
        // dangling separator where it used to sit.
        assert_eq!(header_right("p", "m", "", 500, 0, 200), "p · m · ctx 500/0");
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
        let row: String = (0..120)
            .map(|column| {
                terminal.backend().buffer()[(column, 23)]
                    .symbol()
                    .to_string()
            })
            .collect();
        for index in 1..=9u16 {
            assert!(
                row.contains(&index.to_string()),
                "tab {index} is missing from the footer: {row}"
            );
        }
        // Every tab keeps a click hit-box, so switching still reaches all of them.
        assert_eq!(app.tab_hits.len(), 9);
    }
}
