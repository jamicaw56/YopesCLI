use crate::app::AddProviderField;

use super::*;

/// Rows of the connect list shown before it scrolls inside the box: the same
/// ceiling the command palette uses, so connecting never eats the transcript.
const MAX_CONNECT_ROWS: usize = 9;

/// Inner rows the box needs at a given stage, borders excluded.
fn inner_rows(connect: &ConnectState, app: &App) -> u16 {
    if connect.stage == ConnectStage::AddProvider {
        // Three fields at two rows each, the defaults line, the button, the
        // error/hint line. `field_at_mouse` maps clicks onto exactly these.
        return 9;
    }
    let list = if connect.loading {
        1
    } else if matches!(
        connect.stage,
        ConnectStage::Providers
            | ConnectStage::Models
            | ConnectStage::CustomModels
            | ConnectStage::ModelContextWindow
    ) {
        let items = connect.items_using(app.backend.base_url(), app.backend.model());
        // A failed fetch keeps one row of the box for its message.
        let errors = u16::from(connect.error.is_some());
        (connect.list_rows(&items).len().clamp(1, MAX_CONNECT_ROWS) as u16)
            .saturating_sub(errors)
            .max(1)
    } else {
        0
    };
    // current, label, input, rule, list, hint
    5 + list
}

/// Height the connect box takes in the layout above the composer. Zero when
/// the dialog is closed.
pub(super) fn connect_height(app: &App) -> u16 {
    app.connect_state()
        .map_or(0, |connect| inner_rows(connect, app) + 2)
}

/// The provider dialog, docked above the composer the way the command palette
/// is: a small box that leaves the conversation visible, instead of a
/// full-screen window that hides the session it is about to change.
pub(super) fn draw_connect(
    frame: &mut Frame,
    app: &App,
    connect: &ConnectState,
    spinner: usize,
    area: Rect,
) {
    let width = responsive_palette_width(area.width).max(46);
    let height = (inner_rows(connect, app) + 2).min(area.height);
    if width < 34 || height < 5 || area.width < width {
        return;
    }
    let popup = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y,
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
        .padding(Padding::new(1, 1, 0, 0));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    connect.popup_inner.set(inner);
    if inner.width == 0 || inner.height < 3 {
        return;
    }

    if connect.stage == ConnectStage::AddProvider {
        draw_add_provider_form(frame, connect, inner);
        return;
    }

    // A form, not a text dump: who is connected now, the field with its
    // label and a rule under it, then the list that field filters.
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(inner);
    let width = usize::from(inner.width);

    if let Some(current) = current_connection(app) {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("current ", Style::default().fg(dim())),
                Span::styled(
                    truncate(&current, width.saturating_sub(8)),
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
        spans.push(Span::styled("▌", breathe(spinner, dim(), accent())));
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
        _ => Some(format!("{provider} · {model}")),
    }
}

/// The add-provider form, laid out to the row map `field_at_mouse` clicks on:
/// two rows per field, the button below them, the hint on the last row.
fn draw_add_provider_form(frame: &mut Frame, connect: &ConnectState, inner: Rect) {
    if inner.width < 30 || inner.height < 9 {
        return;
    }
    let active = connect.add_provider_field;
    let width = usize::from(inner.width);

    let field = |number: &str, label: &str, value: &str, field: AddProviderField| {
        let on = active == field;
        let label_line = Line::from(vec![
            Span::styled(
                format!("{number}  "),
                Style::default().fg(blend(accent2(), dim(), 35)),
            ),
            Span::styled(
                label.to_owned(),
                if on {
                    Style::default().fg(accent()).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(muted())
                },
            ),
        ]);
        // The field is a single rule under the value: the box shape belongs to
        // the dialog, so nested boxes inside it only add noise.
        let mut spans = vec![
            Span::styled("   ", Style::default()),
            Span::styled(
                truncate(value, width.saturating_sub(6)),
                Style::default().fg(text()),
            ),
        ];
        if on {
            spans.push(Span::styled("▌", Style::default().fg(accent())));
        }
        let used = spans_width(&spans);
        spans.push(Span::styled(
            " ".repeat(width.saturating_sub(used)),
            Style::default().fg(if on { accent() } else { dim() }),
        ));
        vec![label_line, Line::from(spans)]
    };

    let mut lines = Vec::with_capacity(9);
    lines.extend(field(
        "1",
        "Provider name",
        &connect.custom_name,
        AddProviderField::Name,
    ));
    lines.extend(field(
        "2",
        "Completion URL",
        &connect.custom_endpoint,
        AddProviderField::CompletionUrl,
    ));
    lines.extend(field(
        "3",
        "API key / token",
        &connect.custom_token,
        AddProviderField::Token,
    ));
    lines.push(Line::styled(
        truncate(
            "defaults · Bearer · models come from <endpoint>/models",
            width,
        ),
        Style::default().fg(dim()),
    ));
    let done_active = active == AddProviderField::Done;
    let done_style = if done_active {
        Style::default()
            .fg(Color::Black)
            .bg(accent())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(text()).add_modifier(Modifier::BOLD)
    };
    let done_label = if done_active {
        " ›  Connect  ‹ "
    } else {
        "   Connect     "
    };
    let pad = width.saturating_sub(UnicodeWidthStr::width(done_label));
    lines.push(Line::from(vec![
        Span::styled(done_label.to_owned(), done_style),
        Span::styled(" ".repeat(pad), Style::default()),
    ]));
    let footer = connect.error.as_deref().map_or_else(
        || "tab next · enter continue · esc cancel".to_owned(),
        |error| format!("✗ {error}"),
    );
    lines.push(Line::styled(
        truncate(&footer, width),
        if connect.error.is_some() {
            Style::default().fg(crate::theme::current().error)
        } else {
            Style::default().fg(muted())
        },
    ));
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Two columns for the badge, whatever glyph lands in it: a terminal that
/// draws a symbol wide cannot then shift the name and the tick off their
/// columns for that one row.
fn logo_cell(logo: &str) -> String {
    match UnicodeWidthStr::width(logo) {
        0 => "  ".to_owned(),
        1 => format!("{logo} "),
        _ => logo.to_owned(),
    }
}

pub(super) fn draw_connect_list(
    frame: &mut Frame,
    app: &App,
    connect: &ConnectState,
    step: usize,
    area: Rect,
) {
    if connect.loading {
        let label = connect.loading_detail().trim().to_owned();
        let mut spans = vec![Span::styled(" ", Style::default())];
        spans.extend(live_label(&label, step, muted(), accent()));
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
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
    // The name and the detail share the row: the marker, the badge cell and
    // one space on the left, the tick and the arrow on the right, and the
    // detail sized to the widest one so no row loses its ending to the edge.
    let tail = if connect.stage == ConnectStage::Providers {
        5
    } else {
        4
    };
    let room = usize::from(list.width).saturating_sub(5 + tail);
    let widest_detail = items
        .iter()
        .map(|item| UnicodeWidthStr::width(item.detail.as_str()))
        .max()
        .unwrap_or(0);
    // The name keeps at least 14 columns; anything longer than that in the
    // detail is cut with an ellipsis rather than pushed out of the box.
    let detail_width = widest_detail.min(room.saturating_sub(14));
    let title_width = room.saturating_sub(detail_width).max(8);

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
            selection(accent(), step)
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
            Span::styled(logo_cell(&item.logo), logo_style),
            Span::styled(" ", Style::default()),
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
        spans.push(Span::styled(
            truncate(&item.detail, detail_width),
            detail_style,
        ));
        frame.render_widget(Paragraph::new(Line::from(spans)), rect);
        if chosen {
            highlight(frame, rect, step);
        }
    }
}

pub(super) fn draw_connect_hint(frame: &mut Frame, connect: &ConnectState, area: Rect) {
    let text = match connect.stage {
        ConnectStage::Providers => {
            "↑↓ move · type to filter · enter choose · f2 rename · del remove · esc close"
                .to_owned()
        }
        ConnectStage::AddProvider => {
            "tab switch box · ←/→ auth · enter submit · esc close".to_owned()
        }
        ConnectStage::RenameProvider => "enter rename · esc back".to_owned(),
        ConnectStage::Models => {
            "↑↓ move · type to filter · enter choose · tab context · esc back".to_owned()
        }
        ConnectStage::ApiKey => "enter continue · esc back · empty skips the key".to_owned(),
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
    // The hint drops whole tail segments instead of clipping mid-word: the
    // first keys are the ones a reader needs, so the last ones go first.
    let mut hint = String::new();
    for (index, part) in text.split(" · ").enumerate() {
        let next = if index == 0 {
            part.to_owned()
        } else {
            format!("{hint} · {part}")
        };
        if UnicodeWidthStr::width(next.as_str()) > usize::from(area.width) {
            break;
        }
        hint = next;
    }
    frame.render_widget(Paragraph::new(hint).style(Style::default().fg(dim())), area);
}
