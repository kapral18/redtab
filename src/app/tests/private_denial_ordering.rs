use super::*;

#[test]
fn private_community_late_page_denial_should_not_override_newer_accessible_posts() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    app.load_feed(i, false);
    let tab = app.tabs[i].id;
    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Feed {
            tab,
            generation: app.tabs[i].feed.generation,
            append: false,
            result: Ok(listing("vim", &["initial"], Some("next"))),
        }),
        fetched_at: now(),
        refreshing: false,
        source: ordered_source(1, false),
    });
    app.load_feed(i, true);
    let page_request = app.tabs[i].feed.generation;
    app.fetch_manual(None);
    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Feed {
            tab,
            generation: app.tabs[i].feed.generation,
            append: false,
            result: Ok(listing("vim", &["allowed"], Some("next"))),
        }),
        fetched_at: now(),
        refreshing: false,
        source: ordered_source(3, false),
    });

    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Feed {
            tab,
            generation: page_request,
            append: true,
            result: Err(FeedError::Access(AccessDenied::PrivateCommunity)),
        }),
        fetched_at: now(),
        refreshing: false,
        source: ordered_source(2, false),
    });

    assert_eq!(app.tabs[i].feed.access_denied(), None);
    assert_eq!(app.current_post().unwrap().id, "allowed");
}

#[test]
fn buffered_success_prevents_an_older_denial_from_hiding_the_feed() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    let tab = app.tabs[i].id;
    app.on_msg(ordered_snapshot(
        Msg::Feed {
            tab,
            generation: app.tabs[i].feed.generation,
            append: false,
            result: Ok(listing("vim", &["old"], Some("next"))),
        },
        1,
        false,
        false,
    ));
    app.load_feed(i, true);
    let earlier = app.tabs[i].feed.generation;
    app.load_feed(i, false);
    app.on_msg(ordered_snapshot(
        Msg::Feed {
            tab,
            generation: app.tabs[i].feed.generation,
            append: false,
            result: Ok(listing("vim", &["new"], None)),
        },
        3,
        false,
        false,
    ));

    app.on_msg(ordered_snapshot(
        Msg::Feed {
            tab,
            generation: earlier,
            append: true,
            result: Err(FeedError::Access(AccessDenied::PrivateCommunity)),
        },
        2,
        false,
        false,
    ));

    assert_eq!(app.tabs[i].feed.access_denied(), None);
    assert_eq!(app.current_post().unwrap().id, "old");
    assert!(app.tabs[i].feed.has_new_posts(&app.config));
    assert!(
        app.all_subs
            .groups
            .iter()
            .all(|group| group.error.is_none())
    );
}

#[test]
fn private_community_rechecks_should_keep_cached_posts_restricted_until_success() {
    for action in ["reload", "search", "sort", "reconnect", "failure"] {
        let mut app = app_with(&["vim"]);
        let i = tab_index(&app, "vim");
        app.switch_tab(i);
        app.load_feed(i, false);
        let tab = app.tabs[i].id;
        let generation = app.tabs[i].feed.generation;
        app.on_msg(ordered_snapshot(
            Msg::Feed {
                tab,
                generation,
                append: false,
                result: Ok(listing("vim", &["cached"], None)),
            },
            1,
            false,
            false,
        ));
        app.on_msg(ordered_snapshot(
            Msg::Feed {
                tab,
                generation,
                append: false,
                result: Err(FeedError::Access(AccessDenied::PrivateCommunity)),
            },
            2,
            false,
            false,
        ));

        match action {
            "reload" => app.reload(),
            "search" => app.search("something".into()),
            "sort" => app.set_sort(Sort::New),
            "reconnect" => app.reset_pending_loads(),
            "failure" => app.on_msg(ordered_snapshot(
                Msg::Feed {
                    tab,
                    generation,
                    append: false,
                    result: Err("connection failed".into()),
                },
                3,
                false,
                false,
            )),
            _ => unreachable!(),
        }

        assert_eq!(
            app.tabs[i].feed.access_denied(),
            Some(AccessDenied::PrivateCommunity),
            "{action}"
        );
        assert!(app.current_post().is_none(), "{action}");
        assert_eq!(app.tabs[i].feed.posts[0].id, "cached", "{action}");

        app.on_msg(ordered_snapshot(
            Msg::Feed {
                tab,
                generation: app.tabs[i].feed.generation,
                append: false,
                result: Ok(listing("vim", &["accessible"], None)),
            },
            4,
            false,
            false,
        ));
        assert!(app.tabs[i].feed.access_denied().is_none(), "{action}");
        assert_eq!(app.current_post().unwrap().id, "accessible", "{action}");
    }
}

#[test]
fn private_community_denial_should_outrank_an_unrelated_cursor_error() {
    let mut feed = Feed {
        after: Some("next".into()),
        ..Feed::default()
    };
    feed.failures.insert(
        Some("next".into()),
        FeedFailure {
            error: "network failure".into(),
            source: ordered_source(1, false),
        },
    );
    feed.failures.insert(
        None,
        FeedFailure {
            error: FeedError::Access(AccessDenied::PrivateCommunity),
            source: ordered_source(2, false),
        },
    );

    assert_eq!(feed.access_denied(), Some(AccessDenied::PrivateCommunity));
}

#[test]
fn private_community_older_cached_success_should_not_clear_the_access_restriction() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    app.load_feed(i, false);
    let generation = app.tabs[i].feed.generation;
    let tab = app.tabs[i].id;
    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Feed {
            tab,
            generation,
            append: false,
            result: Err(FeedError::Access(AccessDenied::PrivateCommunity)),
        }),
        fetched_at: now(),
        refreshing: false,
        source: ordered_source(2, false),
    });

    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Feed {
            tab,
            generation,
            append: false,
            result: Ok(listing("vim", &["cached"], None)),
        }),
        fetched_at: now(),
        refreshing: false,
        source: ordered_source(1, true),
    });

    assert_eq!(
        app.tabs[i].feed.access_denied(),
        Some(AccessDenied::PrivateCommunity)
    );
    assert!(app.current_post().is_none());
}

#[test]
fn private_community_newer_network_success_should_clear_an_older_page_denial() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    app.load_feed(i, false);
    let generation = app.tabs[i].feed.generation;
    let tab = app.tabs[i].id;
    app.tabs[i].feed.after = Some("next".into());
    app.on_msg(ordered_snapshot(
        Msg::Feed {
            tab,
            generation,
            append: true,
            result: Err(FeedError::Access(AccessDenied::PrivateCommunity)),
        },
        2,
        false,
        false,
    ));

    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Feed {
            tab,
            generation,
            append: false,
            result: Ok(listing("vim", &["allowed"], Some("next"))),
        }),
        fetched_at: now(),
        refreshing: false,
        source: ordered_source(3, false),
    });

    assert_eq!(app.tabs[i].feed.access_denied(), None);
    assert_eq!(app.current_post().unwrap().id, "allowed");
}

#[test]
fn private_community_older_success_should_keep_a_newer_denial_when_the_cursor_changes() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    app.load_feed(i, false);
    let generation = app.tabs[i].feed.generation;
    let tab = app.tabs[i].id;
    app.tabs[i].feed.after = Some("next".into());
    app.on_msg(ordered_snapshot(
        Msg::Feed {
            tab,
            generation,
            append: true,
            result: Err(FeedError::Access(AccessDenied::PrivateCommunity)),
        },
        3,
        false,
        false,
    ));

    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Feed {
            tab,
            generation,
            append: false,
            result: Ok(listing("vim", &["older"], None)),
        }),
        fetched_at: now(),
        refreshing: false,
        source: ordered_source(2, false),
    });

    assert_eq!(
        app.tabs[i].feed.access_denied(),
        Some(AccessDenied::PrivateCommunity)
    );
    assert!(app.current_post().is_none());
}

#[test]
fn private_community_island_failed_recheck_should_keep_access_restricted() {
    let mut app = app_with(&["vim"]);
    app.all_subs.groups = vec![Group {
        subreddit: "vim".into(),
        posts: listing("vim", &["cached"], None).posts,
        error: Some(FeedError::Access(AccessDenied::PrivateCommunity)),
        ..Group::default()
    }];
    App::refresh_group(&app.pool, &mut app.all_subs.groups[0], 1);
    app.on_msg(ordered_snapshot(
        Msg::Group {
            subreddit: "vim".into(),
            generation: 1,
            result: Err("connection failed".into()),
        },
        3,
        false,
        false,
    ));

    assert_eq!(
        app.all_subs.groups[0]
            .error
            .as_ref()
            .and_then(FeedError::access),
        Some(AccessDenied::PrivateCommunity)
    );
    assert_eq!(app.group_posts(&app.all_subs.groups[0]).count(), 0);
    assert_eq!(app.all_subs.groups[0].posts[0].id, "cached");
    app.on_msg(ordered_snapshot(
        Msg::Group {
            subreddit: "vim".into(),
            generation: 1,
            result: Ok(listing("vim", &["accessible"], None)),
        },
        4,
        false,
        false,
    ));
    assert!(app.all_subs.groups[0].error.is_none());
    assert_eq!(
        app.group_posts(&app.all_subs.groups[0]).next().unwrap().id,
        "accessible"
    );
}
