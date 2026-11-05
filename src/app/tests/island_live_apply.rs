use super::*;

#[test]
fn a_settled_island_answer_keeps_the_selected_post_in_its_slot() {
    let mut app = app_with(&["vim"]);
    app.switch_tab(ALL_SUBS_TAB);
    app.on_msg(island_answer(&app, &["a", "b", "c", "d"]));
    app.all_subs.post = 2;
    app.on_msg(island_answer(&app, &["n", "a", "b", "x", "d"]));
    assert!(app.all_subs.has_new_posts(&app.config));

    app.change_setting(
        "filters.keywords",
        toml::Value::Array(vec!["title n".into()]),
    );

    assert!(!app.all_subs.has_new_posts(&app.config));
    assert_eq!(island_ids(&app), ["a", "b", "c"]);
    assert_eq!(app.current_post().unwrap().id, "c");
}

#[test]
fn navigation_cache_island_progress_shows_cold_posts_but_keeps_established_refreshes_stable() {
    let mut app = app_with(&["vim"]);
    app.switch_tab(ALL_SUBS_TAB);
    app.all_subs.groups[0].loading = true;
    let generation = app.all_subs.groups[0].generation;
    let progress = |id| Msg::GroupProgress {
        subreddit: "vim".into(),
        generation,
        post: Box::new(post(id, "vim")),
        source: Source::default(),
    };
    app.on_msg(progress("a"));
    app.on_msg(progress("b"));
    assert_eq!(app.current_post().unwrap().id, "a");
    assert_eq!(island_ids(&app), ["a", "b"]);
    assert!(app.all_subs.groups[0].loading);
    assert!(!app.all_subs.has_new_posts(&app.config));
    app.on_msg(island_answer(&app, &["a", "b"]));
    app.all_subs.groups[0].loading = true;
    app.on_msg(progress("new"));
    assert_eq!(island_ids(&app), ["a", "b"]);
    app.on_msg(island_answer(&app, &["new", "a", "b"]));
    assert!(app.all_subs.has_new_posts(&app.config));
}

#[test]
fn an_island_holds_every_unseen_post_while_one_ranks_above_a_shown_post() {
    let mut app = app_with(&["vim"]);
    app.config.all_subs_posts_per_group = 3;
    app.on_msg(island_answer(&app, &["a", "b", "c"]));

    app.on_msg(island_answer(&app, &["n1", "a", "b", "n2", "c"]));

    assert!(app.all_subs.has_new_posts(&app.config));
    assert_eq!(island_ids(&app), ["a", "b", "c"]);
}

#[test]
fn unseen_posts_below_the_top_enter_live_around_the_post_being_read() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    let tab = app.tabs[i].id;
    app.on_msg(Msg::Feed {
        tab,
        generation: app.tabs[i].feed.generation,
        append: false,
        result: Ok(listing("vim", &["a", "b"], Some("t3_b"))),
    });
    app.load_feed(i, true);
    app.on_msg(Msg::Feed {
        tab,
        generation: app.tabs[i].feed.generation,
        append: true,
        result: Ok(listing("vim", &["c", "d"], None)),
    });
    app.tabs[i].feed.selected = 1;
    app.tabs[i].feed.offset = 1;
    app.load_feed(i, false);

    app.on_msg(Msg::Feed {
        tab,
        generation: app.tabs[i].feed.generation,
        append: false,
        result: Ok(listing("vim", &["a", "t"], Some("t3_t"))),
    });

    assert!(!app.tabs[i].feed.has_new_posts(&app.config));
    assert_eq!(
        app.tabs[i]
            .feed
            .posts
            .iter()
            .map(|post| post.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b", "t", "c", "d"]
    );
    assert_eq!(app.current_post().unwrap().id, "b");
}

#[test]
fn an_island_that_loses_access_drops_its_new_posts_notice() {
    let mut app = app_with(&["vim"]);
    app.on_msg(island_answer(&app, &["a", "b"]));
    app.on_msg(island_answer(&app, &["n", "a", "b"]));
    assert!(app.all_subs.has_new_posts(&app.config));

    app.on_msg(Msg::Group {
        subreddit: "vim".into(),
        generation: app.all_subs.groups[0].generation,
        result: Err(FeedError::Access(AccessDenied::PrivateCommunity)),
    });

    assert!(!app.all_subs.has_new_posts(&app.config));
}

#[test]
fn a_live_island_answer_keeps_the_selected_post_in_its_slot() {
    let mut app = app_with(&["vim"]);
    app.switch_tab(ALL_SUBS_TAB);
    app.on_msg(island_answer(&app, &["a", "b", "c", "d"]));
    app.all_subs.post = 2;
    assert_eq!(app.current_post().unwrap().id, "c");

    app.on_msg(island_answer(&app, &["a", "x", "b", "c"]));

    assert!(!app.all_subs.has_new_posts(&app.config));
    assert_eq!(island_ids(&app), ["a", "x", "c"]);
    assert_eq!(app.current_post().unwrap().id, "c");

    // Nobody reads an island while All subs is not shown: it takes Reddit's order.
    app.switch_tab(0);
    app.on_msg(island_answer(&app, &["a", "x", "b", "c"]));
    assert_eq!(island_ids(&app), ["a", "x", "b"]);
}

#[test]
fn an_island_applies_unseen_posts_below_its_top_live() {
    let mut app = app_with(&["vim"]);
    app.on_msg(island_answer(&app, &["a", "b"]));

    app.on_msg(island_answer(&app, &["a", "m", "b"]));

    assert!(!app.all_subs.has_new_posts(&app.config));
    assert_eq!(island_ids(&app)[..2], ["a", "m"]);
}

#[test]
fn an_unseen_post_ending_the_first_page_applies_live_with_later_pages_loaded() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    let tab = app.tabs[i].id;
    app.on_msg(Msg::Feed {
        tab,
        generation: app.tabs[i].feed.generation,
        append: false,
        result: Ok(listing("vim", &["a", "b"], Some("t3_b"))),
    });
    app.load_feed(i, true);
    app.on_msg(Msg::Feed {
        tab,
        generation: app.tabs[i].feed.generation,
        append: true,
        result: Ok(listing("vim", &["c", "d"], None)),
    });
    app.load_feed(i, false);

    app.on_msg(Msg::Feed {
        tab,
        generation: app.tabs[i].feed.generation,
        append: false,
        result: Ok(listing("vim", &["a", "b", "t"], Some("t3_t"))),
    });

    assert!(!app.tabs[i].feed.has_new_posts(&app.config));
    assert_eq!(
        app.tabs[i]
            .feed
            .posts
            .iter()
            .map(|post| post.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b", "t", "c", "d"]
    );
}
