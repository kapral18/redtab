use super::*;

#[test]
fn followed_users_should_have_distinct_tabs_and_stay_out_of_community_islands() {
    let mut app = app_with(&[]);
    app.on_msg(Msg::Subscriptions {
        generation: 0,
        result: Ok(vec![
            Subscription::Community {
                name: "same".into(),
                icon: crate::model::SubscriptionIcon::Unknown,
            },
            Subscription::User {
                name: "same".into(),
                icon: None,
            },
            Subscription::User {
                name: "blackvault".into(),
                icon: None,
            },
            Subscription::Community {
                name: "u_ordinary".into(),
                icon: crate::model::SubscriptionIcon::Unknown,
            },
        ]),
    });
    assert_eq!(
        app.all_subs
            .groups
            .iter()
            .map(|group| group.subreddit.as_str())
            .collect::<Vec<_>>(),
        ["same", "u_ordinary"]
    );
    let (communities, follows, opened) = app.sidebar_tabs();
    assert_eq!(communities.len(), 2);
    assert_eq!(follows.len(), 2);
    assert!(opened.is_empty());
    app.open_user("same");
    let user_tab = app.active;
    assert!(
        matches!(app.active_tab().feed_target(), Some(FeedTarget::User(name)) if name == "same")
    );
    assert_eq!(app.active_tab().feed.sort, Sort::New);
    assert_eq!(app.badge(app.active_tab()), UnreadBadge::Unavailable);
    let entries = app.picker_entries("u/same");
    assert_eq!(
        entries,
        [PickerEntry::Tab {
            index: user_tab,
            title: "u/same".into()
        }]
    );
    app.open_subreddit("same");
    assert_ne!(app.active, user_tab);
}

#[test]
fn explicit_user_jump_should_open_a_user_instead_of_a_subreddit() {
    let mut app = app_with(&[]);
    assert_eq!(
        app.picker_entries("u/black-vault"),
        [PickerEntry::OpenUser("black-vault".into())]
    );
    app.picker_accept(&PickerState {
        input: "u/black-vault".into(),
        selected: 0,
    });
    assert_eq!(app.active_tab().title(), "u/black-vault");
    assert!(app.active_tab().subreddit().is_none());
}

#[test]
fn user_feed_pagination_and_sort_should_reuse_the_reader_without_losing_posts() {
    let mut app = app_with(&[]);
    app.open_user("blackvault");
    let i = app.active;
    let tab = app.active_tab().id;
    app.on_msg(Msg::Feed {
        tab,
        generation: app.tabs[i].feed.generation,
        append: false,
        result: Ok(listing("UFOs", &["a"], Some("next"))),
    });
    app.load_feed(i, true);
    app.on_msg(Msg::Feed {
        tab,
        generation: app.tabs[i].feed.generation,
        append: true,
        result: Ok(listing("UFOs", &["b"], None)),
    });
    assert_eq!(
        app.tabs[i]
            .feed
            .posts
            .iter()
            .map(|post| post.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert_eq!(app.current_post().unwrap().id, "a");
    app.set_sort(Sort::Top);
    assert_eq!(app.tabs[i].feed.sort, Sort::Top);
    assert_eq!(app.active_tab().title(), "u/blackvault");
    app.set_sort(Sort::Rising);
    assert_eq!(app.tabs[i].feed.sort, Sort::Top);
}
