use super::*;

use crate::usage::{cell, level};

/// How many weeks the heatmap covers: three months, counted in whole weeks so
/// every column has seven days.
const WEEKS: usize = 13;

/// `/stats`: the global token ledger, its totals, and a heatmap of the last
/// thirteen weeks, drawn as a full-screen panel closed with Esc.
pub(super) fn draw_stats(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(view) = app.stats.as_mut() else {
        return;
    };
    // A turn finishing behind the view still moves the numbers.
    view.refresh();
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(accent()))
        .title_top(Line::styled(
            " tokens · every run on this machine ",
            Style::default().fg(accent()).add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Line::styled(
            " esc close · ↑↓ scroll ",
            Style::default().fg(dim()),
        ))
        .padding(Padding::new(2, 2, 1, 1));
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);

    let lines = stats_lines(view, usize::from(inner.width));
    let visible = usize::from(inner.height.max(1));
    let max_scroll = lines.len().saturating_sub(visible);
    view.scroll = view.scroll.min(max_scroll);
    frame.render_widget(
        Paragraph::new(lines).scroll((view.scroll.min(u16::MAX as usize) as u16, 0)),
        inner,
    );
}

/// The panel body: totals, the heatmap, then the busiest days. Pure in its
/// inputs so a test can read it without a terminal.
fn stats_lines(view: &crate::app::StatsView, width: usize) -> Vec<Line<'static>> {
    let theme = crate::theme::current();
    let ledger = &view.ledger;
    let (prompt, completion) = ledger.totals();
    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(
            format_tokens(prompt + completion),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" tokens ", Style::default().fg(theme.muted)),
        Span::styled(
            format!(
                "↑{} in · ↓{} out",
                format_tokens(prompt),
                format_tokens(completion)
            ),
            Style::default().fg(theme.muted),
        ),
    ]));
    lines.push(Line::styled(
        format!(
            "{} requests · {} active days · busiest {}",
            ledger.requests(),
            ledger.active_days(),
            format_tokens(ledger.peak())
        ),
        Style::default().fg(theme.dim),
    ));
    lines.push(Line::raw(""));

    let today = chrono::Local::now().date_naive();
    let weeks = ledger.weeks(WEEKS, today);
    let peak = ledger.peak();
    // Rows are weekdays, columns are weeks: Monday on top, today at the right.
    const DAY_NAMES: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
    lines.push(Line::styled(
        format!("last {} weeks", WEEKS),
        Style::default()
            .fg(theme.accent2)
            .add_modifier(Modifier::BOLD),
    ));
    for (weekday, name) in DAY_NAMES.iter().enumerate() {
        let mut spans = vec![Span::styled(
            format!("  {name} "),
            Style::default().fg(theme.dim),
        )];
        for week in &weeks {
            let day = week[weekday];
            let intensity = level(day.total(), peak);
            spans.push(Span::styled(
                format!("{} ", cell(intensity)),
                Style::default().fg(match intensity {
                    0 => theme.dim,
                    1 => blend(theme.accent, theme.text, 70),
                    2 => blend(theme.accent, theme.text, 45),
                    3 => theme.accent,
                    _ => theme.accent2,
                }),
            ));
        }
        lines.push(Line::from(spans));
    }
    lines.push(Line::styled(
        format!(
            "  {} · nothing · {} less · {} more · {} busiest",
            cell(0),
            cell(1),
            cell(3),
            cell(4)
        ),
        Style::default().fg(theme.dim),
    ));
    lines.push(Line::raw(""));

    // The busiest days name themselves: a heatmap says when, this says how
    // much, and the two together are what "how much have I used" means.
    let mut busiest: Vec<(&String, u64)> = ledger
        .days
        .iter()
        .map(|(date, day)| (date, day.total()))
        .filter(|(_, total)| *total > 0)
        .collect();
    busiest.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.0.cmp(a.0)));
    if busiest.is_empty() {
        lines.push(Line::styled(
            fit(
                "No tokens recorded yet · this fills in as turns report usage.",
                width,
            ),
            Style::default().fg(theme.muted),
        ));
    } else {
        lines.push(Line::styled(
            "busiest days",
            Style::default()
                .fg(theme.accent2)
                .add_modifier(Modifier::BOLD),
        ));
        for (date, total) in busiest.iter().take(5) {
            let day = ledger.days.get(*date).copied().unwrap_or_default();
            lines.push(Line::from(vec![
                Span::styled(format!("  {date} "), Style::default().fg(theme.muted)),
                Span::styled(
                    format!("{:>8}", format_tokens(*total)),
                    Style::default().fg(theme.text),
                ),
                Span::styled(
                    format!(
                        " · ↑{} ↓{} · {} request{}",
                        format_tokens(day.prompt),
                        format_tokens(day.completion),
                        day.requests,
                        if day.requests == 1 { "" } else { "s" }
                    ),
                    Style::default().fg(theme.dim),
                ),
            ]));
        }
    }
    lines
}

/// Keeps a line inside the panel so a long sentence wraps nowhere.
fn fit(text: &str, width: usize) -> String {
    if UnicodeWidthStr::width(text) <= width {
        return text.to_owned();
    }
    truncate(text, width)
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend};

    /// The view answers with the ledger it was opened on: totals, the heatmap
    /// and the busiest day, in a panel that closes with Esc.
    #[test]
    fn the_stats_view_shows_totals_and_a_heatmap() {
        use crate::usage::{Day, Ledger};
        let mut ledger = Ledger::default();
        let today = chrono::Local::now().date_naive();
        let key = format!(
            "{:04}-{:02}-{:02}",
            chrono::Datelike::year(&today),
            chrono::Datelike::month(&today),
            chrono::Datelike::day(&today)
        );
        ledger.days.insert(
            key,
            Day {
                prompt: 1200,
                completion: 345,
                requests: 2,
            },
        );
        let mut app = crate::app::test_app();
        app.stats = Some(crate::app::StatsView {
            ledger,
            scroll: 0,
            loaded: std::time::Instant::now(),
        });
        let mut terminal = Terminal::new(TestBackend::new(80, 26)).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let screen: String = (0..26)
            .map(|row| {
                (0..80)
                    .map(|column| buffer[(column, row)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<String>>()
            .join("\n");
        assert!(screen.contains("every run on this machine"), "{screen}");
        assert!(screen.contains("tokens"), "{screen}");
        assert!(screen.contains("1.5k"), "{screen}");
        assert!(screen.contains("last 13 weeks"), "{screen}");
        assert!(screen.contains("mon"), "{screen}");
        assert!(screen.contains("busiest days"), "{screen}");
        // Esc closes the view again.
        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(app.stats.is_none());
        // The legend stays inside the panel width.
        assert_eq!(crate::usage::cell(4), "█");
    }
}
