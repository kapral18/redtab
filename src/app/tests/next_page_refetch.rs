use super::*;

#[test]
fn refetching_a_next_page_from_a_new_end_bypasses_the_cache() {
    // Whether or not the page was first asked for past the cache.
    for forced in [true, false] {
        let mut app = app_with(&["vim"]);
        let i = tab_index(&app, "vim");
        app.switch_tab(i);
        let tab = app.tabs[i].id;
        let answer = |app: &mut App, generation: u64, append: bool, posts: &[&str], after| {
            app.on_msg(Msg::Feed {
                tab,
                generation,
                append,
                result: Ok(listing("vim", posts, after)),
            });
        };
        let generation = app.tabs[i].feed.generation;
        answer(&mut app, generation, false, &["a", "b"], Some("t3_b"));
        app.load_feed(i, true);
        let generation = app.tabs[i].feed.generation;
        answer(&mut app, generation, true, &["c", "d"], None);
        if forced {
            app.fetch_manual(Some("t3_b".into()));
        }
        app.load_feed(i, false);

        let generation = app.tabs[i].feed.generation;
        answer(&mut app, generation, false, &["a", "t"], Some("t3_t"));

        let feed = &app.tabs[i].feed;
        let (generation, _) = feed
            .requests
            .iter()
            .find(|(_, cursor)| cursor.as_deref() == Some("t3_t"))
            .unwrap();
        assert!(feed.forced.contains(generation), "forced {forced}");
    }
}

#[test]
fn a_kept_row_an_older_next_page_still_lists_stays() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    let tab = app.tabs[i].id;
    let answer = |app: &mut App, append: bool, posts: &[&str], after: Option<&str>, order| {
        app.on_msg(ordered_snapshot(
            Msg::Feed {
                tab,
                generation: app.tabs[i].feed.generation,
                append,
                result: Ok(listing("vim", posts, after)),
            },
            order,
            false,
            false,
        ));
    };
    answer(&mut app, false, &["a", "b"], Some("t3_b"), 10);
    app.load_feed(i, true);
    answer(&mut app, true, &["c", "d"], Some("t3_d"), 11);
    app.fetch_feed(i, Some("t3_b".into()), false);
    answer(&mut app, true, &["c", "e"], Some("t3_e"), 12);
    app.load_feed(i, true);

    answer(&mut app, true, &["d", "f"], None, 5);

    let ids: Vec<_> = app.tabs[i]
        .feed
        .posts
        .iter()
        .map(|post| post.id.as_str())
        .collect();
    assert!(ids.contains(&"d"), "{ids:?}");
}

#[test]
fn a_row_leaving_the_first_page_leaves_once_the_next_page_answers_without_it() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    let tab = app.tabs[i].id;
    let ids = |app: &App| {
        app.tabs[i]
            .feed
            .posts
            .iter()
            .map(|post| post.id.clone())
            .collect::<Vec<_>>()
    };
    let answer = |app: &mut App, append: bool, posts: &[&str], after: Option<&str>| {
        app.on_msg(Msg::Feed {
            tab,
            generation: app.tabs[i].feed.generation,
            append,
            result: Ok(listing("vim", posts, after)),
        });
    };
    answer(&mut app, false, &["a", "x", "b"], Some("t3_b"));
    app.load_feed(i, true);
    answer(&mut app, true, &["c", "d"], None);

    app.reload();
    answer(&mut app, false, &["a", "c", "b"], Some("t3_b"));
    assert_eq!(ids(&app), ["a", "c", "b", "x", "d"]);
    let restitching = app.tabs[i].feed.generation;
    app.reload();
    let generation = app.tabs[i].feed.generation;
    assert_ne!(generation, restitching);
    assert_eq!(app.tabs[i].feed.requests.get(&generation), Some(&None));
    answer(&mut app, false, &["a", "c", "b"], Some("t3_b"));
    answer(&mut app, true, &["d"], None);

    assert_eq!(ids(&app), ["a", "c", "b", "d"]);
    assert!(!app.tabs[i].feed.loading());
}

#[test]
fn a_post_being_read_survives_the_next_page_refetch_its_drop_starts() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    let tab = app.tabs[i].id;
    let answer = |app: &mut App, append: bool, posts: &[&str], after: Option<&str>| {
        app.on_msg(Msg::Feed {
            tab,
            generation: app.tabs[i].feed.generation,
            append,
            result: Ok(listing("vim", posts, after)),
        });
    };
    answer(&mut app, false, &["a", "b", "c"], Some("t3_c"));
    app.load_feed(i, true);
    answer(&mut app, true, &["d", "e"], None);
    app.focus = Focus::List;
    app.move_selection(2);
    app.load_feed(i, false);

    answer(&mut app, false, &["a", "b", "x"], Some("t3_x"));
    answer(&mut app, true, &["d", "e"], None);

    assert_eq!(app.current_post().unwrap().id, "c");
    assert!(app.tabs[i].feed.posts.iter().any(|post| post.id == "c"));

    // After `r`, a dropped selection keeps its row even while the post waits below.
    app.reload();
    answer(&mut app, false, &["a", "b", "y"], Some("t3_y"));
    assert!(app.tabs[i].feed.posts.iter().any(|post| post.id == "c"));
    assert_eq!(app.current_post().unwrap().id, "y");
}

#[test]
fn stale_first_page_refresh_does_not_block_loading_the_next_page() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    let tab = app.tabs[i].id;
    let first = app.tabs[i].feed.generation;
    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Feed {
            tab,
            generation: first,
            append: false,
            result: Ok(listing("vim", &["a", "b"], Some("t3_b"))),
        }),
        fetched_at: 1,
        refreshing: true,
        source: Source {
            cached: true,
            ..Source::default()
        },
    });

    app.load_feed(i, true);
    let next = app.tabs[i].feed.generation;
    app.on_msg(Msg::Feed {
        tab,
        generation: next,
        append: true,
        result: Ok(listing("vim", &["c"], None)),
    });
    app.on_msg(Msg::Feed {
        tab,
        generation: first,
        append: false,
        result: Ok(listing("vim", &["new", "a"], Some("t3_a"))),
    });

    app.reload();
    assert_ne!(next, first);
    assert_eq!(
        app.tabs[i]
            .feed
            .posts
            .iter()
            .map(|p| p.id.as_str())
            .collect::<Vec<_>>(),
        ["new", "a", "b", "c"]
    );
}

#[test]
fn rows_kept_below_the_first_page_stay_until_the_refetched_next_page_answers() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    let tab = app.tabs[i].id;
    let ids = |app: &App| {
        app.tabs[i]
            .feed
            .posts
            .iter()
            .map(|post| post.id.clone())
            .collect::<Vec<_>>()
    };
    let answer = |app: &mut App, append: bool, posts: &[&str], after: Option<&str>| {
        app.on_msg(Msg::Feed {
            tab,
            generation: app.tabs[i].feed.generation,
            append,
            result: Ok(listing("vim", posts, after)),
        });
    };
    answer(&mut app, false, &["a", "b"], Some("t3_b"));
    app.load_feed(i, true);
    answer(&mut app, true, &["c", "d"], None);

    app.load_feed(i, false);
    answer(&mut app, false, &["a", "t"], Some("t3_t"));
    assert_eq!(ids(&app), ["a", "t", "b", "c", "d"]);
    assert!(
        app.tabs[i]
            .feed
            .requests
            .values()
            .any(|cursor| cursor.as_deref() == Some("t3_t"))
    );
    app.load_feed(i, false);
    answer(&mut app, false, &["a", "u"], Some("t3_u"));
    assert_eq!(ids(&app), ["a", "u", "t", "b", "c", "d"]);
    let next: Vec<_> = app.tabs[i]
        .feed
        .requests
        .values()
        .filter_map(|cursor| cursor.as_deref())
        .collect();
    assert_eq!(next, ["t3_u"]);

    answer(&mut app, true, &["b", "c", "d"], None);
    assert_eq!(ids(&app), ["a", "u", "b", "c", "d"]);
    app.load_feed(i, false);
    answer(&mut app, false, &["a", "u"], Some("t3_u"));
    assert_eq!(ids(&app), ["a", "u", "b", "c", "d"]);
    assert!(!app.tabs[i].feed.loading());
}

#[test]
fn a_next_page_requested_before_the_first_page_changed_is_fetched_from_its_new_end() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    let tab = app.tabs[i].id;
    let ids = |app: &App| {
        app.tabs[i]
            .feed
            .posts
            .iter()
            .map(|post| post.id.clone())
            .collect::<Vec<_>>()
    };
    let answer = |app: &mut App, generation: u64, append: bool, posts: &[&str], after| {
        app.on_msg(Msg::Feed {
            tab,
            generation,
            append,
            result: Ok(listing("vim", posts, after)),
        });
    };
    let generation = app.tabs[i].feed.generation;
    answer(&mut app, generation, false, &["a", "b"], Some("t3_b"));
    app.load_feed(i, true);
    let stale = app.tabs[i].feed.generation;
    app.load_feed(i, false);

    let generation = app.tabs[i].feed.generation;
    answer(&mut app, generation, false, &["a", "t"], Some("t3_t"));
    assert_eq!(ids(&app), ["a", "t", "b"]);
    answer(&mut app, stale, true, &["c", "d"], None);
    assert_eq!(ids(&app), ["a", "t", "b"]);
    let generation = app.tabs[i].feed.generation;
    answer(&mut app, generation, true, &["b", "c", "d"], None);

    assert_eq!(ids(&app), ["a", "t", "b", "c", "d"]);
    assert!(!app.tabs[i].feed.loading());
}
