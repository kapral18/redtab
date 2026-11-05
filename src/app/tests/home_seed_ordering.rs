use super::*;

#[test]
fn orderless_home_completion_cannot_undo_a_later_disk_snapshot() {
    for (retired, overlap) in [(false, false), (true, false), (true, true)] {
        let mut app = app_with(&["vim"]);
        let i = tab_index(&app, "vim");
        app.switch_tab(i);
        let tab = app.tabs[i].id;
        app.load_feed(i, false);
        let generation = app.tabs[i].feed.generation;
        let mut disk = ordered_source(5, true);
        disk.observed = Some(if overlap { (0, 3) } else { (2, 3) });
        if retired {
            disk.order.as_mut().unwrap().generation = [2; 16];
        }
        app.on_msg(Msg::Snapshot {
            message: Box::new(Msg::Feed {
                tab,
                generation,
                append: false,
                result: Ok(listing("vim", &["newer-disk"], None)),
            }),
            fetched_at: now(),
            refreshing: false,
            source: disk,
        });
        app.on_msg(Msg::Snapshot {
            message: Box::new(Msg::Group {
                subreddit: "vim".into(),
                generation: 0,
                result: Ok(listing("vim", &["older-orderless-live"], None)),
            }),
            fetched_at: now(),
            refreshing: false,
            source: Source {
                observed: Some((1, 1)),
                ..Source::default()
            },
        });
        assert_eq!(
            app.tabs[i].feed.posts[0].id, "newer-disk",
            "retired={retired}"
        );
        app.fetch_manual(None);
        app.on_msg(Msg::Snapshot {
            message: Box::new(Msg::Feed {
                tab,
                generation: app.tabs[i].feed.generation,
                append: false,
                result: Ok(listing("vim", &["later-unreserved-refresh"], None)),
            }),
            fetched_at: now(),
            refreshing: false,
            source: Source {
                observed: Some((4, 4)),
                ..Source::default()
            },
        });
        assert_eq!(app.tabs[i].feed.posts[0].id, "later-unreserved-refresh");
    }
}

#[test]
fn home_seed_preserves_a_newer_pending_request_without_discarding_newer_data() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    let tab = app.tabs[i].id;
    app.on_msg(ordered_snapshot(
        Msg::Feed {
            tab,
            generation: app.tabs[i].feed.generation,
            append: false,
            result: Ok(listing("vim", &["old"], None)),
        },
        1,
        false,
        false,
    ));
    app.load_feed(i, false);
    let pending = app.tabs[i].feed.generation;
    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Group {
            subreddit: "vim".into(),
            generation: 0,
            result: Ok(listing("vim", &["home-newer"], Some("A"))),
        }),
        fetched_at: now(),
        refreshing: false,
        source: ordered_source(2, false),
    });
    assert_eq!(app.tabs[i].feed.posts[0].id, "old");
    app.reload();
    assert_eq!(app.tabs[i].feed.posts[0].id, "home-newer");
    let before = app.tabs[i].feed.generation;
    app.load_feed(i, true);
    let next = app.tabs[i].feed.generation;
    assert_ne!(
        next, before,
        "Home supplies the first page while HTTP remains pending"
    );
    app.on_msg(ordered_snapshot(
        Msg::Feed {
            tab,
            generation: next,
            append: true,
            result: Ok(listing("vim", &["tail"], None)),
        },
        2,
        true,
        false,
    ));
    assert!(app.tabs[i].feed.posts.iter().any(|post| post.id == "tail"));

    app.on_msg(ordered_snapshot(
        Msg::Feed {
            tab,
            generation: pending,
            append: false,
            result: Ok(listing("vim", &["old"], None)),
        },
        1,
        true,
        true,
    ));
    assert_eq!(app.tabs[i].feed.posts[0].id, "home-newer");
    app.on_msg(ordered_snapshot(
        Msg::Feed {
            tab,
            generation: pending,
            append: false,
            result: Ok(listing("vim", &["feed-newest"], None)),
        },
        3,
        false,
        false,
    ));
    app.reload();
    assert_eq!(app.tabs[i].feed.posts[0].id, "feed-newest");
}

#[test]
fn home_seeding_keeps_a_feed_request_that_started_later() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    let tab = app.tabs[i].id;
    app.load_feed(i, false);
    let feed_request = app.tabs[i].feed.generation;
    // Home's request was submitted later but started first (lower sequence).
    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Group {
            subreddit: "vim".into(),
            generation: 0,
            result: Ok(listing("vim", &["home-earlier-start"], None)),
        }),
        fetched_at: now(),
        refreshing: false,
        source: ordered_source(2, false),
    });
    assert_eq!(app.tabs[i].feed.posts[0].id, "home-earlier-start");

    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Feed {
            tab,
            generation: feed_request,
            append: false,
            result: Ok(listing("vim", &["feed-later-start"], None)),
        }),
        fetched_at: now(),
        refreshing: false,
        source: ordered_source(3, false),
    });

    app.reload();
    assert_eq!(app.tabs[i].feed.posts[0].id, "feed-later-start");
}

#[test]
fn home_seed_compares_only_the_query_currently_being_requested() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    app.set_sort(Sort::New);
    let tab = app.tabs[i].id;
    app.on_msg(ordered_snapshot(
        Msg::Feed {
            tab,
            generation: app.tabs[i].feed.generation,
            append: false,
            result: Ok(listing("vim", &["new-sort"], None)),
        },
        9,
        false,
        false,
    ));
    app.set_sort(Sort::Hot);
    let pending = app.tabs[i].feed.generation;

    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Group {
            subreddit: "vim".into(),
            generation: 0,
            result: Ok(listing("vim", &["home-hot"], None)),
        }),
        fetched_at: now(),
        refreshing: false,
        source: ordered_source(4, false),
    });
    assert_eq!(app.tabs[i].feed.posts[0].id, "home-hot");
    app.on_msg(ordered_snapshot(
        Msg::Feed {
            tab,
            generation: pending,
            append: false,
            result: Ok(listing("vim", &["old-hot"], None)),
        },
        3,
        true,
        false,
    ));
    assert_eq!(app.tabs[i].feed.posts[0].id, "home-hot");
}

#[test]
fn unorderable_feed_failures_remain_visible_after_home_seeding() {
    let variants = [
        (None, None),
        (ordered_source(3, false).order, None),
        (None, ordered_source(2, false).order),
        (
            ordered_source(3, false).order,
            Some(crate::cache::RequestOrder {
                generation: [99; 16],
                sequence: 2,
            }),
        ),
    ];
    for (home_order, failure_order) in variants {
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
                order: home_order,
                observed: None,
                cached: false,
            },
        });
        app.on_msg(Msg::Snapshot {
            message: Box::new(Msg::Feed {
                tab: app.tabs[i].id,
                generation,
                append: false,
                result: Err("later-started request failed".into()),
            }),
            fetched_at: now(),
            refreshing: false,
            source: Source {
                order: failure_order,
                observed: None,
                cached: false,
            },
        });
        assert_eq!(
            app.tabs[i].feed.error(),
            Some("later-started request failed")
        );
        assert_eq!(app.tabs[i].feed.posts[0].id, "home");
        assert!(!app.tabs[i].feed.refreshing());
    }
}
