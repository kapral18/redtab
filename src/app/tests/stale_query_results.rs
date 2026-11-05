use super::*;

#[test]
fn stale_feed_results_are_dropped_and_pages_append_without_duplicates() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    let id = app.tabs[i].id;
    let stale = app.tabs[i].feed.generation;
    app.load_feed(i, false);
    let current = app.tabs[i].feed.generation;

    app.on_msg(Msg::Feed {
        tab: id,
        generation: stale,
        append: false,
        result: Ok(listing("vim", &["old"], None)),
    });
    assert!(app.tabs[i].feed.posts.is_empty());

    app.on_msg(Msg::Feed {
        tab: id,
        generation: current,
        append: false,
        result: Ok(listing("vim", &["a", "b"], Some("t3_b"))),
    });
    app.load_feed(i, true);
    let next = app.tabs[i].feed.generation;
    app.on_msg(Msg::Feed {
        tab: id,
        generation: next,
        append: true,
        result: Ok(listing("vim", &["b", "c"], None)),
    });

    let ids: Vec<&str> = app.tabs[i]
        .feed
        .posts
        .iter()
        .map(|p| p.id.as_str())
        .collect();
    assert_eq!(ids, ["a", "b", "c"]);
    assert_eq!(app.tabs[i].feed.after, None);
}

/// A failed load keeps the posts only when it asked for the same listing:
/// a reload keeps the reader's place, a new sort shows nothing to page onto.
#[test]
fn a_failed_load_keeps_the_posts_only_for_the_same_query() {
    let fail_after = |change: fn(&mut App)| {
        let mut app = app_with(&["vim"]);
        let i = tab_index(&app, "vim");
        app.switch_tab(i);
        let id = app.tabs[i].id;
        let g = app.tabs[i].feed.generation;
        app.on_msg(Msg::Feed {
            tab: id,
            generation: g,
            append: false,
            result: Ok(listing("vim", &["h1", "h2"], Some("t3_h2"))),
        });
        app.move_selection(1);
        change(&mut app);
        let g = app.tabs[i].feed.generation;
        app.on_msg(Msg::Feed {
            tab: id,
            generation: g,
            append: false,
            result: Err("timeout".into()),
        });
        app
    };

    let reloaded = fail_after(App::reload);
    let resorted = fail_after(|app| app.set_sort(Sort::New));
    let (reloaded, resorted) = (
        &reloaded.tabs[reloaded.active].feed,
        &resorted.tabs[resorted.active].feed,
    );

    assert_eq!(reloaded.posts.len(), 2);
    assert_eq!(reloaded.selected, 1, "the reader keeps their place");
    assert_eq!(reloaded.after.as_deref(), Some("t3_h2"));
    assert!(
        resorted.posts.is_empty(),
        "hot posts must not show under New"
    );
    assert_eq!(
        resorted.after, None,
        "L must not page New from a hot cursor"
    );
    assert_eq!(resorted.error(), Some("timeout"));
}

#[test]
fn changing_feed_query_discards_buffered_results() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    let tab = app.tabs[i].id;
    let answer = |generation, id| Msg::Feed {
        tab,
        generation,
        append: false,
        result: Ok(listing("vim", &[id], None)),
    };
    app.on_msg(answer(app.tabs[i].feed.generation, "old"));
    app.load_feed(i, false);
    app.on_msg(answer(app.tabs[i].feed.generation, "buffered"));
    assert!(app.tabs[i].feed.has_new_posts(&app.config));

    app.set_sort(Sort::New);
    app.on_msg(answer(app.tabs[i].feed.generation, "sorted"));
    app.reload();

    assert!(!app.tabs[i].feed.has_new_posts(&app.config));
    assert_eq!(app.current_post().unwrap().id, "sorted");
    assert!(app.tabs[i].feed.loading());
}

#[test]
fn when_a_streams_query_is_replaced_late_posts_should_not_enter_the_new_query() {
    let mut app = app_with(&["s"]);
    let index = tab_index(&app, "s");
    app.switch_tab(index);
    let tab = app.tabs[index].id;
    let old = app.tabs[index].feed.generation;
    app.tabs[index].feed.sort = Sort::New;
    app.fetch_feed(index, None, false);

    app.on_msg(Msg::FeedProgress {
        tab,
        generation: old,
        post: Box::new(post("obsolete", "s")),
        source: ordered_source(2, false),
    });

    assert!(app.tabs[index].feed.posts.is_empty());
    assert!(app.tabs[index].feed.loading());
}

#[test]
fn pagination_prefetch_should_reject_stale_queries_and_completed_cursors() {
    for stale_query in [true, false] {
        let mut app = app_with(&["vim"]);
        let index = tab_index(&app, "vim");
        app.switch_tab(index);
        app.load_feed(index, false);
        app.on_msg(Msg::Feed {
            tab: app.tabs[index].id,
            generation: app.tabs[index].feed.generation,
            append: false,
            result: Ok(listing("vim", &["a"], Some("a"))),
        });
        if stale_query {
            app.tabs[index].feed.sort = Sort::New;
        } else {
            app.load_feed(index, true);
            app.on_msg(Msg::Feed {
                tab: app.tabs[index].id,
                generation: app.tabs[index].feed.generation,
                append: true,
                result: Ok(listing("vim", &["b"], Some("a"))),
            });
        }

        press(&mut app, crossterm::event::KeyCode::PageDown);
        app.tick_at(Instant::now() + NAV_SETTLE);

        assert!(
            !app.tabs[index].feed.loading_more(),
            "stale_query={stale_query}: scrolling must not request an invalid continuation"
        );
    }
}
