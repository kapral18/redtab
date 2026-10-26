//! Compact preferences dialog using the same gutters and selection style as other menus.
use super::*;
use crate::config::{
    schema::{Kind, SETTINGS, Scope},
    store::Persistence,
};

pub(super) fn draw(f: &mut Frame, app: &mut App) -> Rect {
    if app.settings.managed.is_some() {
        return draw_managed(f, app);
    }
    let field = &SETTINGS[app.settings.selected];
    let account = app.reddit.account();
    let area = popup(f.area(), 84, 29);
    f.render_widget(Clear, area);
    let block = popup_block("Settings", area.width);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.is_empty() {
        return area;
    }
    let width = usize::from(inner.width);
    let file_status = match app.settings.store.persistence {
        Persistence::Saved => "File: Saved",
        Persistence::Temporary => "File: Temporary · session only",
        Persistence::Unsaved => "File: Unsaved · retry or revert",
    };
    let reddit_status = if app.settings.writing() {
        "Reddit: Saving…"
    } else if app.settings.account.pending() {
        "Reddit: Loading…"
    } else if app.settings.account.error.is_some() {
        "Reddit: Unavailable · r reloads"
    } else if app.settings.account.values.is_some() {
        "Reddit: Saved on account"
    } else {
        "Reddit: Sign in to configure"
    };
    let editing = app.settings.editing.as_ref();
    let controls = if editing.is_some() {
        "Enter apply · Esc cancel · F1 help"
    } else if field.scope == Scope::Reddit {
        "Enter toggle · r refresh Reddit · Esc close · F1 help"
    } else {
        "Enter edit · r retry file save · u revert file · Esc close · F1 help"
    };
    let controls = if controls.width() <= width {
        controls
    } else {
        fitting_label(
            &["⏎ edit · r retry · Esc", "⏎ edit · Esc", "⏎ · Esc", "Esc"],
            width,
        )
    };
    let error = app.settings.message.as_ref().or_else(|| {
        if field.scope == Scope::Reddit {
            app.settings.account.error.as_ref()
        } else {
            app.settings.store.error.as_ref()
        }
    });
    let mut bottom = Vec::new();
    if let Some(error) = error {
        bottom.extend(identity_lines(error, width, YELLOW).into_iter().take(3));
    }
    bottom.push(Line::styled(
        reddit_status,
        Style::new().fg(if app.settings.account.error.is_some() {
            YELLOW
        } else {
            SUBTEXT
        }),
    ));
    bottom.push(Line::styled(
        file_status,
        Style::new().fg(if app.settings.store.persistence == Persistence::Saved {
            GREEN
        } else {
            YELLOW
        }),
    ));
    if inner.height >= 20 && !app.config.path.as_os_str().is_empty() {
        bottom.push(Line::styled(
            truncate(&app.config.path.display().to_string(), width),
            Style::new().fg(SUBTEXT),
        ));
    }
    bottom.push(Line::styled(controls, Style::new().fg(SUBTEXT)));
    let reserved_content = if inner.height >= 3 { 2 } else { 1 };
    let bottom_height = bottom
        .len()
        .min(usize::from(inner.height.saturating_sub(reserved_content)));
    if bottom.len() > bottom_height {
        bottom = vec![Line::styled(controls, Style::new().fg(SUBTEXT))];
    }
    let bottom_height = bottom.len() as u16;
    let content_height = inner.height.saturating_sub(bottom_height);
    let mut content = Vec::new();
    let mut edit_row = inner.y;
    if let Some(input) = editing {
        content.push(Line::styled(
            truncate(
                &app.settings.scope_label(field.scope, account.as_ref()),
                width,
            ),
            Style::new().fg(SUBTEXT),
        ));
        content.push(Line::styled(
            truncate(&input.label, width),
            Style::new().fg(ACCENT).bold(),
        ));
        content.extend(
            identity_lines(field.input_hint(), width, SUBTEXT)
                .into_iter()
                .take(usize::from(content_height.saturating_sub(2)).min(3)),
        );
        edit_row = inner.y + content.len() as u16;
    } else {
        let mut rows = Vec::new();
        let mut selected_row = 0;
        let mut selected_end = 0;
        for (index, setting) in SETTINGS.iter().enumerate() {
            if index == 0 || setting.scope != SETTINGS[index - 1].scope {
                rows.push(Line::styled(
                    truncate(
                        &app.settings.scope_label(setting.scope, account.as_ref()),
                        width,
                    ),
                    Style::new().fg(ACCENT).bold(),
                ));
            }
            let selected = index == app.settings.selected;
            if selected {
                selected_row = rows.len();
            }
            let value = setting.value(&app.config);
            let shown = if setting.scope == Scope::Reddit && app.settings.account.values.is_none() {
                if app.settings.account.pending() {
                    "Loading…".into()
                } else if account.is_some() {
                    "Unavailable".into()
                } else {
                    "Sign in".into()
                }
            } else if setting.kind == Kind::Boolean {
                if value.as_bool().unwrap() {
                    "On".into()
                } else {
                    "Off".into()
                }
            } else {
                value.to_string()
            };
            let prefix = if selected { "›" } else { " " };
            let fits = setting.label.width() + shown.width() + 3 <= width;
            let label_width = width.saturating_sub(shown.width() + 3).min(25);
            let text = format!("{prefix} {:label_width$} {shown}", setting.label);
            let style = if selected {
                Style::new().bg(SURFACE1).fg(TEXT).bold()
            } else {
                Style::new().fg(TEXT)
            };
            if !fits {
                rows.push(Line::styled(
                    truncate(&format!("{prefix} {}", setting.label), width),
                    style,
                ));
                rows.push(Line::styled(truncate(&format!("  {shown}"), width), style));
            } else {
                rows.push(Line::styled(truncate(&text, width), style));
            }
            if selected {
                selected_end = rows.len().saturating_sub(1);
            }
        }
        let description_height =
            usize::from(content_height.saturating_sub(1)).min(if content_height > 9 {
                4
            } else if content_height > 5 {
                1
            } else {
                0
            });
        let list_height = usize::from(content_height).saturating_sub(description_height);
        let start = selected_end
            .saturating_sub(list_height.saturating_sub(1))
            .min(selected_row)
            .min(rows.len().saturating_sub(list_height));
        content.extend(rows.into_iter().skip(start).take(list_height));
        content.resize(list_height, Line::default());
        let description = format!("{} · {}", field.file_key(&app.config), field.description);
        content.extend(
            identity_lines(&description, width, SUBTEXT)
                .into_iter()
                .take(description_height),
        );
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
