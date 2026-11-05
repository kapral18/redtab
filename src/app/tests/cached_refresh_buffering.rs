use super::*;

#[test]
fn when_home_reload_falls_back_it_should_keep_the_larger_cached_page_and_selection() {
    let mut app = app_with(&[]);
    app.active = 0;
    app.load_feed(0, false);
    let tab = app.tabs[0].id;
    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Feed {
            tab,
            generation: app.tabs[0].feed.generation,
            append: false,
            result: Ok(listing(
                "s",
                &["old1", "old2", "old3", "old4", "old5", "old6"],
                None,
            )),
        }),
        fetched_at: now(),
        refreshing: false,
        source: ordered_source(1, true),
    });
    app.tabs[0].feed.selected = 4;
    app.fetch_feed(0, None, true);
    let generation = app.tabs[0].feed.generation;

    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::FeedFallback {
            tab,
            generation,
            listing: listing("s", &["a", "b", "c"], Some("next")),
            cursor: "next".into(),
            error: "continuation failed".into(),
        }),
        fetched_at: now(),
        refreshing: false,
        source: ordered_source(2, false),
    });

    let feed = &app.tabs[0].feed;
    assert_eq!(feed.posts.len(), 6);
    assert_eq!(feed.posts[feed.selected].id, "old5");
    assert!(feed.error().is_some() && !feed.loading() && feed.after.is_none());
    assert!(feed.freshness.unwrap().source.cached);
}

#[test]
fn when_a_newer_append_succeeds_an_older_prefetch_failure_should_not_mark_it_failed() {
    let mut app = app_with(&[]);
    app.active = 0;
    app.load_feed(0, false);
    let tab = app.tabs[0].id;
    let head = app.tabs[0].feed.generation;
    app.fetch_feed(0, Some("next".into()), false);
    let append = app.tabs[0].feed.generation;
    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Feed {
            tab,
            generation: append,
            append: true,
            result: Ok(listing("s", &["new"], None)),
        }),
        fetched_at: now(),
        refreshing: false,
        source: ordered_source(3, false),
    });

    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::FeedFallback {
            tab,
            generation: head,
            listing: listing("s", &["a"], Some("next")),
            cursor: "next".into(),
            error: "old failure".into(),
        }),
        fetched_at: now(),
        refreshing: false,
        source: ordered_source(2, false),
    });

    assert!(app.tabs[0].feed.error().is_none());
    assert_eq!(app.tabs[0].feed.posts[0].id, "new");
}

#[test]
fn when_a_cached_page_refresh_streams_it_should_keep_the_suffix_and_selected_post() {
    let mut app = app_with(&["s"]);
    let index = tab_index(&app, "s");
    app.switch_tab(index);
    let tab = app.tabs[index].id;
    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Feed {
            tab,
            generation: app.tabs[index].feed.generation,
            append: false,
            result: Ok(listing("s", &["a", "b"], Some("next"))),
        }),
        fetched_at: now(),
        refreshing: false,
        source: ordered_source(1, true),
    });
    app.tabs[index].feed.selected = 1;
    app.fetch_feed(index, None, true);
    let generation = app.tabs[index].feed.generation;

    app.on_msg(Msg::FeedProgress {
        tab,
        generation,
        post: Box::new(post("new", "s")),
        source: ordered_source(2, false),
    });

    let feed = &app.tabs[index].feed;
    assert_eq!(
        feed.posts.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert_eq!(feed.posts[feed.selected].id, "b");
    assert_eq!(feed.after.as_deref(), Some("next"));
    assert!(feed.loading() && feed.forced.contains(&generation));
    assert!(feed.freshness.unwrap().source.cached);
}

#[test]
fn when_a_forced_cached_feed_refresh_is_retired_it_should_resume_forced_on_return() {
    let mut app = app_with(&["s", "other"]);
    let index = tab_index(&app, "s");
    app.switch_tab(index);
    app.fetch_feed(index, None, true);
    let generation = app.tabs[index].feed.generation;
    let tab = app.tabs[index].id;
    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Feed {
            tab,
            generation,
            append: false,
            result: Ok(listing("s", &["cached"], Some("next"))),
        }),
        fetched_at: now(),
        refreshing: true,
        source: Source {
            cached: true,
            ..Source::default()
        },
    });
    app.switch_tab(tab_index(&app, "other"));
    app.on_msg(Msg::Feed {
        tab,
        generation,
        append: false,
        result: Err(crate::reddit::REQUEST_CANCELLED.into()),
    });
    assert_eq!(app.tabs[index].feed.deferred.get(&None), Some(&true));
    app.feed_settle.last_move = None;

    app.switch_tab(index);

    let feed = &app.tabs[index].feed;
    assert!(
        feed.forced.contains(&feed.generation),
        "return must preserve explicit reload intent"
    );
    assert!(feed.loading() && feed.error().is_none());
    assert!(feed.deferred.is_empty());
}

#[test]
fn background_arrivals_wait_while_existing_posts_keep_the_latest_updates() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    let tab = app.tabs[i].id;
    let answer = |generation, ids: &[&str], order, score| {
        let mut page = listing("vim", ids, Some("next"));
        page.posts
            .iter_mut()
            .find(|post| post.id == "a")
            .unwrap()
            .score = Some(score);
        ordered_snapshot(
            Msg::Feed {
                tab,
                generation,
                append: false,
                result: Ok(page),
            },
            order,
            false,
            false,
        )
    };
    app.on_msg(answer(app.tabs[i].feed.generation, &["a", "b", "c"], 1, 1));
    app.tabs[i].feed.selected = 1;
    app.tabs[i].feed.offset = 1;
    app.fetch_feed(i, None, false);
    let generation = app.tabs[i].feed.generation;

    app.on_msg(answer(generation, &["new", "a", "b", "c"], 2, 42));
    app.on_msg(answer(generation, &["latest", "new", "a", "b", "c"], 3, 43));
    app.on_msg(answer(generation, &["older", "a", "b", "c"], 2, 99));

    assert_eq!(
        app.tabs[i]
            .feed
            .posts
            .iter()
            .map(|p| p.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b", "c"]
    );
    assert_eq!(
        app.tabs[i]
            .feed
            .posts
            .iter()
            .find(|post| post.id == "a")
            .unwrap()
            .score,
        Some(43)
    );
    app.reload();
    assert_eq!(
        app.tabs[i]
            .feed
            .posts
            .iter()
            .map(|p| p.id.as_str())
            .collect::<Vec<_>>(),
        ["latest", "new", "a", "b", "c"]
    );
    assert_eq!(app.current_post().unwrap().id, "latest");
    assert_eq!(
        app.tabs[i]
            .feed
            .posts
            .iter()
            .find(|post| post.id == "a")
            .unwrap()
            .score,
        Some(43)
    );
    assert_eq!(app.tabs[i].feed.offset, 0);
    assert_eq!(app.tabs[i].feed.after.as_deref(), Some("next"));
}

#[test]
fn applying_buffered_pages_shows_the_list_top_and_retains_the_updated_suffix() {
    for (head_order, suffix_order) in [(2, 3), (3, 2)] {
        let mut app = app_with(&["vim"]);
        let i = tab_index(&app, "vim");
        app.switch_tab(i);
        app.config.filters.authors.push("blocked".into());
        let tab = app.tabs[i].id;
        let answer = |generation, append, ids: &[&str], after, order| {
            ordered_snapshot(
                Msg::Feed {
                    tab,
                    generation,
                    append,
                    result: Ok(listing("vim", ids, after)),
                },
                order,
                false,
                false,
            )
        };
        app.on_msg(answer(
            app.tabs[i].feed.generation,
            false,
            &["a", "b"],
            Some("next"),
            1,
        ));
        app.load_feed(i, true);
        app.on_msg(answer(
            app.tabs[i].feed.generation,
            true,
            &["c", "d"],
            None,
            1,
        ));
        app.tabs[i].feed.selected = 1;
        app.tabs[i].feed.offset = 1;
        app.load_feed(i, false);
        app.on_msg(answer(
            app.tabs[i].feed.generation,
            false,
            &["x", "a"],
            Some("next"),
            head_order,
        ));
        app.fetch_feed(i, Some("next".into()), false);
        let mut suffix = listing("vim", &["b", "c", "d"], None);
        suffix.posts[0].score = Some(42);
        suffix.posts[0].author = "blocked".into();
        app.on_msg(ordered_snapshot(
            Msg::Feed {
                tab,
                generation: app.tabs[i].feed.generation,
                append: true,
                result: Ok(suffix),
            },
            suffix_order,
            false,
            false,
        ));
        assert_eq!(
            app.tabs[i]
                .feed
                .posts
                .iter()
                .find(|post| post.id == "b")
                .unwrap()
                .score,
            Some(42)
        );

        assert_eq!(app.state.unread("vim", &app.config), Some(4));
        app.reload();

        assert_eq!(app.current_post().unwrap().id, "x");
        assert_eq!(app.state.unread("vim", &app.config), Some(4));
        assert_eq!(
            app.tabs[i]
                .feed
                .posts
                .iter()
                .find(|post| post.id == "b")
                .unwrap()
                .score,
            Some(42)
        );
        assert_eq!(app.tabs[i].feed.offset, 0);
        assert_eq!(
            app.tabs[i]
                .feed
                .posts
                .iter()
                .map(|post| post.id.as_str())
                .collect::<Vec<_>>(),
            ["x", "a", "b", "c", "d"]
        );
    }
}

#[test]
fn a_fresh_refetch_after_a_write_waits_behind_the_notice_instead_of_jumping() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    let tab = app.tabs[i].id;
    app.on_msg(Msg::Feed {
        tab,
        generation: app.tabs[i].feed.generation,
        append: false,
        result: Ok(listing("vim", &["a", "b", "c"], None)),
    });
    app.tabs[i].feed.selected = 2;
    app.tabs[i].feed.offset = 2;

    app.fetch_feed(i, None, true);
    app.on_msg(Msg::Feed {
        tab,
        generation: app.tabs[i].feed.generation,
        append: false,
        result: Ok(listing("vim", &["new", "a", "b", "c"], None)),
    });

    assert!(app.tabs[i].feed.has_new_posts(&app.config));
    assert_eq!(app.current_post().unwrap().id, "c");
    assert_eq!(app.tabs[i].feed.offset, 2);
}
