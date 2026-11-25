use super::*;

use crate::changelog::Kind;

/// How many bullets of a group show before the `+N more` line, so a release
/// with fifty Changed lines still opens on one screen. `x` lifts the cap.
const GROUP_PREVIEW: usize = 4;

/// The post-update changelog popup: the running version's `CHANGELOG.md`
/// section, counted by kind, grouped, scrolled and small.
pub(super) fn draw_changelog(frame: &mut Frame, app: &mut App, area: Rect) {
    let width = area.width.min(76);
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
    let footer = format!(
        " enter close · ↑↓ scroll · c {} changed · x {} ",
        if view.hide_changed { "show" } else { "hide" },
        if view.expanded { "collapse" } else { "expand" },
    );
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(accent()))
        .title_top(Line::styled(
            format!(" {} ", view.title),
            Style::default().fg(accent()).add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Line::styled(footer, Style::default().fg(dim())))
        .padding(Padding::new(2, 2, 1, 1));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let lines = popup_lines(&view.section, view.hide_changed, view.expanded);
    let visible = usize::from(inner.height.max(1));
    let max_scroll = lines.len().saturating_sub(visible);
    view.scroll = view.scroll.min(max_scroll);
    frame.render_widget(
        Paragraph::new(lines).scroll((view.scroll.min(u16::MAX as usize) as u16, 0)),
        inner,
    );
}

/// The popup body: a count row, then one block per group.
fn popup_lines(
    section: &crate::changelog::Section,
    hide_changed: bool,
    expanded: bool,
) -> Vec<Line<'static>> {
    let theme = crate::theme::current();
    let mut lines: Vec<Line<'static>> = Vec::new();
    let chips: Vec<Span<'static>> = [Kind::Added, Kind::Fixed, Kind::Removed, Kind::Changed]
        .into_iter()
        .filter_map(|kind| {
            let count = section.count(kind);
            (count > 0).then(|| {
                Span::styled(
                    format!(" {}{} {} ", kind.symbol(), count, kind.label()),
                    Style::default()
                        .fg(Color::Black)
                        .bg(kind_color(kind))
                        .add_modifier(Modifier::BOLD),
                )
            })
        })
        .collect();
    if !chips.is_empty() {
        let mut row: Vec<Span<'static>> = Vec::new();
        for (index, chip) in chips.into_iter().enumerate() {
            if index > 0 {
                row.push(Span::raw(" "));
            }
            row.push(chip);
        }
        lines.push(Line::from(row));
        lines.push(Line::raw(""));
    }
    let mut first = true;
    for group in &section.groups {
        if hide_changed && group.kind == Kind::Changed {
            continue;
        }
        if !first {
            lines.push(Line::raw(""));
        }
        first = false;
        let color = kind_color(group.kind);
        if !group.title.is_empty() {
            lines.push(Line::styled(
                format!("{} {}", group.kind.symbol(), group.title),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ));
        }
        let shown = if expanded {
            group.items.len()
        } else {
            group.items.len().min(GROUP_PREVIEW)
        };
        lines.extend(group.items[..shown].iter().map(|item| {
            Line::from(vec![
                Span::styled(
                    group.kind.symbol().to_owned() + " ",
                    Style::default().fg(color),
                ),
                Span::styled(
                    item.strip_prefix("- ").unwrap_or(item).to_owned(),
                    Style::default().fg(theme.text),
                ),
            ])
        }));
        let hidden = group.items.len() - shown;
        if hidden > 0 {
            lines.push(Line::styled(
                format!("{} {hidden} more · x to expand", group.kind.symbol()),
                Style::default().fg(theme.dim),
            ));
        }
    }
    lines
}

/// The color a group is drawn in, also used for its count chip.
fn kind_color(kind: Kind) -> Color {
    let theme = crate::theme::current();
    match kind {
        Kind::Added => theme.added,
        Kind::Fixed => theme.accent2,
        Kind::Removed => theme.removed,
        Kind::Changed => theme.warning,
        Kind::Other => theme.muted,
    }
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend};

    use super::popup_lines;

    fn screen(app: &mut crate::app::App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(90, 30)).unwrap();
        terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..30)
            .map(|row| {
                (0..90)
                    .map(|column| buffer[(column, row)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<String>>()
            .join("\n")
    }

    /// One version, counted by kind: the popup shows the chips, the groups in
    /// reading order and only one heading per group.
    #[test]
    fn the_popup_shows_one_version_grouped_by_kind() {
        let source = "# Changelog\n\n## [Unreleased]\n\n### Added\n\n- Only unreleased.\n\n## [9.9.9]\n\n### Changed\n\n- Tweak one.\n\n### Added\n\n- New thing one.\n\n### Added\n\n- New thing two.\n\n### Fixed\n\n- Bug one.\n";
        let section = crate::changelog::parse_for_tests(source, "9.9.9").expect("section");
        let mut app = crate::app::test_app();
        app.changelog = Some(crate::app::ChangeLogView {
            title: "What's new in v9.9.9".to_owned(),
            section,
            scroll: 0,
            hide_changed: false,
            expanded: false,
        });
        let screen = screen(&mut app);
        assert!(screen.contains("What's new in v9.9.9"), "{screen}");
        assert!(screen.contains("+2 added"), "{screen}");
        assert!(screen.contains("~1 fixed"), "{screen}");
        assert!(!screen.contains("only unreleased"), "{screen}");
        assert_eq!(screen.matches("+ Added").count(), 1, "{screen}");
        assert!(screen.contains("New thing one"), "{screen}");
        assert!(screen.contains("Bug one"), "{screen}");
        assert!(screen.contains("· Changed"), "{screen}");
    }

    /// The cap hides the tail of a group behind a `+N more` line, `x` lifts
    /// it, and `c` drops the Changed group without touching its chip.
    #[test]
    fn the_cap_and_the_changed_toggle_keep_the_box_short() {
        let source = "## [1.0.0]\n\n### Changed\n\n- Changed one.\n- Changed two.\n- Changed three.\n- Changed four.\n- Changed five.\n\n### Added\n\n- Added one.\n";
        let section = crate::changelog::parse_for_tests(source, "1.0.0").expect("section");
        let capped = popup_lines(&section, false, false);
        let text: Vec<String> = capped
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect();
        assert!(
            text.iter()
                .any(|line| line.contains("1 more · x to expand")),
            "{text:?}"
        );
        assert!(
            !text.iter().any(|line| line.contains("Changed five")),
            "{text:?}"
        );

        let expanded = popup_lines(&section, false, true);
        let text: Vec<String> = expanded
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect();
        assert!(
            text.iter().any(|line| line.contains("Changed five")),
            "{text:?}"
        );
        assert!(
            !text.iter().any(|line| line.contains("more · x")),
            "{text:?}"
        );

        let hidden = popup_lines(&section, true, false);
        let text: Vec<String> = hidden
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect();
        assert!(
            !text.iter().any(|line| line.contains("Changed one")),
            "{text:?}"
        );
        // The count row still reports what is hidden.
        assert!(text[0].contains("·5 changed"), "{text:?}");
    }

    /// `c` and `x` change what the popup shows without closing it, and Esc
    /// still closes and records the version as read.
    #[test]
    fn the_shortcut_keys_toggle_the_view_and_esc_closes() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let source = "## [1.0.0]\n\n### Changed\n\n- One.\n- Two.\n";
        let section = crate::changelog::parse_for_tests(source, "1.0.0").expect("section");
        let mut app = crate::app::test_app();
        app.changelog = Some(crate::app::ChangeLogView {
            title: "What's new in v1.0.0".to_owned(),
            section,
            scroll: 0,
            hide_changed: false,
            expanded: false,
        });
        let press = |app: &mut crate::app::App, code: KeyCode| {
            app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
        };
        press(&mut app, KeyCode::Char('c'));
        assert!(app.changelog.as_ref().expect("open").hide_changed);
        press(&mut app, KeyCode::Char('c'));
        assert!(!app.changelog.as_ref().expect("open").hide_changed);
        press(&mut app, KeyCode::Char('x'));
        assert!(app.changelog.as_ref().expect("open").expanded);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.changelog.as_ref().expect("open").scroll, 1);
        press(&mut app, KeyCode::Esc);
        assert!(app.changelog.is_none());
    }
}
