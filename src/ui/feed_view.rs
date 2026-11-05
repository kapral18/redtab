use super::*;

/// A user feed's profile block: the karma line and up to two description lines.
pub(super) const USER_PROFILE_ROWS: usize = 3;

/// A user's profile page summary above their posts: the avatar beside the karma line,
/// then the description, wrapped to at most a third of the pane.
pub(super) fn draw_profile(
    f: &mut Frame,
    app: &App,
    area: Rect,
    profile: &crate::model::UserProfile,
    pictures: &mut ListPictures,
    fixed: Option<usize>,
) -> Rect {
    const TEXT_START: usize = ICON_COLS as usize + 3;
    let text_width = usize::from(area.width).saturating_sub(TEXT_START).max(1);
    let limit = usize::from(area.height / 3).min(fixed.unwrap_or(usize::MAX));
    let mut wrapped: Vec<String> = profile
        .lines
        .iter()
        .flat_map(|line| {
            textwrap::wrap(
                line,
                textwrap::Options::new(text_width)
                    .word_splitter(textwrap::WordSplitter::NoHyphenation),
            )
            .into_iter()
            .map(|line| line.into_owned())
            .collect::<Vec<_>>()
        })
        .collect();
    if wrapped.is_empty() || limit == 0 {
        return area;
    }
    if wrapped.len() > limit {
        wrapped.truncate(limit);
        if let Some(last) = wrapped.last_mut() {
            *last = truncate(&format!("{last}…"), text_width);
        }
    }
    let lines: Vec<Line> = wrapped
        .into_iter()
        .enumerate()
        .map(|(index, text)| {
            Line::from(vec![
                Span::styled(
                    if index == 0 {
                        format!("  {:width$} ", "@", width = usize::from(ICON_COLS))
                    } else {
                        " ".repeat(TEXT_START)
                    },
                    Style::new().fg(OVERLAY),
                ),
                Span::styled(
                    text,
                    if index == 0 {
                        Style::new().fg(TEXT).add_modifier(Modifier::BOLD)
                    } else {
                        Style::new().fg(SUBTEXT)
                    },
                ),
            ])
        })
        .collect();
    let height = fixed.map_or(lines.len(), |_| limit) as u16;
    let [head, _, rest] = Layout::vertical([
        Constraint::Length(height),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(area);
    if app.thumbnails_on()
        && let Some(url) = app.usable_thumb(profile.icon.as_deref())
    {
        pictures.thumbs.push(ImageSpot {
            url: url.to_string(),
            area: Rect {
                x: head.x + 2,
                y: head.y,
                width: ICON_COLS,
                height: 1,
            },
        });
    }
    f.render_widget(Paragraph::new(lines), head);
    rest
}

pub(super) fn draw_feed(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    pictures: &mut ListPictures,
) -> Option<Rect> {
    let waiting = app
        .reddit
        .pause()
        .map(|pause| format!("Waiting for Reddit · {}s", pause.seconds));
    let tab = app.active_tab();
    let feed = &tab.feed;
    let access = tab.subreddit().and_then(|_| feed.access_denied());
    let mut chips = vec![list_view_chip(app)];
    if access.is_none() && feed.has_new_posts(&app.config) {
        chips.insert(0, new_posts_chip(app));
    }
    let search_tab = matches!(tab.kind, TabKind::Search);
    let retry = search::list_reading_key(app, "r");
    match &feed.search {
        Some(q) => {
            if !search_tab {
                chips.push(chip(format!("🔍 {q}"), SKY, SURFACE0));
            }
            chips.push(chip(
                format!("⇅ {}", feed.search_sort.as_str()),
                SUBTEXT,
                SURFACE0,
            ));
            chips.push(chip(
                format!("◷ {}", feed.search_time.as_str()),
                SUBTEXT,
                SURFACE0,
            ));
        }
        None => {
            chips.push(sort_chip(feed.sort));
            if feed.sort.takes_time() {
                chips.push(chip(format!("◷ {}", feed.time.as_str()), SUBTEXT, SURFACE0));
            }
        }
    }
    if let Some(name) = tab
        .subreddit()
        .filter(|name| crate::reddit::is_community(name) && access.is_none())
    {
        chips.push(membership_chip(
            app,
            &crate::model::MembershipTarget::Community(name.into()),
        ));
    }
    if let TabKind::User { name, .. } = &tab.kind {
        chips.push(membership_chip(
            app,
            &crate::model::MembershipTarget::User(name.clone()),
        ));
    }
    let badge = app.badge(tab);
    if access.is_none()
        && let Some(badge) = unread_chip(badge)
    {
        chips.push(badge);
    }
    if feed.loading() && !feed.loading_more() && !feed.receiving() && !feed.posts.is_empty() {
        chips.push(chip(
            format!("{} loading", loading::glyph(app.loading_frame)),
            OVERLAY,
            SURFACE0,
        ));
    }
    if access.is_some() {
        chips.insert(0, chip("Private community", YELLOW, SURFACE0));
    }
    if access.is_none() && feed.error().is_some() && !feed.posts.is_empty() {
        let label = if feed.page_failed() {
            "✗ next page failed"
        } else {
            "✗ reload failed"
        };
        chips.splice(
            0..0,
            [
                chip(label, RED, SURFACE0),
                chip(format!("{retry} retry"), RED, SURFACE0),
            ],
        );
    }
    if let Some(freshness) = feed.freshness.filter(|_| access.is_none()) {
        chips.push(chip(
            freshness.label(
                crate::app::now(),
                (feed.loading() || feed.refreshing()) && !feed.loading_more(),
                feed.error().is_some(),
                app.health == Health::Down,
            ),
            OVERLAY,
            SURFACE0,
        ));
    }
    let title = tab.title();
    let community = tab.subreddit();
    let error = feed.error();
    let posts: Vec<&Post> = app.config.visible(&feed.posts).collect();
    let selected = feed.selected;
    let current = feed.shows_current_query();
    let offset = feed.offset;
    let scroll = feed.cards.clone();
    let mut body = if search_tab {
        search::feed_header(f, app, area, chips)
    } else {
        view_header(f, area, &title, chips)
    };
    // A fixed block, loading or not, keeps the list still when the profile arrives.
    if let TabKind::User { name, .. } = &tab.kind {
        let status;
        let profile = match app.user_profile(name) {
            Some(profile) => profile,
            None => {
                status = crate::model::UserProfile::new(
                    None,
                    &if app.profile_failed(name) {
                        format!("Profile unavailable — press {retry} to retry")
                    } else {
                        "Loading profile…".into()
                    },
                );
                &status
            }
        };
        body = draw_profile(f, app, body, profile, pictures, Some(USER_PROFILE_ROWS));
    }
    if app.reddit.needs_sign_in() && posts.is_empty() {
        empty_state(
            f,
            body,
            "○",
            "Connect Reddit",
            &format!(
                "Press {} to connect your account",
                search::reading_key(app, "SPC p a")
            ),
        );
        return None;
    }
    if access.is_some() {
        let compact = body.width < 36 || body.height < 8;
        let open = search::list_reading_key(app, "o");
        let guidance = if compact {
            format!("Approved members only · {open} Reddit")
        } else {
            format!(
                "Only approved members can view this community. Press {open} to open Reddit and check access."
            )
        };
        empty_state(f, body, "🔒", "Private community", &guidance);
        return None;
    }
    if let Some(e) = error.filter(|_| posts.is_empty()) {
        let title = if feed.page_failed() {
            "Could not load more posts"
        } else {
            "Could not load posts"
        };
        empty_state(
            f,
            body,
            "✗",
            title,
            &format!("{e} — press {retry} to retry"),
        );
        return None;
    }
    if posts.is_empty() {
        // Until the current sort or search answers, an earlier empty answer says nothing.
        if !current || feed.receiving() {
            return loading::draw(
                f,
                body,
                loading::Kind::Initial,
                waiting.as_deref().unwrap_or("Loading posts"),
                app.loading_frame,
            );
        } else if !feed.posts.is_empty() {
            empty_state(
                f,
                body,
                "∅",
                "Every loaded post is hidden",
                "F2 Settings: check Show NSFW and hidden keywords, flairs or authors",
            );
        } else if search_tab {
            empty_content_state(
                f,
                body,
                "No results",
                &search::empty_hint(app),
                app.config.show_nsfw,
            );
        } else if feed.search.is_some() {
            let edit = search::list_reading_key(app, "/");
            let clear = search::list_reading_key(app, "Esc");
            let action = format!("{edit} edits the search, {clear} clears it");
            let hint = if app.config.show_nsfw {
                action
            } else {
                format!("{NSFW_EMPTY_HINT} {action}.")
            };
            empty_content_state(f, body, "No results", &hint, app.config.show_nsfw);
        } else if !app.config.show_nsfw {
            empty_content_state(f, body, "No visible posts", NSFW_EMPTY_HINT, false);
        } else {
            let sort = search::list_reading_key(app, "SPC f s");
            let source = search::list_reading_key(app, "SPC o c");
            empty_state(
                f,
                body,
                "∅",
                "No posts here",
                &if matches!(tab.kind, TabKind::User { .. }) {
                    format!("{sort} sorts · {source} opens this profile")
                } else {
                    format!("{sort} changes sort; SPC b b opens a subreddit")
                },
            );
        }
        return None;
    }
    let show_subreddit = matches!(
        tab.kind,
        TabKind::Home | TabKind::User { .. } | TabKind::Search
    ) || matches!(community, Some("all" | "popular"));
    let mut spec = ListSpec {
        posts: &posts,
        selected,
        show_selection: true,
        show_subreddit,
        offset,
        scroll,
        append_area: None,
    };
    let last_visible = draw_post_list(f, app, body, &mut spec, pictures);
    let (selected, offset, scroll, append_area) =
        (spec.selected, spec.offset, spec.scroll, spec.append_area);
    app.set_list_viewport(selected, offset, scroll, last_visible);
    let append_area = append_area?;
    let feed = &app.active_tab().feed;
    if feed.loading_more() || feed.receiving() {
        loading::draw(
            f,
            append_area,
            loading::Kind::More,
            waiting.as_deref().unwrap_or("Loading posts"),
            app.loading_frame,
        )
    } else if feed.page_failed() {
        draw_pagination_error(
            f,
            append_area,
            &format!("✗ Next page failed · {retry} retries"),
        );
        None
    } else if let Some((label, _)) = feed_activity(feed, !app.reddit.needs_sign_in()) {
        let kind = if !feed.shows_current_query() {
            loading::Kind::Initial
        } else {
            loading::Kind::Background
        };
        loading::draw(
            f,
            append_area,
            kind,
            waiting.as_deref().unwrap_or(label),
            app.loading_frame,
        )
    } else {
        None
    }
}

pub(super) fn draw_bookmarks(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    pictures: &mut ListPictures,
) {
    let count = app.visible_bookmarks().count();
    let body = view_header(
        f,
        area,
        "★ Bookmarks",
        vec![
            list_view_chip(app),
            chip(
                format!("{count} bookmark{}", if count == 1 { "" } else { "s" }),
                SUBTEXT,
                SURFACE0,
            ),
        ],
    );
    let posts: Vec<&Post> = app.visible_bookmarks().map(|b| &b.post).collect();
    if posts.is_empty() {
        empty_state(
            f,
            body,
            "★",
            if count == 0 && !app.state.bookmarks().is_empty() {
                "No visible bookmarks"
            } else {
                "No bookmarks yet"
            },
            if count == 0 && !app.state.bookmarks().is_empty() {
                NSFW_EMPTY_HINT
            } else {
                "press b on any post to keep it here"
            },
        );
        return;
    }
    let selected = app.active_tab().feed.selected.min(posts.len() - 1);
    let mut spec = ListSpec {
        posts: &posts,
        selected,
        show_selection: true,
        show_subreddit: true,
        offset: app.active_tab().feed.offset,
        scroll: app.active_tab().feed.cards.clone(),
        append_area: None,
    };
    let last_visible = draw_post_list(f, app, body, &mut spec, pictures);
    let (selected, offset, scroll) = (spec.selected, spec.offset, spec.scroll);
    app.set_list_viewport(selected, offset, scroll, last_visible);
}
