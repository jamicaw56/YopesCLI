use super::*;

/// The post-update changelog popup: the running version's section of
/// `CHANGELOG.md`, short and scrolled inside a bordered box.
pub(super) fn draw_changelog(frame: &mut Frame, app: &mut App, area: Rect) {
    let width = area.width.min(72);
    let height = area.height.min(28);
    let popup = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    let Some(view) = app.changelog.as_mut() else {
        return;
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(accent()))
        .title_top(Line::styled(
            format!(" {} ", view.title),
            Style::default().fg(accent()).add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Line::styled(
            " enter close · ↑↓ scroll ",
            Style::default().fg(dim()),
        ))
        .padding(Padding::new(2, 2, 1, 1));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let lines: Vec<Line<'static>> = view
        .lines
        .iter()
        .map(|line| {
            if line.starts_with("- ") {
                let (bullet, rest) = line.split_at(2);
                Line::from(vec![
                    Span::styled(bullet.to_owned(), Style::default().fg(accent2())),
                    Span::styled(rest.to_owned(), Style::default().fg(text())),
                ])
            } else if line.is_empty() {
                Line::raw("")
            } else {
                Line::styled(
                    line.clone(),
                    Style::default()
                        .fg(crate::theme::current().accent2)
                        .add_modifier(Modifier::BOLD),
                )
            }
        })
        .collect();
    let visible = usize::from(inner.height.max(1));
    let max_scroll = lines.len().saturating_sub(visible);
    view.scroll = view.scroll.min(max_scroll);
    frame.render_widget(
        Paragraph::new(lines).scroll((view.scroll.min(u16::MAX as usize) as u16, 0)),
        inner,
    );
}
