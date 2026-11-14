use super::*;

pub(super) fn draw_header(frame: &mut Frame, app: &mut App, area: Rect) {
    let inner = padded(area, 2);
    let mut right = if app.backend.model().is_empty() {
        format!("{} · {}", app.backend.provider_label(), app.spent_cost())
    } else {
        format!(
            "{} · {} · {}",
            app.backend.provider_label(),
            app.backend.model(),
            app.spent_cost()
        )
    };
    if app.context_tokens() > 0 {
        right.push_str(&format!(" · ctx {}", format_tokens(app.context_tokens())));
    }
    let right_width = (UnicodeWidthStr::width(right.as_str()) as u16).min(inner.width / 2);
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(4), Constraint::Length(right_width)])
        .split(inner);

    // No product branding in the working view: the mode dot, the permission
    // badge and the session tabs are what need the space.
    let mut title_spans = vec![
        Span::styled(
            "●",
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
    ];
    match app.tools_mode {
        ToolsMode::Auto => {
            title_spans.push(Span::styled(
                " [YOLO]",
                Style::default().fg(warning()).add_modifier(Modifier::BOLD),
            ));
        }
        ToolsMode::Ask => {
            title_spans.push(Span::styled(" [ASK]", Style::default().fg(muted())));
        }
        ToolsMode::Off => {
            title_spans.push(Span::styled(" [OFF]", Style::default().fg(muted())));
        }
        ToolsMode::On => {}
    }
    title_spans.push(Span::styled("  ", Style::default()));

    // Session tabs: `◐` while a tab streams, `!` when it wants a decision.
    // Click or Alt+1..9 to switch; the active one is accent + bold.
    app.tab_hits.clear();
    let active = app.tab_index();
    let mut x = inner.x
        + title_spans
            .iter()
            .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
            .sum::<usize>() as u16;
    let end_x = columns[0].x + columns[0].width;
    for index in 0..app.tab_count() {
        let streaming = app.tab_streaming(index);
        let busy = app.tab_busy(index);
        let marker = if streaming {
            "◐ "
        } else if busy {
            "! "
        } else {
            ""
        };
        let label = format!("{} {}", index + 1, app.tab_title(index));
        let sep_w: u16 = if index > 0 { 3 } else { 0 };
        let marker_w: u16 = if marker.is_empty() { 0 } else { 2 };
        let label_w = UnicodeWidthStr::width(label.as_str()) as u16;
        if x.saturating_add(sep_w + marker_w + label_w) > end_x {
            break;
        }
        if index > 0 {
            title_spans.push(Span::styled(" │ ", Style::default().fg(muted())));
            x = x.saturating_add(sep_w);
        }
        if !marker.is_empty() {
            let marker_style = if busy && !streaming {
                Style::default().fg(warning()).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(accent())
            };
            title_spans.push(Span::styled(marker, marker_style));
            x = x.saturating_add(marker_w);
        }
        let style = if index == active {
            Style::default().fg(accent()).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(muted())
        };
        title_spans.push(Span::styled(label, style));
        x = x.saturating_add(label_w);
        let start = x.saturating_sub(label_w + marker_w);
        app.tab_hits.push((start, x, index));
    }
    let title = Line::from(title_spans);
    frame.render_widget(Paragraph::new(title), columns[0]);

    if right_width > 0 {
        frame.render_widget(
            Paragraph::new(right)
                .alignment(Alignment::Right)
                .style(Style::default().fg(muted())),
            columns[1],
        );
    }
}

pub(super) fn draw_separator(frame: &mut Frame, area: Rect) {
    let inner = padded(area, 2);
    if inner.width == 0 {
        return;
    }
    frame.render_widget(
        Paragraph::new("─".repeat(usize::from(inner.width))).style(Style::default().fg(muted())),
        inner,
    );
}

pub(super) fn composer_height(app: &App, width: u16) -> u16 {
    let content_width = width.saturating_sub(6).max(1);
    let lines = wrapped_line_count(&app.input.with_cursor(), content_width);
    (lines as u16).clamp(1, 6)
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
            "⚡",
            Style::default().fg(warning()).add_modifier(Modifier::BOLD),
        )
    } else if pulse {
        (
            "›",
            Style::default().fg(accent()).add_modifier(Modifier::BOLD),
        )
    } else if app.is_busy() {
        ("›", Style::default().fg(muted()))
    } else {
        (
            "›",
            Style::default().fg(accent()).add_modifier(Modifier::BOLD),
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
    let cursor_style = Style::default().fg(accent());
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
        // rows, so the header never costs a second scan.
        let mut counted = 0usize;
        for (index, task) in app.session.tasks.iter().enumerate() {
            counted += usize::from(task.done);
            match index {
                0..=5 => {
                    let mark = if task.done { "[x]" } else { "[ ]" };
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("{mark} "),
                            Style::default().fg(if task.done { muted() } else { accent() }),
                        ),
                        Span::styled(task.text.clone(), Style::default().fg(text())),
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

    lines.insert(
        0,
        Line::from(vec![
            Span::styled(format!("{marker} "), Style::default().fg(muted())),
            Span::styled(
                format!("Progress {done}/{total}"),
                Style::default().fg(accent()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" · todo_write · click to fold", Style::default().fg(muted())),
        ]),
    );
    frame.render_widget(Paragraph::new(lines), inner);
}

pub(super) fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let inner = padded(area, 2);
    let count = app.session.messages.len();
    let right = if count == 1 {
        "1 message".to_owned()
    } else {
        format!("{count} messages")
    };
    let right_width =
        if inner.width >= 60 && count > 0 && app.notice.is_none() && !app.is_streaming() {
            (right.chars().count() as u16).min(inner.width / 3)
        } else {
            0
        };
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(8), Constraint::Length(right_width)])
        .split(inner);

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
                SPINNER[app.spinner % SPINNER.len()],
                running.call.function.name
            ),
            accent(),
        )
    } else if let Some(elapsed) = app.elapsed_seconds() {
        // Compaction is a request of its own: say so instead of "thinking".
        if app.compaction_active() {
            (
                format!(
                    "{} compacting context · {elapsed:.1}s · esc stop",
                    SPINNER[app.spinner % SPINNER.len()]
                ),
                accent(),
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
            let mut text = format!(
                "{} {progress} · {elapsed:.1}s",
                SPINNER[app.spinner % SPINNER.len()]
            );
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
            (text, accent())
        }
    } else if app.fetching_models {
        (
            format!(
                "{} fetching models · esc stop",
                SPINNER[app.spinner % SPINNER.len()]
            ),
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
        // The mode is the idle status's first word: Tab cycles build · plan · chat.
        let text = if app.tools_mode == ToolsMode::Auto {
            format!(
                "mode {} · [yolo] · shift+tab permissions · ctrl+t tabs · enter send · / commands",
                app.mode.label()
            )
        } else {
            format!(
                "mode {} · shift+tab yolo · ctrl+t tabs · enter send · / commands · F1 help",
                app.mode.label()
            )
        };
        let color = match app.mode {
            Mode::Build if app.tools_mode == ToolsMode::Auto => warning(),
            Mode::Build => muted(),
            _ => accent(),
        };
        (text, color)
    };
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(color)),
        columns[0],
    );

    if right_width > 0 {
        frame.render_widget(
            Paragraph::new(right)
                .alignment(Alignment::Right)
                .style(Style::default().fg(muted())),
            columns[1],
        );
    }
}
