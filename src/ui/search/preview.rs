use super::*;

/// Posts results: the same title line as rows, then the feed's sort, time and status chips.
pub(in crate::ui) fn feed_header(
    f: &mut Frame,
    app: &App,
    area: Rect,
    chips: Vec<Span<'static>>,
) -> Rect {
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

/// The preview pane: the selected row's feed drawn like any feed, Card or Compact,
/// with its pictures, starting at its top and choosing no post.
pub(in crate::ui) fn draw_preview(
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
