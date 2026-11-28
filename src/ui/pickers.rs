use super::*;

pub(super) fn draw_model_picker(frame: &mut Frame, app: &App, picker: &ListPicker, area: Rect) {
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

    let prov_label = picker.provider_filter.as_deref().unwrap_or("All");
    let fetch_status = if app.fetching_models {
        format!(" {LIVE_DOT} fetching ")
    } else {
        String::new()
    };

    let title_top = Line::from(vec![
        Span::styled(
            format!(" models{filter_field} "),
            Style::default().fg(muted()),
        ),
        Span::styled(
            format!("[{prov_label}]"),
            if picker.provider_filter.is_some() {
                Style::default().fg(accent()).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(muted())
            },
        ),
        Span::styled(
            if fetch_status.is_empty() {
                " · Tab cycle · Ctrl+R fetch ".to_owned()
            } else {
                fetch_status
            },
            if app.fetching_models {
                Style::default().fg(accent()).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(muted())
            },
        ),
    ]);

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(dim()))
        .title_top(title_top)
        .title_bottom(Line::styled(
            " ↑↓ move · enter switch · tab provider · ctrl+r fetch · esc cancel ",
            Style::default().fg(muted()),
        ));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let models = app.model_matches();
    if models.is_empty() {
        frame.render_widget(
            Paragraph::new("No matches.").style(Style::default().fg(muted())),
            inner,
        );
        return;
    }
    let rows = app.model_picker_rows();
    let selected = picker.selected.min(models.len() - 1);
    // The popup shows a window of the rows, moving with the selection.
    let visible = inner.height as usize;
    let selected_row = rows
        .iter()
        .position(|row| matches!(row, ModelRow::Model(index) if *index == selected))
        .unwrap_or(0);
    let start = selected_row.saturating_sub(visible.saturating_sub(1));
    for (offset, entry) in rows.iter().skip(start).take(visible).enumerate() {
        let row = Rect::new(inner.x, inner.y + offset as u16, inner.width, 1);
        let ModelRow::Model(index) = entry else {
            let ModelRow::Header(provider) = entry else {
                unreachable!()
            };
            // Section header, styled like the /connect provider divider.
            let label = format!(" {provider} ");
            let remaining = usize::from(row.width)
                .saturating_sub(label.chars().count())
                .saturating_sub(3);
            frame.render_widget(
                Paragraph::new(Line::styled(
                    format!("  ─{label}{}", "─".repeat(remaining)),
                    Style::default().fg(muted()),
                )),
                row,
            );
            continue;
        };
        let model = &models[*index];
        let chosen = *index == selected;
        let marker = if chosen { "› " } else { "  " };

        let is_active_model = model.active_provider && model.id == app.backend.model();
        let display_title = if model.name.trim().is_empty() {
            model.id.clone()
        } else {
            model.name.clone()
        };
        let title_style = if chosen {
            selection(accent(), app.spinner)
        } else {
            Style::default().fg(text())
        };

        let active_mark = if is_active_model { "✓ " } else { "  " };
        let provider_tag = format!("[{}]", model.provider);
        let right_tag_len = active_mark.chars().count() + provider_tag.chars().count();

        let left_space = usize::from(inner.width).saturating_sub(right_tag_len + 4);
        let id_suffix = if !model.name.trim().is_empty() && model.name.trim() != model.id.trim() {
            format!(" ({})", model.id)
        } else {
            String::new()
        };

        let full_title = format!("{}{}", display_title, id_suffix);
        let fitted_title = fit_to(&full_title, left_space);
        let title_width = UnicodeWidthStr::width(fitted_title.as_str());
        let pad_len = left_space.saturating_sub(title_width);
        let padding = " ".repeat(pad_len);

        let line = Line::from(vec![
            Span::styled(marker, Style::default().fg(accent())),
            Span::styled(fitted_title, title_style),
            Span::raw(padding),
            Span::styled(
                active_mark,
                Style::default().fg(added()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(provider_tag, Style::default().fg(muted())),
        ]);
        frame.render_widget(Paragraph::new(line), row);
        if chosen {
            highlight(frame, row, app.spinner);
        }
    }
}

pub(super) fn draw_session_picker(frame: &mut Frame, app: &App, picker: &ListPicker, area: Rect) {
    let width = responsive_palette_width(area.width);
    let height = (app.session_picker_count().clamp(1, 12) as u16)
        .saturating_add(2)
        .min(area.height);
    if width < 24 || height < 3 {
        return;
    }
    let popup = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    let filter_field = if picker.filter.is_empty() {
        String::new()
    } else {
        format!(" · {}", picker.filter)
    };
    let loading_status = app
        .session_load_progress()
        .map(|(loaded, total)| format!(" · {LIVE_DOT} loading {loaded}/{total}"))
        .unwrap_or_default();
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(dim()))
        .title_top(Line::styled(
            format!(" sessions · Alt+H{filter_field}{loading_status} "),
            Style::default().fg(accent()).add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Line::styled(
            " ↑↓ move · enter load · type to filter · Alt+H/esc close ",
            Style::default().fg(dim()),
        ));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let sessions = app.session_matches();
    if sessions.is_empty() {
        let empty = if app.session_load_progress().is_some() {
            "Loading sessions..."
        } else {
            "No matching sessions."
        };
        frame.render_widget(
            Paragraph::new(empty).style(Style::default().fg(muted())),
            inner,
        );
        return;
    }
    let selected = picker.selected.min(sessions.len() - 1);
    let active: Option<PathBuf> = app.active_session_path().cloned();
    // Rows show a window of the whole list, moving with the selection.
    let visible = inner.height as usize;
    let start = selected.saturating_sub(visible.saturating_sub(1));
    for (offset, entry) in sessions.iter().skip(start).take(visible).enumerate() {
        let index = start + offset;
        let row = Rect::new(inner.x, inner.y + offset as u16, inner.width, 1);
        let chosen = index == selected;
        let marker = if chosen { "› " } else { "  " };
        let current = active.as_ref() == Some(&entry.path);
        let right = if current {
            format!("{} · now", age_label(entry.modified))
        } else {
            age_label(entry.modified)
        };
        let right_width = 8;
        let left_width = inner.width.saturating_sub(right_width as u16 + 4) as usize;
        let title = entry.title.clone();
        let title_style = if chosen {
            selection(accent(), app.spinner)
        } else {
            Style::default().fg(text())
        };
        let line = Line::from(vec![
            Span::styled(marker, Style::default().fg(accent())),
            Span::styled(format!("{} ", fit_to(&title, left_width)), title_style),
            Span::styled(fit_to(&right, right_width), Style::default().fg(muted())),
        ]);
        frame.render_widget(Paragraph::new(line), row);
        if chosen {
            highlight(frame, row, app.spinner);
        }
    }
}

pub(super) fn draw_revert_picker(frame: &mut Frame, app: &App, picker: &ListPicker, area: Rect) {
    let width = responsive_palette_width(area.width);
    if width < 24 || area.height < 3 {
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
        .border_style(Style::default().fg(dim()))
        .title_top(Line::styled(
            format!(" revert{filter_field} "),
            Style::default().fg(dim()),
        ))
        .title_bottom(Line::styled(
            " ↑↓ move · enter edit · esc cancel ",
            Style::default().fg(dim()),
        ));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let matches = app.revert_matches();
    if matches.is_empty() {
        frame.render_widget(
            Paragraph::new("No matching prompts.").style(Style::default().fg(muted())),
            inner,
        );
        return;
    }
    let selected = picker.selected.min(matches.len() - 1);
    // Rows show a window of the whole list, moving with the selection.
    let visible = inner.height as usize;
    let start = selected.saturating_sub(visible.saturating_sub(1));
    for (offset, (_, excerpt)) in matches.iter().skip(start).take(visible).enumerate() {
        let row = Rect::new(inner.x, inner.y + offset as u16, inner.width, 1);
        let chosen = start + offset == selected;
        let marker = if chosen { "› " } else { "  " };
        let title_style = if chosen {
            selection(accent(), app.spinner)
        } else {
            Style::default().fg(text())
        };
        let left_width = inner.width.saturating_sub(4) as usize;
        let line = Line::from(vec![
            Span::styled(marker, Style::default().fg(accent())),
            Span::styled(fit_to(excerpt, left_width), title_style),
        ]);
        frame.render_widget(Paragraph::new(line), row);
        if chosen {
            highlight(frame, row, app.spinner);
        }
    }
}

pub(super) fn draw_timeline_picker(frame: &mut Frame, app: &App, picker: &ListPicker, area: Rect) {
    let width = responsive_palette_width(area.width);
    if width < 24 || area.height < 3 {
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
        .border_style(Style::default().fg(dim()))
        .title_top(Line::styled(
            format!(" timeline{filter_field} "),
            Style::default().fg(dim()),
        ))
        .title_bottom(Line::styled(
            " ↑↓ move · enter jump · esc cancel ",
            Style::default().fg(dim()),
        ));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let matches = app.timeline_matches();
    if matches.is_empty() {
        frame.render_widget(
            Paragraph::new("No matching prompts.").style(Style::default().fg(muted())),
            inner,
        );
        return;
    }
    let selected = picker.selected.min(matches.len() - 1);
    let visible = inner.height as usize;
    let start = selected.saturating_sub(visible.saturating_sub(1));
    for (offset, (_, excerpt)) in matches.iter().skip(start).take(visible).enumerate() {
        let row = Rect::new(inner.x, inner.y + offset as u16, inner.width, 1);
        let chosen = start + offset == selected;
        let marker = if chosen { "› " } else { "  " };
        let title_style = if chosen {
            selection(accent(), app.spinner)
        } else {
            Style::default().fg(text())
        };
        let left_width = inner.width.saturating_sub(4) as usize;
        let line = Line::from(vec![
            Span::styled(marker, Style::default().fg(accent())),
            Span::styled(fit_to(excerpt, left_width), title_style),
        ]);
        frame.render_widget(Paragraph::new(line), row);
        if chosen {
            highlight(frame, row, app.spinner);
        }
    }
}

/// Rough age of a saved session file: now / minutes / hours / days.
pub(super) fn age_label(modified: Option<SystemTime>) -> String {
    let Some(modified) = modified else {
        return String::new();
    };
    let seconds = modified.elapsed().map_or(0, |age| age.as_secs());
    if seconds < 60 {
        "now".to_owned()
    } else if seconds < 3_600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h", seconds / 3_600)
    } else {
        format!("{}d", seconds / 86_400)
    }
}

pub(super) fn draw_palette(frame: &mut Frame, app: &App, palette: &[Suggestion], area: Rect) {
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
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(dim()))
        .title_top(Line::styled(
            format!(
                " commands {}/{} ",
                app.palette_selected() + 1,
                palette.len()
            ),
            Style::default().fg(dim()),
        ))
        .title_bottom(Line::styled(
            " ↑↓ move · tab complete · enter run ",
            Style::default().fg(dim()),
        ));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let selected = app.palette_selected();
    // The rows are a window of the whole list, moving with the selection:
    // more matches than the popup fits scroll instead of growing it.
    let visible = usize::from(inner.height);
    let start = selected.saturating_sub(visible.saturating_sub(1));
    let name_col_width = (usize::from(inner.width) * 35 / 100).clamp(18, 28);
    for (offset, command) in palette.iter().skip(start).take(visible).enumerate() {
        let index = start + offset;
        let row = Rect::new(inner.x, inner.y + offset as u16, inner.width, 1);
        let chosen = index == selected;
        let marker = if chosen { "› " } else { "  " };
        let name = command.label.as_str();
        let name_style = if chosen {
            selection(accent(), app.spinner)
        } else {
            Style::default().fg(text())
        };
        let description_style = if chosen {
            Style::default().fg(text())
        } else {
            Style::default().fg(muted())
        };
        let desc_col_width = usize::from(inner.width).saturating_sub(name_col_width + 3);
        let line = Line::from(vec![
            Span::styled(marker, Style::default().fg(accent())),
            Span::styled(fit_to(name, name_col_width), name_style),
            Span::raw(" "),
            Span::styled(
                truncate(command.description, desc_col_width),
                description_style,
            ),
        ]);
        frame.render_widget(Paragraph::new(line), row);
        if chosen {
            highlight(frame, row, app.spinner);
        }
    }
}
