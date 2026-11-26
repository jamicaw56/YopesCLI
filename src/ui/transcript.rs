use super::markdown::markdown_lines;
use super::*;

/// One two-column landing-page hint. These labels are ASCII, so fixed-width
/// formatting keeps both columns aligned without another layout pass.
fn landing_hint_row(
    left_key: &'static str,
    left_action: &'static str,
    right_key: &'static str,
    right_action: &'static str,
) -> Line<'static> {
    let key = || Style::default().fg(accent2()).add_modifier(Modifier::BOLD);
    Line::from(vec![
        Span::styled(format!("{left_key:<9}"), key()),
        Span::styled(format!("{left_action:<17}"), Style::default().fg(muted())),
        Span::styled(format!("{right_key:<10}"), key()),
        Span::styled(format!("{right_action:<16}"), Style::default().fg(muted())),
    ])
}

/// How many servers the landing page names before the rest move to `/mcp`.
const MAX_MCP_ROWS: usize = 4;

/// The landing page's MCP block: one row per configured server with the state
/// it reached. Read-only: the client snapshot never starts a server here.
///
/// The block is centred line by line, so every row is padded to one width;
/// otherwise a long failure message would drag its columns left of the others.
fn mcp_rows(width: u16) -> Vec<Line<'static>> {
    let report = crate::mcp::report();
    if report.servers.is_empty() && report.problems.is_empty() {
        return Vec::new();
    }
    // Names line up in one column, as long as the longest one stays short.
    let name_width = report
        .servers
        .iter()
        .map(|server| server.name.chars().count())
        .max()
        .unwrap_or(4)
        .clamp(4, 18);
    // What a state may say before it would run past the terminal.
    let tail_width = usize::from(width)
        .saturating_sub(11 + name_width + 2)
        .max(16);
    let mut rows: Vec<(String, String, String, bool)> = Vec::new();
    for (index, server) in report.servers.iter().take(MAX_MCP_ROWS).enumerate() {
        rows.push((
            if index == 0 {
                "mcp".to_owned()
            } else {
                String::new()
            },
            server.name.clone(),
            clip_to(&server.detail(), tail_width),
            server.is_failure(),
        ));
    }
    let hidden = report.servers.len().saturating_sub(MAX_MCP_ROWS);
    if hidden > 0 {
        rows.push((
            String::new(),
            String::new(),
            format!("+{hidden} more · /mcp"),
            false,
        ));
    }
    // A broken config file has to speak for itself: no server row can carry it.
    if let Some(problem) = report.problems.first() {
        rows.push((
            if report.servers.is_empty() {
                "mcp".to_owned()
            } else {
                String::new()
            },
            String::new(),
            format!(
                "config · {}",
                clip_to(problem, tail_width.saturating_sub(9))
            ),
            true,
        ));
    }
    let tail_width = rows
        .iter()
        .map(|(_, _, tail, _)| tail.chars().count())
        .max()
        .unwrap_or(1);
    rows.into_iter()
        .map(|(label, name, tail, failure)| {
            Line::from(vec![
                Span::styled(
                    format!("{label:<11}"),
                    Style::default().fg(accent2()).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{name:<name_width$}", name_width = name_width),
                    Style::default().fg(text()),
                ),
                Span::styled(
                    format!("  {tail:<tail_width$}"),
                    Style::default().fg(if failure { error() } else { muted() }),
                ),
            ])
        })
        .collect()
}
/// One line, at most `width` characters, with an ellipsis when it was cut.
fn clip_to(text: &str, width: usize) -> String {
    let flat = text.replace(['\r', '\n'], " ");
    let mut characters = flat.chars();
    let mut result = characters.by_ref().take(width).collect::<String>();
    if characters.next().is_some() {
        result.pop();
        result.push('…');
    }
    result
}

/// How many messages keep their rendered markdown in the cache: a very long
/// session evicts the oldest instead of holding every message's lines.
const MARKDOWN_CACHE_LIMIT: usize = 300;

/// A cheap content fingerprint for the markdown cache: two different texts
/// with the same byte length must not serve a stale render.
fn content_hash(content: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}

/// Wrapped row counts for one message's rendered lines, computed once when
/// the markdown cache fills: the transcript walk reuses them instead of
/// re-wrapping every character of the session on every frame.
/// The key a framed tool result is cached under: the output, the call name,
/// the expansion, whether a diff exists, and the diff view all shape the frame.
fn tool_frame_key(
    app: &App,
    message: &Message,
    width: u16,
    call_name: Option<&str>,
) -> crate::app::ToolFrameKey {
    let call_id = message.tool_call_id.as_deref().unwrap_or_default();
    crate::app::ToolFrameKey {
        width,
        content_len: message.content.len(),
        content_hash: content_hash(&message.content),
        name_hash: content_hash(call_name.unwrap_or_default()),
        expanded: app.expanded.contains(call_id),
        has_report: app.changes.contains_key(call_id),
        report_files: app
            .changes
            .get(call_id)
            .map_or(0, |report| report.diffs.len()),
        image_rows: message
            .images
            .iter()
            .map(|image| app.cached_image(&image.name).map_or(0, <[_]>::len))
            .sum(),
        diff_sides: app.diff_sides,
    }
}

fn line_counts(lines: &[Line<'static>], width: u16) -> Vec<usize> {
    lines
        .iter()
        .map(|line| {
            let mut scratch = String::new();
            for span in &line.spans {
                scratch.push_str(&span.content);
            }
            wrapped_line_count(&scratch, width)
        })
        .collect()
}

/// Wrapped rows for one transcript line: the same measure the walk applies
/// to lines rebuilt this frame.
fn measure_line(line: &Line<'_>, scratch: &mut String, width: u16) -> usize {
    scratch.clear();
    for span in &line.spans {
        scratch.push_str(&span.content);
    }
    wrapped_line_count(scratch, width)
}

/// One slot of transcript output: an owned line for the small per-message
/// pieces, or a shared block of rendered markdown whose lines live in the
/// cache. A frame only clones the blocks the visible window shows.
enum Slot<'a> {
    Line(Line<'a>),
    Block {
        lines: std::sync::Arc<[Line<'static>]>,
        /// Total wrapped rows of the block, measured once when it is built.
        rows: usize,
    },
}

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
                Style::default()
                    .fg(crate::theme::current().accent2)
                    .add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
            // Where the session works: the home directory reads as `~`.
            Line::styled(
                format!("workspace  {}", workspace_label(app.working_dir())),
                Style::default().fg(muted()),
            ),
        ];
        // Servers the config lists, with the state each one reached: the
        // landing page is where a failed connection shows up without a command.
        welcome.extend(mcp_rows(inner.width));
        welcome.push(Line::raw(""));
        // The startup screen carries no mascot: the workspace line is what the
        // eye needs, not an animation to watch.
        welcome.push(Line::styled(
            "A quiet place to think out loud.",
            Style::default().fg(text()),
        ));
        welcome.push(Line::raw(""));
        if !app.backend.is_configured() {
            welcome.push(Line::styled(
                "Custom API provider required — connect your own endpoint.",
                Style::default().fg(muted()),
            ));
            welcome.push(Line::styled(
                "Run /provider connect to configure a custom API provider.",
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
        welcome.push(Line::raw(""));
        welcome.push(Line::styled(
            "quick keys",
            Style::default().fg(accent2()).add_modifier(Modifier::BOLD),
        ));
        welcome.extend([
            landing_hint_row("Alt+H", "sessions", "/", "commands"),
            landing_hint_row("Enter", "send", "Alt+Enter", "new line"),
            landing_hint_row("Tab", "mode", "Shift+Tab", "permissions"),
            landing_hint_row("Ctrl+T", "new tab", "Alt+1-9", "switch tabs"),
            landing_hint_row("Ctrl+N", "new session", "Ctrl+R", "rename"),
            landing_hint_row("Ctrl+G", "paste image", "F1", "all shortcuts"),
        ]);
        frame.render_widget(Paragraph::new(welcome).alignment(Alignment::Center), inner);
        return;
    }

    // Rows for newly seen images are rendered up front: the message loop
    // borrows the session immutably, so it can only read the finished cache.
    // The tool frame clips at width - 5 (border, padding, scrollbar column), so
    // rows are built to that width: wider rows lose their right edge.
    app.warm_image_cache(inner.width.saturating_sub(5).clamp(4, 96));

    // The transcript is assembled as slots: owned lines for the small
    // per-message pieces, shared blocks for cached markdown. A frame only
    // clones the blocks the visible window shows (see the render step).
    let mut slots: Vec<Slot<'_>> = Vec::new();
    // Click rows as (slot index, line offset inside the slot, key): a slot can
    // be a whole framed block, so the offset must be added to the slot's own
    // screen row instead of indexing the slot table with it.
    let mut rows: Vec<(usize, usize, String)> = Vec::new();
    // Copied out: the summary is owned so the session can stay borrowed.
    let compaction = app.compaction_text().map(str::to_owned);
    // Call names by tool-call id, built in one forward pass: the per-result
    // lookup below no longer rescans the session backwards for every tool
    // message.
    let mut call_names: std::collections::HashMap<&str, &str> =
        std::collections::HashMap::with_capacity(app.session.messages.len());
    for message in &app.session.messages {
        if let Some(calls) = &message.tool_calls {
            for call in calls {
                call_names
                    .entry(call.id.as_str())
                    .or_insert(call.function.name.as_str());
            }
        }
    }
    // The role of the last non-tool message seen so far: the label walk below
    // reads it instead of rescanning backwards for every assistant message.
    let mut last_speaker: Option<Role> = None;
    for (index, message) in app.session.messages.iter().enumerate() {
        let call_name = (message.role == Role::Tool)
            .then(|| {
                message
                    .tool_call_id
                    .as_deref()
                    .and_then(|id| call_names.get(id).copied())
            })
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
            // The message still counts as the latest speaker for the label
            // walk, exactly like the backward scan it replaces.
            last_speaker = Some(Role::Assistant);
            continue;
        }
        if !slots.is_empty() {
            slots.push(Slot::Line(Line::raw("")));
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
            let start = slots.len();
            let key = tool_frame_key(app, message, inner.width, call_name);
            let frame = match app.tool_frames.get(&index) {
                Some(frame) if frame.key == key => frame,
                _ => {
                    let name = call_name.map(str::to_owned);
                    let mut frame_rows: Vec<(usize, String)> = Vec::new();
                    let framed = tool_result_lines(
                        app,
                        message,
                        inner.width,
                        &mut frame_rows,
                        0,
                        name.as_deref(),
                    );
                    let counts = line_counts(&framed, inner.width);
                    app.tool_frames.insert(
                        index,
                        crate::app::ToolFrame {
                            key,
                            rows: counts.iter().sum(),
                            clicks: frame_rows.iter().map(|(row, _)| *row).collect(),
                            lines: framed.into(),
                        },
                    );
                    app.tool_frames.get(&index).expect("just inserted")
                }
            };
            // The framed result is one shared block: its wrap rows are measured
            // once when it is built, and click rows are rebased to this slot.
            for offset in &frame.clicks {
                rows.push((
                    start,
                    *offset,
                    message.tool_call_id.clone().unwrap_or_default(),
                ));
            }
            slots.push(Slot::Block {
                lines: frame.lines.clone(),
                rows: frame.rows,
            });
            continue;
        }
        let show_label = if message.role == Role::Assistant {
            let is_first_response = last_speaker.is_none_or(|speaker| speaker == Role::User);
            let is_end_stream = index + 1 == app.session.messages.len();
            is_first_response || is_end_stream
        } else {
            true
        };
        // From here the current non-tool message is the latest speaker for
        // every later message, including the early-continue paths below.
        last_speaker = Some(message.role);
        if show_label {
            slots.push(Slot::Line(Line::styled(
                label,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )));
        }
        // A failed request shows its provider message here, where the answer
        // would have been, instead of as a footer notice.
        if let Some(error) = &message.error {
            slots.push(Slot::Line(Line::styled(
                format!("✗ {error}"),
                Style::default().fg(crate::theme::current().error),
            )));
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
            let start = slots.len();
            if think_count > 2 && !streaming {
                rows.push((start, 0, reason_key));
            }
            for line in reasoning {
                slots.push(Slot::Line(line));
            }
        }
        if message.content.is_empty() && streaming && !has_calls && message.reasoning.is_empty() {
            slots.push(Slot::Line(Line::styled(
                caret(app.spinner),
                Style::default().fg(accent()),
            )));
            continue;
        }
        if !message.content.is_empty() {
            if !streaming {
                // A completed message's content cannot change in place: a
                // revert, a compaction, or a new session clears the cache.
                // Only the newest message — the one a stopped stream can
                // still replace — pays for the full content hash every
                // frame; the rest match on width and length.
                let is_newest = index + 1 == app.session.messages.len();
                let content_len = message.content.len();
                if let Some((cached_width, cached_hash, cached_lines, cached_rows, cached_len)) =
                    app.markdown_cache.get(&index)
                {
                    if *cached_width == inner.width
                        && *cached_len == content_len
                        && (!is_newest || *cached_hash == content_hash(&message.content))
                    {
                        // The block is shared, never copied: a frame clones
                        // only the lines the visible window shows (see the
                        // render step below), and its wrap rows come from the
                        // cache instead of re-wrapping every character.
                        slots.push(Slot::Block {
                            lines: cached_lines.clone(),
                            rows: *cached_rows,
                        });
                    } else {
                        let content_key = content_hash(&message.content);
                        let parsed = markdown_lines(&message.content, inner.width);
                        let counts = line_counts(&parsed, inner.width);
                        let block_rows: usize = counts.iter().sum();
                        let arc_lines: std::sync::Arc<[Line<'static>]> = parsed.into();
                        app.markdown_cache.insert(
                            index,
                            (
                                inner.width,
                                content_key,
                                arc_lines.clone(),
                                block_rows,
                                content_len,
                            ),
                        );
                        slots.push(Slot::Block {
                            lines: arc_lines,
                            rows: block_rows,
                        });
                    }
                } else {
                    let content_key = content_hash(&message.content);
                    let parsed = markdown_lines(&message.content, inner.width);
                    let counts = line_counts(&parsed, inner.width);
                    let block_rows: usize = counts.iter().sum();
                    let arc_lines: std::sync::Arc<[Line<'static>]> = parsed.into();
                    app.markdown_cache.insert(
                        index,
                        (
                            inner.width,
                            content_key,
                            arc_lines.clone(),
                            block_rows,
                            content_len,
                        ),
                    );
                    slots.push(Slot::Block {
                        lines: arc_lines,
                        rows: block_rows,
                    });
                    // One entry per message: a very long session must not
                    // keep every message's rendered lines in memory forever.
                    while app.markdown_cache.len() > MARKDOWN_CACHE_LIMIT {
                        let Some(oldest) = app.markdown_cache.keys().copied().min() else {
                            break;
                        };
                        app.markdown_cache.remove(&oldest);
                    }
                }
            } else {
                // Only the tail is parsed while it streams: running the whole
                // markdown parser over the reply so far on every frame is
                // what made typing lag. The full parse happens once when the
                // message finishes, and the result is cached.
                let tail = streaming_tail(&message.content);
                if tail.len() != message.content.len() {
                    slots.push(Slot::Line(Line::styled(
                        "… earlier reply hidden while it streams".to_owned(),
                        Style::default().fg(muted()),
                    )));
                }
                // The tail is rebuilt every frame while it streams, so it
                // stays plain owned lines.
                for line in markdown_lines(tail, inner.width) {
                    slots.push(Slot::Line(line));
                }
            }
        }
        // Stats belong under a finished answer, not under a tool-call turn.
        if let Some(usage) = message.usage {
            if (!message.content.is_empty() || !message.reasoning.is_empty()) && !has_calls {
                slots.push(Slot::Line(usage_line(usage)));
            }
        }
        if let Some(calls) = &message.tool_calls {
            for call in calls
                .iter()
                .filter(|call| !is_hidden_tool(&call.function.name))
            {
                let mut spans = vec![
                    Span::styled("→ ".to_owned(), Style::default().fg(accent())),
                    Span::styled(
                        call.function.name.clone(),
                        Style::default().fg(text()).add_modifier(Modifier::BOLD),
                    ),
                ];
                // A delegate row shows its run: a live spinner while the
                // subagent works, a check or cross once it ends, and the
                // model-given name instead of the raw task text.
                if call.function.name == "delegate" {
                    if let Some(agent) = app.subagent_for_call(&call.id) {
                        let (glyph, color) = match agent.status() {
                            crate::app::SubagentStatus::Running => (spinner(app.spinner), accent()),
                            crate::app::SubagentStatus::Done => ("✓", muted()),
                            crate::app::SubagentStatus::Failed => ("✗", error()),
                        };
                        spans.push(Span::styled(
                            format!(" {glyph}"),
                            Style::default().fg(color),
                        ));
                        spans.push(Span::styled(
                            format!(
                                "  subagent · {} · {}",
                                agent.row_label(),
                                agent.state.label()
                            ),
                            Style::default().fg(muted()),
                        ));
                        slots.push(Slot::Line(Line::from(spans)));
                        continue;
                    }
                }
                spans.push(Span::styled(
                    format!("  {}", tools::preview(call)),
                    Style::default().fg(muted()),
                ));
                slots.push(Slot::Line(Line::from(spans)));
            }
        }
        if streaming {
            // A streaming message's content is plain lines (never a cached
            // block), so the caret still lands on the last one.
            match slots.last_mut() {
                Some(Slot::Line(line)) => {
                    let line_len = spans_width(&line.spans);
                    if line_len + 2 < usize::from(inner.width) {
                        line.spans.push(Span::styled(
                            format!(" {}", caret(app.spinner)),
                            Style::default().fg(accent()),
                        ));
                    } else {
                        slots.push(Slot::Line(Line::styled(
                            caret(app.spinner),
                            Style::default().fg(accent()),
                        )));
                    }
                }
                _ => slots.push(Slot::Line(Line::styled(
                    caret(app.spinner),
                    Style::default().fg(accent()),
                ))),
            }
        }
    }

    // Compaction streams into its own buffer: show it live so a long summary
    // does not look like a frozen screen.
    if let Some(summary) = compaction {
        if !slots.is_empty() {
            slots.push(Slot::Line(Line::raw("")));
        }
        slots.push(Slot::Line(Line::styled(
            "compacting context",
            // Warning color, so compaction reads apart from the thinking
            // caret above, which stays in the accent color.
            Style::default().fg(warning()).add_modifier(Modifier::BOLD),
        )));
        if summary.trim().is_empty() {
            slots.push(Slot::Line(Line::styled(
                caret(app.spinner),
                Style::default().fg(accent()),
            )));
        } else {
            let parsed = markdown_lines(&summary, inner.width);
            let counts = line_counts(&parsed, inner.width);
            slots.push(Slot::Block {
                lines: parsed.into(),
                rows: counts.iter().sum(),
            });
        }
    }

    // Screen row of every slot, accounting for wrapped text above. Shared
    // blocks carry their wrap row total from the cache, so one long session
    // no longer re-wraps or re-measures its history on every keystroke: only
    // the small owned lines get measured.
    let mut starts: Vec<usize> = Vec::with_capacity(slots.len());
    let mut next_row = 0;
    let mut scratch = String::new();
    for slot in &slots {
        starts.push(next_row);
        let count = match slot {
            Slot::Block { rows, .. } => *rows,
            Slot::Line(line) => measure_line(line, &mut scratch, inner.width),
        };
        next_row += count;
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
    // Only the visible window is materialized: a long session's history
    // stays in the shared blocks, and a frame clones just the blocks and
    // lines on screen.
    let mut visible: Vec<Line<'_>> = Vec::new();
    for slot in &slots[first..last] {
        match slot {
            Slot::Line(line) => visible.push(line.clone()),
            Slot::Block { lines, .. } => visible.extend(lines.iter().cloned()),
        }
    }
    frame.render_widget(
        Paragraph::new(Text::from(visible))
            .wrap(Wrap { trim: false })
            .scroll((hidden.min(u16::MAX as usize) as u16, 0)),
        inner,
    );

    // Map tool frames to screen rows so a click can hit one.
    let scroll = app.scroll;
    for (slot, offset, call_id) in rows {
        let Some(&start) = starts.get(slot) else {
            continue;
        };
        let screen_row = start + offset;
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

    /// The whole terminal buffer as one string, rows in order.
    fn rendered_text(terminal: &ratatui::Terminal<ratatui::backend::TestBackend>) -> String {
        let area = terminal.backend().buffer().area;
        (0..area.height)
            .flat_map(|row| (0..area.width).map(move |column| (row, column)))
            .map(|(row, column)| {
                terminal.backend().buffer()[(column, row)]
                    .symbol()
                    .to_string()
            })
            .collect()
    }

    /// A long edit diff folds inside its frame, and a click on any of its rows
    /// expands it in place.
    #[test]
    fn an_edit_diff_frame_opens_on_click() {
        use crate::diff::FileDiff;
        use crate::session::{Message, Role, ToolCall};
        use ratatui::{Terminal, backend::TestBackend};

        let mut app = crate::app::test_app();
        app.session.messages.push(Message {
            role: Role::Assistant,
            content: String::new(),
            reasoning: String::new(),
            tool_calls: Some(vec![ToolCall::new("call_edit", "edit_file", "{}")]),
            tool_call_id: None,
            usage: None,
            images: Vec::new(),
            error: None,
        });
        let before = (0..40)
            .map(|line| format!("old line {line}\n"))
            .collect::<String>();
        let after = (0..40)
            .map(|line| format!("new line {line}\n"))
            .collect::<String>();
        app.changes.insert(
            "call_edit".to_owned(),
            crate::app::ChangeReport {
                diffs: vec![FileDiff::new("src/main.rs", &before, &after, false)],
                side_by_side: app.diff_sides,
            },
        );
        app.session.messages.push(Message {
            role: Role::Tool,
            content: "Edited src/main.rs".to_owned(),
            reasoning: String::new(),
            tool_calls: None,
            tool_call_id: Some("call_edit".to_owned()),
            usage: None,
            images: Vec::new(),
            error: None,
        });

        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let folded = rendered_text(&terminal);
        assert!(folded.contains("click to expand"), "no fold hint: {folded}");
        let row = app
            .output_rows
            .first()
            .map(|(row, _)| *row)
            .expect("the diff frame registers click rows");
        app.handle_mouse(crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: 4,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        });
        assert!(
            app.expanded.contains("call_edit"),
            "the click did not expand the diff"
        );
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let opened = rendered_text(&terminal);
        assert!(
            opened.contains("click to collapse"),
            "not expanded: {opened}"
        );
    }

    #[test]
    fn a_hidden_tool_result_leaves_no_trace_in_the_transcript() {
        use crate::session::{Message, Role, ToolCall};
        use ratatui::{Terminal, backend::TestBackend};

        let mut app = crate::app::test_app();
        app.session
            .messages
            .push(Message::new(Role::User, "plan the work"));
        app.session.messages.push(Message {
            role: Role::Assistant,
            content: String::new(),
            reasoning: String::new(),
            tool_calls: Some(vec![ToolCall::new("call_1", "todo_write", "{}")]),
            tool_call_id: None,
            usage: None,
            images: Vec::new(),
            error: None,
        });
        // A message between the call and its result forces the name lookup to
        // reach past the immediately preceding message.
        app.session
            .messages
            .push(Message::new(Role::User, "second"));
        app.session.messages.push(Message {
            role: Role::Tool,
            content: "Wrote 3 todos.".to_owned(),
            reasoning: String::new(),
            tool_calls: None,
            tool_call_id: Some("call_1".to_owned()),
            usage: None,
            images: Vec::new(),
            error: None,
        });

        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let text = rendered_text(&terminal);
        assert!(
            !text.contains("Wrote 3 todos."),
            "hidden tool result leaked into the transcript"
        );
        assert!(text.contains("plan the work"), "user message missing");
    }

    #[test]
    fn a_completed_message_keeps_its_cached_render_without_rehashing() {
        use crate::session::{Message, Role};
        use ratatui::{Terminal, backend::TestBackend};

        let mut app = crate::app::test_app();
        app.session
            .messages
            .push(Message::new(Role::User, "first prompt"));
        app.session
            .messages
            .push(Message::new(Role::Assistant, "the reply text"));
        // A third message makes the reply a completed, non-newest message.
        app.session
            .messages
            .push(Message::new(Role::User, "second prompt"));

        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();

        // Same length, different text: a completed message's render is served
        // from the cache without re-hashing, per the invariant that in-place
        // content edits only happen while streaming, and every structural
        // change (revert, compaction, new session) clears the cache.
        app.session.messages[1].content = "the reply TEXT".to_owned();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let text = rendered_text(&terminal);
        assert!(
            text.contains("the reply text"),
            "cached render expected for a completed message"
        );
        assert!(
            !text.contains("the reply TEXT"),
            "re-hashing should not happen"
        );
    }

    #[test]
    fn the_newest_message_still_re_renders_when_its_content_changes() {
        use crate::session::{Message, Role};
        use ratatui::{Terminal, backend::TestBackend};

        let mut app = crate::app::test_app();
        app.session
            .messages
            .push(Message::new(Role::User, "first prompt"));
        app.session
            .messages
            .push(Message::new(Role::Assistant, "the reply text"));

        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();

        // The newest message keeps the full content hash: a stopped stream
        // can still replace it, so its render must follow the content.
        app.session.messages[1].content = "the reply TEXT".to_owned();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let text = rendered_text(&terminal);
        assert!(
            text.contains("the reply TEXT"),
            "newest message should follow its content"
        );
    }

    #[test]
    fn benchmark_draw_transcript() {
        use crate::session::{Message, Role, ToolCall};
        use ratatui::{Terminal, backend::TestBackend};

        let mut app = crate::app::test_app();
        // 240 messages: user + assistant pairs, the assistant carrying a
        // multi-line reply with reasoning and a tool call, like a long
        // working session.
        for i in 0..120 {
            app.session.messages.push(Message {
                role: Role::User,
                content: format!("Question {i}: how do I make the transcript render faster?"),
                reasoning: String::new(),
                tool_calls: None,
                tool_call_id: None,
                usage: None,
                images: Vec::new(),
                error: None,
            });
            app.session.messages.push(Message {
                role: Role::Assistant,
                content: format!(
                    "Answer {i}.\n\nHere is a longer reply with **markdown** and \
                     several sentences so it wraps a few times in a 100-column \
                     terminal. The `read_file` tool ran first.\n\n- one point\n- \
                     another point\n- a third point"
                ),
                reasoning: format!("Reasoning for question {i}\nsecond line\nthird line"),
                tool_calls: Some(vec![ToolCall::new(
                    format!("call_{i}"),
                    "read_file",
                    "{\"path\":\"src/ui/transcript.rs\"}",
                )]),
                tool_call_id: None,
                usage: None,
                images: Vec::new(),
                error: None,
            });
        }

        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();

        // 120 tool results whose calls sit 120-360 messages earlier: the
        // worst case for the removed backward scans.
        for i in 0..120 {
            app.session.messages.push(Message {
                role: Role::Tool,
                content: "ok".to_owned(),
                reasoning: String::new(),
                tool_calls: None,
                tool_call_id: Some(format!("call_{i}")),
                usage: None,
                images: Vec::new(),
                error: None,
            });
        }

        // Warm-up frame fills the markdown cache, like a session already on
        // screen.
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();

        let count = 20;
        let mut total = std::time::Duration::ZERO;
        let mut worst = std::time::Duration::ZERO;
        for _ in 0..count {
            let start = std::time::Instant::now();
            terminal
                .draw(|frame| crate::ui::draw(frame, &mut app))
                .unwrap();
            let elapsed = start.elapsed();
            total += elapsed;
            worst = worst.max(elapsed);
        }
        // Print the timings only when asked: the steady-state frame cost of
        // a long session depends on the host, so it is not a hard assert.
        if std::env::var("YOPES_BENCH").is_err() {
            return;
        }
        panic!(
            "[BENCHMARK] 360 msgs + 120 distant results: avg = {:.2} ms, worst = {:.2} ms",
            total.as_secs_f64() * 1000.0 / f64::from(count),
            worst.as_secs_f64() * 1000.0
        );
    }

    /// A long reply streams in, one frame per appended chunk: the completed
    /// history must stay cached while only the newest message is laid out.
    #[test]
    fn benchmark_streaming_reply() {
        use crate::session::{Message, Role};
        use ratatui::{Terminal, backend::TestBackend};

        let mut app = crate::app::test_app();
        for i in 0..200 {
            app.session
                .messages
                .push(Message::new(Role::User, format!("question {i}")));
            app.session.messages.push(Message::new(
                Role::Assistant,
                "a short finished answer".repeat(5),
            ));
        }
        app.session
            .messages
            .push(Message::new(Role::Assistant, String::new()));
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let chunk = "streamed words and more words ";
        let mut total = std::time::Duration::ZERO;
        let mut worst = std::time::Duration::ZERO;
        for _ in 0..50 {
            app.session
                .messages
                .last_mut()
                .unwrap()
                .content
                .push_str(chunk);
            let start = std::time::Instant::now();
            terminal
                .draw(|frame| crate::ui::draw(frame, &mut app))
                .unwrap();
            let elapsed = start.elapsed();
            total += elapsed;
            worst = worst.max(elapsed);
        }
        assert!(rendered_text(&terminal).contains("streamed words"));
        if std::env::var("YOPES_BENCH").is_err() {
            return;
        }
        panic!(
            "[BENCHMARK] streaming 50 frames over 400 msgs: avg = {:.2} ms, worst = {:.2} ms",
            total.as_secs_f64() * 1000.0 / 50.0,
            worst.as_secs_f64() * 1000.0
        );
    }

    /// Switching tabs redraws the target tab's whole history: its tool frames
    /// and markdown must be served from the per-tab caches, not rebuilt.
    #[test]
    fn benchmark_tab_switch() {
        use crate::session::{Message, Role, ToolCall};
        use ratatui::{Terminal, backend::TestBackend};

        let mut app = crate::app::test_app();
        app.open_tab();
        app.with_tab(0, |app| {
            for i in 0..200 {
                app.session
                    .messages
                    .push(Message::new(Role::User, format!("q {i}")));
                app.session.messages.push(Message {
                    role: Role::Assistant,
                    content: String::new(),
                    reasoning: String::new(),
                    tool_calls: Some(vec![ToolCall::new(format!("call_{i}"), "todo_write", "{}")]),
                    tool_call_id: None,
                    usage: None,
                    images: Vec::new(),
                    error: None,
                });
                app.session.messages.push(Message {
                    role: Role::Tool,
                    content: "Wrote 3 todos.".to_owned(),
                    reasoning: String::new(),
                    tool_calls: None,
                    tool_call_id: Some(format!("call_{i}")),
                    usage: None,
                    images: Vec::new(),
                    error: None,
                });
            }
        });
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let mut total = std::time::Duration::ZERO;
        let mut worst = std::time::Duration::ZERO;
        for round in 0..20 {
            app.select_tab(round % 2);
            let start = std::time::Instant::now();
            terminal
                .draw(|frame| crate::ui::draw(frame, &mut app))
                .unwrap();
            let elapsed = start.elapsed();
            total += elapsed;
            worst = worst.max(elapsed);
        }
        if std::env::var("YOPES_BENCH").is_err() {
            return;
        }
        panic!(
            "[BENCHMARK] tab switch 20 rounds, 600 msgs/tab: avg = {:.2} ms, worst = {:.2} ms",
            total.as_secs_f64() * 1000.0 / 20.0,
            worst.as_secs_f64() * 1000.0
        );
    }
}
