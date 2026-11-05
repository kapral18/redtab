use super::*;

#[test]
fn private_community_cross_view_access_should_keep_source_precedence() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    let group_generation = app.all_subs.groups[0].generation;
    app.on_msg(ordered_snapshot(
        Msg::Group {
            subreddit: "vim".into(),
            generation: group_generation,
            result: Ok(listing("vim", &["cached"], None)),
        },
        1,
        false,
        false,
    ));
    let denial = || Msg::Group {
        subreddit: "vim".into(),
        generation: group_generation,
        result: Err(FeedError::Access(AccessDenied::PrivateCommunity)),
    };
    app.on_msg(ordered_snapshot(denial(), 3, false, false));
    let success = |app: &App| Msg::Feed {
        tab: app.tabs[i].id,
        generation: app.tabs[i].feed.generation,
        append: false,
        result: Ok(listing("vim", &["accessible"], None)),
    };
    app.on_msg(ordered_snapshot(success(&app), 2, true, false));
    assert_eq!(
        app.tabs[i].feed.access_denied(),
        Some(AccessDenied::PrivateCommunity)
    );
    assert_eq!(app.group_posts(&app.all_subs.groups[0]).count(), 0);
    app.on_msg(ordered_snapshot(success(&app), 4, false, false));
    app.on_msg(ordered_snapshot(denial(), 2, false, false));
    assert!(app.tabs[i].feed.access_denied().is_none());
    assert!(app.all_subs.groups[0].error.is_none());
    assert_eq!(app.group_posts(&app.all_subs.groups[0]).count(), 1);
}

#[test]
fn request_refusals_and_private_denials_should_preserve_their_cross_view_scope() {
    for access in [AccessDenied::PrivateCommunity, AccessDenied::Forbidden] {
        for from_group in [true, false] {
            let mut app = app_with(&["vim"]);
            let i = tab_index(&app, "vim");
            let group_generation = app.all_subs.groups[0].generation;
            app.on_msg(ordered_snapshot(
                Msg::Group {
                    subreddit: "vim".into(),
                    generation: group_generation,
                    result: Ok(listing("vim", &["cached"], None)),
                },
                1,
                false,
                false,
            ));
            let feed_message = |app: &App, result| Msg::Feed {
                tab: app.tabs[i].id,
                generation: app.tabs[i].feed.generation,
                append: false,
                result,
            };
            let denial = if from_group {
                Msg::Group {
                    subreddit: "vim".into(),
                    generation: group_generation,
                    result: Err(FeedError::Access(access)),
                }
            } else {
                feed_message(&app, Err(FeedError::Access(access)))
            };
            app.on_msg(ordered_snapshot(denial, 2, false, false));
            if access == AccessDenied::Forbidden {
                assert!(app.tabs[i].feed.access_denied().is_none());
                assert_eq!(app.group_posts(&app.all_subs.groups[0]).count(), 1);
                continue;
            }
            app.switch_tab(ALL_SUBS_TAB);
            assert_eq!(app.group_posts(&app.all_subs.groups[0]).count(), 0);
            app.open_current_community();
            assert_eq!(app.active_tab().subreddit(), Some("vim"));
            assert_eq!(app.active_tab().feed.access_denied(), Some(access));
            assert!(app.current_post().is_none());

            let success = if from_group {
                feed_message(&app, Ok(listing("vim", &["accessible"], None)))
            } else {
                Msg::Group {
                    subreddit: "vim".into(),
                    generation: group_generation,
                    result: Ok(listing("vim", &["accessible"], None)),
                }
            };
            app.on_msg(ordered_snapshot(success, 3, false, false));
            assert!(app.active_tab().feed.access_denied().is_none());
            assert!(app.all_subs.groups[0].error.is_none());
            assert_eq!(app.group_posts(&app.all_subs.groups[0]).count(), 1);
            assert_eq!(app.current_post().unwrap().id, "accessible");
        }
    }
}

#[test]
fn private_community_navigation_should_not_move_hidden_posts_or_prefetch() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let access = AccessDenied::PrivateCommunity;
    for code in [
        KeyCode::Char('j'),
        KeyCode::Char('k'),
        KeyCode::Char('G'),
        KeyCode::Char('g'),
        KeyCode::PageDown,
        KeyCode::PageUp,
        KeyCode::Null,
    ] {
        let mut app = app_with(&["vim"]);
        let i = tab_index(&app, "vim");
        app.switch_tab(i);
        app.load_feed(i, false);
        let tab = app.tabs[i].id;
        let generation = app.tabs[i].feed.generation;
        app.on_msg(ordered_snapshot(
            Msg::Feed {
                tab,
                generation,
                append: false,
                result: Ok(listing("vim", &["a", "b", "c", "d", "e"], Some("next"))),
            },
            1,
            false,
            false,
        ));
        app.tabs[i].feed.selected = 2;
        app.on_msg(ordered_snapshot(
            Msg::Feed {
                tab,
                generation,
                append: false,
                result: Err(FeedError::Access(access)),
            },
            2,
            false,
            false,
        ));

        if code == KeyCode::Null {
            app.prefetch_list(usize::MAX);
        } else {
            crate::keys::handle(&mut app, KeyEvent::new(code, KeyModifiers::NONE));
        }

        assert_eq!(app.tabs[i].feed.selected, 2, "{access:?} {code:?}");
        assert_eq!(
            app.tabs[i].feed.generation, generation,
            "{access:?} {code:?}"
        );
        assert!(!app.tabs[i].feed.loading(), "{access:?} {code:?}");
        assert!(app.current_post().is_none());
        app.reload();
        assert!(app.tabs[i].feed.loading(), "explicit recheck: {code:?}");
        assert_eq!(app.tabs[i].feed.access_denied(), Some(access));
    }
}

#[test]
fn private_community_recreated_views_should_keep_the_known_restriction() {
    let mut app = app_with(&[]);
    app.open_subreddit("vim");
    app.on_msg(ordered_snapshot(
        Msg::Feed {
            tab: app.active_tab().id,
            generation: app.active_tab().feed.generation,
            append: false,
            result: Err(FeedError::Access(AccessDenied::PrivateCommunity)),
        },
        2,
        false,
        false,
    ));
    app.sync_islands(vec!["VIM".into()]);
    assert_eq!(
        app.all_subs.groups[0]
            .error
            .as_ref()
            .and_then(FeedError::access),
        Some(AccessDenied::PrivateCommunity)
    );
    app.close_active_tab();
    app.open_subreddit("VIM");
    assert_eq!(
        app.active_tab().feed.access_denied(),
        Some(AccessDenied::PrivateCommunity)
    );
}
