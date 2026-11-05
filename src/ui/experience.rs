use super::*;
use crate::app::experience::{Form, Screen};

mod browse;
mod footer;
mod form;
mod menu;

use browse::*;
use footer::*;
use form::*;
use menu::*;

pub(super) fn draw(f: &mut Frame, app: &mut App) -> Option<Rect> {
    let screen = app.experience.screens.last()?;
    let mut browse_scroll = None;
    let loading_more = app.experience.loading_more();
    let mut loader_visible = false;
    let title = match screen {
        Screen::Menu { title, .. } => title.as_str(),
        Screen::Input(form) => &form.title,
        Screen::Browse { page, .. } => &page.title,
        Screen::Loading(_) => "Reddit",
        Screen::Confirm { .. } => "Confirm action",
    };
    let preferred = match screen {
        Screen::Menu { choices, .. } => choices
            .iter()
            .map(|c| c.label.width())
            .chain([title.width()])
            .max()
            .unwrap_or(36)
            .saturating_add(6)
            .clamp(44, 84),
        Screen::Browse { page, .. } => page
            .entries
            .iter()
            .map(|e| e.title.width())
            .chain(
                crate::model::unformatted(&spoiler_text(&page.text, false))
                    .lines()
                    .map(UnicodeWidthStr::width),
            )
            .chain([title.width()])
            .max()
            .unwrap_or(40)
            .saturating_add(6)
            .clamp(48, 84),
        Screen::Input(_) => 72,
        _ => 72,
    };
    let width = popup(f.area(), preferred as u16, u16::MAX)
        .width
        .saturating_sub(4);
    let error_message = app
        .experience
        .error()
        .map(|message| format!("✗ {}", clean(message)));
    let wanted_error_rows = error_message
        .as_deref()
        .map_or(0, |message| wrapped_rows(message, width).min(4) + 1);
    let body_minimum = if matches!(screen, Screen::Input(_)) {
        2
    } else {
        1
    };
    let mut area = popup(
        f.area(),
        preferred as u16,
        (body_rows(screen, width, app.config.show_nsfw)
            + wanted_error_rows
            + 3
            + if loading_more { 4 } else { 0 })
        .min(u16::MAX as usize) as u16,
    );
    let minimum_height = body_minimum + 4; // Borders, feedback and primary controls.
    if wanted_error_rows > 0 && usize::from(area.height) < minimum_height {
        area.height = f.area().height.min(minimum_height as u16);
        area.y = f.area().y + (f.area().height - area.height) / 2;
    }
    let mut block = popup_block(&clean(title), area.width);
    if let Screen::Menu {
        choices, selected, ..
    } = screen
        && menu_rows(choices, *selected, width as usize).0.len()
            > area.height.saturating_sub(3) as usize
    {
        block = block.title_bottom(
            Line::from(format!(" {} / {} ", selected + 1, choices.len())).right_aligned(),
        );
    }
    let inner = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return Some(area);
    }
    let body_minimum = body_minimum
        .min(usize::from(inner.height).saturating_sub(2))
        .max(1);
    let error_rows =
        wanted_error_rows.min(usize::from(inner.height).saturating_sub(body_minimum + 1));
    let [body, error, hint] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(error_rows as u16),
        Constraint::Length(1),
    ])
    .areas(inner);
    match screen {
        Screen::Menu {
            choices, selected, ..
        } => {
            let (lines, selected_rows) = menu_rows(choices, *selected, body.width as usize);
            let offset = selected_rows
                .end
                .saturating_sub(body.height as usize)
                .min(selected_rows.start);
            f.render_widget(
                Paragraph::new(lines.into_iter().skip(offset).collect::<Vec<_>>()),
                body,
            );
        }
        Screen::Input(form) => draw_form(f, form, body),
        Screen::Confirm { label, .. } => f.render_widget(
            Paragraph::new(clean(label))
                .style(Style::new().fg(YELLOW))
                .wrap(Wrap { trim: false }),
            body,
        ),
        Screen::Loading(query) => {
            let actions = matches!(query, crate::reddit::experience::Query::Inspect(_));
            let label = match (actions, app.experience.pending.is_some()) {
                (true, true) => "Loading actions…",
                (true, false) => "Could not load actions.",
                (false, true) => "Loading from Reddit…",
                (false, false) => "Could not load this page.",
            };
            if app.experience.pending.is_some() {
                let progress = Rect::new(body.x, body.y, body.width, body.height.min(1));
                loader_visible = loading::draw(
                    f,
                    progress,
                    loading::Kind::Initial,
                    label,
                    app.loading_frame,
                )
                .is_some();
                let destination = Rect::new(
                    body.x,
                    body.y + body.height.min(2),
                    body.width,
                    body.height.saturating_sub(2),
                );
                f.render_widget(
                    Paragraph::new(query.url().unwrap_or_default()).wrap(Wrap { trim: false }),
                    destination,
                );
            } else {
                f.render_widget(
                    Paragraph::new(format!("{label}\n\n{}", query.url().unwrap_or_default()))
                        .wrap(Wrap { trim: false }),
                    body,
                );
            }
        }
        Screen::Browse {
            query,
            page,
            selected,
            scroll,
            ..
        } => {
            let (visible, scroll) = draw_browse(f, app, query, page, *selected, *scroll, body);
            loader_visible = visible;
            browse_scroll = Some(scroll);
        }
    }
    if let Some(message) = error_message
        && !error.is_empty()
    {
        let border = error.height > 1;
        let limit = usize::from(error.height) - usize::from(border);
        let mut lines: Vec<_> = textwrap::wrap(&message, usize::from(error.width))
            .into_iter()
            .map(|line| line.into_owned())
            .collect();
        if lines.len() > limit {
            lines.truncate(limit);
            if let Some(last) = lines.last_mut() {
                *last = truncate(&format!("{last}…"), usize::from(error.width));
            }
        }
        f.render_widget(
            Paragraph::new(lines.into_iter().map(Line::raw).collect::<Vec<_>>())
                .style(Style::new().fg(RED))
                .block(
                    Block::default()
                        .borders(if border { Borders::TOP } else { Borders::NONE })
                        .border_style(Style::new().fg(SURFACE1)),
                ),
            error,
        );
    }
    let pending_footer =
        app.experience.pending.is_some() && (app.experience.writing() || !loader_visible);
    if pending_footer && inner.width > 2 {
        let kind = if app.experience.writing() {
            loading::Kind::Action
        } else if app.experience.refreshing {
            loading::Kind::Background
        } else if loading_more {
            loading::Kind::More
        } else {
            loading::Kind::Initial
        };
        let text = footer(app, screen, inner.width.saturating_sub(2) as usize, false);
        f.render_widget(
            loading::compact(kind, &text, inner.width as usize, app.loading_frame),
            hint,
        );
    } else {
        f.render_widget(
            Paragraph::new(footer(app, screen, inner.width as usize, loader_visible))
                .style(Style::new().fg(OVERLAY)),
            hint,
        );
    }
    if let Some(scroll) = browse_scroll
        && let Some(Screen::Browse { scroll: shown, .. }) = app.experience.screens.last_mut()
    {
        *shown = scroll;
    }
    Some(area)
}

#[cfg(test)]
mod tests;
