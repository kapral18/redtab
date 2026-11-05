use super::*;

#[test]
fn entering_a_feed_should_apply_waiting_posts_but_hold_later_arrivals() {
    let feed_app = || {
        let mut app = app_with(&["vim"]);
        let i = tab_index(&app, "vim");
        app.switch_tab(i);
        let tab = app.tabs[i].id;
        app.on_msg(Msg::Feed {
            tab,
            generation: app.tabs[i].feed.generation,
            append: false,
            result: Ok(listing("vim", &["a", "b"], None)),
        });
        (app, i, tab)
    };
    let answer = |app: &mut App, i: usize, tab: u64| {
        app.on_msg(Msg::Feed {
            tab,
            generation: app.tabs[i].feed.generation,
            append: false,
            result: Ok(listing("vim", &["n", "a", "b"], None)),
        });
    };

    // New posts that waited while you were away show on arrival.
    let (mut app, i, tab) = feed_app();
    app.load_feed(i, false);
    answer(&mut app, i, tab);
    assert!(app.tabs[i].feed.has_new_posts(&app.config));
    app.switch_tab(0);
    app.switch_tab(i);
    assert!(!app.tabs[i].feed.has_new_posts(&app.config));
    assert_eq!(app.current_post().unwrap().id, "n");

    let (mut app, i, tab) = feed_app();
    app.switch_tab(0);
    app.load_feed(i, false);
    app.switch_tab(i);
    answer(&mut app, i, tab);
    assert!(app.tabs[i].feed.has_new_posts(&app.config));
    assert_eq!(app.current_post().unwrap().id, "a");

    // Arriving through the sidebar starts the load once navigation settles.
    let (mut app, i, tab) = feed_app();
    app.switch_tab(0);
    if let Some(freshness) = &mut app.tabs[i].feed.freshness {
        freshness.fetched_at = 0;
    }
    app.focus = Focus::Sidebar;
    app.switch_tab(i);
    assert!(!app.tabs[i].feed.loading());
    let _ = app.tick_at(Instant::now() + NAV_SETTLE);
    assert!(app.tabs[i].feed.loading());
    answer(&mut app, i, tab);
    assert!(app.tabs[i].feed.has_new_posts(&app.config));
    assert_eq!(app.current_post().unwrap().id, "a");

    let (mut app, i, _) = feed_app();
    app.switch_tab(0);
    app.all_subs.groups[0].needs_refresh = true;
    app.picture_settle.due(Instant::now() + NAV_SETTLE);
    app.ensure_group_loaded(0);
    assert!(app.all_subs.groups[0].loading);
    app.switch_tab(i);
    app.on_msg(island_answer(&app, &["n", "a", "b"]));
    assert!(app.tabs[i].feed.has_new_posts(&app.config));
    assert_eq!(app.current_post().unwrap().id, "a");

    // Including when the island answers from its cache first.
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    let tab = app.tabs[i].id;
    app.on_msg(ordered_snapshot(
        Msg::Feed {
            tab,
            generation: app.tabs[i].feed.generation,
            append: false,
            result: Ok(listing("vim", &["a", "b"], None)),
        },
        1,
        false,
        false,
    ));
    app.switch_tab(0);
    app.all_subs.groups[0].needs_refresh = true;
    app.picture_settle.due(Instant::now() + NAV_SETTLE);
    app.ensure_group_loaded(0);
    app.switch_tab(i);
    app.on_msg(ordered_snapshot(
        island_answer(&app, &["a", "b"]),
        2,
        true,
        true,
    ));
    app.on_msg(ordered_snapshot(
        island_answer(&app, &["n", "a", "b"]),
        3,
        false,
        false,
    ));
    assert!(app.tabs[i].feed.has_new_posts(&app.config));
    assert_eq!(app.current_post().unwrap().id, "a");

    // Moving before it answers means you are reading: they wait behind the notice.
    let (mut app, i, tab) = feed_app();
    app.switch_tab(0);
    app.load_feed(i, false);
    app.switch_tab(i);
    app.focus = Focus::List;
    app.move_selection(1);
    answer(&mut app, i, tab);
    assert!(app.tabs[i].feed.has_new_posts(&app.config));
    assert_eq!(app.current_post().unwrap().id, "b");

    // So does moving before navigation settles starts the load.
    let (mut app, i, tab) = feed_app();
    app.switch_tab(0);
    if let Some(freshness) = &mut app.tabs[i].feed.freshness {
        freshness.fetched_at = 0;
    }
    app.focus = Focus::List;
    app.switch_tab(i);
    assert!(!app.tabs[i].feed.loading());
    app.move_selection(1);
    let _ = app.tick_at(Instant::now() + NAV_SETTLE * 3);
    assert!(app.tabs[i].feed.loading());
    answer(&mut app, i, tab);
    assert!(app.tabs[i].feed.has_new_posts(&app.config));
    assert_eq!(app.current_post().unwrap().id, "b");

    // And moving before the community's island load answers.
    let (mut app, i, _) = feed_app();
    app.switch_tab(0);
    app.all_subs.groups[0].needs_refresh = true;
    app.picture_settle.due(Instant::now() + NAV_SETTLE);
    app.ensure_group_loaded(0);
    assert!(app.all_subs.groups[0].loading);
    app.switch_tab(i);
    app.focus = Focus::List;
    app.move_selection(1);
    app.on_msg(island_answer(&app, &["n", "a", "b"]));
    assert!(app.tabs[i].feed.has_new_posts(&app.config));
    assert_eq!(app.current_post().unwrap().id, "b");

    for buffered in [false, true] {
        for card_scroll in [false, true] {
            let (mut app, i, tab) = feed_app();
            app.focus = Focus::List;
            if card_scroll {
                if app.state.list_view() != crate::state::ListView::Card {
                    app.state.toggle_list_view();
                }
                app.tabs[i].feed.cards.line = 3;
            } else {
                app.move_selection(1);
            }
            app.switch_tab(0);
            app.load_feed(i, false);
            if buffered {
                answer(&mut app, i, tab);
            }
            app.switch_tab(i);
            if !buffered {
                answer(&mut app, i, tab);
            }
            assert_eq!(app.tabs[i].feed.has_new_posts(&app.config), !buffered);
            assert_eq!(
                app.current_post().unwrap().id,
                if buffered {
                    "n"
                } else if card_scroll {
                    "a"
                } else {
                    "b"
                }
            );
            if buffered {
                assert_eq!(app.tabs[i].feed.cards.line, 0);
            }
        }
    }

    // `r` before navigation settles ends the arrival: later loads wait behind the notice.
    let (mut app, i, tab) = feed_app();
    app.switch_tab(0);
    if let Some(freshness) = &mut app.tabs[i].feed.freshness {
        freshness.fetched_at = 0;
    }
    app.focus = Focus::Sidebar;
    app.switch_tab(i);
    app.reload();
    app.on_msg(Msg::Feed {
        tab,
        generation: app.tabs[i].feed.generation,
        append: false,
        result: Ok(listing("vim", &["a", "b"], None)),
    });
    let _ = app.tick_at(Instant::now() + NAV_SETTLE * 3);
    if let Some(freshness) = &mut app.tabs[i].feed.freshness {
        freshness.fetched_at = 0;
    }
    app.ensure_feed_loaded(i);
    assert!(app.tabs[i].feed.loading());
    answer(&mut app, i, tab);
    assert!(app.tabs[i].feed.has_new_posts(&app.config));
    assert_eq!(app.current_post().unwrap().id, "a");
}

#[test]
fn manual_fetch_with_arrivals_shows_the_list_top_but_a_forced_next_page_keeps_position() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    let tab = app.tabs[i].id;
    app.on_msg(Msg::Feed {
        tab,
        generation: app.tabs[i].feed.generation,
        append: false,
        result: Ok(listing("vim", &["a", "b", "c"], Some("t3_c"))),
    });
    app.tabs[i].feed.selected = 2;
    app.tabs[i].feed.offset = 2;
    app.reload();

    app.on_msg(Msg::Feed {
        tab,
        generation: app.tabs[i].feed.generation,
        append: false,
        result: Ok(listing("vim", &["new", "a", "b", "c"], Some("t3_c"))),
    });

    assert_eq!(app.current_post().unwrap().id, "new");
    assert_eq!(app.tabs[i].feed.offset, 0);

    app.tabs[i].feed.selected = 3;
    app.tabs[i].feed.offset = 3;
    app.fetch_manual(Some("t3_c".into()));
    app.on_msg(Msg::Feed {
        tab,
        generation: app.tabs[i].feed.generation,
        append: true,
        result: Ok(listing("vim", &["d", "e"], None)),
    });

    assert_eq!(app.current_post().unwrap().id, "c");
    assert_eq!(app.tabs[i].feed.offset, 3);
}

#[test]
fn entering_all_subs_should_apply_waiting_posts_but_hold_later_arrivals() {
    let subs_app = || {
        let mut app = app_with(&["vim"]);
        app.switch_tab(ALL_SUBS_TAB);
        app.on_msg(island_answer(&app, &["a", "b", "c"]));
        app
    };

    // New posts that waited while you were away show on arrival.
    let mut app = subs_app();
    app.on_msg(island_answer(&app, &["n", "a", "b"]));
    assert!(app.all_subs.has_new_posts(&app.config));
    app.switch_tab(0);
    app.switch_tab(ALL_SUBS_TAB);
    assert!(!app.all_subs.has_new_posts(&app.config));
    assert_eq!(island_ids(&app), ["n", "a", "b"]);

    let mut app = subs_app();
    app.switch_tab(0);
    app.all_subs.groups[0].needs_refresh = true;
    app.switch_tab(ALL_SUBS_TAB);
    app.picture_settle.due(Instant::now() + NAV_SETTLE);
    app.groups_drawn(&[0], &[], &[]);
    assert!(app.all_subs.groups[0].loading);
    app.on_msg(island_answer(&app, &["n", "a", "b"]));
    assert!(app.all_subs.has_new_posts(&app.config));
    assert_eq!(app.current_post().unwrap().id, "a");

    let mut app = subs_app();
    app.switch_tab(0);
    app.all_subs.groups[0].needs_refresh = true;
    app.ensure_group_loaded(0);
    assert!(app.all_subs.groups[0].loading);
    app.focus = Focus::Sidebar;
    app.switch_tab(ALL_SUBS_TAB);
    app.on_msg(island_answer(&app, &["n", "a", "b"]));
    assert!(app.all_subs.has_new_posts(&app.config));
    assert_eq!(app.current_post().unwrap().id, "a");

    // Through the sidebar, loads start only once navigation settles.
    let mut app = subs_app();
    app.focus = Focus::Sidebar;
    app.switch_tab(0);
    app.all_subs.groups[0].needs_refresh = true;
    app.switch_tab(ALL_SUBS_TAB);
    app.groups_drawn(&[0], &[], &[]);
    assert!(!app.all_subs.groups[0].loading);
    app.picture_settle.due(Instant::now() + NAV_SETTLE);
    app.groups_drawn(&[0], &[], &[]);
    assert!(app.all_subs.groups[0].loading);
    app.on_msg(island_answer(&app, &["n", "a", "b"]));
    assert!(app.all_subs.has_new_posts(&app.config));

    // A later load is a refresh while you sit there: it waits behind the notice.
    let mut app = subs_app();
    app.switch_tab(0);
    app.switch_tab(ALL_SUBS_TAB);
    app.picture_settle.due(Instant::now() + NAV_SETTLE);
    app.groups_drawn(&[0], &[], &[]);
    assert!(!app.all_subs.groups[0].loading);
    app.on_msg(island_answer(&app, &["n", "a", "b"]));
    assert!(app.all_subs.has_new_posts(&app.config));

    // Moving first means you are reading: they wait behind the notice.
    let mut app = subs_app();
    app.switch_tab(0);
    app.all_subs.groups[0].needs_refresh = true;
    app.switch_tab(ALL_SUBS_TAB);
    app.focus = Focus::List;
    app.move_selection(1);
    app.picture_settle.due(Instant::now() + NAV_SETTLE);
    app.groups_drawn(&[0], &[], &[]);
    assert!(app.all_subs.groups[0].loading);
    app.on_msg(island_answer(&app, &["n", "a", "b"]));
    assert!(app.all_subs.has_new_posts(&app.config));
    assert_eq!(app.current_post().unwrap().id, "b");

    for buffered in [false, true] {
        let mut app = subs_app();
        app.all_subs.post = 1;
        app.switch_tab(0);
        app.all_subs.groups[0].needs_refresh = true;
        app.picture_settle.due(Instant::now() + NAV_SETTLE);
        app.ensure_group_loaded(0);
        assert!(app.all_subs.groups[0].loading);
        if buffered {
            app.on_msg(island_answer(&app, &["n", "a", "b"]));
        }
        app.switch_tab(ALL_SUBS_TAB);
        if !buffered {
            app.on_msg(island_answer(&app, &["n", "a", "b"]));
        }
        assert_eq!(app.all_subs.has_new_posts(&app.config), !buffered);
        assert_eq!(
            app.current_post().unwrap().id,
            if buffered { "n" } else { "b" }
        );
    }
}
