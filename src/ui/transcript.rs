use super::markdown::markdown_lines;
use super::*;

pub(super) fn draw_transcript(frame: &mut Frame, app: &mut App, area: Rect) {
    app.output_rows.clear();
    let inner = Block::default()
        .padding(Padding::new(2, 2, 1, 1))
        .inner(area);
    app.transcript_height = inner.height;
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    if app.session.messages.is_empty() {
        // The welcome names the model; the working view carries no branding.
        let model = app.backend.model();
        let heading = if model.is_empty() {
            app.backend.provider_label().to_owned()
        } else {
            model.to_owned()
        };
        let mut welcome = vec![
            Line::styled(
                heading,
                Style::default().fg(accent()).add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
        ];
        // The mascot belongs to the startup screen only, never to a reply.
        welcome.extend(mascot_frame(app.spinner));
        welcome.push(Line::raw(""));
        welcome.push(Line::styled(
            "A quiet place to think out loud.",
            Style::default().fg(text()),
        ));
        welcome.push(Line::raw(""));
        if !app.backend.is_configured() {
            welcome.push(Line::styled(
                "No provider connected.",
                Style::default().fg(muted()),
            ));
            welcome.push(Line::styled(
                "Run /connect to add a provider or a custom endpoint.",
                Style::default().fg(muted()),
            ));
        } else if app.backend.model().is_empty() {
            welcome.push(Line::styled(
                "No model selected.",
                Style::default().fg(muted()),
            ));
            welcome.push(Line::styled(
                "Run /models or /connect to choose a model.",
                Style::default().fg(muted()),
            ));
        } else {
            welcome.push(Line::styled(
                "Type a message to begin.",
                Style::default().fg(muted()),
            ));
        }
        welcome.push(Line::styled(
            "Type / to see commands.",
            Style::default().fg(muted()),
        ));
        frame.render_widget(Paragraph::new(welcome).alignment(Alignment::Center), inner);
        return;
    }

    // Rows for newly seen images are rendered up front: the message loop
    // borrows the session immutably, so it can only read the finished cache.
    app.warm_image_cache(inner.width.clamp(4, 48));

    let mut lines = Vec::new();
    let mut rows: Vec<(usize, String)> = Vec::new();
    // Copied out: the summary is owned so the session can stay borrowed.
    let compaction = app.compaction_text().map(str::to_owned);
    for (index, message) in app.session.messages.iter().enumerate() {
        let call_name = (message.role == Role::Tool)
            .then(|| tool_call_name(app, index))
            .flatten();
        // A hidden tool leaves no trace in the transcript; its effect shows in
        // the Progress panel instead.
        if message.role == Role::Tool && call_name.is_some_and(is_hidden_tool) {
            continue;
        }
        let has_calls = message.tool_calls.as_ref().is_some_and(|calls| {
            calls
                .iter()
                .any(|call| !is_hidden_tool(&call.function.name))
        });
        let streaming = index + 1 == app.session.messages.len() && app.is_streaming();
        // A reply that only produced reasoning is still shown: dropping it
        // would hide long thinking that ended without a final answer.
        if message.role == Role::Assistant
            && message.content.is_empty()
            && message.reasoning.trim().is_empty()
            && !has_calls
            && !streaming
            && message.error.is_none()
        {
            continue;
        }
        if !lines.is_empty() {
            lines.push(Line::raw(""));
        }
        let (label, color) = match message.role {
            Role::User => ("you", muted()),
            Role::Tool => ("tool", muted()),
            // The label names the responding model, not the product.
            Role::Assistant => {
                let model = app.backend.model();
                (if model.is_empty() { "assistant" } else { model }, accent())
            }
        };
        if message.role == Role::Tool {
            let start = lines.len();
            let name = call_name.map(str::to_owned);
            let framed =
                tool_result_lines(app, message, inner.width, &mut rows, start, name.as_deref());
            lines.extend(framed);
            continue;
        }
        let show_label = if message.role == Role::Assistant {
            let is_first_response = index == 0
                || app.session.messages[..index]
                    .iter()
                    .rev()
                    .find(|m| m.role != Role::Tool)
                    .is_some_and(|m| m.role == Role::User);
            let is_end_stream = index + 1 == app.session.messages.len();
            is_first_response || is_end_stream
        } else {
            true
        };
        if show_label {
            lines.push(Line::styled(
                label,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ));
        }
        // A failed request shows its provider message here, where the answer
        // would have been, instead of as a footer notice.
        if let Some(error) = &message.error {
            lines.push(Line::styled(
                format!("✗ {error}"),
                Style::default().fg(crate::theme::current().error),
            ));
            continue;
        }
        // Thinking: shown in full while streaming or expanded, otherwise a
        // one-line summary that a click expands like a tool result.
        let reason_key = format!("reason-{index}");
        let think_count = message
            .reasoning
            .split('\n')
            .filter(|line| !line.trim().is_empty())
            .count();
        let reasoning = reasoning_lines(message, streaming, app.expanded.contains(&reason_key));
        if !reasoning.is_empty() {
            let start = lines.len();
            if think_count > 2 && !streaming {
                rows.push((start, reason_key));
            }
            lines.extend(reasoning);
        }
        if message.content.is_empty() && streaming && !has_calls && message.reasoning.is_empty() {
            lines.push(Line::styled(
                caret(app.spinner),
                Style::default().fg(accent()),
            ));
            continue;
        }
        if !message.content.is_empty() {
            if !streaming {
                if let Some((cached_width, cached_len, cached_lines)) =
                    app.markdown_cache.get(&index)
                {
                    if *cached_width == inner.width && *cached_len == message.content.len() {
                        lines.extend(cached_lines.clone());
                    } else {
                        let parsed = markdown_lines(&message.content, inner.width);
                        app.markdown_cache
                            .insert(index, (inner.width, message.content.len(), parsed.clone()));
                        lines.extend(parsed);
                    }
                } else {
                    let parsed = markdown_lines(&message.content, inner.width);
                    app.markdown_cache
                        .insert(index, (inner.width, message.content.len(), parsed.clone()));
                    lines.extend(parsed);
                }
            } else {
                // Only the tail is parsed while it streams: running the whole
                // markdown parser over the reply so far on every frame is
                // what made typing lag. The full parse happens once when the
                // message finishes, and the result is cached.
                let tail = streaming_tail(&message.content);
                if tail.len() != message.content.len() {
                    lines.push(Line::styled(
                        "… earlier reply hidden while it streams".to_owned(),
                        Style::default().fg(muted()),
                    ));
                }
                lines.extend(markdown_lines(tail, inner.width));
            }
        }
        // Pasted screenshots show up as pixel art directly under the text.
        for image in &message.images {
            match app.cached_image(&image.name) {
                Some(rows) => lines.extend(rows.iter().cloned()),
                None => lines.push(Line::styled(
                    format!(" {}", image.name),
                    Style::default().fg(muted()),
                )),
            }
        }
        // Stats belong under a finished answer, not under a tool-call turn.
        if let Some(usage) = message.usage {
            if (!message.content.is_empty() || !message.reasoning.is_empty()) && !has_calls {
                lines.push(usage_line(usage));
            }
        }
        if let Some(calls) = &message.tool_calls {
            for call in calls
                .iter()
                .filter(|call| !is_hidden_tool(&call.function.name))
            {
                lines.push(Line::from(vec![
                    Span::styled("→ ", Style::default().fg(accent())),
                    Span::styled(
                        call.function.name.clone(),
                        Style::default().fg(text()).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("  {}", tools::preview(call)),
                        Style::default().fg(muted()),
                    ),
                ]));
            }
        }
        if streaming {
            if let Some(line) = lines.last_mut() {
                let line_len = spans_width(&line.spans);
                if line_len + 2 < usize::from(inner.width) {
                    line.spans.push(Span::styled(
                        format!(" {}", caret(app.spinner)),
                        Style::default().fg(accent()),
                    ));
                } else {
                    lines.push(Line::styled(
                        caret(app.spinner),
                        Style::default().fg(accent()),
                    ));
                }
            }
        }
    }

    // Compaction streams into its own buffer: show it live so a long summary
    // does not look like a frozen screen.
    if let Some(summary) = compaction {
        if !lines.is_empty() {
            lines.push(Line::raw(""));
        }
        lines.push(Line::styled(
            "compacting context",
            // Warning color, so compaction reads apart from the thinking
            // caret above, which stays in the accent color.
            Style::default().fg(warning()).add_modifier(Modifier::BOLD),
        ));
        if summary.trim().is_empty() {
            lines.push(Line::styled(
                caret(app.spinner),
                Style::default().fg(accent()),
            ));
        } else {
            lines.extend(markdown_lines(&summary, inner.width));
        }
    }

    // Screen row of every transcript line, accounting for wrapped text above.
    let mut starts: Vec<usize> = Vec::with_capacity(lines.len());
    let mut next_row = 0;
    let mut scratch = String::new();
    for line in &lines {
        starts.push(next_row);
        scratch.clear();
        for span in &line.spans {
            scratch.push_str(&span.content);
        }
        next_row += wrapped_line_count(&scratch, inner.width);
    }
    let total_lines = next_row;
    let maximum = total_lines.saturating_sub(usize::from(inner.height));
    if !app.follow_tail {
        app.scroll = app.scroll.min(maximum);
        if app.scroll == maximum {
            app.follow_tail = true;
        }
    }
    // ratatui's scroll offset is a u16, so a long session could not follow
    // its own tail: the offset wrapped and the live output sat hidden above a
    // stale view that looked frozen. Render only the visible window of lines
    // and scroll inside it, with the tail anchored at the real bottom (see
    // `transcript_window`), so the newest output is always on screen.
    let (first, hidden, last) = transcript_window(
        &starts,
        total_lines,
        app.scroll,
        app.follow_tail,
        usize::from(inner.height),
    );
    app.scroll = starts[first] + hidden;
    lines.truncate(last);
    lines.drain(..first);
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .scroll((hidden.min(u16::MAX as usize) as u16, 0)),
        inner,
    );

    // Map tool frames to screen rows so a click can hit one.
    let scroll = app.scroll;
    for (line_index, call_id) in rows {
        let Some(&screen_row) = starts.get(line_index) else {
            continue;
        };
        if screen_row < scroll {
            continue;
        }
        let offset = (screen_row - scroll) as u16;
        if offset >= inner.height {
            continue;
        }
        app.output_rows
            .push((inner.y.saturating_add(offset), call_id));
    }
}

/// The window of transcript lines to render, as `(first, hidden, last)`:
/// render `lines[first..last]` with its first `hidden` rows scrolled off.
/// Returns the effective top row too: `starts[first] + hidden`.
///
/// ratatui's scroll offset is a u16, so a long session could not follow its
/// own tail: the offset wrapped and the newest output sat hidden above a
/// stale view. Rendering only the visible lines removes that ceiling.
///
/// Following the tail anchors at the real bottom: the window always ends at
/// the last line, so the live output stays on screen even when the wrapped
/// row estimate drifts from what ratatui actually renders. Scrolled-up
/// positions keep mapping through the estimate, as before.
fn transcript_window(
    starts: &[usize],
    total_lines: usize,
    scroll: usize,
    follow_tail: bool,
    height: usize,
) -> (usize, usize, usize) {
    if starts.is_empty() {
        return (0, 0, 0);
    }
    let height = height.max(1);
    if follow_tail {
        // Walk back from the last line until a viewport's worth of rows is
        // covered; the overflow hides above the first included line.
        let mut accumulated = 0usize;
        let mut first = starts.len();
        while first > 0 && accumulated < height {
            first -= 1;
            accumulated += starts.get(first + 1).copied().unwrap_or(total_lines) - starts[first];
        }
        (first, accumulated.saturating_sub(height), starts.len())
    } else {
        let first = starts
            .partition_point(|&row| row <= scroll)
            .saturating_sub(1);
        let last = starts.partition_point(|&row| row < scroll + height);
        (first, scroll - starts[first], last)
    }
}

#[cfg(test)]
mod tests {
    use super::transcript_window;

    /// A fake `starts` table: every logical line wraps to two rows.
    fn two_row_starts(count: usize) -> (Vec<usize>, usize) {
        let starts = (0..count).map(|index| index * 2).collect();
        (starts, count * 2)
    }

    #[test]
    fn the_tail_window_always_ends_at_the_last_line() {
        let (starts, total) = two_row_starts(40);
        let (first, hidden, last) = transcript_window(&starts, total, 0, true, 10);
        assert_eq!(last, 40, "the last line stays in the window");
        assert_eq!(starts[first] + hidden + 10, total, "the bottom anchors");
        assert!(hidden < 2, "only the first line is clipped: {hidden}");

        // A scrolled-up position that lands past the tail still anchors at
        // the bottom when following.
        let (first, hidden, last) = transcript_window(&starts, total, 999, true, 10);
        assert_eq!(last, 40);
        assert_eq!(starts[first] + hidden + 10, total);
    }

    #[test]
    fn a_scrolled_up_window_covers_the_viewport() {
        let (starts, total) = two_row_starts(40);
        let (first, hidden, last) = transcript_window(&starts, total, 13, false, 10);
        assert_eq!(starts[first] + hidden, 13, "the scroll maps exactly");
        assert!(starts[last].saturating_sub(starts[first]) >= 10, "covers");
    }

    #[test]
    fn a_short_transcript_shows_everything_from_the_top() {
        let (starts, total) = two_row_starts(3);
        let (first, hidden, last) = transcript_window(&starts, total, 0, true, 30);
        assert_eq!((first, hidden, last), (0, 0, 3));
        let (first, hidden, last) = transcript_window(&starts, total, 0, false, 30);
        assert_eq!((first, hidden, last), (0, 0, 3));
    }

    #[test]
    fn an_empty_transcript_window_is_a_no_op() {
        assert_eq!(transcript_window(&[], 0, 0, true, 10), (0, 0, 0));
        assert_eq!(transcript_window(&[], 0, 5, false, 10), (0, 0, 0));
    }
}
