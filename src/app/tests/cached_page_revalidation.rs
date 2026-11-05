use super::*;

#[test]
fn cached_refresh_keeps_loaded_later_pages_and_their_cursor() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    let tab = app.tabs[i].id;
    let generation = app.tabs[i].feed.generation;
    app.on_msg(Msg::Feed {
        tab,
        generation,
        append: false,
        result: Ok(listing("vim", &["a", "b"], Some("t3_b"))),
    });
    app.on_msg(Msg::Feed {
        tab,
        generation,
        append: true,
        result: Ok(listing("vim", &["c", "d"], Some("t3_d"))),
    });
    app.tabs[i].feed.selected = 3;
    app.reload();

    app.on_msg(Msg::Feed {
        tab,
        generation: app.tabs[i].feed.generation,
        append: false,
        result: Ok(listing("vim", &["new", "a"], Some("t3_a"))),
    });

    assert_eq!(app.current_post().unwrap().id, "new");
    assert_eq!(app.tabs[i].feed.after.as_deref(), Some("t3_d"));
    assert_eq!(
        app.tabs[i]
            .feed
            .posts
            .iter()
            .map(|p| p.id.as_str())
            .collect::<Vec<_>>(),
        ["new", "a", "b", "c", "d"]
    );
}

#[test]
fn revalidating_a_cached_next_page_replaces_that_page() {
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
    let generation = app.tabs[i].feed.generation;
    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Feed {
            tab,
            generation,
            append: true,
            result: Ok(listing("vim", &["c", "d"], Some("t3_d"))),
        }),
        fetched_at: 1,
        refreshing: true,
        source: Source {
            cached: true,
            ..Source::default()
        },
    });
    app.tabs[i].feed.selected = 2;

    app.on_msg(Msg::Feed {
        tab,
        generation,
        append: true,
        result: Ok(listing("vim", &["x", "c"], Some("t3_c"))),
    });

    assert!(!app.tabs[i].feed.has_new_posts(&app.config));
    assert_eq!(
        app.tabs[i]
            .feed
            .posts
            .iter()
            .map(|p| p.id.as_str())
            .collect::<Vec<_>>(),
        // d may have moved onto the next page: it stays until that page answers.
        ["a", "b", "x", "c", "d"]
    );
    assert_eq!(app.current_post().unwrap().id, "c");
    assert_eq!(app.tabs[i].feed.after.as_deref(), Some("t3_c"));
}

#[test]
fn cache_refresh_keeps_updated_posts_that_move_across_pages() {
    let mut feed = Feed::default();
    feed.replace_page(
        None,
        listing("vim", &["a", "b"], None).posts,
        Some("b".into()),
        ordered_source(1, false),
    );
    feed.replace_page(
        Some("b".into()),
        listing("vim", &["c", "d"], None).posts,
        None,
        ordered_source(2, false),
    );
    let mut updated = post("c", "vim");
    updated.title = "updated title".into();

    feed.replace_page(
        None,
        vec![updated, post("a", "vim")],
        Some("a".into()),
        ordered_source(3, false),
    );

    assert_eq!(
        feed.posts.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
        ["c", "a", "d"]
    );
    assert_eq!(feed.posts[0].title, "updated title");
    // The moved item no longer belongs to the second page's replacement range.
    feed.replace_page(
        Some("b".into()),
        listing("vim", &["x", "d"], None).posts,
        None,
        ordered_source(4, false),
    );
    assert_eq!(
        feed.posts.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
        ["c", "a", "x", "d"]
    );
}

#[test]
fn newer_cached_page_updates_an_overlapping_post_after_restart() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    let tab = app.tabs[i].id;
    app.on_msg(ordered_snapshot(
        Msg::Feed {
            tab,
            generation: app.tabs[i].feed.generation,
            append: false,
            result: Ok(listing("vim", &["a", "b"], Some("b"))),
        },
        1,
        true,
        false,
    ));
    app.load_feed(i, true);
    let mut later = listing("vim", &["a", "c"], None);
    later.posts[0].title = "newer cached title".into();

    app.on_msg(ordered_snapshot(
        Msg::Feed {
            tab,
            generation: app.tabs[i].feed.generation,
            append: true,
            result: Ok(later),
        },
        2,
        true,
        false,
    ));

    assert_eq!(app.tabs[i].feed.posts[1].title, "newer cached title");
    assert_eq!(
        app.tabs[i]
            .feed
            .posts
            .iter()
            .map(|p| p.id.as_str())
            .collect::<Vec<_>>(),
        ["b", "a", "c"]
    );
}
