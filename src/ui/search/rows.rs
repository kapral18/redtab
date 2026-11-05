use super::*;

pub(super) fn entry_lines(
    row: &crate::app::search::Row,
    width: u16,
    height: usize,
    selected: bool,
    focused: bool,
) -> Vec<Line<'static>> {
    if height == 0 {
        return Vec::new();
    }
    let width = usize::from(width);
    let gap = usize::from(height >= 3);
    let text_rows = height - gap;
    let text_width = width.saturating_sub(TEXT_START).max(1);
    let wrap = |text: &str, limit: usize| {
        let mut wrapped = textwrap::wrap(
            text,
            textwrap::Options::new(text_width).word_splitter(textwrap::WordSplitter::NoHyphenation),
        )
        .into_iter()
        .map(|line| line.into_owned())
        .collect::<Vec<_>>();
        if wrapped.len() > limit {
            wrapped.truncate(limit);
            if let Some(last) = wrapped.last_mut() {
                *last = truncate(&format!("{last}…"), text_width);
            }
        }
        wrapped
    };
    let bg = if selected {
        Style::new().bg(SURFACE0)
    } else {
        Style::new()
    };
    let mut lines = Vec::new();
    let name_rows = if row.detail.is_empty() {
        text_rows
    } else {
        text_rows.saturating_sub(1).max(1)
    };
    for (index, text) in wrap(&row.label, name_rows).into_iter().enumerate() {
        let mut spans = vec![
            Span::styled(
                if index == 0 {
                    format!(
                        "{} {:width$} ",
                        selection_marker(selected, focused),
                        if matches!(row.target, FeedTarget::User(_)) {
                            "@"
                        } else {
                            "#"
                        },
                        width = usize::from(ICON_COLS)
                    )
                } else {
                    " ".repeat(TEXT_START)
                },
                bg.fg(if focused { ACCENT } else { OVERLAY }),
            ),
            Span::styled(text, bg.fg(TEXT).add_modifier(Modifier::BOLD)),
        ];
        pad_to(&mut spans, width, bg);
        lines.push(Line::from(spans));
    }
    if !row.detail.is_empty() {
        let detail_rows = text_rows.saturating_sub(lines.len()).min(3);
        for text in wrap(&row.detail, detail_rows) {
            let mut spans = vec![
                Span::styled(" ".repeat(TEXT_START), bg),
                Span::styled(text, bg.fg(SUBTEXT)),
            ];
            pad_to(&mut spans, width, bg);
            lines.push(Line::from(spans));
        }
    }
    if gap > 0 {
        lines.push(Line::from(""));
    }
    lines
}

pub(super) fn row_window(app: &App, area: Rect) -> (usize, usize, usize) {
    let status_rows = match app.search.rows_status() {
        Some(RowsStatus::MoreFailed | RowsStatus::DiscoveryFailed) => 2,
        Some(_) => 1,
        None => 0,
    };
    let fits = usize::from(area.height.saturating_sub(2)).saturating_sub(status_rows);
    let rows = app.search.rows();
    if rows.is_empty() {
        return (0, 0, fits);
    }
    let row_height =
        |row: &crate::app::search::Row| entry_lines(row, area.width, fits, false, false).len();
    let selected = app.search.selected.min(rows.len() - 1);
    let mut offset = app.search.offset.min(selected);
    let mut occupied: usize = rows[offset..=selected].iter().map(row_height).sum();
    while occupied > fits && offset < selected {
        occupied -= row_height(&rows[offset]);
        offset += 1;
    }
    let mut used = 0;
    let mut count = 0;
    for row in &rows[offset..] {
        if used >= fits {
            break;
        }
        used += row_height(row);
        count += 1;
    }
    (offset, count, fits)
}

pub(in crate::ui) fn shown_communities(app: &App, area: Rect) -> Vec<String> {
    if !app.search.shows_rows() {
        return Vec::new();
    }
    let (offset, fits, _) = row_window(app, area);
    app.search
        .rows()
        .into_iter()
        .skip(offset)
        .take(fits)
        .filter_map(|row| match row.target {
            FeedTarget::Subreddit(name) if crate::reddit::is_community(&name) => Some(name),
            _ => None,
        })
        .collect()
}

pub(in crate::ui) fn reading_key(app: &App, key: &str) -> String {
    if app.search_editing() {
        format!("Esc then {key}")
    } else {
        key.to_string()
    }
}

pub(in crate::ui) fn list_reading_key(app: &App, key: &str) -> String {
    if app.post_focused() {
        format!("C-w h then {key}")
    } else if app.focus == Focus::Sidebar && !app.search_editing() && key == "Esc" {
        "Esc then Esc".into()
    } else {
        reading_key(app, key)
    }
}

pub(super) fn retry_key(app: &App) -> Option<String> {
    if app.active != crate::app::SEARCH_TAB {
        None
    } else {
        Some(list_reading_key(app, "r"))
    }
}

pub(super) fn retry_message(app: &App, error: &str) -> String {
    match retry_key(app) {
        Some(key) => format!("{error} — press {key} to retry"),
        None => format!("{error} — return to Search to retry"),
    }
}

pub(in crate::ui) fn empty_hint(app: &App) -> String {
    let action = if app.active != crate::app::SEARCH_TAB || app.focus != Focus::List {
        "Focus Search results to change the query or kind"
    } else if crate::keys::map::search_kind_keys(app) {
        "Try other words or Tab for another kind. Reddit's search can briefly answer with nothing; r asks again"
    } else {
        "Try other words. Reddit's search can briefly answer with nothing; r asks again"
    };
    if app.config.show_nsfw {
        action.to_string()
    } else {
        format!("{NSFW_EMPTY_HINT} {action}.")
    }
}

pub(super) fn draw_rows(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    pictures: &mut ListPictures,
) -> Option<Rect> {
    let [title, _, body] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(area);
    draw_title(f, app, title);
    let (offset, shown, _) = row_window(app, area);
    app.fill_search_rows(offset + shown + shown.max(PAGE_LIMIT) >= app.search.rows().len());
    let (offset, fits, height) = row_window(app, area);
    app.search.offset = offset;
    let search = &app.search;
    let rows = search.rows();
    if rows.is_empty() {
        if search.rows_loading() {
            let label = match search.empty_retry() {
                Some((attempt, total)) => {
                    format!("Reddit sent no results · asking again ({attempt} of {total})")
                }
                None => "Searching Reddit".into(),
            };
            return loading::draw(f, body, loading::Kind::Initial, &label, app.loading_frame);
        }
        if let Some(error) = search.rows_error() {
            empty_state(f, body, "✗", "Search failed", &retry_message(app, error));
        } else {
            empty_state(f, body, "∅", "No matches", &empty_hint(app));
        }
        return None;
    }
    let focused = app.focus == Focus::List && app.active == crate::app::SEARCH_TAB;
    // A page loading or failing below the rows says so on the last line.
    let status = search.rows_status().map(|status| match status {
        RowsStatus::Discovering => vec![Span::styled(
            format!(
                "   {} loading suggested communities",
                loading::glyph(app.loading_frame)
            ),
            Style::new().fg(OVERLAY),
        )],
        RowsStatus::LoadingMore => vec![Span::styled(
            format!("   {} loading more", loading::glyph(app.loading_frame)),
            Style::new().fg(OVERLAY),
        )],
        RowsStatus::MoreFailed | RowsStatus::DiscoveryFailed => {
            let action = if status == RowsStatus::MoreFailed {
                "reloads"
            } else {
                "retries"
            };
            vec![
                Span::styled(
                    match retry_key(app) {
                        Some(key) => format!("   ✗ {key} {action}"),
                        None => "   ✗ return to Search to retry".into(),
                    },
                    Style::new().fg(RED),
                ),
                Span::styled(
                    format!("     {}", search.rows_error().unwrap()),
                    Style::new().fg(RED),
                ),
            ]
        }
    });
    let mut lines = Vec::new();
    for (index, row) in rows.iter().enumerate().skip(offset).take(fits) {
        let selected = index == search.selected;
        // The community's icon or the user's avatar covers the `# ` or `@` glyph, as in the sidebar.
        let icon = match (&row.target, &row.profile) {
            (FeedTarget::Subreddit(name), _) => app.shown_icon(name),
            (FeedTarget::User(_), Some(profile)) if app.thumbnails_on() => {
                app.usable_thumb(profile.icon.as_deref())
            }
            _ => None,
        };
        if let Some(url) = icon {
            pictures.thumbs.push(ImageSpot {
                url: url.to_string(),
                area: Rect {
                    x: body.x + 2,
                    y: body.y + lines.len() as u16,
                    width: ICON_COLS,
                    height: 1,
                },
            });
        }
        lines.extend(entry_lines(row, body.width, height, selected, focused));
    }
    if let Some(status) = status {
        lines.truncate(usize::from(body.height).saturating_sub(status.len()));
        lines.extend(status.into_iter().map(Line::from));
    }
    f.render_widget(Paragraph::new(lines), body);
    None
}
