use super::*;

#[test]
fn when_home_append_finishes_before_a_fallback_refresh_it_should_keep_the_cached_head() {
    let mut app = app_with(&[]);
    app.active = 0;
    app.load_feed(0, false);
    let tab = app.tabs[0].id;
    let cached = listing(
        "s",
        &["old1", "old2", "old3", "old4", "old5", "old6"],
        Some("next"),
    );
    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Feed {
            tab,
            generation: app.tabs[0].feed.generation,
            append: false,
            result: Ok(cached.clone()),
        }),
        fetched_at: now(),
        refreshing: false,
        source: ordered_source(1, true),
    });
    app.tabs[0].feed.selected = 4;
    app.fetch_feed(0, None, false);
    let head = app.tabs[0].feed.generation;
    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Feed {
            tab,
            generation: head,
            append: false,
            result: Ok(cached),
        }),
        fetched_at: now(),
        refreshing: true,
        source: ordered_source(1, true),
    });
    app.load_feed(0, true);
    let append = app.tabs[0].feed.generation;
    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Feed {
            tab,
            generation: append,
            append: true,
            result: Ok(listing("s", &["tail"], None)),
        }),
        fetched_at: now(),
        refreshing: false,
        source: ordered_source(3, false),
    });

    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::FeedFallback {
            tab,
            generation: head,
            listing: listing("s", &["a", "b", "c"], Some("next")),
            cursor: "next".into(),
            error: "prefetch failed".into(),
        }),
        fetched_at: now(),
        refreshing: false,
        source: ordered_source(2, false),
    });

    let feed = &app.tabs[0].feed;
    assert_eq!(feed.posts.len(), 7);
    assert_eq!(feed.posts[feed.selected].id, "old5");
    assert_eq!(feed.posts.last().unwrap().id, "tail");
    assert!(!feed.loading() && !feed.refreshing());
}

#[test]
fn when_an_append_is_retired_it_should_resume_its_cursor_without_reloading_the_head() {
    let mut app = app_with(&["s", "other"]);
    let index = tab_index(&app, "s");
    app.switch_tab(index);
    let tab = app.tabs[index].id;
    app.on_msg(Msg::Feed {
        tab,
        generation: app.tabs[index].feed.generation,
        append: false,
        result: Ok(listing("s", &["head"], Some("next"))),
    });
    app.load_feed(index, true);
    let generation = app.tabs[index].feed.generation;
    app.switch_tab(tab_index(&app, "other"));
    app.on_msg(Msg::Feed {
        tab,
        generation,
        append: true,
        result: Err(crate::reddit::REQUEST_CANCELLED.into()),
    });
    app.feed_settle.last_move = None;

    app.switch_tab(index);

    let feed = &app.tabs[index].feed;
    assert_eq!(
        feed.requests.get(&feed.generation),
        Some(&Some("next".into()))
    );
    assert_eq!(feed.pending.len(), 1);
    assert_eq!(feed.posts[0].id, "head");
    assert!(feed.error().is_none());
}

#[test]
fn when_home_starts_with_three_posts_it_should_fill_without_a_tab_switch() {
    let server = crate::reddit::tests::local_http_server();
    let base = format!("http://{}", server.server_addr());
    let handler = std::thread::spawn(move || {
        let mut completed = 0;
        let deadline = Instant::now() + Duration::from_secs(5);
        while completed < 2 {
            let request = server
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap()
                .unwrap();
            let url = url::Url::parse(&format!("http://fixture{}", request.url())).unwrap();
            let body: String = match url.path() {
                "/svc/shreddit/feeds/home-feed" => {
                    if url
                        .query_pairs()
                        .any(|(key, value)| key == "cursor" && value == "next&batch")
                    {
                        (0..25)
                            .map(|id| {
                                crate::reddit::tests::native_post(
                                    &format!("recommended{id}"),
                                    "unjoined",
                                )
                            })
                            .collect()
                    } else {
                        "<shreddit-post id='t3_a' post-title='a' subreddit-name='s' author='reader' permalink='/r/s/comments/a/title/' post-type='text'></shreddit-post><shreddit-post id='t3_b' post-title='b' subreddit-name='s' author='reader' permalink='/r/s/comments/b/title/' post-type='text'></shreddit-post><shreddit-post id='t3_c' post-title='c' subreddit-name='s' author='reader' permalink='/r/s/comments/c/title/' post-type='text'></shreddit-post><faceplate-partial id='feed-next-page-partial' src='/svc/shreddit/feeds/home-feed?sort=BEST&amp;cursor=next%26batch'></faceplate-partial>".into()
                    }
                }
                path => panic!("unexpected path: {path}"),
            };
            request
                .respond(crate::reddit::tests::served::Response::from_string(body))
                .unwrap();
            completed += 1;
        }
    });
    let (mut app, messages) = app_at(&base, &[]);

    app.switch_tab(0);
    while app.tabs[0].feed.posts.len() < 28 || app.tabs[0].feed.loading() {
        app.on_msg(
                messages
                    .recv_timeout(Duration::from_secs(2))
                    .unwrap_or_else(|error|panic!("Home did not continue: {error:?}; loaded={} loading={} posts={} failure={:?}",app.tabs[0].feed.loaded,app.tabs[0].feed.loading(),app.tabs[0].feed.posts.len(),app.tabs[0].feed.error())),
            );
    }
    handler.join().unwrap();
    let generation = app.tabs[0].feed.generation;
    app.switch_tab(BOOKMARKS_TAB);
    app.switch_tab(0);
    app.load_settled(Instant::now() + NAV_SETTLE);

    assert_eq!(app.tabs[0].feed.posts.len(), 28);
    assert_eq!(app.current_post().unwrap().id, "a");
    assert_eq!(app.tabs[0].feed.posts[3].id, "recommended0");
    assert_eq!(app.tabs[0].feed.generation, generation);
    assert!(!app.tabs[0].feed.loading());
    assert!(app.tabs[0].feed.after.is_none());
}

#[test]
fn when_home_has_a_short_cached_head_it_should_keep_the_continuation_during_refresh() {
    let mut app = app_with(&[]);
    app.switch_tab(0);
    let tab = app.tabs[0].id;
    let first = app.tabs[0].feed.generation;

    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Feed {
            tab,
            generation: first,
            append: false,
            result: Ok(listing("joined", &["a", "b", "c"], Some("next"))),
        }),
        fetched_at: now() - crate::cache::CONTENT_TTL,
        source: ordered_source(1, true),
        refreshing: true,
    });
    assert!(
        app.tabs[0].feed.loading(),
        "the cached primer should continue automatically"
    );
    let next = app.tabs[0].feed.generation;
    app.tabs[0].feed.selected = 1;
    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Feed {
            tab,
            generation: next,
            append: true,
            result: Ok(listing(
                "unjoined",
                &["d", "e", "f", "g", "h", "i"],
                Some("later"),
            )),
        }),
        fetched_at: now(),
        source: ordered_source(3, false),
        refreshing: false,
    });
    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Feed {
            tab,
            generation: first,
            append: false,
            result: Ok(listing("joined", &["a", "b", "c"], Some("different-next"))),
        }),
        fetched_at: now(),
        source: ordered_source(2, false),
        refreshing: false,
    });
    app.switch_tab(BOOKMARKS_TAB);
    app.switch_tab(0);
    app.load_settled(Instant::now() + NAV_SETTLE);

    assert_eq!(app.tabs[0].feed.posts.len(), 9);
    assert_eq!(app.tabs[0].feed.after.as_deref(), Some("later"));
    assert_eq!(app.current_post().unwrap().id, "b");
    assert!(!app.tabs[0].feed.loading());
}

#[test]
fn when_home_finishes_offscreen_it_should_continue_only_after_returning() {
    let mut app = app_with(&[]);
    app.switch_tab(0);
    let tab = app.tabs[0].id;
    let generation = app.tabs[0].feed.generation;
    app.switch_tab(BOOKMARKS_TAB);
    app.on_msg(Msg::Feed {
        tab,
        generation,
        append: false,
        result: Ok(listing("joined", &["a", "b", "c"], Some("next"))),
    });
    assert!(
        !app.tabs[0].feed.loading(),
        "inactive Home must not fetch more"
    );

    app.switch_tab(0);
    app.tick_at(Instant::now() + NAV_SETTLE);

    assert!(
        app.tabs[0].feed.loading(),
        "returning should resume the short list"
    );
    assert_eq!(app.tabs[0].feed.after.as_deref(), Some("next"));
}

#[test]
fn when_home_pages_it_should_keep_a_list_separate_from_all_subs() {
    let mut app = app_with(&["rust"]);
    app.switch_tab(0);
    let id = app.active_tab().id;
    let generation = app.active_tab().feed.generation;
    app.on_msg(Msg::Feed {
        tab: id,
        generation,
        append: false,
        result: Ok(listing("rust", &["a", "b"], Some("t3_b"))),
    });
    app.load_feed(0, true);
    let generation = app.active_tab().feed.generation;
    app.on_msg(Msg::Feed {
        tab: id,
        generation,
        append: true,
        result: Ok(listing("neovim", &["b", "c"], None)),
    });

    assert_eq!(
        app.tabs[0]
            .feed
            .posts
            .iter()
            .map(|p| p.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b", "c"]
    );
    assert_eq!(app.tabs[0].feed.sort, Sort::Best);
    assert_eq!(app.current_post().unwrap().id, "a");
    assert!(app.all_subs.groups[0].posts.is_empty());
}
