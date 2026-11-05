use super::*;

#[test]
fn intent_order_should_put_unread_notifications_first_in_the_reddit_menu() {
    let (mut app, _dir, _fixture) = app();
    app.reddit_menu();
    let Some(Screen::Menu { choices, .. }) = app.experience.screens.last() else {
        panic!("menu missing")
    };
    assert!(matches!(
        choices[0].command,
        Command::Read(Query::Inbox(Inbox::Unread))
    ));
    sequence(&mut app, "sc");
    assert!(app.overlay.is_none());
    assert_eq!(app.active, crate::app::SEARCH_TAB);
    assert_eq!(app.search.kind, SearchKind::Communities);
}

#[test]
fn intent_order_should_put_reply_first_and_destructive_delete_last() {
    let (mut app, _dir, _fixture) = app();
    app.details_menu(Details {
        target: Target {
            id: "t1_comment".into(),
            author: "reader".into(),
            subreddit: "rust".into(),
            permalink: "/comments/post/_/comment".into(),
        },
        raw_text: "body".into(),
        saved: false,
        hidden: false,
        vote: 0,
        editable: true,
        crosspost: None,
    });
    let Some(Screen::Menu { choices, .. }) = app.experience.screens.last() else {
        panic!("menu missing")
    };
    assert!(matches!(
        &choices[0].command,
        Command::Input(Form {
            kind: FormKind::Reply(_),
            ..
        })
    ));
    assert!(matches!(
        &choices.last().unwrap().command,
        Command::Confirm(_, Action::Delete { .. })
    ));
    sequence(&mut app, "c");
    assert!(
        matches!(app.experience.screens.last(), Some(Screen::Input(Form { kind: FormKind::Reply(target), .. })) if target.id == "t1_comment")
    );
    key(&mut app, KeyCode::Esc);
    sequence(&mut app, "D");
    assert!(
        matches!(app.experience.screens.last(), Some(Screen::Confirm { action: Action::Delete { target }, .. }) if target.id == "t1_comment")
    );
    key(&mut app, KeyCode::Char('l'));
    key(&mut app, KeyCode::Right);
    crate::keys::handle(
        &mut app,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL),
    );
    assert!(app.experience.pending.is_none());
    assert!(matches!(
        app.experience.screens.last(),
        Some(Screen::Confirm { .. })
    ));
}

#[test]
fn deleted_authors_should_not_offer_profile_or_chat_actions() {
    let (mut app, _dir, _fixture) = app();
    app.details_menu(Details {
        target: Target {
            id: "t1_comment".into(),
            author: "[deleted]".into(),
            subreddit: "rust".into(),
            permalink: "/comments/post/_/comment".into(),
        },
        raw_text: "body".into(),
        saved: false,
        hidden: false,
        vote: 0,
        editable: true,
        crosspost: None,
    });
    let Some(Screen::Menu { choices, .. }) = app.experience.screens.last() else {
        panic!("menu missing")
    };
    assert!(
        !choices
            .iter()
            .any(|choice| matches!(&choice.command, Command::Read(Query::Profile(_))))
    );
    assert!(!choices.iter().any(
        |choice| matches!(&choice.command,Command::Native(path) if path.contains("[deleted]"))
    ));
}

#[test]
fn fresh_profile_post_inspection_should_keep_profile_actions_and_native_identity() {
    for (subreddit, subtype, permalink, profile) in [
        (
            "u_blackvault",
            "user",
            "/r/u_blackvault/comments/post/title/",
            true,
        ),
        (
            "u_ordinary",
            "public",
            "/r/u_ordinary/comments/post/title/",
            false,
        ),
    ] {
        let fixture = Fixture::new(move |_| {
            Reply::json(json!([
                {"kind":"Listing","data":{"children":[{"kind":"t3","data":{
                    "id":"post", "name":"t3_post", "title":"Submission", "author":"reader",
                    "subreddit":subreddit, "subreddit_type":subtype, "permalink":permalink,
                    "selftext":"Markdown source", "saved":true, "likes":true
                }}]}},
                {"kind":"Listing","data":{"children":[]}}
            ]))
        });
        let canonical = if profile {
            "/user/blackvault/comments/post/title/"
        } else {
            permalink
        };
        let Response::Details(details) = fixture
            .reddit()
            .experience(&Request::Read {
                query: Query::Inspect(Target {
                    id: "t3_post".into(),
                    author: "reader".into(),
                    subreddit: subreddit.into(),
                    permalink: canonical.into(),
                }),
                after: None,
            })
            .unwrap()
        else {
            panic!("details missing")
        };
        assert_eq!(details.target.permalink, canonical);
        assert!(details.saved);
        assert_eq!(details.vote, 1);
        assert_eq!(details.raw_text, "Markdown source");
        let (mut app, _dir, _fixture) = app();
        app.details_menu(details);
        let Some(Screen::Menu {
            choices, native, ..
        }) = app.experience.screens.last()
        else {
            panic!("menu missing")
        };
        assert_eq!(native, canonical);
        assert_eq!(
            choices
                .iter()
                .any(|choice| choice.label == "Profile information"),
            profile
        );
        assert_eq!(
            choices
                .iter()
                .any(|choice| choice.label == "Community information"),
            !profile
        );
        assert_eq!(
            choices
                .iter()
                .any(|choice| choice.label.starts_with("Profile controls ·")),
            profile
        );
    }
}
