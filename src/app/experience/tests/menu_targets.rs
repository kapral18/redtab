use super::*;

#[test]
fn user_feed_tools_should_put_unfollow_first_and_capture_the_person() {
    let (mut app, _dir, _fixture) = app();
    app.on_msg(Msg::Subscriptions {
        generation: 0,
        result: Ok(vec![Subscription::User {
            name: "blackvault".into(),
            icon: None,
        }]),
    });
    app.open_user("blackvault");
    app.tabs[app.active].feed.posts.push(post_page().post);
    app.focus = Focus::List;

    app.community_menu();

    let Some(Screen::Menu { title, choices, .. }) = app.experience.screens.last() else {
        panic!("profile menu missing")
    };
    assert_eq!(title, "u/blackvault");
    assert_eq!(choices[0].label, "Unfollow");
    key(&mut app, KeyCode::Enter);
    assert!(
        matches!(&app.overlay, Some(Overlay::Membership { target: MembershipTarget::User(name), join: false }) if name == "blackvault")
    );
    assert!(
        app.membership.is_none(),
        "confirmation must not write before Enter"
    );
    key(&mut app, KeyCode::Esc);
    assert!(app.membership.is_none());
}

#[test]
fn community_menu_should_target_an_empty_selected_island() {
    let (mut app, _dir, _fixture) = app();
    app.on_msg(Msg::Subscriptions {
        generation: 0,
        result: Ok((vec!["empty".into()])
            .into_iter()
            .map(|name| crate::model::Subscription::Community {
                name,
                icon: crate::model::SubscriptionIcon::Unknown,
            })
            .collect()),
    });
    app.active = ALL_SUBS_TAB;

    app.community_menu();

    let Some(Screen::Menu { title, native, .. }) = app.experience.screens.last() else {
        panic!("menu missing")
    };
    assert_eq!(title, "r/empty");
    assert_eq!(native, "/r/empty");
}

#[test]
fn community_menu_should_target_the_selected_community_discovery_result() {
    let (mut app, _dir, _fixture) = app();
    app.open_subreddit("underneath");
    app.experience.screens.push(Screen::Browse {
        pagination: Default::default(),
        query: Query::Communities,
        page: Page {
            url: "/subreddits".into(),
            title: "Communities".into(),
            entries: vec![Entry {
                icon: Default::default(),
                title: "r/picked".into(),
                body: String::new(),
                target: None,
                query: Some(Query::Community("picked".into())),
                post: None,
            }],
            after: None,
            text: String::new(),
        },
        selected: 0,
        scroll: 0,
    });
    app.overlay = Some(Overlay::Experience);

    sequence(&mut app, " c");

    let Some(Screen::Menu { title, .. }) = app.experience.screens.last() else {
        panic!("menu missing")
    };
    assert_eq!(title, "r/picked");
}

#[test]
fn community_menu_on_a_moderator_list_should_keep_its_parent_community() {
    let (mut app, _dir, _fixture) = app();
    app.open_subreddit("underneath");
    app.experience.screens.push(Screen::Browse {
        pagination: Default::default(),
        query: Query::Moderators("moderated".into()),
        page: Page {
            title: "Moderators".into(),
            text: String::new(),
            after: None,
            url: "/r/moderated/about/moderators".into(),
            entries: vec![Entry {
                icon: Default::default(),
                title: "u/mod".into(),
                body: String::new(),
                target: None,
                query: Some(Query::Profile("mod".into())),
                post: None,
            }],
        },
        selected: 0,
        scroll: 0,
    });
    app.overlay = Some(Overlay::Experience);

    sequence(&mut app, " c");

    let Some(Screen::Menu { title, .. }) = app.experience.screens.last() else {
        panic!("menu missing")
    };
    assert_eq!(title, "r/moderated");
}

#[test]
fn community_read_menu_action_should_keep_its_captured_target_and_save_locally() {
    let (mut app, dir, _fixture) = app();
    app.open_subreddit("s");
    let mut post = post_page().post;
    post.subreddit = "s".into();
    app.state.observe(
        Some("s"),
        std::slice::from_ref(&post),
        Freshness {
            fetched_at: now(),
            source: Source::default(),
        },
        now(),
    );
    app.community_menu();
    let Some(Screen::Menu { choices, .. }) = app.experience.screens.last() else {
        panic!("menu missing")
    };
    let command = choices
        .iter()
        .find(|choice| choice.label == "Mark community read")
        .unwrap()
        .command
        .clone();
    app.active = 0;

    app.execute_experience(command);

    assert!(app.state.is_read(&post.id));
    assert_eq!(
        State::load(&dir.path().join("state.json"))
            .0
            .unread("s", &app.config),
        Some(0)
    );
    assert!(app.overlay.is_none());
    assert!(app.experience.pending.is_none());
}

#[test]
fn collection_source_tools_should_use_the_selected_post_before_sidebar_or_query_context() {
    for focus in [Focus::Sidebar, Focus::List] {
        for (subreddit, permalink, title) in [
            ("rust", "/r/rust/comments/post/title/", "r/rust"),
            (
                "u_blackvault",
                "/user/blackvault/comments/post/title/",
                "u/blackvault",
            ),
            (
                "u_ordinary",
                "/r/u_ordinary/comments/post/title/",
                "r/u_ordinary",
            ),
        ] {
            let (mut app, _dir, _fixture) = app();
            app.open_user("collection-owner");
            app.focus = focus;
            let mut post = post_page().post;
            post.subreddit = subreddit.into();
            post.permalink = permalink.into();
            app.experience.screens.push(Screen::Browse {
                pagination: Default::default(),
                query: Query::User {
                    name: "collection-owner".into(),
                    feed: UserFeed::Posts,
                },
                page: Page {
                    title: "Posts".into(),
                    url: "/user/collection-owner/submitted/".into(),
                    entries: vec![Entry {
                        icon: Default::default(),
                        title: "Selected post".into(),
                        body: String::new(),
                        target: Some(Target::from_post(&post)),
                        post: Some(post),
                        query: None,
                    }],
                    after: None,
                    text: String::new(),
                },
                selected: 0,
                scroll: 0,
            });
            app.overlay = Some(Overlay::Experience);

            sequence(&mut app, " c");

            let Some(Screen::Menu {
                title: actual,
                choices,
                ..
            }) = app.experience.screens.last()
            else {
                panic!("menu missing")
            };
            assert_eq!(actual, title);
            assert_eq!(
                choices
                    .iter()
                    .any(|choice| choice.label == "Mark community read"),
                title.starts_with("r/")
            );
        }
    }
}

#[test]
fn when_an_action_menu_targets_a_collection_item_its_browser_link_and_invalidation_should_use_that_item()
 {
    let (mut app, _dir, _fixture) = app();
    let target = Target {
        id: "t3_changed".into(),
        author: "reader".into(),
        subreddit: "rust".into(),
        permalink: "/r/rust/comments/changed/title/".into(),
    };
    app.details_menu(Details {
        target,
        raw_text: "body".into(),
        saved: false,
        hidden: false,
        vote: 0,
        editable: true,
        crosspost: None,
    });
    assert_eq!(
        app.experience_native_path(),
        "/r/rust/comments/changed/title/"
    );
    assert!(app.experience_invalidations().iter().any(|entry| matches!(entry, crate::worker::Invalidation::Post(link) if link == "/r/rust/comments/changed/title/")));
}

#[test]
fn when_a_comment_is_selected_actions_should_target_the_comment_instead_of_its_post() {
    let (mut app, _dir, _fixture) = app();
    let page = post_page();
    let post = page.post.clone();
    let link = post.permalink.clone();
    let generation = app.show_post(post, &link).unwrap();
    app.focus = Focus::Post;
    app.on_msg(Msg::Post {
        generation,
        result: Ok(Box::new(page)),
    });
    let view = app.post_view.as_mut().unwrap();
    view.cursor = 0;
    view.free_scroll = false;
    let actual = app.comment_target().unwrap();
    assert!(actual.id.starts_with("t1_"));
    assert!(actual.permalink.starts_with("/comments/1x4v4sn/_/"));
    app.show_actions();
    assert!(matches!(
        &app.experience.pending,
        Some((_, Request::Read { query: Query::Inspect(target), after: None })) if target == &actual
    ));
    app.post_view.as_mut().unwrap().free_scroll = true;
    assert!(app.comment_target().is_none());
}
