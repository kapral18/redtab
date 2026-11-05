use super::*;

#[test]
fn manual_metadata_refresh_keeps_the_selected_post_and_screen_row() {
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
    app.tabs[i].feed.selected = 1;
    app.tabs[i].feed.offset = 1;
    app.reload();

    let mut updated = listing("vim", &["a", "b", "c"], None);
    updated.posts[1].score = Some(42);
    app.on_msg(Msg::Feed {
        tab,
        generation: app.tabs[i].feed.generation,
        append: false,
        result: Ok(updated),
    });

    assert_eq!(app.current_post().unwrap().id, "b");
    assert_eq!(app.current_post().unwrap().score, Some(42));
    assert_eq!(app.tabs[i].feed.selected - app.tabs[i].feed.offset, 0);
}

#[test]
fn unseen_posts_below_every_known_post_on_the_last_page_apply_without_a_notice() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    let tab = app.tabs[i].id;
    let answer = |app: &App, ids: &[&str]| Msg::Feed {
        tab,
        generation: app.tabs[i].feed.generation,
        append: false,
        result: Ok(listing("vim", ids, Some("t3_end"))),
    };
    app.on_msg(answer(&app, &["a", "b", "c"]));

    app.load_feed(i, false);
    app.on_msg(answer(&app, &["a", "c", "t"]));

    assert!(!app.tabs[i].feed.has_new_posts(&app.config));
    let ids = |app: &App| {
        app.tabs[i]
            .feed
            .posts
            .iter()
            .map(|post| post.id.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&app), ["a", "c", "t"]);
    assert_eq!(app.current_post().unwrap().id, "a");

    app.tabs[i].feed.selected = 1;
    app.tabs[i].feed.offset = 1;
    app.reload();
    app.on_msg(answer(&app, &["a", "c", "t", "u"]));

    assert_eq!(ids(&app), ["a", "c", "t", "u"]);
    assert_eq!(app.current_post().unwrap().id, "c");
    assert_eq!(app.tabs[i].feed.offset, 1);
}

#[test]
fn an_island_holding_only_unseen_posts_keeps_showing_its_posts_until_r() {
    let mut app = app_with(&["vim"]);
    app.on_msg(island_answer(&app, &["a", "b"]));

    app.on_msg(island_answer(&app, &["x", "y"]));

    assert!(app.all_subs.has_new_posts(&app.config));
    assert_eq!(island_ids(&app), ["a", "b"]);
    app.reload();
    assert_eq!(island_ids(&app), ["x", "y"]);
}

#[test]
fn a_held_island_keeps_its_order_and_selection_until_r() {
    let mut app = app_with(&["vim"]);
    app.on_msg(island_answer(&app, &["a", "b", "c", "d"]));
    app.all_subs.post = 2;
    assert_eq!(app.current_post().unwrap().id, "c");

    let mut held = island_answer(&app, &["n", "d", "c"]);
    if let Msg::Group {
        result: Ok(listing),
        ..
    } = &mut held
    {
        listing.posts[2].score = Some(99);
    }
    app.on_msg(held);

    assert!(app.all_subs.has_new_posts(&app.config));
    assert_eq!(island_ids(&app), ["a", "b", "c"]);
    assert_eq!(app.current_post().unwrap().id, "c");
    assert_eq!(app.current_post().unwrap().score, Some(99));
    app.reload();
    assert_eq!(island_ids(&app), ["n", "d", "c"]);
}

#[test]
fn reported_cached_reordering_should_wait_without_announcing_new_posts() {
    let mut app = app_with(&["vim"]);
    app.switch_tab(ALL_SUBS_TAB);
    app.on_msg(ordered_snapshot(
        island_answer(&app, &["a", "b"]),
        1,
        true,
        true,
    ));
    app.on_msg(ordered_snapshot(
        island_answer(&app, &["b", "a"]),
        2,
        false,
        false,
    ));
    assert_eq!(island_ids(&app), ["a", "b"]);
    assert!(!app.all_subs.has_new_posts(&app.config));
    app.reload();
    assert_eq!(island_ids(&app), ["b", "a"]);
}

#[test]
fn reported_cached_then_live_lists_should_keep_posts_until_refresh() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    let tab = app.tabs[i].id;
    let generation = app.tabs[i].feed.generation;
    let answer = |names: &[&str]| Msg::Feed {
        tab,
        generation,
        append: false,
        result: Ok(listing("vim", names, None)),
    };
    app.on_msg(ordered_snapshot(answer(&["a", "b"]), 1, true, true));
    let mut live = listing("vim", &["n", "b", "a"], None);
    live.posts[2].score = Some(123);
    app.on_msg(ordered_snapshot(
        Msg::Feed {
            tab,
            generation,
            append: false,
            result: Ok(live),
        },
        2,
        false,
        false,
    ));
    let ids = |app: &App| {
        app.tabs[i]
            .feed
            .posts
            .iter()
            .map(|post| post.id.as_str())
            .collect::<Vec<_>>()
            .join(",")
    };
    assert_eq!(ids(&app), "a,b");
    assert_eq!(app.tabs[i].feed.posts[0].score, Some(123));
    assert!(app.tabs[i].feed.has_new_posts(&app.config));
    app.reload();
    assert_eq!(ids(&app), "n,b,a");
}

#[test]
fn leaving_a_tab_cancels_its_pending_r_jump() {
    for focus in [Focus::List, Focus::Sidebar] {
        let mut app = app_with(&["vim", "zsh"]);
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
        app.reload();
        let generation = app.tabs[i].feed.generation;
        app.focus = focus;
        app.switch_tab(tab_index(&app, "zsh"));

        app.on_msg(Msg::Feed {
            tab,
            generation,
            append: false,
            result: Ok(listing("vim", &["new", "a", "b", "c"], None)),
        });

        let feed = &app.tabs[i].feed;
        assert_eq!(
            app.config
                .visible(&feed.posts)
                .nth(feed.selected)
                .unwrap()
                .id,
            "c",
            "{focus:?}"
        );
        assert!(feed.has_new_posts(&app.config), "{focus:?}");
    }
}

#[test]
fn waiting_arrivals_hold_unseen_tail_posts_so_nothing_new_shows_above_kept_rows() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    let tab = app.tabs[i].id;
    let answer = |app: &App, ids: &[&str]| Msg::Feed {
        tab,
        generation: app.tabs[i].feed.generation,
        append: false,
        result: Ok(listing("vim", ids, None)),
    };
    app.on_msg(answer(&app, &["a", "b", "c"]));
    app.load_feed(i, false);

    app.on_msg(answer(&app, &["n", "a", "b", "t"]));

    let ids = |app: &App| {
        app.tabs[i]
            .feed
            .posts
            .iter()
            .map(|post| post.id.clone())
            .collect::<Vec<_>>()
    };
    assert!(app.tabs[i].feed.has_new_posts(&app.config));
    assert_eq!(ids(&app), ["a", "b", "c"]);
    app.reload();
    assert_eq!(ids(&app), ["n", "a", "b", "t"]);
}

#[test]
fn page_boundary_updates_do_not_announce_new_posts() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
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
    app.load_feed(i, false);

    app.on_msg(answer(
        app.tabs[i].feed.generation,
        false,
        &["a", "b", "c"],
        Some("next"),
        2,
    ));

    assert!(!app.tabs[i].feed.has_new_posts(&app.config));
    assert_eq!(
        app.tabs[i]
            .feed
            .posts
            .iter()
            .map(|post| post.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b", "c", "d"]
    );
}

#[test]
fn a_held_answer_that_stops_leading_applies_instead_of_vanishing() {
    for home in [false, true] {
        let mut app = app_with(&["vim"]);
        let i = if home { 0 } else { tab_index(&app, "vim") };
        app.switch_tab(i);
        let tab = app.tabs[i].id;
        let answer = |app: &mut App, append: bool, posts: &[&str]| {
            app.on_msg(Msg::Feed {
                tab,
                generation: app.tabs[i].feed.generation,
                append,
                result: Ok(listing(
                    "vim",
                    posts,
                    Some(&format!("t3_{}", posts[posts.len() - 1])),
                )),
            });
        };
        answer(&mut app, false, &["a", "b"]);
        app.load_feed(i, false);
        answer(&mut app, false, &["x", "a", "y"]);
        assert!(app.tabs[i].feed.has_new_posts(&app.config));

        app.load_feed(i, true);
        answer(&mut app, true, &["x", "c"]);

        assert!(!app.tabs[i].feed.has_new_posts(&app.config));
        assert!(
            app.tabs[i].feed.posts.iter().any(|post| post.id == "y"),
            "{home}"
        );
    }
}

#[test]
fn a_feed_showing_nothing_takes_its_answer_at_once() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    let tab = app.tabs[i].id;
    app.on_msg(Msg::Feed {
        tab,
        generation: app.tabs[i].feed.generation,
        append: false,
        result: Ok(listing("vim", &[], None)),
    });
    app.load_feed(i, false);

    app.on_msg(Msg::Feed {
        tab,
        generation: app.tabs[i].feed.generation,
        append: false,
        result: Ok(listing("vim", &["a", "b"], None)),
    });

    assert!(!app.tabs[i].feed.has_new_posts(&app.config));
    assert_eq!(app.current_post().unwrap().id, "a");
}
