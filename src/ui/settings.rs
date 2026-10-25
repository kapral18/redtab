//! Compact preferences dialog using the same gutters and selection style as other menus.
use super::*;
use crate::config::{
    schema::{Kind, SETTINGS},
    store::Persistence,
};

pub(super) fn draw(f: &mut Frame, app: &mut App) -> Rect {
    if app.settings.managed.is_some() {
        return draw_managed(f, app);
    }
    let field = &SETTINGS[app.settings.selected];
    let width = popup(f.area(), 84, 25).width.saturating_sub(4);
    let description_rows = SETTINGS
        .iter()
        .map(|setting| {
            identity_lines(
                &format!("{} · {}", setting.key, setting.description),
                usize::from(width),
                SUBTEXT,
            )
            .len()
            .min(4)
        })
        .max()
        .unwrap_or(0);
    let error = app
        .settings
        .message
        .as_ref()
        .or(app.settings.store.error.as_ref());
    let bottom_rows = 2
        + usize::from(!app.config.path.as_os_str().is_empty())
        + error.map_or(0, |e| wrapped_rows(e, width).min(3));
    let body_rows = if app.settings.editing.is_some() {
        2 + wrapped_rows(field.input_hint(), width).min(3)
    } else {
        SETTINGS.len() + 1 + description_rows
    };
    let area = popup(f.area(), 84, (2 + body_rows + bottom_rows).min(25) as u16);
    f.render_widget(Clear, area);
    let block = popup_block("Settings", area.width);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.is_empty() {
        return area;
    }
    let width = usize::from(inner.width);
    let status = match app.settings.store.persistence {
        Persistence::Saved => "Saved",
        Persistence::Temporary => "Temporary · session only",
        Persistence::Unsaved => "Unsaved · retry or revert",
    };
    let color = if app.settings.store.persistence == Persistence::Saved {
        GREEN
    } else {
        YELLOW
    };
    let editing = app.settings.editing.as_ref();
    let controls = if editing.is_some() {
        "Enter apply · Esc cancel · F1 help"
    } else {
        "Enter edit · r retry save · u revert to file · Esc close · F1 help"
    };
    let controls = if width >= controls.width() {
        controls
    } else if editing.is_some() {
        fitting_label(
            &[
                "Enter apply · Esc cancel",
                "⏎ apply · Esc",
                "⏎ · Esc",
                "Esc",
            ],
            width,
        )
    } else {
        fitting_label(
            &[
                "⏎ edit · r save · u revert · Esc",
                "⏎ edit · Esc close",
                "⏎ · Esc",
                "Esc",
            ],
            width,
        )
    };
    let mut bottom = vec![Line::styled(status, Style::new().fg(color))];
    let error = app
        .settings
        .message
        .as_ref()
        .or(app.settings.store.error.as_ref());
    if let Some(error) = error {
        let mut lines = identity_lines(error, width, YELLOW);
        if lines.len() > 3 {
            lines.truncate(3);
            let text = format!(
                "{}…",
                truncate(&lines[2].to_string(), width.saturating_sub(1))
            );
            lines[2] = Line::styled(text, Style::new().fg(YELLOW));
        }
        bottom.extend(lines);
    }
    if !app.settings.store.current.path.as_os_str().is_empty() && inner.height >= 12 {
        bottom.push(Line::styled(
            truncate(
                &app.settings.store.current.path.display().to_string(),
                width,
            ),
            Style::new().fg(OVERLAY),
        ));
    }
    bottom.push(Line::styled(controls, Style::new().fg(SUBTEXT)));
    // Keep the actual action and persistence state visible before optional descriptions.
    let bottom_height = bottom
        .len()
        .min(usize::from(inner.height.saturating_sub(1))) as u16;
    if bottom.len() > usize::from(bottom_height) {
        bottom = vec![
            Line::styled(status, Style::new().fg(color)),
            Line::styled(controls, Style::new().fg(SUBTEXT)),
        ];
        bottom.truncate(usize::from(bottom_height));
    }
    let content_height = inner.height.saturating_sub(bottom_height);
    let mut content = Vec::new();
    let mut edit_row = inner.y;
    if let Some(input) = editing {
        content.push(Line::styled(
            truncate(&input.label, width),
            Style::new().fg(ACCENT).bold(),
        ));
        let available = content_height.saturating_sub(2).min(3);
        content.extend(
            identity_lines(field.input_hint(), width, SUBTEXT)
                .into_iter()
                .take(usize::from(available)),
        );

        edit_row = inner.y + content.len() as u16;
    } else {
        let description = identity_lines(
            &format!("{} · {}", field.key, field.description),
            width,
            SUBTEXT,
        );
        let description_height =
            description_rows.min(usize::from(content_height.saturating_sub(1)));
        let list_height = usize::from(content_height)
            .saturating_sub(description_height + usize::from(content_height > 5));
        let start = app
            .settings
            .selected
            .saturating_sub(list_height.saturating_sub(1))
            .min(SETTINGS.len().saturating_sub(list_height));
        for (index, setting) in SETTINGS.iter().enumerate().skip(start).take(list_height) {
            let value = setting.value(&app.config);
            let shown = if setting.kind == Kind::Boolean {
                if value.as_bool().unwrap() {
                    "On".into()
                } else {
                    "Off".into()
                }
            } else {
                value.to_string()
            };
            let selected = index == app.settings.selected;
            let label_width = width.saturating_sub(5).min(27);
            let label = truncate(setting.label, label_width);
            let text = if width >= 24 {
                format!(
                    "{} {:label_width$} {}",
                    if selected { "›" } else { " " },
                    label,
                    shown
                )
            } else {
                format!(
                    "{} {} {}",
                    if selected { "›" } else { " " },
                    setting.label,
                    shown
                )
            };
            content.push(Line::styled(
                truncate(&text, width),
                if selected {
                    Style::new().bg(SURFACE1).fg(TEXT).bold()
                } else {
                    Style::new().fg(TEXT)
                },
            ));
        }
        if content_height > 5 {
            content.push(Line::default());
        }
        content.extend(description.into_iter().take(description_height));
    }
    f.render_widget(
        Paragraph::new(content),
        Rect::new(inner.x, inner.y, inner.width, content_height),
    );
    if let Some(input) = editing.filter(|_| content_height > 0) {
        let row = edit_row.min(inner.y + content_height - 1);
        let caret = input.value[..input.cursor].width();
        let scroll = caret.saturating_sub(width.saturating_sub(1));
        f.render_widget(
            Paragraph::new(input.value.as_str())
                .scroll((0, scroll.min(u16::MAX as usize) as u16))
                .style(Style::new().bg(SURFACE0).fg(TEXT)),
            Rect::new(inner.x, row, inner.width, 1),
        );
        if app.help_scroll.is_none() {
            f.set_cursor_position((
                inner.x + caret.saturating_sub(scroll).min(width.saturating_sub(1)) as u16,
                row,
            ));
        }
    }
    f.render_widget(
        Paragraph::new(bottom),
        Rect::new(
            inner.x,
            inner.y + content_height,
            inner.width,
            bottom_height,
        ),
    );
    area
}

fn draw_managed(f: &mut Frame, app: &mut App) -> Rect {
    use crate::app::settings::ManagedSave;
    let width = usize::from(popup(f.area(), 84, 25).width.saturating_sub(4)).max(1);
    let mut lines = Vec::new();
    let confirming = matches!(app.settings.managed, Some(ManagedSave::Confirm(_)));
    match app.settings.managed.as_ref().unwrap() {
        ManagedSave::Confirm(plan) => {
            lines.extend(identity_lines(
                &format!("Source: {}", plan.source().display()),
                width,
                TEXT,
            ));
            lines.extend(identity_lines(
                &format!("Config: {}", plan.target().display()),
                width,
                SUBTEXT,
            ));
            lines.push(Line::default());
            for change in &plan.changes {
                lines.extend(identity_lines(change, width, ACCENT));
            }
            lines.push(Line::default());
            lines.extend(identity_lines(
                "Updates the source and applies this config. Configured chezmoi hooks may run.",
                width,
                SUBTEXT,
            ));
        }
        ManagedSave::Checking(_) => lines.extend(loading::wrapped(
            loading::Kind::Action,
            "Checking chezmoi…",
            width,
            app.loading_frame,
        )),
        ManagedSave::Applying(..) => lines.extend(loading::wrapped(
            loading::Kind::Action,
            "Saving through chezmoi…",
            width,
            app.loading_frame,
        )),
    }
    let area = popup(f.area(), 84, (lines.len() + 3).min(25) as u16);
    f.render_widget(Clear, area);
    let title = if confirming {
        "Save through chezmoi?"
    } else {
        "Settings"
    };
    let block = popup_block(title, area.width);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.is_empty() {
        return area;
    }
    let controls = if confirming {
        fitting_label(
            &[
                "Enter save · Esc session only · j/k scroll · F1 help",
                "⏎ save · Esc keep · j/k scroll",
                "⏎ save · Esc keep",
                "⏎ · Esc",
                "⏎/Esc",
                "Esc",
            ],
            inner.width as usize,
        )
    } else {
        fitting_label(
            &["Please wait · F1 help", "Please wait", "wait"],
            inner.width as usize,
        )
    };
    let height = inner.height.saturating_sub(1) as usize;
    app.settings.managed_scroll = app
        .settings
        .managed_scroll
        .min(lines.len().saturating_sub(height));
    f.render_widget(
        Paragraph::new(
            lines
                .into_iter()
                .skip(app.settings.managed_scroll)
                .take(height)
                .collect::<Vec<_>>(),
        ),
        Rect::new(inner.x, inner.y, inner.width, height as u16),
    );
    f.render_widget(
        Line::styled(controls, Style::new().fg(SUBTEXT)),
        Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, 1),
    );
    area
}
