use super::*;

#[test]
fn membership_progress_should_remain_visible_when_no_source_is_selected() {
    let mut app = app_with(&[]);
    app.set_membership(MembershipTarget::User("blackvault".into()), true);
    app.active = BOOKMARKS_TAB;

    app.show_membership();

    assert!(
        app.status
            .as_ref()
            .is_some_and(|status| status.text == "Following u/blackvault…")
    );
    assert!(app.membership.is_some());
}

#[test]
fn when_membership_is_pending_it_should_ignore_duplicate_writes_and_old_or_cached_lists() {
    let mut app = app_with(&["rust"]);
    let old = app.all_subs.generation;
    app.set_membership(
        crate::model::MembershipTarget::Community("armenia".into()),
        true,
    );
    let generation = app.membership.as_ref().unwrap().generation;
    app.set_membership(
        crate::model::MembershipTarget::Community("neovim".into()),
        true,
    );
    assert_eq!(app.membership.as_ref().unwrap().generation, generation);
    app.on_msg(Msg::Subscriptions {
        generation: old,
        result: Ok((vec![])
            .into_iter()
            .map(|name| crate::model::Subscription::Community {
                name,
                icon: crate::model::SubscriptionIcon::Unknown,
            })
            .collect()),
    });
    assert_eq!(app.joined("rust"), Some(true));
    app.on_msg(Msg::Membership {
        generation: generation + 1,
        result: Ok(()),
    });
    assert!(!app.membership.as_ref().unwrap().checking);
    app.on_msg(Msg::Membership {
        generation,
        result: Ok(()),
    });
    let read = app.all_subs.generation;
    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Subscriptions {
            generation: read,
            result: Ok((vec!["armenia".into()])
                .into_iter()
                .map(|name| crate::model::Subscription::Community {
                    name,
                    icon: crate::model::SubscriptionIcon::Unknown,
                })
                .collect()),
        }),
        fetched_at: now(),
        refreshing: false,
        source: Source {
            cached: true,
            ..Source::default()
        },
    });
    assert!(app.membership.is_some());
    assert_eq!(app.joined("armenia"), Some(false));
    app.on_msg(Msg::Subscriptions {
        generation: read,
        result: Ok((vec!["armenia".into(), "rust".into()])
            .into_iter()
            .map(|name| crate::model::Subscription::Community {
                name,
                icon: crate::model::SubscriptionIcon::Unknown,
            })
            .collect()),
    });
    assert!(app.membership.is_none());
    app.on_msg(Msg::Subscriptions {
        generation: old,
        result: Ok((vec![])
            .into_iter()
            .map(|name| crate::model::Subscription::Community {
                name,
                icon: crate::model::SubscriptionIcon::Unknown,
            })
            .collect()),
    });
    assert_eq!(app.joined("armenia"), Some(true));
}

#[test]
fn when_membership_controls_are_used_it_should_resolve_empty_views_and_preserve_text_input() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = app_with(&["rust"]);
    assert_eq!(
        app.community().as_deref(),
        Some("rust"),
        "empty All subs island"
    );
    sequence(&mut app, " m");
    assert!(
        matches!(&app.overlay, Some(Overlay::Membership { target: MembershipTarget::Community(subreddit), join: false }) if subreddit == "rust")
    );
    press(&mut app, KeyCode::Esc);
    assert!(app.membership.is_none());
    app.open_subreddit("armenia");
    assert_eq!(app.community().as_deref(), Some("armenia"));
    app.focus = Focus::Sidebar;
    sequence(&mut app, " m");
    assert!(
        matches!(&app.overlay, Some(Overlay::Membership { target: MembershipTarget::Community(subreddit), join: true }) if subreddit == "armenia")
    );
    press(&mut app, KeyCode::Char('o'));
    assert!(app.membership.is_none());
    assert!(app.overlay.is_none());
    crate::keys::handle(
        &mut app,
        KeyEvent::new(KeyCode::Char('S'), KeyModifiers::CONTROL),
    );
    assert!(app.overlay.is_none());
    app.overlay = Some(Overlay::Search {
        input: String::new(),
    });
    sequence(&mut app, " m");
    assert!(matches!(&app.overlay, Some(Overlay::Search { input }) if input == " m"));
}

#[test]
fn when_subscription_refresh_fails_it_should_retain_membership_and_expose_retry() {
    let mut app = app_with(&["rust"]);
    app.reload();
    let generation = app.all_subs.generation;
    app.on_msg(Msg::Subscriptions {
        generation,
        result: Err("offline".into()),
    });

    assert!(!app.all_subs.loading);
    assert_eq!(app.all_subs.error.as_deref(), Some("offline"));
    assert_eq!(app.all_subs.groups[0].subreddit, "rust");
    app.on_msg(Msg::Subscriptions {
        generation: generation - 1,
        result: Ok((vec![])
            .into_iter()
            .map(|name| crate::model::Subscription::Community {
                name,
                icon: crate::model::SubscriptionIcon::Unknown,
            })
            .collect()),
    });
    assert_eq!(app.all_subs.groups.len(), 1);
}

#[test]
fn a_failed_user_follow_should_reconcile_without_confusing_a_same_named_community() {
    let mut app = app_with(&["blackvault"]);
    app.open_user("blackvault");
    app.set_membership(MembershipTarget::User("blackvault".into()), true);
    let generation = app.membership.as_ref().unwrap().generation;
    app.set_membership(MembershipTarget::User("other".into()), true);
    assert_eq!(
        app.membership.as_ref().unwrap().target,
        MembershipTarget::User("blackvault".into())
    );
    app.on_msg(Msg::Membership {
        generation,
        result: Err("follow denied".into()),
    });
    app.on_msg(Msg::Subscriptions {
        generation: app.all_subs.generation,
        result: Ok(vec![Subscription::Community {
            name: "blackvault".into(),
            icon: crate::model::SubscriptionIcon::Unknown,
        }]),
    });
    assert!(app.membership.is_none());
    assert_eq!(
        app.subscribed(&MembershipTarget::User("blackvault".into())),
        Some(false)
    );
    assert!(
        app.status
            .as_ref()
            .is_some_and(|status| status.error && status.text == "follow denied")
    );
}

#[test]
fn a_membership_write_answered_after_sign_out_should_settle_instead_of_waiting() {
    let mut app = app_with(&[]);
    app.reddit = Reddit::disconnected(false).unwrap();
    app.membership = Some(crate::app::membership::MembershipChange {
        generation: 7,
        target: MembershipTarget::Community("rust".into()),
        join: true,
        checking: false,
        error: None,
    });
    app.on_membership(7, Ok(()));
    assert!(app.membership.is_none(), "nothing could ever confirm it");
    assert!(app.status.as_ref().is_some_and(|status| status.error));
}
