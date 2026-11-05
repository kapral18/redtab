use super::*;

pub(super) fn preview_text<'a>(
    query: &crate::reddit::experience::Query,
    page: &'a crate::reddit::experience::Page,
    selected: usize,
) -> &'a str {
    if !page.text.is_empty() && !matches!(query, crate::reddit::experience::Query::Thread(_)) {
        ""
    } else {
        page.entries
            .get(selected)
            .map_or("", |entry| entry.body.as_str())
    }
}

pub(super) fn preview_rows(
    query: &crate::reddit::experience::Query,
    page: &crate::reddit::experience::Page,
    width: u16,
) -> usize {
    let mut rows = 0;
    for index in 0..page.entries.len() {
        rows = rows.max(
            reading_lines(preview_text(query, page, index), width)
                .len()
                .min(10),
        );
        if rows == 10 {
            break;
        }
    }
    rows
}

pub(super) fn empty_browse_text(
    query: &crate::reddit::experience::Query,
    page: &crate::reddit::experience::Page,
    show_nsfw: bool,
) -> String {
    use crate::reddit::experience::Query;
    if !page.text.is_empty() {
        page.text.clone()
    } else if !show_nsfw
        && matches!(
            query,
            Query::User { .. } | Query::Communities | Query::Multi { .. }
        )
    {
        format!("No visible items.\n\n{NSFW_EMPTY_HINT}")
    } else {
        "No items.".into()
    }
}

pub(super) fn body_rows(screen: &Screen, width: u16, show_nsfw: bool) -> usize {
    match screen {
        Screen::Menu {
            choices, selected, ..
        } => menu_rows(choices, *selected, width as usize)
            .0
            .len()
            .clamp(1, 18),
        Screen::Input(form) => form_layout(form, width as usize).lines.len().max(1),
        Screen::Browse { query, page, .. } => {
            if page.entries.is_empty() {
                reading_lines(&empty_browse_text(query, page, show_nsfw), width)
                    .len()
                    .clamp(1, 18)
            } else {
                let summary = if matches!(query, crate::reddit::experience::Query::Thread(_)) {
                    0
                } else {
                    reading_lines(&page.text, width).len().min(8)
                };
                let preview = preview_rows(query, page, width);
                summary
                    + usize::from(summary > 0)
                    + page.entries.len().min(12)
                    + preview
                    + usize::from(preview > 0)
            }
        }
        Screen::Confirm { label, .. } => wrapped_rows(&clean(label), width).max(1),
        Screen::Loading(query) => 3 + wrapped_rows(&query.url().unwrap_or_default(), width).min(4),
    }
}

pub(super) fn draw_browse(
    f: &mut Frame,
    app: &App,
    query: &crate::reddit::experience::Query,
    page: &crate::reddit::experience::Page,
    selected: usize,
    scroll: usize,
    body: Rect,
) -> (bool, usize) {
    let loading_more = app.experience.loading_more();
    let show_nsfw = app.config.show_nsfw;
    if loading_more && page.entries.is_empty() {
        return (
            loading::draw(
                f,
                body,
                loading::Kind::More,
                "Loading results",
                app.loading_frame,
            )
            .is_some(),
            0,
        );
    }
    if page.entries.is_empty() {
        let text = empty_browse_text(query, page, show_nsfw);
        let lines = reading_lines(&text, body.width);
        let scroll = scroll
            .min(lines.len().saturating_sub(body.height as usize))
            .min(u16::MAX as usize);
        f.render_widget(Paragraph::new(lines).scroll((scroll as u16, 0)), body);
        return (false, scroll);
    }
    let summary = if matches!(query, crate::reddit::experience::Query::Thread(_)) {
        ""
    } else {
        &page.text
    };
    let preview = preview_text(query, page, selected);
    let loader_rows = if loading_more {
        body.height.saturating_sub(2).min(3)
    } else {
        0
    };
    let available = body.height.saturating_sub(loader_rows);
    let has_preview = preview_rows(query, page, body.width) > 0;
    let summary_rows = reading_lines(summary, body.width).len().min(8);
    let text_reserve = if summary_rows > 0 {
        usize::from(available >= 2)
    } else if has_preview {
        usize::from(available.saturating_sub(1).min(2))
    } else {
        0
    };
    let list_limit = if has_preview && available >= 6 {
        usize::from(available / 2)
    } else {
        12
    };
    let list_rows = page
        .entries
        .len()
        .min(list_limit)
        .min(usize::from(available).saturating_sub(text_reserve));
    let remaining = usize::from(available).saturating_sub(list_rows);
    let summary_rows = summary_rows.min(remaining);
    let gap = usize::from(summary_rows > 0 && remaining > summary_rows);
    let [summary_area, _, list, loader, preview_area] = Layout::vertical([
        Constraint::Length(summary_rows as u16),
        Constraint::Length(gap as u16),
        Constraint::Length(list_rows as u16),
        Constraint::Length(loader_rows),
        Constraint::Min(0),
    ])
    .areas(body);
    let summary_lines = reading_lines(summary, summary_area.width);
    let preview_lines = reading_lines(preview, preview_area.width);
    let summary_max = if summary_area.is_empty() {
        0
    } else {
        summary_lines
            .len()
            .saturating_sub(summary_area.height as usize)
    };
    let preview_max = if preview_area.is_empty() {
        0
    } else {
        preview_lines
            .len()
            .saturating_sub(usize::from(preview_area.height) - usize::from(preview_area.height > 1))
    };
    let scroll = scroll
        .min(summary_max.max(preview_max))
        .min(u16::MAX as usize);
    f.render_widget(
        Paragraph::new(summary_lines).scroll((scroll as u16, 0)),
        summary_area,
    );
    let offset = selected.saturating_sub((list.height as usize).saturating_sub(1));
    let rows: Vec<_> = page
        .entries
        .iter()
        .enumerate()
        .skip(offset)
        .take(list.height as usize)
        .map(|(index, entry)| {
            let style = choice_style(index == selected);
            let mut spans = vec![Span::styled(
                truncate(
                    &format!(
                        "{} {}",
                        if index == selected { "▌" } else { " " },
                        clean(&entry.title)
                    ),
                    list.width as usize,
                ),
                style,
            )];
            pad_to(&mut spans, list.width as usize, style);
            Line::from(spans)
        })
        .collect();
    f.render_widget(Paragraph::new(rows), list);
    if has_preview && !preview_area.is_empty() {
        f.render_widget(
            Paragraph::new(preview_lines)
                .scroll((scroll as u16, 0))
                .block(
                    Block::default()
                        .borders(if preview_area.height > 1 {
                            Borders::TOP
                        } else {
                            Borders::NONE
                        })
                        .border_style(Style::new().fg(SURFACE1)),
                ),
            preview_area,
        );
    }
    let loader_visible = loading_more
        && loading::draw(
            f,
            loader,
            loading::Kind::More,
            "Loading results",
            app.loading_frame,
        )
        .is_some();
    (loader_visible, scroll)
}

pub(super) fn clean(text: &str) -> String {
    spoiler_text(text, false)
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .filter(|c| *c != PICTURE)
        .filter(|c| !crate::model::text_format(*c))
        .collect()
}

pub(super) fn reading_lines(text: &str, width: u16) -> Vec<Line<'static>> {
    if text.is_empty() {
        return Vec::new();
    }
    let text = spoiler_text(text, false).replace(PICTURE, "[image] ");
    rich_text::wrap(&text, usize::from(width), Style::default())
}
