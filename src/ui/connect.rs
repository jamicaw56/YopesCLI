use crate::app::AddProviderField;

use super::*;

pub(super) fn draw_connect(
    frame: &mut Frame,
    app: &App,
    connect: &ConnectState,
    spinner: usize,
    area: Rect,
) {
    let width = (area.width.saturating_sub(4) * 92 / 100)
        .clamp(60, 120)
        .min(area.width.saturating_sub(2));
    let height = (area.height.saturating_sub(2) * 90 / 100)
        .clamp(16, 40)
        .min(area.height.saturating_sub(2));
    if width < 34 || height < 10 {
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
        .title_top(Line::from(vec![
            Span::styled(
                " connect ",
                Style::default().fg(accent()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("· {} ", connect.stage.label()),
                Style::default().fg(muted()),
            ),
        ]))
        .title_bottom(Line::styled(
            format!(" {} ", connect.stage.hint()),
            Style::default().fg(dim()),
        ))
        .padding(Padding::new(2, 2, 1, 1));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    if inner.width == 0 || inner.height < 4 {
        return;
    }

    if connect.stage == ConnectStage::AddProvider {
        draw_add_provider_form(frame, connect, inner);
        return;
    }

    // A form, not a text dump: the connection in use, then the field with its
    // label, a rule that closes the field, the list, and the key hints.
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(2),
            Constraint::Length(1),
        ])
        .split(inner);
    let width = usize::from(inner.width);

    if let Some(current) = current_connection(app) {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("current  ", Style::default().fg(dim())),
                Span::styled(
                    truncate(&current, width.saturating_sub(9)),
                    Style::default().fg(muted()),
                ),
            ])),
            rows[0],
        );
    }
    frame.render_widget(
        Paragraph::new(Line::styled(
            connect.input_label().to_owned(),
            Style::default().fg(dim()).add_modifier(Modifier::BOLD),
        )),
        rows[1],
    );
    let mut spans = vec![
        Span::styled("▸ ", Style::default().fg(accent())),
        Span::styled(connect.input().to_owned(), Style::default().fg(text())),
    ];
    if connect.uses_cursor() {
        spans.push(Span::styled("▌", Style::default().fg(accent())));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), rows[2]);
    frame.render_widget(
        Paragraph::new(Span::styled("─".repeat(width), Style::default().fg(dim()))),
        rows[3],
    );

    draw_connect_list(frame, app, connect, spinner, rows[4]);
    draw_connect_hint(frame, connect, rows[5]);
}

/// The provider and model the session is on now, so the dialog says what a
/// change replaces.
fn current_connection(app: &App) -> Option<String> {
    let provider = app.backend.provider_label();
    let model = app.backend.model();
    match (provider.is_empty(), model.is_empty()) {
        (true, true) => None,
        (false, true) => Some(provider.to_owned()),
        (true, false) => Some(model.to_owned()),
        (false, false) => Some(format!("{provider} · {model}")),
    }
}

fn draw_add_provider_form(frame: &mut Frame, connect: &ConnectState, inner: Rect) {
    connect.popup_inner.set(inner);
    if inner.width < 30 || inner.height < 12 {
        return;
    }
    let active = connect.add_provider_field;

    let field_line =
        |label: &str, value: &str, is_active: bool, width: u16| -> (Line<'static>, Line<'static>) {
            let label_style = if is_active {
                Style::default().fg(accent()).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(muted())
            };
            let label_line = Line::from(vec![Span::styled(label.to_owned(), label_style)]);
            let bracket_style = if is_active {
                Style::default().fg(accent()).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(muted())
            };
            let text_style = if is_active {
                Style::default().fg(text()).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(text())
            };
            let mut spans = vec![
                Span::styled("[ ", bracket_style),
                Span::styled(
                    truncate(value, usize::from(width).saturating_sub(6)),
                    text_style,
                ),
            ];
            if is_active {
                spans.push(Span::styled("▌", Style::default().fg(accent())));
            }
            spans.push(Span::styled(" ]", bracket_style));
            (label_line, Line::from(spans))
        };

    let mut lines = Vec::new();

    // 1. Provider name:
    let (lbl, val) = field_line(
        "Provider name:",
        &connect.custom_name,
        active == AddProviderField::Name,
        inner.width,
    );
    lines.push(lbl);
    lines.push(val);

    // 2. Completion URL:
    let (lbl, val) = field_line(
        "Completion URL:",
        &connect.custom_endpoint,
        active == AddProviderField::CompletionUrl,
        inner.width,
    );
    lines.push(lbl);
    lines.push(val);

    // 3. (optional) Model fetching URL:
    let (lbl, val) = field_line(
        "(optional) Model fetching URL:",
        &connect.custom_models_url,
        active == AddProviderField::ModelsUrl,
        inner.width,
    );
    lines.push(lbl);
    lines.push(val);

    // 4. Models: name / modelID (two box split)
    let models_active =
        active == AddProviderField::ModelName || active == AddProviderField::ModelId;
    lines.push(Line::from(vec![Span::styled(
        "Models:",
        if models_active {
            Style::default().fg(accent()).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(muted())
        },
    )]));

    let half = inner.width.saturating_sub(4) / 2;
    let name_active = active == AddProviderField::ModelName;
    let id_active = active == AddProviderField::ModelId;

    let mut models_spans = Vec::new();
    models_spans.push(Span::styled(
        "name: [ ",
        if name_active {
            Style::default().fg(accent()).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(muted())
        },
    ));
    models_spans.push(Span::styled(
        truncate(
            &connect.custom_model_name,
            usize::from(half).saturating_sub(12),
        ),
        Style::default().fg(text()),
    ));
    if name_active {
        models_spans.push(Span::styled("▌", Style::default().fg(accent())));
    }
    models_spans.push(Span::styled(
        " ]  ",
        if name_active {
            Style::default().fg(accent()).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(muted())
        },
    ));

    models_spans.push(Span::styled(
        "modelID: [ ",
        if id_active {
            Style::default().fg(accent()).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(muted())
        },
    ));
    models_spans.push(Span::styled(
        truncate(
            &connect.custom_model_id,
            usize::from(half).saturating_sub(14),
        ),
        Style::default().fg(text()),
    ));
    if id_active {
        models_spans.push(Span::styled("▌", Style::default().fg(accent())));
    }
    models_spans.push(Span::styled(
        " ]",
        if id_active {
            Style::default().fg(accent()).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(muted())
        },
    ));
    lines.push(Line::from(models_spans));

    // 5. Context Window:
    let cw_display = if connect.custom_model_context_window.is_empty() {
        "128000"
    } else {
        &connect.custom_model_context_window
    };
    let (lbl, val) = field_line(
        "Context Window:",
        cw_display,
        active == AddProviderField::ContextWindow,
        inner.width,
    );
    lines.push(lbl);
    lines.push(val);

    lines.push(Line::raw(""));

    // 6. Done button
    let done_active = active == AddProviderField::Done;
    let done_style = if done_active {
        Style::default()
            .fg(Color::Black)
            .bg(accent())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(text()).add_modifier(Modifier::BOLD)
    };
    lines.push(Line::from(vec![Span::styled(
        if done_active {
            " › [ Done ] ‹ "
        } else {
            "   [ Done ]   "
        },
        done_style,
    )]));

    // 7. Hint or error
    if let Some(error) = &connect.error {
        lines.push(Line::styled(
            format!("✗ {error}"),
            Style::default().fg(crate::theme::current().error),
        ));
    } else {
        lines.push(Line::styled(
            "Tab switch box · Enter submit · Esc cancel",
            Style::default().fg(muted()),
        ));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

pub(super) fn draw_connect_list(
    frame: &mut Frame,
    app: &App,
    connect: &ConnectState,
    step: usize,
    area: Rect,
) {
    if connect.loading {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(spinner(step), Style::default().fg(accent())),
                Span::styled(connect.loading_detail(), Style::default().fg(muted())),
            ])),
            area,
        );
        return;
    }
    let mut area = area;
    if let Some(error) = &connect.error {
        if area.height == 0 {
            return;
        }
        let line = Rect::new(area.x, area.y, area.width, 1);
        frame.render_widget(
            Paragraph::new(truncate(error, usize::from(area.width)))
                .style(Style::default().fg(crate::theme::current().error)),
            line,
        );
        area = Rect {
            y: area.y.saturating_add(1),
            height: area.height.saturating_sub(1),
            ..area
        };
    }
    if !matches!(
        connect.stage,
        ConnectStage::Providers
            | ConnectStage::Models
            | ConnectStage::CustomModels
            | ConnectStage::ModelContextWindow
    ) {
        return;
    }

    let items = connect.items_using(app.backend.base_url(), app.backend.model());
    if items.is_empty() {
        if connect.error.is_none() {
            frame.render_widget(
                Paragraph::new("No matches.").style(Style::default().fg(muted())),
                area,
            );
        }
        return;
    }
    connect.list_area.set(area);
    let height = usize::from(area.height);
    if height == 0 {
        return;
    }
    let selected = connect.list_row().min(items.len() - 1);
    // Sections (custom, models.dev, endpoint actions) get spacer and rule
    // rows between them, skipped by the selection and the mouse.
    let rows = connect.list_rows(&items);
    let selected_row = list_row_of(&rows, selected);
    let start_row = selected_row.saturating_sub(height.saturating_sub(1));
    let start = start_row.min(rows.len().saturating_sub(height));
    let list = area;
    let title_width = (usize::from(list.width) * 55 / 100).max(16);

    for (offset, row) in rows.iter().skip(start).take(height).enumerate() {
        let rect = Rect::new(list.x, list.y + offset as u16, list.width, 1);
        let ListRow::Item(index) = row else {
            // Spacers stay blank; rules name the section below them.
            let ListRow::Rule(label) = row else { continue };
            let remaining = usize::from(list.width)
                .saturating_sub(label.chars().count())
                .saturating_sub(3);
            frame.render_widget(
                Paragraph::new(Line::styled(
                    format!("  ─{label}{}", "─".repeat(remaining)),
                    Style::default().fg(muted()),
                )),
                rect,
            );
            continue;
        };
        let item = &items[*index];
        let chosen = *index == selected;
        let marker = if chosen { "› " } else { "  " };
        let logo_style = if chosen {
            Style::default().fg(text()).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(accent())
        };
        let title_style = if chosen {
            Style::default().fg(text()).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(text())
        };
        let detail_style = if chosen {
            Style::default().fg(text())
        } else {
            Style::default().fg(muted())
        };
        let mut spans = vec![
            Span::styled(marker, Style::default().fg(accent())),
            Span::styled(item.logo.to_string(), logo_style),
            Span::styled("  ", Style::default()),
            Span::styled(fit_to(&item.title, title_width), title_style),
        ];
        // A tick marks the provider and model this session is already using.
        spans.push(Span::styled(
            if item.active { "✓ " } else { "  " },
            Style::default().fg(added()).add_modifier(Modifier::BOLD),
        ));
        if connect.stage == ConnectStage::Providers {
            spans.push(Span::styled("→", Style::default().fg(muted())));
            spans.push(Span::styled("  ", Style::default()));
        } else {
            spans.push(Span::styled("  ", Style::default()));
        }
        spans.push(Span::styled(item.detail.clone(), detail_style));
        frame.render_widget(Paragraph::new(Line::from(spans)), rect);
    }
}

pub(super) fn draw_connect_hint(frame: &mut Frame, connect: &ConnectState, area: Rect) {
    let text = match connect.stage {
        ConnectStage::Providers => {
            "↑↓ move · type to filter · enter choose · f2 rename · delete / ctrl+d remove · esc close"
                .to_owned()
        }
        ConnectStage::AddProvider => {
            "tab switch box · enter submit · esc close".to_owned()
        }
        ConnectStage::RenameProvider => {
            "enter rename · esc back".to_owned()
        }
        ConnectStage::Models => {
            "↑↓ move · type to filter · enter choose · tab context · esc back".to_owned()
        }
        ConnectStage::ApiKey => connect.required_env().map_or_else(
            || "enter continue · esc back · empty skips the key".to_owned(),
            |name| format!("enter continue · esc back · empty uses ${name}"),
        ),
        ConnectStage::Endpoint => "enter continue · esc back".to_owned(),
        ConnectStage::ModelsEndpoint => "enter continue · blank uses <endpoint>/models".to_owned(),
        ConnectStage::CustomToken => "enter continue · esc back".to_owned(),
        ConnectStage::ModelId => {
            if connect.has_custom_models() {
                "enter continue · empty keeps saved models · esc back".to_owned()
            } else {
                "enter continue · esc back".to_owned()
            }
        }
        ConnectStage::ModelName => "enter continue · empty uses the model id · esc back".to_owned(),
        ConnectStage::ModelContextWindow => {
            if connect.custom_flow {
                "↑↓ preset · type custom · enter add model · esc back".to_owned()
            } else {
                "↑↓ preset · type custom · enter save · esc back".to_owned()
            }
        }
        ConnectStage::CustomModels => {
            "↑↓ choose · enter use · tab context · d delete · esc back".to_owned()
        }
        ConnectStage::Password => {
            "enter save · esc back · empty stores without encryption".to_owned()
        }
        ConnectStage::Unlock => "enter unlock · esc back".to_owned(),
    };
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(muted())),
        area,
    );
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend};

    /// The dialog is a form: the connection in use, the field's label, and the
    /// stage's hint, not one undifferentiated text block.
    #[test]
    fn the_connect_dialog_reads_as_a_form() {
        let mut app = crate::app::test_app();
        app.begin_connect();
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal
            .draw(|frame| {
                if let Some(connect) = app.connect_state() {
                    super::draw_connect(frame, &app, connect, 0, frame.area());
                }
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let screen: String = (0..30)
            .map(|row| {
                (0..100)
                    .map(|column| buffer[(column, row)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(screen.contains("connect"), "{screen}");
        assert!(screen.contains("provider"), "{screen}");
        assert!(screen.contains("current"), "{screen}");
        assert!(screen.contains("pick a provider"), "{screen}");
        assert!(screen.contains("OpenAI"), "the live connection: {screen}");
    }
}
