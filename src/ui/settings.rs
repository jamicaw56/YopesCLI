use super::*;

/// The `/settings` popup: one row per `/set` key, the current value on the
/// right, changed with the arrow keys.
pub(super) fn draw_settings(frame: &mut Frame, app: &App, area: Rect) {
    let width = area.width.min(54);
    let height = (SETTING_ROWS.len() as u16).saturating_add(2).min(area.height);
    if width < 24 || height < 4 {
        return;
    }
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
            " settings ",
            Style::default().fg(accent()).add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Line::styled(
            " ↑↓ row · ←→ change · esc close ",
            Style::default().fg(dim()),
        ));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    // The value column keeps the rows lined up whatever the value's width.
    let value_width = SETTING_ROWS
        .iter()
        .map(|row| UnicodeWidthStr::width(app.setting_value(row.key).as_str()))
        .max()
        .unwrap_or(0)
        .clamp(4, usize::from(inner.width).saturating_sub(20));
    let label_width = usize::from(inner.width).saturating_sub(value_width + 4);
    for (index, row) in SETTING_ROWS.iter().enumerate() {
        if index as u16 >= inner.height {
            break;
        }
        let chosen = index == app.settings_index;
        let line = Rect::new(inner.x, inner.y + index as u16, inner.width, 1);
        let marker = if chosen { "› " } else { "  " };
        let label_style = if chosen {
            Style::default().fg(text()).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(text())
        };
        let value_style = if chosen {
            Style::default().fg(accent()).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(muted())
        };
        let arrows = if chosen { "‹ ›" } else { "   " };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(marker, Style::default().fg(accent())),
                Span::styled(fit_to(row.label, label_width), label_style),
                Span::styled(" ", Style::default()),
                Span::styled(
                    fit_to(&app.setting_value(row.key), value_width),
                    value_style,
                ),
                Span::styled(format!(" {arrows}"), Style::default().fg(dim())),
            ])),
            line,
        );
    }
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend};

    /// The popup draws every `/set` row with its live value, and the selected
    /// row is the one the arrows act on.
    #[test]
    fn the_settings_popup_lists_every_row_with_its_value() {
        let mut app = crate::app::test_app();
        app.command("/settings");
        app.settings_index = 1;
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let screen: String = (0..24)
            .map(|row| {
                (0..80)
                    .map(|column| buffer[(column, row)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<String>>()
            .join("\n");
        assert!(screen.contains(" settings "), "{screen}");
        assert!(screen.contains("Permission"), "{screen}");
        assert!(screen.contains("Theme"), "{screen}");
        assert!(screen.contains("Reasoning effort"), "{screen}");
        assert!(screen.contains("Auto compact"), "{screen}");
        // The values come from the live state, not from a stale copy.
        assert!(screen.contains(crate::theme::current_name()), "{screen}");
        assert!(screen.contains("off"), "auto compact is off in the test app: {screen}");
    }
}
