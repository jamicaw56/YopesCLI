use super::*;

pub(super) fn draw_header(frame: &mut Frame, app: &mut App, area: Rect) {
    let inner = padded(area, 2);

    // The header carries the mode and the passive provider line. The
    // permission badge and the session tabs live in the footer's right side,
    // next to the context load, so the tab bar sits where the eye already
    // reads state and every clickable target shares one row.
    let title_spans = vec![
        Span::styled(
            "◆",
            Style::default()
                .fg(app.mode.color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" {}", app.mode.label()),
            Style::default()
                .fg(app.mode.color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ", Style::default()),
    ];
    let title_w = title_spans
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum::<usize>() as u16;
    let available = inner.width.saturating_sub(title_w);
    let right = header_right(
        app.backend.provider_label(),
        app.backend.model(),
        &app.spent_cost(),
        available as usize,
    );
    let right_width = (UnicodeWidthStr::width(right.as_str()) as u16).min(available);
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
    format!(
        "ctx {}/{}",
        format_tokens(context),
        format_tokens(window)
    )
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
    let context = app.context_tokens();
    let window = app
        .model_context_window()
        .map(|w| w.max(context))
        .unwrap_or_else(|| DEFAULT_CONTEXT_WINDOW.max(context));
    let ctx = context_label(context, window);
    let (badge, badge_style) = permission_badge(app.tools_mode);
    let badge_w = UnicodeWidthStr::width(badge) as u16;
    let gap: u16 = 3; // " · " between the three groups
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
    let full: u16 = (0..tab_count).map(|index| tab_w(index, &labels[index])).sum::<u16>() + seps;
    let smallest: u16 = (0..tab_count).map(|index| tab_w(index, "9")).sum::<u16>() + seps;

    // The badge and the tabs never drop; the context load is the segment that
    // yields on a narrow terminal, and the tab labels shrink before any tab
    // disappears from the row.
    let ctx_w = UnicodeWidthStr::width(ctx.as_str()) as u16;
    let ctx_cost = if ctx.is_empty() { 0 } else { gap + ctx_w };
    let room = inner.width.saturating_sub(badge_w + gap + plus_w);
    let keep_ctx = ctx_cost > 0 && full + ctx_cost <= room;
    let budget = room.saturating_sub(if keep_ctx { ctx_cost } else { 0 });
    // The active tab earns twice the share, so its title stays readable.
    let active = app.tab_index();
    let mut labels = labels;
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
    // right column, so the hit-boxes below land on real screen columns.
    let mut shown = tab_count;
    let mut used = 0u16;
    for index in 0..tab_count {
        let sep = if index > 0 { sep_w } else { 0 };
        let w = sep + tab_w(index, &labels[index]);
        if used + w > budget {
            shown = index;
            break;
        }
        used += w;
    }
    let overflow = tab_count - shown;
    if overflow == 0 {
        used += plus_w;
    } else {
        // The `+N` marker takes the columns the dropped tabs would have used.
        used += UnicodeWidthStr::width(format!(" +{overflow}").as_str()) as u16;
    }
    if keep_ctx {
        used += ctx_cost;
    }
    let right_width = (badge_w + gap + used).min(inner.width);
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(right_width)])
        .split(inner);

    // Session tabs: a turning ring while a tab streams, `!` when it wants a
    // decision. Click or Alt+1..9 to switch; the active one is accent + bold.
    app.tab_hits.clear();
    app.tab_close_hits.clear();
    app.tab_new_hit = None;
    let row = area.y;
    let mut spans = vec![
        Span::styled(badge, badge_style),
        Span::styled(" · ", Style::default().fg(dim())),
    ];
    let mut cursor = columns[1].x + badge_w + gap;
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
        app.tab_close_hits.push((cursor, cursor + close_w, row, index));
        cursor += close_w;
    }
    if shown == tab_count {
        spans.push(Span::styled(
            " +",
            Style::default().fg(accent()).add_modifier(Modifier::BOLD),
        ));
        let start = cursor;
        cursor += plus_w;
        app.tab_new_hit = Some((start + 1, start + plus_w, row));
    } else if overflow > 0 {
        // Hidden tabs are never silent: the count says what did not fit, and
        // Alt+1..9 still reaches every one of them.
        let count = format!(" +{overflow}");
        spans.push(Span::styled(count.clone(), Style::default().fg(dim())));
        cursor += UnicodeWidthStr::width(count.as_str()) as u16;
    }
    if keep_ctx {
        spans.push(Span::styled(" · ", Style::default().fg(dim())));
        spans.push(Span::styled(ctx, Style::default().fg(muted())));
        cursor += gap;
    }
    let _ = cursor;
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
    use super::{context_label, header_right};

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
        let row = screen_row(&terminal, 23);
        for index in 1..=9u16 {
            assert!(
                row.contains(&index.to_string()),
                "tab {index} is missing from the footer: {row}"
            );
        }
        // Every tab keeps a click hit-box, so switching still reaches all of them.
        assert_eq!(app.tab_hits.len(), 9);
        // The permission badge shares the row, and the context load sits at
        // its right end.
        assert!(row.contains("[ON]"), "the badge is missing: {row}");
    }

    /// The context load closes the footer row: with messages in the session it
    /// is the rightmost segment, after the badge and the tabs.
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
        let footer = screen_row(&terminal, 23);
        let trimmed = footer.trim_end();
        assert!(
            trimmed.ends_with("ctx") || trimmed.contains("ctx "),
            "the context load is missing from the footer: {footer}"
        );
        // Order: badge, then tabs, then the context load.
        let badge = footer.find("[ON]").expect("badge");
        let tabs = footer.find(" │ ").expect("tabs");
        let ctx = footer.find("ctx ").expect("context load");
        assert!(badge < tabs && tabs < ctx, "wrong order: {footer}");
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

    /// The header keeps only the mode dot and the provider line: the
    /// permission badge and the tabs moved to the footer.
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
        let footer = screen_row(&terminal, 23);
        assert!(!header.contains("[ON]"), "badge in header: {header}");
        assert!(
            !header.contains(" │ "),
            "tab separator in header: {header}"
        );
        assert!(header.contains("build"), "mode dot in header: {header}");
        assert!(footer.contains("[ON]"), "no badge in footer: {footer}");
        assert!(footer.contains(" │ "), "no tabs in footer: {footer}");
    }
}
