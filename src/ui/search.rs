//! The Search tab: query bar with kind tabs, result or Explore rows, and the feed preview.

use super::*;
use crate::app::search::RowsStatus;
use crate::reddit::experience::{PAGE_LIMIT, SearchKind};
use crate::worker::FeedTarget;

const KINDS: [(SearchKind, &str); 3] = [
    (SearchKind::Posts, "Posts"),
    (SearchKind::Communities, "Communities"),
    (SearchKind::People, "People"),
];

/// Below this width the kind tabs move under the query.
const ONE_BAR: u16 = 64;
const TEXT_START: usize = ICON_COLS as usize + 3;

/// The query and kind tabs across the whole tab, above the list and the preview;
/// returns the area below.
pub(super) fn draw_bar(f: &mut Frame, app: &App, area: Rect, margin: Margin) -> Rect {
    let inner = area.inner(margin);
    let one_line = inner.width >= ONE_BAR;
    let [bar, rest] = Layout::vertical([
        Constraint::Length(if one_line { 1 } else { 2 }),
        Constraint::Min(0),
    ])
    .areas(area);
    let bar = bar.inner(margin);
    if one_line {
        let [input, kinds] =
            Layout::horizontal([Constraint::Min(1), Constraint::Length(kinds_width())]).areas(bar);
        draw_input(f, app, input);
        draw_kinds(f, app, kinds);
    } else {
        let [input, kinds] =
            Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(bar);
        draw_input(f, app, input);
        draw_kinds(f, app, kinds);
    }
    rest
}

/// The list pane below the bar; returns the area showing a loader, as `draw_feed` does.
pub(super) fn draw(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    pictures: &mut ListPictures,
) -> Option<Rect> {
    if app.search.shows_rows() {
        draw_rows(f, app, area, pictures)
    } else {
        draw_feed(f, app, area, pictures)
    }
}

fn kind_label(kind: SearchKind) -> &'static str {
    KINDS
        .iter()
        .find(|(candidate, _)| *candidate == kind)
        .map_or("Posts", |(_, label)| label)
}

fn kinds_width() -> u16 {
    let labels: usize = KINDS.iter().map(|(_, label)| label.width() + 2).sum();
    (labels + KINDS.len() - 1 + 1) as u16
}

fn draw_input(f: &mut Frame, app: &App, area: Rect) {
    let editing = app.search_editing();
    let field = &app.search.query;
    let style = Style::new()
        .fg(TEXT)
        .bg(if editing { SURFACE1 } else { SURFACE0 });
    let mut spans = vec![Span::styled(
        " 🔍 ",
        style.fg(if editing { ACCENT } else { OVERLAY }),
    )];
    if field.value.is_empty() && !editing {
        spans.push(Span::styled(
            format!(
                "Search {}{}",
                kind_label(app.search.kind).to_lowercase(),
                if app.active == crate::app::SEARCH_TAB {
                    " (/ to type)"
                } else {
                    ""
                }
            ),
            style.fg(OVERLAY),
        ));
    } else if field.value.is_empty() {
        spans.push(Span::styled("▏", style.fg(ACCENT)));
        spans.push(Span::styled(
            format!("Search {}", kind_label(app.search.kind).to_lowercase()),
            style.fg(OVERLAY),
        ));
    } else {
        let room = usize::from(area.width).saturating_sub(5);
        let (before, after) = field.value.split_at(field.cursor);
        let shown_before = input_tail(before, room.saturating_sub(after.width().min(room / 2)));
        spans.push(Span::styled(shown_before, style));
        if editing {
            spans.push(Span::styled("▏", style.fg(ACCENT)));
        }
        let left = room.saturating_sub(line_width(&spans).saturating_sub(4));
        spans.push(Span::styled(truncate(after, left), style));
    }
    pad_to(&mut spans, usize::from(area.width), style);
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The kinds read as tabs: the one a query searches is lit, Tab / S-Tab move it.
/// Explore shows no kind's results, so there the chosen kind is only bold.
fn draw_kinds(f: &mut Frame, app: &App, area: Rect) {
    let exploring = app.search.exploring();
    let mut spans = vec![Span::raw(" ")];
    for (i, (kind, label)) in KINDS.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("│", Style::new().fg(SURFACE1)));
        }
        let style = if kind == app.search.kind && exploring {
            Style::new().fg(TEXT).add_modifier(Modifier::BOLD)
        } else if kind == app.search.kind {
            Style::new()
                .fg(ACCENT)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::new().fg(SUBTEXT)
        };
        spans.push(Span::styled(format!(" {label} "), style));
    }
    f.render_widget(
        Paragraph::new(Line::from(fit_spans(spans, area.width.into()))),
        area,
    );
}

/// What the rows are; the footer carries the keys. An Explore subtitle that does
/// not fit whole is left out rather than cut.
fn draw_title(f: &mut Frame, app: &App, area: Rect) {
    let search = &app.search;
    let width = usize::from(area.width);
    let mut spans = if search.exploring() {
        let mut spans = vec![Span::styled(
            " Explore",
            Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
        )];
        let subtitle = "  popular feeds and suggested communities";
        if line_width(&spans) + subtitle.width() <= width {
            spans.push(Span::styled(subtitle, Style::new().fg(SUBTEXT)));
        }
        spans
    } else {
        vec![
            Span::styled(
                format!(" {}", kind_label(search.kind)),
                Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  matching “{}”", search.text()),
                Style::new().fg(SUBTEXT),
            ),
        ]
    };
    spans = fit_spans(spans, width);
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Posts results: the same title line as rows, then the feed's sort, time and status chips.
pub(super) fn feed_header(f: &mut Frame, app: &App, area: Rect, chips: Vec<Span<'static>>) -> Rect {
    let [title, status, body] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(area);
    draw_title(f, app, title);
    let mut spans = vec![Span::raw(" ")];
    for (index, chip) in chips.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw(" "));
        }
        spans.push(chip);
    }
    f.render_widget(
        Paragraph::new(Line::from(fit_spans(spans, area.width.into()))),
        status,
    );
    body
}

fn entry_lines(
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

fn row_window(app: &App, area: Rect) -> (usize, usize, usize) {
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

pub(super) fn shown_communities(app: &App, area: Rect) -> Vec<String> {
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

pub(super) fn reading_key(app: &App, key: &str) -> String {
    if app.search_editing() {
        format!("Esc then {key}")
    } else {
        key.to_string()
    }
}

pub(super) fn list_reading_key(app: &App, key: &str) -> String {
    if app.post_focused() {
        format!("C-w h then {key}")
    } else if app.focus == Focus::Sidebar && !app.search_editing() && key == "Esc" {
        "Esc then Esc".into()
    } else {
        reading_key(app, key)
    }
}

fn retry_key(app: &App) -> Option<String> {
    if app.active != crate::app::SEARCH_TAB {
        None
    } else {
        Some(list_reading_key(app, "r"))
    }
}

fn retry_message(app: &App, error: &str) -> String {
    match retry_key(app) {
        Some(key) => format!("{error} — press {key} to retry"),
        None => format!("{error} — return to Search to retry"),
    }
}

pub(super) fn empty_hint(app: &App) -> String {
    let action = if app.active != crate::app::SEARCH_TAB || app.focus != Focus::List {
        "Focus Search results to change the query or kind"
    } else if crate::keys::map::search_kind_keys(app) {
        "Try other words, or Tab for another kind"
    } else {
        "Try other words"
    };
    if app.config.show_nsfw {
        action.to_string()
    } else {
        format!("{NSFW_EMPTY_HINT} {action}.")
    }
}

fn draw_rows(
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
            return loading::draw(
                f,
                body,
                loading::Kind::Initial,
                "Searching Reddit",
                app.loading_frame,
            );
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

/// The preview pane: the selected row's feed drawn like any feed, Card or Compact,
/// with its pictures, starting at its top and choosing no post.
pub(super) fn draw_preview(
    f: &mut Frame,
    app: &App,
    area: Rect,
    pictures: &mut ListPictures,
) -> Option<Rect> {
    let search = &app.search;
    // With no rows the list pane says why; the preview stays blank beside it.
    let row = search.selected_row()?;
    let name = match &row.target {
        FeedTarget::Subreddit(name) => format!("r/{name}"),
        FeedTarget::User(name) => format!("u/{name}"),
        FeedTarget::Home => "Home".into(),
    };
    let preview = &search.preview;
    let current = preview.target.as_ref() == Some(&row.target);
    // While typing, Enter leaves the query; it opens only from the results.
    let opens = app.focus == Focus::List && app.overlay.is_none() && !app.search_editing();
    let mut chips = vec![
        chip("hot", SUBTEXT, SURFACE0),
        chip(
            if opens {
                "Enter opens"
            } else {
                "focus results"
            },
            OVERLAY,
            SURFACE0,
        ),
    ];
    if current && preview.has_new_posts(&app.config) {
        chips.insert(0, new_posts_chip(app));
    }
    let stale = current && preview.error.is_some() && !preview.posts.is_empty();
    if stale {
        chips.splice(
            0..0,
            [
                chip("✗ refresh failed", RED, SURFACE0),
                chip(
                    match retry_key(app) {
                        Some(key) => format!("{key} retry"),
                        None => "return to Search to retry".into(),
                    },
                    RED,
                    SURFACE0,
                ),
            ],
        );
    }
    let mut body = view_header(f, area, &name, chips);
    if let Some(profile) = &row.profile {
        body = draw_profile(f, app, body, profile, pictures, None);
    }
    if !current || (preview.loading && preview.posts.is_empty()) {
        return loading::draw(
            f,
            body,
            loading::Kind::Initial,
            "Loading preview",
            app.loading_frame,
        );
    }
    if let Some(error) = preview.error.as_ref().filter(|_| !stale) {
        empty_state(f, body, "✗", "Preview failed", &retry_message(app, error));
        return None;
    }
    let posts: Vec<&Post> = app.config.visible(&preview.posts).collect();
    if posts.is_empty() {
        empty_state(
            f,
            body,
            "∅",
            "No posts",
            if opens {
                "Enter opens the feed"
            } else {
                "Focus Search results to open the feed"
            },
        );
        return None;
    }
    let mut spec = ListSpec {
        posts: &posts,
        selected: 0,
        show_selection: false,
        show_subreddit: !matches!(row.target, FeedTarget::Subreddit(ref name) if crate::reddit::is_community(name)),
        offset: 0,
        scroll: CardScroll::default(),
        append_area: None,
    };
    draw_post_list(f, app, body, &mut spec, pictures);
    None
}
