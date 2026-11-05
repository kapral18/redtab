use super::*;

#[test]
fn ancestor_navigation_should_follow_only_the_latest_uninterrupted_cold_feed() {
    let mut app = app_with(&["a", "b", "c", "d"]);
    let a = tab_index(&app, "a");
    app.active = a;
    app.tabs[a].feed.posts = vec![post("a1", "a")];
    app.tabs[a].feed.loaded = true;
    let c = tab_index(&app, "c");
    app.tabs[c].feed.loaded = true;
    app.tabs[c].feed.freshness = Some(Freshness {
        fetched_at: now() - crate::cache::CONTENT_TTL - 1,
        source: Source::default(),
    });
    app.open_current_post();
    for key in "]s".chars() {
        press(&mut app, crossterm::event::KeyCode::Char(key));
    }
    let b = tab_index(&app, "b");
    assert_eq!(app.active, b);
    assert!(app.post_view.is_none());
    let b_generation = app.tabs[b].feed.generation;
    press(&mut app, crossterm::event::KeyCode::Char('g'));
    app.on_msg(Msg::FeedProgress {
        tab: app.tabs[b].id,
        generation: b_generation,
        post: Box::new(post("b1", "b")),
        source: Source::default(),
    });
    assert!(
        app.post_view.is_none(),
        "a partial key must not be interrupted"
    );
    app.feed_settle.arrive(
        app.active_tab().id,
        Instant::now() + Duration::from_secs(60),
    );
    press(&mut app, crossterm::event::KeyCode::Char('t'));
    assert_eq!(app.active, c);
    app.on_msg(Msg::Feed {
        tab: app.tabs[b].id,
        generation: b_generation,
        append: false,
        result: Ok(listing("b", &["b1"], None)),
    });
    assert!(app.post_view.is_none(), "an older feed must not take focus");
    app.load_settled(Instant::now() + NAV_SETTLE);
    let c_generation = app.tabs[c].feed.generation;
    app.on_msg(Msg::FeedProgress {
        tab: app.tabs[c].id,
        generation: c_generation,
        post: Box::new(post("c1", "c")),
        source: Source::default(),
    });
    assert_eq!(app.current_post().unwrap().id, "c1");
    assert_eq!(app.focus, Focus::Post);
    for key in "]s".chars() {
        press(&mut app, crossterm::event::KeyCode::Char(key));
    }
    let d = tab_index(&app, "d");
    assert_eq!(app.active, d);
    app.load_settled(Instant::now() + NAV_SETTLE);
    let d_generation = app.tabs[d].feed.generation;
    press(&mut app, crossterm::event::KeyCode::Char('j'));
    app.on_msg(Msg::Feed {
        tab: app.tabs[d].id,
        generation: d_generation,
        append: false,
        result: Ok(listing("d", &["d1"], None)),
    });
    assert!(
        app.post_view.is_none(),
        "user navigation cancels automatic detail follow"
    );
    assert_eq!(app.focus, Focus::List);

    let mut app = app_with(&["x", "y", "z"]);
    let x = tab_index(&app, "x");
    app.active = x;
    app.tabs[x].feed.posts = vec![post("x1", "x")];
    app.tabs[x].feed.loaded = true;
    app.open_current_post();
    for key in "]s".chars() {
        press(&mut app, crossterm::event::KeyCode::Char(key));
    }
    let y = tab_index(&app, "y");
    let y_generation = app.tabs[y].feed.generation;
    app.on_msg(Msg::Feed {
        tab: app.tabs[y].id,
        generation: y_generation,
        append: false,
        result: Err("offline".into()),
    });
    assert_eq!(app.active, y);
    assert!(app.post_view.is_none());
    for key in "gt".chars() {
        press(&mut app, crossterm::event::KeyCode::Char(key));
    }
    let z = tab_index(&app, "z");
    assert_eq!(app.active, z);
    app.load_settled(Instant::now() + NAV_SETTLE);
    let generation = app.tabs[z].feed.generation;
    app.show_auth();
    app.on_msg(Msg::Feed {
        tab: app.tabs[z].id,
        generation,
        append: false,
        result: Ok(listing("z", &["z1"], None)),
    });
    assert!(matches!(app.overlay, Some(Overlay::Auth)));
    app.overlay = None;
    app.tick();
    assert!(
        app.post_view.is_none(),
        "a covered view must not reopen later"
    );

    let mut app = app_with(&["m", "n"]);
    let m = tab_index(&app, "m");
    app.active = m;
    app.tabs[m].feed.posts = vec![post("m1", "m")];
    app.tabs[m].feed.loaded = true;
    app.open_current_post();
    for key in "]s".chars() {
        press(&mut app, crossterm::event::KeyCode::Char(key));
    }
    let n = tab_index(&app, "n");
    let generation = app.tabs[n].feed.generation;
    press(&mut app, crossterm::event::KeyCode::Char('g'));
    app.on_msg(Msg::Feed {
        tab: app.tabs[n].id,
        generation,
        append: false,
        result: Ok(listing("n", &["n1"], None)),
    });
    assert!(app.post_view.is_none());
    press(&mut app, crossterm::event::KeyCode::Esc);
    app.tick();
    assert_eq!(app.current_post().unwrap().id, "n1");
    assert_eq!(app.focus, Focus::Post, "Esc cancels only the prefix");

    let mut app = app_with(&["v", "w"]);
    let v = tab_index(&app, "v");
    app.active = v;
    app.tabs[v].feed.posts = vec![post("v1", "v")];
    app.tabs[v].feed.loaded = true;
    app.open_current_post();
    for key in "]s".chars() {
        press(&mut app, crossterm::event::KeyCode::Char(key));
    }
    let w = tab_index(&app, "w");
    let generation = app.tabs[w].feed.generation;
    press(&mut app, crossterm::event::KeyCode::Char('g'));
    app.on_msg(Msg::Feed {
        tab: app.tabs[w].id,
        generation,
        append: false,
        result: Ok(listing("w", &["w1"], None)),
    });
    crate::keys::paste(&mut app, "]s");
    app.tick();
    assert!(app.post_view.is_none(), "paste cancels the deferred follow");

    let mut app = app_with(&["q", "r"]);
    let q = tab_index(&app, "q");
    app.active = q;
    app.tabs[q].feed.posts = vec![post("q1", "q")];
    app.tabs[q].feed.loaded = true;
    app.open_current_post();
    for key in "]s".chars() {
        press(&mut app, crossterm::event::KeyCode::Char(key));
    }
    let r = tab_index(&app, "r");
    let generation = app.tabs[r].feed.generation;
    press(&mut app, crossterm::event::KeyCode::Char('q'));
    assert!(!app.quit, "q returns to the list while a post is pending");
    app.on_msg(Msg::Feed {
        tab: app.tabs[r].id,
        generation,
        append: false,
        result: Ok(listing("r", &["r1"], None)),
    });
    assert!(app.post_view.is_none());
}

#[test]
fn ancestor_navigation_from_deep_comments_should_follow_loaded_feeds() {
    for width in [80, 200] {
        let mut app = app_with(&["a", "b", "c"]);
        app.config.show_nsfw = false;
        for (name, ids) in [
            ("a", vec!["a1", "a2"]),
            ("b", vec!["b1"]),
            ("c", vec!["c1"]),
        ] {
            let index = tab_index(&app, name);
            let feed = &mut app.tabs[index].feed;
            feed.posts = ids.iter().map(|id| post(id, name)).collect();
            if name == "b" {
                let mut hidden = post("hidden", "b");
                hidden.nsfw = true;
                feed.posts.insert(0, hidden);
            }
            feed.loaded = true;
            feed.shown_query = Some(feed.query());
        }
        app.active = tab_index(&app, "a");
        app.resize(width);
        app.open_current_post();
        let generation = app.post_view.as_ref().unwrap().generation;
        app.on_msg(Msg::Post {
            generation,
            result: Ok(Box::new(PostPage {
                post: post("a1", "a"),
                body: String::new(),
                comments: vec![
                    comment("root", vec![comment("deep", vec![], None)], None),
                    comment("next", vec![], None),
                ],
            })),
        });
        let view = app.post_view.as_mut().unwrap();
        view.free_scroll = false;
        view.cursor = 1;
        for key in "]]".chars() {
            press(&mut app, crossterm::event::KeyCode::Char(key));
        }
        assert_eq!(app.post_view.as_ref().unwrap().cursor, 2);
        for key in "]p".chars() {
            press(&mut app, crossterm::event::KeyCode::Char(key));
        }
        assert_eq!(app.current_post().unwrap().id, "a2");
        for key in "]s".chars() {
            press(&mut app, crossterm::event::KeyCode::Char(key));
        }
        assert_eq!(app.active_tab().subreddit(), Some("b"));
        assert_eq!(app.current_post().unwrap().id, "b1");
        assert_eq!(app.focus, Focus::Post, "width={width}");
        for key in "[s".chars() {
            press(&mut app, crossterm::event::KeyCode::Char(key));
        }
        assert_eq!(app.current_post().unwrap().id, "a2");
        for key in "gt".chars() {
            press(&mut app, crossterm::event::KeyCode::Char(key));
        }
        assert_eq!(app.current_post().unwrap().id, "b1");
        for key in "gT".chars() {
            press(&mut app, crossterm::event::KeyCode::Char(key));
        }
        assert_eq!(app.current_post().unwrap().id, "a2");
        assert_eq!(app.focus, Focus::Post);
        press(&mut app, crossterm::event::KeyCode::Tab);
        assert_eq!(app.current_post().unwrap().id, "b1");
        press(&mut app, crossterm::event::KeyCode::BackTab);
        assert_eq!(app.current_post().unwrap().id, "a2");
        app.all_subs.groups[0].posts = vec![post("a1", "a")];
        for key in "gT".chars() {
            press(&mut app, crossterm::event::KeyCode::Char(key));
        }
        assert_eq!(app.active, SEARCH_TAB);
        for key in "gT".chars() {
            press(&mut app, crossterm::event::KeyCode::Char(key));
        }
        assert_eq!(app.current_post().unwrap().id, "a1");
        for key in "gT".chars() {
            press(&mut app, crossterm::event::KeyCode::Char(key));
        }
        assert_eq!(app.active, BOOKMARKS_TAB);
        assert!(app.post_view.is_none());
        for key in "gt".chars() {
            press(&mut app, crossterm::event::KeyCode::Char(key));
        }
        assert_eq!(app.current_post().unwrap().id, "a1");
        assert_eq!(app.focus, Focus::Post);
    }

    let mut app = app_with(&["a"]);
    let a = tab_index(&app, "a");
    for index in [0, a] {
        app.tabs[index].feed.posts = vec![post("same", "a")];
        app.tabs[index].feed.loaded = true;
    }
    app.active = 0;
    app.open_current_post();
    let view = app.post_view.as_mut().unwrap();
    view.scroll = 31;
    let generation = view.generation;
    for key in "]s".chars() {
        press(&mut app, crossterm::event::KeyCode::Char(key));
    }
    assert_eq!(app.active, a);
    let view = app.post_view.as_ref().unwrap();
    assert_eq!((view.generation, view.scroll), (generation, 31));
    for key in "gt".chars() {
        press(&mut app, crossterm::event::KeyCode::Char(key));
    }
    assert_eq!(app.active, 0);
    let view = app.post_view.as_ref().unwrap();
    assert_eq!((view.generation, view.scroll), (generation, 31));
}

#[test]
fn ancestor_navigation_should_follow_all_subs_groups_and_leave_empty_destinations_usable() {
    let mut app = app_with(&["a", "b", "c"]);
    app.resize(80);
    app.all_subs.groups[0].posts = vec![post("a1", "a")];
    app.all_subs.groups[2].posts = vec![post("c1", "c")];
    app.open_current_post();
    for key in "]s".chars() {
        press(&mut app, crossterm::event::KeyCode::Char(key));
    }
    assert_eq!(app.all_subs.group, 1);
    assert!(app.post_view.is_none());
    app.picture_settle.due(Instant::now() + NAV_SETTLE);
    app.groups_drawn(&[1], &[], &[]);
    let generation = app.all_subs.groups[1].generation;
    app.on_msg(Msg::Group {
        subreddit: "b".into(),
        generation,
        result: Ok(listing("b", &["b1"], None)),
    });
    assert_eq!(app.current_post().unwrap().id, "b1");
    assert_eq!(app.focus, Focus::Post);
    for key in "]s".chars() {
        press(&mut app, crossterm::event::KeyCode::Char(key));
    }
    assert_eq!(app.current_post().unwrap().id, "c1");
    assert_eq!(app.focus, Focus::Post);
    let generation = app.post_view.as_ref().unwrap().generation;
    for key in "]s".chars() {
        press(&mut app, crossterm::event::KeyCode::Char(key));
    }
    assert_eq!(app.post_view.as_ref().unwrap().generation, generation);
    for key in "[s".chars() {
        press(&mut app, crossterm::event::KeyCode::Char(key));
    }
    assert_eq!(app.current_post().unwrap().id, "b1");
    app.all_subs.groups[0].posts.clear();
    app.all_subs.groups[0].freshness = Some(Freshness {
        fetched_at: now(),
        source: Source::default(),
    });
    for key in "[s".chars() {
        press(&mut app, crossterm::event::KeyCode::Char(key));
    }
    assert_eq!(app.all_subs.group, 0);
    assert!(app.post_view.is_none());
    assert_eq!(app.focus, Focus::List);
    for key in "]s".chars() {
        press(&mut app, crossterm::event::KeyCode::Char(key));
    }
    assert_eq!(app.current_post().unwrap().id, "b1");
    assert_eq!(app.focus, Focus::Post);
    app.resize(200);
    app.focus_main();
    app.all_subs.groups[2].posts.clear();
    app.all_subs.groups[2].freshness = Some(Freshness {
        fetched_at: now(),
        source: Source::default(),
    });
    app.move_all_subs_group(1);
    assert_eq!(app.all_subs.group, 2);
    app.open_current_post();
    assert_eq!(app.current_post().unwrap().id, "b1");
    for key in "]s".chars() {
        press(&mut app, crossterm::event::KeyCode::Char(key));
    }
    assert_eq!(app.all_subs.group, 2);
    assert!(
        app.post_view.is_none(),
        "the selected post's community owns the move"
    );
    for key in "[s".chars() {
        press(&mut app, crossterm::event::KeyCode::Char(key));
    }
    assert_eq!(app.current_post().unwrap().id, "b1");
    app.all_subs.groups[2].freshness = Some(Freshness {
        fetched_at: now() - crate::cache::CONTENT_TTL - 1,
        source: Source::default(),
    });
    for key in "]s".chars() {
        press(&mut app, crossterm::event::KeyCode::Char(key));
    }
    assert!(app.post_view.is_none());
    app.picture_settle.due(Instant::now() + NAV_SETTLE);
    app.groups_drawn(&[2], &[], &[]);
    let generation = app.all_subs.groups[2].generation;
    app.on_msg(Msg::Group {
        subreddit: "c".into(),
        generation,
        result: Ok(listing("c", &["fresh"], None)),
    });
    assert_eq!(app.current_post().unwrap().id, "fresh");
    assert_eq!(app.focus, Focus::Post);
}
