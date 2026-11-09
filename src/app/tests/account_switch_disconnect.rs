use super::*;

#[test]
fn when_the_active_saved_login_is_unnamed_selection_should_reach_the_state_save_guard() {
    use crate::reddit::tests::{Fixture, Reply};
    let fixture =
        Fixture::new(|_| Reply::json(serde_json::json!({"data":{"id":"reader","name":"reader"}})));
    for transient_identity in [false, true] {
        let (mut app, _messages) = app_at(&fixture.base, &[]);
        if transient_identity {
            app.reddit.verify_account().unwrap();
        }
        let directory = tempfile::tempdir().unwrap();
        app.state_path = directory.path().join("blocked.json");
        std::fs::create_dir(&app.state_path).unwrap();
        let id = app.reddit.saved_id().unwrap();
        app.auth.phase = crate::auth::Phase::Accounts;
        app.auth.active_id = Some(id.clone());
        app.auth.accounts = vec![crate::reddit::SavedAccount {
            id,
            name: "Existing connection".into(),
            expired: false,
            source: None,
        }];
        app.overlay = Some(Overlay::Auth);

        crate::keys::handle(
            &mut app,
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Enter,
                crossterm::event::KeyModifiers::NONE,
            ),
        );

        assert!(matches!(app.overlay, Some(Overlay::Auth)));
        assert!(
            app.auth
                .message
                .as_deref()
                .unwrap()
                .contains("connection unchanged")
        );
        assert!(!app.auth.busy());
        assert!(app.take_replacement_messages().is_none());
    }
}

#[test]
fn help_should_survive_account_replacement_and_consume_its_dismissal_key() {
    let mut app = app_with(&[]);
    let directory = tempfile::tempdir().unwrap();
    app.state_path = directory.path().join("state.json");
    app.overlay = Some(Overlay::Auth);
    app.auth.phase = crate::auth::Phase::Disconnecting;
    crate::keys::handle(
        &mut app,
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::F(1),
            crossterm::event::KeyModifiers::NONE,
        ),
    );

    app.install_connection(Reddit::disconnected(false).unwrap());

    assert_eq!(app.help_scroll, Some(0));
    crate::keys::handle(
        &mut app,
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('q'),
            crossterm::event::KeyModifiers::NONE,
        ),
    );
    assert!(app.help_scroll.is_none());
    assert!(!app.quit);
    assert_eq!(app.health, Health::SignedOut);
}

#[test]
fn when_disconnecting_it_should_clear_account_payloads_and_preserve_saved_history() {
    use crate::reddit::tests::{Fixture, Reply};
    let fixture = Fixture::new(|_| {
        Reply::json(serde_json::json!({"data":{"id":"readerid", "name":"reader"}}))
    });
    let (mut app, _old_messages) = app_at(&fixture.base, &[]);
    let directory = tempfile::tempdir().unwrap();
    let account_path = directory.path().join("accounts/readerid/state.json");
    std::fs::create_dir_all(account_path.parent().unwrap()).unwrap();
    app.state_path = account_path.clone();
    app.reddit.verify_account().unwrap();
    let retired = app.reddit.clone();
    app.tabs[0].feed.posts = vec![post("private", "rust")];
    app.state.toggle_bookmark(&post("saved", "rust"), 1000);
    app.pending_media.insert("/private".into());
    app.images
        .insert("private-image".into(), ImageSlot::Loading);
    app.save_state();

    app.install_connection(Reddit::disconnected(false).unwrap());

    assert_eq!(app.health, Health::SignedOut);
    assert!(app.tabs.iter().all(|tab| tab.feed.posts.is_empty()));
    assert!(app.all_subs.groups.is_empty());
    assert!(app.state.bookmarks().is_empty());
    assert!(app.images.is_empty());
    assert!(app.pending_media.is_empty());
    assert!(app.video().is_none());
    assert!(app.post_view.is_none());
    assert!(!app.auth.busy());
    assert!(app.overlay.is_none());
    assert_eq!(
        app.state_path,
        directory.path().join("signed-out/state.json")
    );
    assert!(app.take_replacement_messages().is_some());
    assert!(
        retired
            .verify_account()
            .unwrap_err()
            .to_string()
            .contains("replaced")
    );
    assert!(State::load(&account_path).0.is_bookmarked("saved"));
}

#[test]
fn when_reading_state_cannot_be_saved_disconnect_should_not_start() {
    let mut app = app_with(&[]);
    let directory = tempfile::tempdir().unwrap();
    app.state_path = directory.path().join("blocked.json");
    std::fs::create_dir(&app.state_path).unwrap();
    app.auth.active_id = app.reddit.saved_id();
    app.auth.request_disconnect();

    app.confirm_auth();

    assert_eq!(app.auth.phase, crate::auth::Phase::DisconnectConfirm);
    assert!(!app.auth.busy());
    assert!(
        app.auth
            .message
            .as_deref()
            .unwrap()
            .contains("connection unchanged")
    );
    assert!(!app.reddit.needs_sign_in());
    assert!(app.take_replacement_messages().is_none());
}

#[test]
fn when_reading_state_cannot_be_saved_a_saved_account_switch_should_not_start() {
    let mut app = app_with(&[]);
    let directory = tempfile::tempdir().unwrap();
    app.state_path = directory.path().join("blocked.json");
    std::fs::create_dir(&app.state_path).unwrap();
    app.auth.phase = crate::auth::Phase::Accounts;
    app.auth.accounts = vec![crate::reddit::SavedAccount {
        id: "second".into(),
        name: "u/second".into(),
        expired: false,
        source: None,
    }];

    app.select_auth();

    assert_eq!(app.auth.phase, crate::auth::Phase::Accounts);
    assert!(!app.auth.busy());
    assert!(
        app.auth
            .message
            .as_deref()
            .unwrap()
            .contains("connection unchanged")
    );
    assert!(!app.reddit.needs_sign_in());
    assert!(app.take_replacement_messages().is_none());
}

#[test]
fn when_selecting_the_current_healthy_account_it_should_close_without_loading_or_saving() {
    let mut app = app_with(&[]);
    app.reddit = Reddit::from_saved(
        &crate::reddit::tests::saved_export("reader", "reader"),
        false,
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    app.state_path = directory.path().join("state.json");
    let id = app.reddit.saved_id().unwrap();
    app.auth.phase = crate::auth::Phase::Accounts;
    app.auth.active_id = Some(id.clone());
    app.auth.accounts = vec![crate::reddit::SavedAccount {
        id,
        name: "u/reader".into(),
        expired: false,
        source: None,
    }];
    app.overlay = Some(Overlay::Auth);

    crate::keys::handle(
        &mut app,
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ),
    );

    assert!(app.overlay.is_none());
    assert!(!app.auth.busy());
    assert!(app.take_replacement_messages().is_none());
    assert!(!app.state_path.exists());
}

#[test]
fn a_failed_account_switch_persists_individual_reads_without_clearing_the_feed() {
    let (mut app, i) = unread_badge_app();
    let directory = tempfile::tempdir().unwrap();
    app.state_path = directory.path().join("state.json");
    app.state.mark_read(&post("new1", "s"), now());
    app.auth.phase = crate::auth::Phase::Accounts;
    app.auth.accounts = vec![crate::reddit::SavedAccount {
        id: "second".into(),
        name: "u/second".into(),
        expired: true,
        source: None,
    }];

    app.select_auth();

    let saved = State::load(&app.state_path).0;
    assert!(saved.is_read("new1"));
    assert!(!saved.is_read("new2"));
    assert_eq!(saved.unread("s", &app.config), Some(1));
    assert_eq!(app.badge(&app.tabs[i]), UnreadBadge::Count(1));
    assert!(app.auth.message.as_deref().unwrap().contains("expired"));
    assert!(!app.auth.busy());
}

#[test]
fn when_reconnecting_the_same_verified_account_it_should_keep_the_selected_post_and_reading_state()
{
    use crate::reddit::tests::{Fixture, Reply};
    let fixture = Fixture::new(|_| {
        Reply::json(serde_json::json!({"data":{"id":"sameaccount", "name":"reader"}}))
    });
    let (mut app, _messages) = app_at(&fixture.base, &[]);
    let directory = tempfile::tempdir().unwrap();
    app.state_path = directory.path().join("state.json");
    app.reddit.verify_account().unwrap();
    app.active = 0;
    app.tabs[0].feed.posts = vec![post("one", "rust"), post("two", "rust")];
    app.tabs[0].feed.selected = 1;
    app.tabs[0].feed.offset = 1;
    app.tabs[0].feed.loaded = true;
    app.state.toggle_bookmark(&post("saved", "rust"), 1000);
    let connection = fixture.reddit();
    connection.verify_account().unwrap();
    app.save_state();
    app.install_connection(connection);
    assert_eq!(app.tabs[0].feed.selected, 1);
    assert_eq!(app.tabs[0].feed.offset, 1);
    assert_eq!(app.tabs[0].feed.posts[1].id, "two");
    assert!(app.state.is_bookmarked("saved"));
    assert_eq!(
        app.state_path,
        directory.path().join("accounts/sameaccount/state.json")
    );
    assert!(app.take_replacement_messages().is_some());
}

#[test]
fn when_membership_fails_or_is_ambiguous_it_should_follow_the_fresh_account_list() {
    for (confirmed, read_fails) in [(false, false), (true, false), (false, true)] {
        let mut app = app_with(&["rust"]);
        app.open_subreddit("armenia");
        app.set_membership(
            crate::model::MembershipTarget::Community("armenia".into()),
            true,
        );
        let generation = app.membership.as_ref().unwrap().generation;
        app.on_msg(Msg::Membership {
            generation,
            result: Err("write unavailable".into()),
        });
        let result = if read_fails {
            Err("read unavailable".into())
        } else if confirmed {
            Ok(vec![
                Subscription::Community {
                    name: "armenia".into(),
                    icon: crate::model::SubscriptionIcon::Unknown,
                },
                Subscription::Community {
                    name: "rust".into(),
                    icon: crate::model::SubscriptionIcon::Unknown,
                },
            ])
        } else {
            Ok(vec![Subscription::Community {
                name: "rust".into(),
                icon: crate::model::SubscriptionIcon::Unknown,
            }])
        };
        app.on_msg(Msg::Subscriptions {
            generation: app.all_subs.generation,
            result,
        });
        assert!(app.membership.is_none());
        assert_eq!(
            app.joined("armenia"),
            if read_fails { None } else { Some(confirmed) }
        );
        assert_eq!(
            app.joined("rust"),
            if read_fails { None } else { Some(true) }
        );
        assert!(
            matches!(app.active_tab().kind, TabKind::Subreddit { pinned, .. } if pinned == confirmed)
        );
    }
}
