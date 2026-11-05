use super::*;

#[test]
fn closing_a_followed_user_should_explain_how_to_unfollow() {
    let mut app = app(&[]);
    app.on_msg(Msg::Subscriptions {
        generation: 0,
        result: Ok(vec![crate::model::Subscription::User {
            name: "blackvault".into(),
            icon: None,
        }]),
    });
    app.open_user("blackvault");
    let tab = app.active_tab().id;

    press_keys(&mut app, " bd");

    assert_eq!(app.active_tab().id, tab);
    assert!(
        app.status
            .as_ref()
            .is_some_and(|status| status.text.contains("SPC m to unfollow"))
    );
}

#[test]
fn followed_user_header_should_show_the_unfollow_shortcut() {
    let mut app = app(&[]);
    app.on_msg(Msg::Subscriptions {
        generation: 0,
        result: Ok(vec![crate::model::Subscription::User {
            name: "blackvault".into(),
            icon: None,
        }]),
    });
    app.open_user("blackvault");

    let shown = screen(&mut app, 120, 24);

    assert!(
        shown.contains("Following") && shown.contains("SPC m unfollow"),
        "{shown}"
    );
    assert!(
        shown.lines().last().unwrap().contains("unfollow"),
        "{shown}"
    );
}

#[cfg(unix)]
#[test]
fn user_follow_confirmation_should_name_the_profile_and_action_in_small_terminals() {
    for (join, title, control) in [
        (true, "Follow on Reddit", "follow"),
        (false, "Unfollow on Reddit", "unfollow"),
    ] {
        let mut app = app(&[]);
        app.overlay = Some(Overlay::Membership {
            target: crate::model::MembershipTarget::User("blackvault".into()),
            join,
        });

        let shown = screen(&mut app, 48, 8);

        assert!(
            shown.contains(title) && shown.contains("u/blackvault"),
            "{shown}"
        );
        assert!(
            shown.contains(control) && shown.contains("Esc cancel"),
            "{shown}"
        );
        assert!(shown.contains("o open profile"), "{shown}");
    }
}

#[test]
fn following_should_show_submitted_posts_in_card_and_compact_views() {
    for mode in [ListView::Card, ListView::Compact] {
        let mut app = app(&[]);
        app.on_msg(Msg::Subscriptions {
            generation: 0,
            result: Ok(vec![crate::model::Subscription::User {
                name: "blackvault".into(),
                icon: None,
            }]),
        });
        app.open_user("blackvault");
        if app.state.list_view() != mode {
            app.state.toggle_list_view();
        }
        let mut post = post_with("p", Media::None);
        post.subreddit = "UFOs".into();
        post.title = "A submitted post from another community".into();
        app.on_msg(Msg::Feed {
            tab: app.active_tab().id,
            generation: app.active_tab().feed.generation,
            append: false,
            result: Ok(crate::model::Listing {
                posts: vec![post],
                after: Some("next".into()),
            }),
        });

        let actual = screen(&mut app, 120, 24);

        assert!(actual.contains("FOLLOWING"), "{actual}");
        assert!(actual.contains("u/blackvault"), "{actual}");
        assert!(
            actual.contains("A submitted post from another community"),
            "{actual}"
        );
        assert!(actual.contains("r/UFOs"), "{actual}");
        assert!(
            !actual.contains("r/u_blackvault") && !actual.contains("Joined"),
            "{actual}"
        );
        assert!(app.current_post().is_some());
    }
}

#[test]
fn unknown_follow_status_should_not_claim_the_person_is_unfollowed() {
    let mut app = app(&[]);
    app.open_user("blackvault");
    app.all_subs.freshness = None;

    let shown = screen(&mut app, 120, 24);

    assert!(
        shown.contains("Following unknown") && shown.contains("SPC m check"),
        "{shown}"
    );
    assert!(!shown.contains("Not following"), "{shown}");
}

#[test]
fn followed_user_sidebar_tools_should_offer_profile_actions_without_subreddit_actions() {
    let mut app = app(&[]);
    app.open_user("blackvault");
    app.focus = Focus::Sidebar;
    press_keys(&mut app, " c");

    let actual = screen(&mut app, 120, 24);

    assert!(
        actual.contains("u/blackvault") && actual.contains("Open submitted posts"),
        "{actual}"
    );
    assert!(actual.contains("Open profile in browser"), "{actual}");
    assert!(
        !actual.contains("Mark community read")
            && !actual.contains("Wiki")
            && !actual.contains("Rules"),
        "{actual}"
    );
}
