//! The delegate UI: the live sidebar of subagent runs, the `/agents` list,
//! and the viewer that shows one run's full response.

use super::markdown::markdown_lines;
use super::*;

/// Width of the right sidebar: wide enough for a status glyph, an elapsed
/// time and a task excerpt.
pub(super) const SIDEBAR_WIDTH: u16 = 26;
/// The narrowest terminal that still shows the sidebar instead of stealing
/// transcript width.
pub(super) const SIDEBAR_MIN_WIDTH: u16 = 90;

/// One sidebar row: the status glyph, the elapsed time and the task excerpt.
fn agent_row(agent: &crate::app::Subagent, spinner: usize) -> Line<'static> {
    let (glyph, color) = match agent.status {
        crate::app::SubagentStatus::Running => (SPINNER[spinner % SPINNER.len()], accent()),
        crate::app::SubagentStatus::Done => ("✓", muted()),
        crate::app::SubagentStatus::Failed => ("✗", error()),
    };
    let elapsed_ms = match agent.status {
        crate::app::SubagentStatus::Running => {
            u64::try_from(agent.started.elapsed().as_millis()).unwrap_or(0)
        }
        _ => agent.elapsed_ms,
    };
    let elapsed = crate::app::format_duration_ms(elapsed_ms);
    let excerpt = agent.task_excerpt();
    Line::from(vec![
        Span::styled(format!("{glyph} "), Style::default().fg(color)),
        Span::styled(format!("{elapsed} "), Style::default().fg(muted())),
        Span::styled(excerpt, Style::default().fg(text())),
    ])
}

pub(super) fn draw_agents_sidebar(frame: &mut Frame, app: &mut App, area: Rect) {
    if area.width < 16 || area.height < 3 {
        app.sidebar_hits.clear();
        return;
    }
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(muted()))
        .title_top(Line::styled(
            format!(" agents {} ", app.subagents.len()),
            Style::default().fg(accent()).add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Line::styled(
            " click to view ",
            Style::default().fg(muted()),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    app.sidebar_hits.clear();
    let visible = usize::from(inner.height);
    // The runs stack downward; the newest stay visible when the list outgrows
    // the panel, so live work is never pushed off screen.
    let skip = app.subagents.len().saturating_sub(visible);
    for (row, agent) in app.subagents.iter().skip(skip).enumerate() {
        let y = inner.y.saturating_add(row as u16);
        frame.render_widget(
            Paragraph::new(agent_row(agent, app.spinner)),
            Rect::new(inner.x, y, inner.width, 1),
        );
        app.sidebar_hits
            .push((area.x, area.x + area.width, y, skip + row));
    }
}

/// The centered overlay that shows one subagent run: the task, the streamed
/// response and the reasoning, scrollable while the run streams.
pub(super) fn draw_agents_viewer(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(agent) = app.viewed_subagent() else {
        return;
    };
    let width = (area.width * 4 / 5).clamp(40, area.width.saturating_sub(4));
    let height = (area.height * 4 / 5).clamp(12, area.height.saturating_sub(2));
    let popup = Rect::new(
        area.x + (area.width.saturating_sub(width)) / 2,
        area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    );
    let inner_width = width.saturating_sub(4).max(8);

    let status = agent.status_label();
    let elapsed_ms = match agent.status {
        crate::app::SubagentStatus::Running => {
            u64::try_from(agent.started.elapsed().as_millis()).unwrap_or(0)
        }
        _ => agent.elapsed_ms,
    };
    let mut title = format!(
        " subagent #{} · {status} · {} ",
        agent.id,
        crate::app::format_duration_ms(elapsed_ms)
    );
    if let Some((prompt, completion)) = agent.usage {
        title.push_str(&format!(
            "· {} tok ",
            crate::app::format_tokens(prompt.saturating_add(completion))
        ));
    }
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(match agent.status {
            crate::app::SubagentStatus::Running => accent(),
            crate::app::SubagentStatus::Done => muted(),
            crate::app::SubagentStatus::Failed => error(),
        }))
        .title_top(Line::styled(
            title,
            Style::default().fg(accent()).add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Line::styled(
            " ↑↓ scroll · esc close ",
            Style::default().fg(muted()),
        ));

    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::styled(
        "task",
        Style::default().fg(muted()).add_modifier(Modifier::BOLD),
    ));
    for line in wrapped_plain(&agent.task, usize::from(inner_width)) {
        lines.push(Line::styled(line, Style::default().fg(text())));
    }
    lines.push(Line::raw(""));
    if let Some(note) = &agent.note {
        lines.push(Line::styled(
            format!("↻ {note}"),
            Style::default().fg(warning()),
        ));
        lines.push(Line::raw(""));
    }
    if !agent.reasoning.trim().is_empty() {
        lines.push(Line::styled(
            "reasoning",
            Style::default().fg(muted()).add_modifier(Modifier::BOLD),
        ));
        lines.extend(markdown_lines(&agent.reasoning, inner_width));
        lines.push(Line::raw(""));
    }
    if !agent.answer.trim().is_empty() {
        lines.push(Line::styled(
            "response",
            Style::default().fg(muted()).add_modifier(Modifier::BOLD),
        ));
        lines.extend(markdown_lines(&agent.answer, inner_width));
    } else if agent.status == crate::app::SubagentStatus::Running {
        lines.push(Line::styled(
            caret(app.spinner),
            Style::default().fg(accent()),
        ));
    } else {
        lines.push(Line::styled("(no response)", Style::default().fg(muted())));
    }

    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);
    let viewport = usize::from(inner.height);
    let maximum = lines.len().saturating_sub(viewport);
    app.subagent_scroll = app.subagent_scroll.min(maximum);
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .scroll((app.subagent_scroll.min(u16::MAX as usize) as u16, 0)),
        inner,
    );
}

/// The `/agents` popup: the runs, newest first, filtered by what was typed.
pub(super) fn draw_agents_picker(frame: &mut Frame, app: &App, picker: &ListPicker, area: Rect) {
    let width = responsive_palette_width(area.width);
    if width < 20 {
        return;
    }
    let popup = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y,
        width,
        area.height,
    );
    let filter_field = if picker.filter.is_empty() {
        String::new()
    } else {
        format!(" · {}", picker.filter)
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(muted()))
        .title_top(Line::styled(
            format!(" agents{filter_field} "),
            Style::default().fg(muted()),
        ))
        .title_bottom(Line::styled(
            " ↑↓ move · enter view · type to filter · esc cancel ",
            Style::default().fg(muted()),
        ));
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);

    let matches = app.agents_matches();
    let visible = usize::from(inner.height);
    let selected = picker.selected.min(matches.len().saturating_sub(1));
    // The selection stays in view when the list is longer than the popup.
    let start = selected
        .saturating_sub(visible.saturating_sub(1))
        .min(matches.len().saturating_sub(visible.min(matches.len())));
    for (row, &index) in matches.iter().skip(start).take(visible).enumerate() {
        let list_row = start + row;
        let agent = &app.subagents[index];
        let style = if list_row == selected {
            Style::default().fg(accent()).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(text())
        };
        let (glyph, glyph_style) = match agent.status {
            crate::app::SubagentStatus::Running => (
                SPINNER[app.spinner % SPINNER.len()],
                Style::default().fg(accent()),
            ),
            crate::app::SubagentStatus::Done => ("✓", Style::default().fg(muted())),
            crate::app::SubagentStatus::Failed => ("✗", Style::default().fg(error())),
        };
        let line = Line::from(vec![
            Span::styled(format!("{} ", glyph), glyph_style),
            Span::styled(agent.task_excerpt(), style),
        ]);
        frame.render_widget(
            Paragraph::new(line),
            Rect::new(inner.x, inner.y.saturating_add(row as u16), inner.width, 1),
        );
    }
}

/// Plain text wrapped to `width`: the task shows verbatim instead of as
/// Markdown, since the subagent task is a prompt, not a reply.
fn wrapped_plain(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for line in text.lines() {
        let mut current = String::new();
        for word in line.split_whitespace() {
            let candidate = if current.is_empty() {
                word.to_owned()
            } else {
                format!("{current} {word}")
            };
            if UnicodeWidthStr::width(candidate.as_str()) > width && !current.is_empty() {
                lines.push(std::mem::take(&mut current));
                current = word.to_owned();
            } else {
                current = candidate;
            }
        }
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}
