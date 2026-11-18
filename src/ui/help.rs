use super::*;

pub(super) fn draw_help(frame: &mut Frame, app: &mut App, area: Rect) {
    let width = area.width.min(68);
    let height = area.height.min(32);
    let popup = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, popup);

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(dim()))
        .title_top(Line::styled(
            // The version lives in the help title: `/set` has no room for a
            // command that exists only to print one number.
            format!(" help · v{} ", crate::update::current_version()),
            Style::default().fg(accent()).add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Line::styled(
            " esc close · ↑↓ scroll ",
            Style::default().fg(dim()),
        ))
        .padding(Padding::new(2, 2, 1, 1));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let mut lines = vec![
        help_section("keys"),
        help_row("enter", "send message"),
        help_row("alt+enter", "new line"),
        help_row("/", "open command palette"),
        help_row("tab", "cycle mode: build · plan · chat / complete command"),
        help_row("shift+tab", "cycle permission: on · yolo · ask"),
        help_row("↑ ↓", "palette, conversation, or diff"),
        help_row("esc", "stop streaming / interrupt tools / close"),
        help_row("enter", "run the pending tool"),
        help_row("a", "always allow that tool this session"),
        help_row("v", "toggle unified / side-by-side diffs"),
        help_row("ctrl+v", "paste image / text"),
        help_row("ctrl+g", "grab image from the clipboard"),
        help_row("right-click", "paste image / text"),
        help_row("click run", "open a subagent's response in the viewer"),
        help_row("ctrl+p", "fold / unfold the Progress panel"),
        help_row("ctrl+r", "rename this session"),
        help_row("ctrl+n", "new session"),
        help_row("ctrl+u", "clear input"),
        help_row("ctrl+w", "delete previous word"),
        help_row("ctrl+t", "new tab"),
        help_row("alt+w", "close tab"),
        help_row("alt+←/→", "switch tab"),
        help_row("alt+1-9", "jump to tab"),
        help_row("ctrl+c", "save and quit"),
        Line::raw(""),
        help_section("commands"),
    ];
    for command in COMMANDS {
        lines.push(help_row(command.display_name(), command.description));
    }
    // The popup scrolls on a short terminal instead of clipping silently.
    let visible = usize::from(inner.height.max(1));
    let max_scroll = lines.len().saturating_sub(visible);
    app.help_scroll = app.help_scroll.min(max_scroll);
    frame.render_widget(
        Paragraph::new(lines).scroll((app.help_scroll.min(u16::MAX as usize) as u16, 0)),
        inner,
    );
}

pub(super) fn help_section(title: &'static str) -> Line<'static> {
    Line::styled(
        format!("{title}"),
        Style::default()
            .fg(crate::theme::current().accent2)
            .add_modifier(Modifier::BOLD),
    )
}

pub(super) fn help_row(key: impl Into<String>, description: &'static str) -> Line<'static> {
    let key = key.into();
    Line::from(vec![
        Span::styled(format!("{key:<14}"), Style::default().fg(text())),
        Span::styled(description, Style::default().fg(muted())),
    ])
}
