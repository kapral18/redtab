use super::*;

#[test]
fn when_homes_prefetched_continuation_fails_it_should_keep_the_starter_and_offer_retry() {
    use crate::reddit::tests::{Fixture, Reply};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    for failure in [0, 1, 2] {
        let attempts = Arc::new(AtomicUsize::new(0));
        let count = attempts.clone();
        let fixture = Fixture::new(move |request| {
            if request.url.contains("cursor=next") {
                count.fetch_add(1, Ordering::SeqCst);
                return Reply{status:if failure==0 {500}else{200},body:if failure==2 {
                        "<shreddit-post id='t3_d' post-title='d' subreddit-name='s' author='reader' permalink='/r/s/comments/d/title/' post-type='text'></shreddit-post><faceplate-partial id='feed-next-page-partial' src='/svc/shreddit/feeds/home-feed?cursor=next'></faceplate-partial>"
                    }else{"not a feed"}.into(),headers:vec![]};
            }
            if request.url.starts_with("/svc/shreddit/feeds/home-feed") {
                return Reply{status:200,body:"<shreddit-post id='t3_a' post-title='a' subreddit-name='s' author='reader' permalink='/r/s/comments/a/title/' post-type='text'></shreddit-post><shreddit-post id='t3_b' post-title='b' subreddit-name='s' author='reader' permalink='/r/s/comments/b/title/' post-type='text'></shreddit-post><shreddit-post id='t3_c' post-title='c' subreddit-name='s' author='reader' permalink='/r/s/comments/c/title/' post-type='text'></shreddit-post><faceplate-partial id='feed-next-page-partial' src='/svc/shreddit/feeds/home-feed?cursor=next'></faceplate-partial>".into(),headers:vec![]};
            }
            panic!("unexpected Home lookup: {}", request.url);
        });
        let directory = tempfile::tempdir().unwrap();
        let cache_path = directory.path().join("cache");
        let reddit = fixture.reddit();
        let key = crate::cache::Cache::key(
            &reddit.cache_namespace(),
            r#"["browser-home-v2",{"Listing":{"sort":"Best","time":"Day"}},null]"#,
        );
        let (pool, messages) = Pool::start(
            reddit.clone(),
            3,
            Some(crate::cache::Cache::new(cache_path.clone())),
        );
        let mut app = App::new(
            Config::default(),
            pool,
            reddit,
            directory.path().join("state.json"),
            Picker::halfblocks(),
        );
        app.active = 0;

        app.load_feed(0, false);
        let deadline = Instant::now() + Duration::from_secs(3);
        while app.tabs[0].feed.loading() {
            assert!(Instant::now() < deadline);
            app.on_msg(messages.recv_timeout(deadline.saturating_duration_since(Instant::now())).unwrap_or_else(|error|panic!("case {failure}: {error:?}; received {} posts; continuation attempts {}",app.tabs[0].feed.posts.len(),attempts.load(Ordering::SeqCst))));
        }

        let feed = &app.tabs[0].feed;
        assert_eq!(
            feed.posts
                .iter()
                .map(|post| post.id.as_str())
                .collect::<Vec<_>>(),
            // Posts streamed before the continuation failed stay shown.
            if failure == 2 {
                &["a", "b", "c", "d"][..]
            } else {
                &["a", "b", "c"][..]
            }
        );
        assert!(feed.loaded && feed.page_failed());
        assert!(feed.after.as_deref().unwrap().ends_with("cursor=next"));
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            1,
            "failed continuation retried automatically"
        );
        assert!(
            crate::cache::Cache::new(cache_path)
                .load(&key, now())
                .is_none(),
            "failed combined Home load must not cache the three-post fallback"
        );
    }
}

#[test]
fn when_home_continuation_fails_it_should_wait_for_an_explicit_retry() {
    let mut app = app_with(&[]);
    app.switch_tab(0);
    let tab = app.tabs[0].id;
    app.on_msg(Msg::Feed {
        tab,
        generation: app.tabs[0].feed.generation,
        append: false,
        result: Ok(listing("joined", &["a", "b", "c"], Some("next"))),
    });
    app.on_msg(Msg::Feed {
        tab,
        generation: app.tabs[0].feed.generation,
        append: true,
        result: Err("offline".into()),
    });
    app.switch_tab(BOOKMARKS_TAB);
    app.switch_tab(0);
    app.load_settled(Instant::now() + NAV_SETTLE);
    assert!(app.tabs[0].feed.page_failed());
    assert!(!app.tabs[0].feed.loading());

    app.load_feed(0, true);

    assert!(app.tabs[0].feed.loading());
    assert!(app.tabs[0].feed.error().is_none());
}

#[test]
fn when_home_repeats_a_cursor_it_should_stop_automatic_paging() {
    let mut app = app_with(&[]);
    app.switch_tab(0);
    let tab = app.tabs[0].id;
    app.on_msg(Msg::Feed {
        tab,
        generation: app.tabs[0].feed.generation,
        append: false,
        result: Ok(listing("joined", &["a"], Some("loop"))),
    });
    app.on_msg(Msg::Feed {
        tab,
        generation: app.tabs[0].feed.generation,
        append: true,
        result: Ok(listing("unjoined", &["b"], Some("loop"))),
    });

    assert!(!app.tabs[0].feed.loading());
    assert_eq!(app.tabs[0].feed.posts.len(), 2);
}

#[test]
fn when_home_filters_hide_every_page_it_should_bound_automatic_fetches() {
    let mut app = app_with(&[]);
    app.config.filters.keywords = vec!["title".into()];
    app.switch_tab(0);
    let tab = app.tabs[0].id;

    for page in 0..4 {
        app.on_msg(Msg::Feed {
            tab,
            generation: app.tabs[0].feed.generation,
            append: page != 0,
            result: Ok(listing(
                "unjoined",
                &["hidden"],
                Some(&format!("next{page}")),
            )),
        });
        assert_eq!(app.tabs[0].feed.loading(), page < 3, "page {page}");
    }

    assert_eq!(app.config.visible(&app.tabs[0].feed.posts).count(), 0);
    app.load_feed(0, true);
    assert!(
        app.tabs[0].feed.loading(),
        "L can continue after the automatic limit"
    );
}

#[test]
fn when_a_streamed_append_fails_it_should_keep_usable_posts_and_offer_cursor_retry() {
    let mut app = app_with(&["s"]);
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
    app.on_msg(Msg::FeedProgress {
        tab,
        generation,
        post: Box::new(post("arrived", "s")),
        source: ordered_source(2, false),
    });

    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Feed {
            tab,
            generation,
            append: true,
            result: Err("stream cut".into()),
        }),
        fetched_at: now(),
        refreshing: false,
        source: ordered_source(2, false),
    });

    let feed = &app.tabs[index].feed;
    assert_eq!(
        feed.posts.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
        ["head", "arrived"]
    );
    assert!(feed.page_failed() && !feed.loading());
    assert_eq!(feed.after.as_deref(), Some("next"));
}

#[test]
fn pagination_reload_should_retry_a_failed_unseen_page_without_an_automatic_retry() {
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
            result: Ok(listing("vim", &["a"], Some("a"))),
        },
        1,
        true,
        true,
    ));
    app.load_feed(i, true);
    app.on_msg(Msg::Feed {
        tab,
        generation: app.tabs[i].feed.generation,
        append: true,
        result: Err("offline".into()),
    });
    app.on_msg(ordered_snapshot(
        Msg::Feed {
            tab,
            generation: first,
            append: false,
            result: Ok(listing("vim", &["a"], Some("a"))),
        },
        2,
        false,
        false,
    ));

    app.move_selection(1);

    app.tick_at(Instant::now() + NAV_SETTLE);
    assert!(
        !app.tabs[i].feed.loading(),
        "navigation must not retry the failed cursor"
    );
    press(&mut app, crossterm::event::KeyCode::Char('r'));
    assert!(
        app.tabs[i].feed.loading_more(),
        "r must resume the failed continuation"
    );
    assert_eq!(app.tabs[i].feed.posts[0].id, "a");
    let generation = app.tabs[i].feed.generation;
    press(&mut app, crossterm::event::KeyCode::Char('r'));
    assert_eq!(
        app.tabs[i].feed.generation, generation,
        "r must not duplicate an active continuation"
    );
}

#[test]
fn an_older_home_success_keeps_a_newer_feed_failure_visible() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    app.on_msg(ordered_snapshot(
        Msg::Feed {
            tab: app.tabs[i].id,
            generation: app.tabs[i].feed.generation,
            append: false,
            result: Ok(listing("vim", &["initial"], None)),
        },
        1,
        false,
        false,
    ));
    app.reload();
    app.on_msg(ordered_snapshot(
        Msg::Feed {
            tab: app.tabs[i].id,
            generation: app.tabs[i].feed.generation,
            append: false,
            result: Err("newer request failed".into()),
        },
        3,
        false,
        false,
    ));
    app.on_msg(ordered_snapshot(
        Msg::Group {
            subreddit: "vim".into(),
            generation: 0,
            result: Ok(listing("vim", &["useful-older-success"], None)),
        },
        2,
        false,
        false,
    ));
    assert_eq!(app.tabs[i].feed.error(), Some("newer request failed"));
    app.reload();
    assert_eq!(app.tabs[i].feed.posts[0].id, "useful-older-success");
    assert!(
        app.tabs[i].feed.loading(),
        "r retries the failed head after applying the buffered page"
    );
}

#[test]
fn a_failed_load_is_reported_and_stops_automatic_paging() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    let id = app.tabs[i].id;
    let g = app.tabs[i].feed.generation;
    app.on_msg(Msg::Feed {
        tab: id,
        generation: g,
        append: false,
        result: Ok(listing("vim", &["a", "b"], Some("t3_b"))),
    });
    app.load_feed(i, true);
    let g = app.tabs[i].feed.generation;
    app.on_msg(Msg::Feed {
        tab: id,
        generation: g,
        append: true,
        result: Err("timeout".into()),
    });

    assert!(
        app.status
            .as_ref()
            .is_some_and(|s| s.error && s.text.contains("timeout"))
    );
    app.move_selection(1);
    assert!(
        !app.tabs[i].feed.loading(),
        "must not retry on every key after a failure"
    );
}
