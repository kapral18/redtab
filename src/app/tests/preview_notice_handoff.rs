use super::*;

#[test]
fn post_notice_preview_entry_and_return_should_share_applied_and_waiting_posts() {
    let mut app = app_with(&[]);
    app.open_search(None);
    app.search_preview_drawn(true);
    app.tick_at(Instant::now() + NAV_SETTLE + search::PREVIEW_SETTLE);
    let generation = app.search.preview.generation;
    let preview = |ids: &[&str], after| Msg::Feed {
        tab: crate::worker::PREVIEW_FEED,
        generation,
        append: false,
        result: Ok(listing("popular", ids, after)),
    };
    app.on_msg(ordered_snapshot(
        preview(&["a", "b"], Some("old-end")),
        1,
        true,
        true,
    ));
    app.on_msg(ordered_snapshot(
        preview(&["leader", "b", "a"], Some("fresh-end")),
        2,
        false,
        false,
    ));
    assert_eq!(app.search.preview.posts[0].id, "a");
    app.open_search_row();
    let child = app.active;
    let tab = app.active_tab().id;
    assert_eq!(
        app.active_tab()
            .feed
            .posts
            .iter()
            .map(|p| p.id.as_str())
            .collect::<Vec<_>>(),
        ["leader", "b", "a"]
    );
    assert_eq!(app.active_tab().feed.after.as_deref(), Some("fresh-end"));
    assert!(!app.active_tab().feed.has_new_posts(&app.config));
    let feed = |app: &App, ids: &[&str]| Msg::Feed {
        tab,
        generation: app.tabs[child].feed.generation,
        append: false,
        result: Ok(listing("popular", ids, Some("fresh-end"))),
    };
    app.on_msg(ordered_snapshot(feed(&app, &["a", "b"]), 1, true, false));
    assert_eq!(app.active_tab().feed.posts[0].id, "leader");
    app.on_msg(ordered_snapshot(
        feed(&app, &["later", "leader", "b", "a"]),
        3,
        false,
        false,
    ));
    assert!(app.active_tab().feed.has_new_posts(&app.config));
    assert!(app.return_to_search());
    assert_eq!(app.search.preview.posts[0].id, "leader");
    assert_eq!(app.search.preview.update.as_ref().unwrap()[0].id, "later");
    app.open_search_row();
    assert_eq!(app.active_tab().feed.posts[0].id, "later");
    assert!(!app.active_tab().feed.has_new_posts(&app.config));
    assert!(app.return_to_search());
    assert_eq!(app.search.preview.posts[0].id, "later");
    assert!(app.search.preview.update.is_none());
}

#[test]
fn post_notice_handoff_should_preserve_newer_feeds_and_distinct_queries() {
    let mut app = app_with(&["rust"]);
    let child = tab_index(&app, "rust");
    app.switch_tab(child);
    app.on_msg(ordered_snapshot(
        Msg::Feed {
            tab: app.active_tab().id,
            generation: app.active_tab().feed.generation,
            append: false,
            result: Ok(listing("rust", &["newer"], None)),
        },
        5,
        false,
        false,
    ));
    app.open_search(None);
    app.search.discovered.entries.push(search::Row {
        profile: None,
        label: "r/rust".into(),
        detail: String::new(),
        target: FeedTarget::Subreddit("rust".into()),
    });
    app.search.selected = 2;
    app.search_preview_drawn(true);
    app.tick_at(Instant::now() + NAV_SETTLE + search::PREVIEW_SETTLE);
    let generation = app.search.preview.generation;
    app.on_msg(ordered_snapshot(
        Msg::Feed {
            tab: crate::worker::PREVIEW_FEED,
            generation,
            append: false,
            result: Ok(listing("rust", &["older"], None)),
        },
        4,
        false,
        false,
    ));
    app.open_search_row();
    assert_eq!(app.active_tab().feed.posts[0].id, "newer");
    assert!(app.return_to_search());
    assert_eq!(app.search.preview.posts[0].id, "newer");

    app.tabs[child].feed.sort = Sort::Top;
    app.load_feed(child, false);
    app.on_msg(ordered_snapshot(
        Msg::Feed {
            tab: app.tabs[child].id,
            generation: app.tabs[child].feed.generation,
            append: false,
            result: Ok(listing("rust", &["top-result"], None)),
        },
        6,
        false,
        false,
    ));
    let preview = app.search.preview.posts.clone();
    app.open_search_row();
    assert_eq!(app.active_tab().feed.sort, Sort::Top);
    assert_eq!(app.active_tab().feed.posts[0].id, "top-result");
    assert!(app.return_to_search());
    assert_eq!(app.search.preview.posts, preview);
}

#[test]
fn post_notice_preview_should_remember_loaded_feed_pages_when_ranking_changes() {
    let mut app = app_with(&["rust"]);
    let child = tab_index(&app, "rust");
    app.switch_tab(child);
    app.on_msg(ordered_snapshot(
        Msg::Feed {
            tab: app.active_tab().id,
            generation: app.active_tab().feed.generation,
            append: false,
            result: Ok(listing("rust", &["a", "b"], Some("page2"))),
        },
        1,
        false,
        false,
    ));
    app.load_feed(child, true);
    app.on_msg(ordered_snapshot(
        Msg::Feed {
            tab: app.active_tab().id,
            generation: app.active_tab().feed.generation,
            append: true,
            result: Ok(listing("rust", &["tail"], None)),
        },
        2,
        false,
        false,
    ));
    app.open_search(None);
    app.search.discovered.entries.push(search::Row {
        profile: None,
        label: "r/rust".into(),
        detail: String::new(),
        target: FeedTarget::Subreddit("rust".into()),
    });
    app.search.selected = 2;
    app.search_preview_drawn(true);
    app.tick_at(Instant::now() + NAV_SETTLE + search::PREVIEW_SETTLE);
    let generation = app.search.preview.generation;
    assert!(app.search.preview.loading);
    app.on_msg(ordered_snapshot(
        Msg::Feed {
            tab: crate::worker::PREVIEW_FEED,
            generation,
            append: false,
            result: Ok(listing("rust", &["a", "b"], Some("page2"))),
        },
        1,
        true,
        true,
    ));
    app.on_msg(ordered_snapshot(
        Msg::Feed {
            tab: crate::worker::PREVIEW_FEED,
            generation,
            append: false,
            result: Ok(listing("rust", &["tail", "a", "b"], None)),
        },
        3,
        false,
        false,
    ));
    assert!(!app.search.preview.has_new_posts(&app.config));
    assert_eq!(
        app.search
            .preview
            .posts
            .iter()
            .map(|p| p.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b", "tail"]
    );
}

#[test]
fn post_notice_return_should_not_erase_a_newer_preview_failure() {
    let mut app = app_with(&["rust"]);
    let child = tab_index(&app, "rust");
    app.switch_tab(child);
    app.on_msg(ordered_snapshot(
        Msg::Feed {
            tab: app.active_tab().id,
            generation: app.active_tab().feed.generation,
            append: false,
            result: Ok(listing("rust", &["newer"], None)),
        },
        2,
        false,
        false,
    ));
    app.open_search(None);
    app.search.discovered.entries.push(search::Row {
        profile: None,
        label: "r/rust".into(),
        detail: String::new(),
        target: FeedTarget::Subreddit("rust".into()),
    });
    app.search.selected = 2;
    app.search_preview_drawn(true);
    app.tick_at(Instant::now() + NAV_SETTLE + search::PREVIEW_SETTLE);
    let generation = app.search.preview.generation;
    app.on_msg(ordered_snapshot(
        Msg::Feed {
            tab: crate::worker::PREVIEW_FEED,
            generation,
            append: false,
            result: Ok(listing("rust", &["older"], None)),
        },
        1,
        true,
        true,
    ));
    app.on_msg(ordered_snapshot(
        Msg::Feed {
            tab: crate::worker::PREVIEW_FEED,
            generation,
            append: false,
            result: Err("still offline".into()),
        },
        3,
        false,
        false,
    ));
    app.open_search_row();
    assert!(app.return_to_search());
    assert_eq!(app.search.preview.posts[0].id, "newer");
    assert_eq!(app.search.preview.error.as_deref(), Some("still offline"));

    app.load_feed(child, false);
    app.on_msg(ordered_snapshot(
        Msg::Feed {
            tab: app.tabs[child].id,
            generation: app.tabs[child].feed.generation,
            append: false,
            result: Ok(listing("rust", &["newer"], None)),
        },
        4,
        false,
        false,
    ));
    app.open_search_row();
    assert!(app.return_to_search());
    assert!(app.search.preview.error.is_none());
}

#[test]
fn post_notice_preview_should_show_arrivals_when_its_old_posts_are_hidden() {
    let mut app = app_with(&[]);
    app.config.show_nsfw = false;
    app.open_search(None);
    app.search_preview_drawn(true);
    app.tick_at(Instant::now() + NAV_SETTLE + search::PREVIEW_SETTLE);
    let generation = app.search.preview.generation;
    let mut hidden = listing("popular", &["hidden"], None);
    hidden.posts[0].nsfw = true;
    app.on_msg(ordered_snapshot(
        Msg::Feed {
            tab: crate::worker::PREVIEW_FEED,
            generation,
            append: false,
            result: Ok(hidden),
        },
        1,
        true,
        true,
    ));
    app.on_msg(ordered_snapshot(
        Msg::Feed {
            tab: crate::worker::PREVIEW_FEED,
            generation,
            append: false,
            result: Ok(listing("popular", &["visible"], None)),
        },
        2,
        false,
        false,
    ));
    assert_eq!(
        app.config
            .visible(&app.search.preview.posts)
            .map(|p| p.id.as_str())
            .collect::<Vec<_>>(),
        ["visible"]
    );
    assert!(app.search.preview.update.is_none());
}

#[test]
fn preview_refresh_should_apply_waiting_posts_without_resetting_selected_rows() {
    use crate::reddit::experience::SearchKind;
    for searched in [false, true] {
        let mut app = app_with(&[]);
        app.open_search(None);
        let row = search::Row {
            profile: None,
            label: "r/rust".into(),
            detail: String::new(),
            target: FeedTarget::Subreddit("rust".into()),
        };
        if searched {
            app.search.kind = SearchKind::Communities;
            app.search.query.value = "rust".into();
            app.search.results.entries.push(row);
        } else {
            app.search.discovered.entries.push(row);
            app.search.selected = 2;
        }
        app.search.preview.target = Some(FeedTarget::Subreddit("rust".into()));
        app.search.preview.posts = listing("rust", &["a"], None).posts;
        app.search.preview.update = Some(listing("rust", &["n", "a"], None).posts);
        let rows = app.search.rows();
        let selected = app.search.selected;
        app.reload_search();
        assert_eq!(app.search.preview.posts[0].id, "n");
        assert_eq!(app.search.rows(), rows);
        assert_eq!(app.search.selected, selected);
    }
}

#[test]
fn reported_islands_and_search_previews_should_hold_live_membership() {
    let mut app = app_with(&["vim"]);
    app.switch_tab(ALL_SUBS_TAB);
    app.on_msg(ordered_snapshot(
        island_answer(&app, &["a", "b"]),
        1,
        true,
        true,
    ));
    app.on_msg(ordered_snapshot(
        island_answer(&app, &["n", "b", "a"]),
        2,
        false,
        false,
    ));
    assert_eq!(island_ids(&app), ["a", "b"]);
    assert!(app.all_subs.has_new_posts(&app.config));
    app.reload();
    assert_eq!(island_ids(&app), ["n", "b", "a"]);

    app.open_search(None);
    app.search_preview_drawn(true);
    app.tick_at(Instant::now() + search::PREVIEW_SETTLE);
    let generation = app.search.preview.generation;
    let answer = |names: &[&str]| Msg::Feed {
        tab: crate::worker::PREVIEW_FEED,
        generation,
        append: false,
        result: Ok(listing("popular", names, None)),
    };
    app.on_msg(ordered_snapshot(answer(&["a", "b"]), 3, true, true));
    app.on_msg(ordered_snapshot(answer(&["n", "b", "a"]), 4, false, false));
    assert_eq!(
        app.search
            .preview
            .posts
            .iter()
            .map(|post| post.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    app.reload_search();
    assert_eq!(
        app.search
            .preview
            .posts
            .iter()
            .map(|post| post.id.as_str())
            .collect::<Vec<_>>(),
        ["n", "b", "a"]
    );
    assert_eq!(
        app.search.preview.generation, generation,
        "apply the held preview without another request"
    );
}
