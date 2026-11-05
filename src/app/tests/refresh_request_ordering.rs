use super::*;

#[test]
fn cached_paging_cannot_undo_a_newer_live_refresh() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    let tab = app.tabs[i].id;
    let mut fresh = listing("vim", &["b", "x"], Some("x"));
    fresh.posts[0].title = "updated title".into();
    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Feed {
            tab,
            generation: app.tabs[i].feed.generation,
            append: false,
            result: Ok(fresh),
        }),
        fetched_at: now(),
        refreshing: false,
        source: Source {
            cached: false,
            ..Source::default()
        },
    });
    app.load_feed(i, true);
    let generation = app.tabs[i].feed.generation;

    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Feed {
            tab,
            generation,
            append: true,
            result: Ok(listing("vim", &["b", "c"], None)),
        }),
        fetched_at: now() - 1,
        refreshing: false,
        source: Source {
            cached: true,
            ..Source::default()
        },
    });

    assert_eq!(
        app.tabs[i]
            .feed
            .posts
            .iter()
            .map(|p| p.id.as_str())
            .collect::<Vec<_>>(),
        ["b", "x", "c"]
    );
    assert_eq!(app.tabs[i].feed.posts[0].title, "updated title");
    let mut newer = listing("vim", &["b", "c"], None);
    newer.posts[0].title = "newest title".into();
    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Feed {
            tab,
            generation,
            append: true,
            result: Ok(newer),
        }),
        fetched_at: now() + 1,
        refreshing: false,
        source: Source {
            cached: false,
            ..Source::default()
        },
    });
    app.reload();
    assert_eq!(
        app.tabs[i]
            .feed
            .posts
            .iter()
            .map(|p| p.id.as_str())
            .collect::<Vec<_>>(),
        ["x", "b", "c"]
    );
    assert_eq!(app.tabs[i].feed.posts[1].title, "newest title");
}

#[test]
fn clock_rollback_does_not_suppress_a_live_page_update() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    let tab = app.tabs[i].id;
    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Feed {
            tab,
            generation: app.tabs[i].feed.generation,
            append: false,
            result: Ok(listing("vim", &["a", "b"], Some("b"))),
        }),
        fetched_at: 1000,
        refreshing: false,
        source: Source {
            cached: false,
            ..Source::default()
        },
    });
    app.load_feed(i, true);
    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Feed {
            tab,
            generation: app.tabs[i].feed.generation,
            append: true,
            result: Ok(listing("vim", &["c", "d"], None)),
        }),
        fetched_at: 1000,
        refreshing: false,
        source: Source {
            cached: false,
            ..Source::default()
        },
    });
    app.reload();
    let mut updated = listing("vim", &["c", "a"], Some("a"));
    updated.posts[0].title = "updated after clock correction".into();

    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Feed {
            tab,
            generation: app.tabs[i].feed.generation,
            append: false,
            result: Ok(updated),
        }),
        fetched_at: 900,
        refreshing: false,
        source: Source {
            cached: false,
            ..Source::default()
        },
    });

    assert_eq!(
        app.tabs[i].feed.posts[0].title,
        "updated after clock correction"
    );
    assert_eq!(
        app.tabs[i]
            .feed
            .posts
            .iter()
            .map(|p| p.id.as_str())
            .collect::<Vec<_>>(),
        ["c", "a", "b", "d"]
    );
}

#[test]
fn a_home_refresh_supersedes_an_older_feed_revalidation() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    let tab = app.tabs[i].id;
    let first = app.tabs[i].feed.generation;
    app.on_msg(ordered_snapshot(
        Msg::Feed {
            tab,
            generation: first,
            append: false,
            result: Ok(listing("vim", &["cached"], None)),
        },
        1,
        true,
        true,
    ));
    app.switch_tab(ALL_SUBS_TAB);
    app.reload();
    app.on_msg(ordered_snapshot(
        Msg::Group {
            subreddit: "vim".into(),
            generation: 1,
            result: Ok(listing("vim", &["new"], None)),
        },
        3,
        false,
        false,
    ));

    app.on_msg(ordered_snapshot(
        Msg::Feed {
            tab,
            generation: first,
            append: false,
            result: Ok(listing("vim", &["old"], None)),
        },
        2,
        false,
        false,
    ));

    assert_eq!(app.tabs[i].feed.posts[0].id, "cached");
    app.switch_tab(i);
    app.reload();
    assert_eq!(app.tabs[i].feed.posts[0].id, "new");
}

#[test]
fn reload_recovers_a_failed_revalidation_of_a_loaded_page() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    let tab = app.tabs[i].id;
    app.on_msg(ordered_snapshot(
        Msg::Feed {
            tab,
            generation: app.tabs[i].feed.generation,
            append: false,
            result: Ok(listing("vim", &["a"], Some("a"))),
        },
        1,
        false,
        false,
    ));
    app.load_feed(i, true);
    let page_generation = app.tabs[i].feed.generation;
    app.on_msg(ordered_snapshot(
        Msg::Feed {
            tab,
            generation: page_generation,
            append: true,
            result: Ok(listing("vim", &["b"], Some("b"))),
        },
        2,
        true,
        true,
    ));
    app.on_msg(Msg::Feed {
        tab,
        generation: page_generation,
        append: true,
        result: Err("offline".into()),
    });

    app.reload();
    let requests: Vec<_> = app.tabs[i]
        .feed
        .requests
        .iter()
        .filter(|(id, _)| app.tabs[i].feed.pending.contains(id))
        .map(|(id, cursor)| (*id, cursor.clone()))
        .collect();
    for (generation, cursor) in requests {
        let response = if cursor.is_none() {
            listing("vim", &["a"], Some("a"))
        } else {
            let mut response = listing("vim", &["b"], Some("b"));
            response.posts[0].title = "recovered page".into();
            response
        };
        app.on_msg(ordered_snapshot(
            Msg::Feed {
                tab,
                generation,
                append: cursor.is_some(),
                result: Ok(response),
            },
            generation + 10,
            false,
            false,
        ));
    }

    let feed = &app.tabs[i].feed;
    assert_eq!(feed.error(), None);
    assert_eq!(
        feed.posts
            .iter()
            .map(|post| post.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert_eq!(feed.posts[1].title, "recovered page");
}

#[test]
fn later_started_live_feed_wins_when_cache_ordering_is_unavailable() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    app.load_feed(i, false);
    let generation = app.tabs[i].feed.generation;
    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Group {
            subreddit: "vim".into(),
            generation: 0,
            result: Ok(listing("vim", &["home"], None)),
        }),
        fetched_at: now(),
        refreshing: false,
        source: Source {
            order: None,
            observed: Some((1, 1)),
            cached: false,
        },
    });
    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Feed {
            tab: app.tabs[i].id,
            generation,
            append: false,
            result: Ok(listing("vim", &["later-started-feed"], None)),
        }),
        fetched_at: now(),
        refreshing: false,
        source: Source {
            order: None,
            observed: Some((2, 2)),
            cached: false,
        },
    });
    app.reload();
    assert_eq!(app.tabs[i].feed.posts[0].id, "later-started-feed");
}

#[test]
fn a_late_home_snapshot_cannot_replace_a_forced_refresh() {
    let mut app = app_with(&["vim"]);
    app.reload();
    app.on_msg(Msg::Group {
        subreddit: "vim".into(),
        generation: 1,
        result: Ok(listing("vim", &["new"], None)),
    });

    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Group {
            subreddit: "vim".into(),
            generation: 0,
            result: Ok(listing("vim", &["old"], None)),
        }),
        fetched_at: 1,
        refreshing: true,
        source: Source {
            cached: true,
            ..Source::default()
        },
    });

    assert_eq!(app.all_subs.groups[0].posts[0].id, "new");
    assert!(!app.all_subs.groups[0].loading);
}

#[test]
fn a_kept_request_that_started_earlier_cannot_mark_a_newer_page_failed() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    let tab = app.tabs[i].id;
    app.load_feed(i, false);
    let feed_request = app.tabs[i].feed.generation;
    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Group {
            subreddit: "vim".into(),
            generation: 0,
            result: Ok(listing("vim", &["home-later-start"], None)),
        }),
        fetched_at: now(),
        refreshing: false,
        source: ordered_source(3, false),
    });

    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Feed {
            tab,
            generation: feed_request,
            append: false,
            result: Err("timeout".into()),
        }),
        fetched_at: now(),
        refreshing: false,
        source: ordered_source(2, false),
    });

    let feed = &app.tabs[i].feed;
    assert_eq!(feed.posts[0].id, "home-later-start");
    assert_eq!(feed.error(), None);
    assert!(app.status.as_ref().is_none_or(|s| !s.error));
}

#[test]
fn revisiting_a_stale_tab_does_not_duplicate_its_head_revalidation() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    let generation = app.tabs[i].feed.generation;
    app.on_msg(ordered_snapshot(
        Msg::Feed {
            tab: app.tabs[i].id,
            generation,
            append: false,
            result: Ok(listing("vim", &["cached"], Some("next"))),
        },
        1,
        true,
        true,
    ));
    for _ in 0..3 {
        app.switch_tab(0);
        app.switch_tab(i);
    }
    assert_eq!(app.tabs[i].feed.generation, generation);
    app.load_feed(i, true);
    assert_ne!(
        app.tabs[i].feed.generation, generation,
        "paging stays independent"
    );
}
