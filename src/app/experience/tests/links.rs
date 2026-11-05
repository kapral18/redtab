use super::*;

#[test]
fn when_a_reddit_url_is_pasted_it_should_open_its_thread_or_feed_and_retain_invalid_input() {
    for (link, expected) in [
        (
            "https://old.reddit.com/r/rust/comments/abc/title/def/?context=3",
            "thread",
        ),
        ("https://redd.it/abc", "thread"),
        ("https://reddit.com/r/rust/?share_id=ignored", "rust"),
        ("https://www.reddit.com/r/popular/", "popular"),
        ("https://www.reddit.com/user/spez/", "user"),
        ("https://example.com/r/rust/", "error"),
    ] {
        let (mut app, _dir, _fixture) = app();
        sequence(&mut app, " ou");
        crate::keys::paste(&mut app, link);
        key(&mut app, KeyCode::Enter);

        match expected {
            "thread" => assert!(matches!(&app.experience.pending,
                    Some((_, Request::Read { query: Query::Thread(path), .. }))
                    if path == "/r/rust/comments/abc/title/def/" || path == "/comments/abc/")),
            "rust" | "popular" => assert!(
                matches!(&app.active_tab().kind, TabKind::Subreddit { name, .. } if name == expected)
            ),
            "user" => assert!(
                matches!(&app.active_tab().kind, TabKind::User { name, .. } if name == "spez")
            ),
            _ => {
                assert!(app.experience.error.is_some());
                assert_eq!(app.experience.input().unwrap().fields[0].value, link);
                assert!(app.experience.pending.is_none());
            }
        }
    }
}

#[test]
fn when_a_pasted_thread_loads_navigation_should_follow_its_source_and_keep_comment_focus() {
    for profile in [false, true] {
        let (mut app, _dir, _fixture) = app();
        let mut page = post_page();
        let existing = if profile {
            page.post.permalink = "/user/writer/comments/1x4v4sn/title/".into();
            page.post.subreddit = "u_writer".into();
            None
        } else {
            app.open_subreddit("Neovim");
            Some(app.active_tab().id)
        };
        app.switch_tab(0);
        if profile {
            app.show_sidebar = false;
            app.focus_sidebar();
        }
        sequence(&mut app, " ou");
        crate::keys::paste(
            &mut app,
            "https://reddit.com/r/wrong/comments/1x4v4sn/title/c1/",
        );
        key(&mut app, KeyCode::Enter);
        let generation = app.experience.pending.as_ref().unwrap().0;
        assert!(matches!(app.active_tab().kind, TabKind::Home));

        app.on_experience(
            generation,
            Err("Unavailable".into()),
            crate::cache::Freshness {
                fetched_at: crate::app::now(),
                source: Default::default(),
            },
        );
        assert!(matches!(app.active_tab().kind, TabKind::Home));
        key(&mut app, KeyCode::Char('r'));
        let generation = app.experience.pending.as_ref().unwrap().0;
        app.on_experience(
            generation,
            Ok(Response::Thread(Box::new(page))),
            crate::cache::Freshness {
                fetched_at: crate::app::now(),
                source: Default::default(),
            },
        );

        if profile {
            assert!(
                matches!(&app.active_tab().kind, TabKind::User { name, .. } if name == "writer")
            );
            assert!(!app.sidebar_peek);
            assert!(!app.show_sidebar);
        } else {
            assert_eq!(app.active_tab().id, existing.unwrap());
            assert_eq!(app.active_tab().subreddit(), Some("Neovim"));
        }
        assert_eq!(app.tabs.len(), 5);
        assert_eq!(app.focus, Focus::Post);
        assert!(app.overlay.is_none());
        assert_eq!(app.post_view.as_ref().unwrap().shown_post().id, "1x4v4sn");
        assert_eq!(app.comment_target().unwrap().id, "t1_c1");
    }
}

#[test]
fn when_reading_links_it_should_follow_comment_selection_and_keep_spoilers_hidden() {
    let (mut app, _dir, _fixture) = app();
    let mut source: serde_json::Value =
        serde_json::from_str(include_str!("../../../../tests/fixtures/post.json")).unwrap();
    source[0]["data"]["children"][0]["data"]["selftext_html"] =
        json!("<p><a href='https://example.org/post?x=1&amp;y=2'>post guide</a></p>");
    source[1]["data"]["children"][0]["data"]["body_html"] = json!(
        "<p><a href='/r/rust/wiki/index'>comment guide</a> <span class='md-spoiler-text'><a href='https://example.org/secret'>secret</a></span></p>"
    );
    let page = crate::reddit::parse::parse_post_page(&source.to_string()).unwrap();
    app.overlay = Some(Overlay::Experience);
    app.experience.screens.push(Screen::Browse {
        pagination: Default::default(),
        query: Query::Inbox(Inbox::All),
        page: Page {
            title: "Saved post".into(),
            text: String::new(),
            entries: vec![Entry {
                icon: Default::default(),
                title: page.post.title.clone(),
                body: String::new(),
                target: Some(Target::from_post(&page.post)),
                query: None,
                post: Some(page.post.clone()),
            }],
            after: None,
            url: "/saved".into(),
        },
        selected: 0,
        scroll: 0,
    });
    key(&mut app, KeyCode::Enter);
    assert!(app.overlay.is_none());
    let generation = app.post_view.as_ref().unwrap().generation;
    app.on_msg(Msg::Post {
        generation,
        result: Ok(Box::new(page)),
    });
    app.focus = Focus::Post;
    app.post_view.as_mut().unwrap().free_scroll = true;

    sequence(&mut app, "gx");
    let Some(Screen::Menu { choices, .. }) = app.experience.screens.last() else {
        panic!("missing post links")
    };
    assert_eq!(choices.len(), 1);
    assert_eq!(choices[0].label, "post guide");
    key(&mut app, KeyCode::Enter);
    let Some(Screen::Menu { choices, .. }) = app.experience.screens.last() else {
        panic!("missing link actions")
    };
    assert!(
        matches!(&choices[0].command, Command::OpenUrl(url) if url == "https://example.org/post?x=1&y=2")
    );
    assert!(
        matches!(&choices[1].command, Command::CopyUrl(url) if url == "https://example.org/post?x=1&y=2")
    );
    app.config.commands.browser = vec!["/redtab-test-missing-browser".into()];
    key(&mut app, KeyCode::Enter);
    assert!(
        app.status
            .as_ref()
            .is_some_and(|status| status.text.contains("Could not open link"))
    );
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Esc);

    sequence(&mut app, "yl");
    key(&mut app, KeyCode::Enter);
    let Some(Screen::Menu {
        choices, selected, ..
    }) = app.experience.screens.last()
    else {
        panic!("missing copy link actions")
    };
    assert!(
        matches!(&choices[*selected].command, Command::CopyUrl(url) if url == "https://example.org/post?x=1&y=2")
    );
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Esc);

    let view = app.post_view.as_mut().unwrap();
    view.free_scroll = false;
    view.cursor = 0;
    sequence(&mut app, "gx");
    let Some(Screen::Menu { choices, .. }) = app.experience.screens.last() else {
        panic!("missing comment links")
    };
    assert_eq!(choices.len(), 1);
    assert_eq!(choices[0].label, "comment guide");
    assert!(
        matches!(&choices[0].command, Command::BodyLink(link, _) if link.url == "https://www.reddit.com/r/rust/wiki/index")
    );
    assert!(app.experience.pending.is_none());
    key(&mut app, KeyCode::Esc);
    app.post_view.as_mut().unwrap().reveal_spoilers = true;
    sequence(&mut app, "gx");
    let Some(Screen::Menu { choices, .. }) = app.experience.screens.last() else {
        panic!("missing revealed links")
    };
    assert_eq!(choices.len(), 2);
    assert_eq!(choices[1].label, "secret");
    key(&mut app, KeyCode::Esc);
    let view = app.post_view.as_mut().unwrap();
    view.reveal_spoilers = false;
    view.page.as_mut().unwrap().post.spoiler = true;
    sequence(&mut app, "gx");
    assert!(app.overlay.is_none());
}

#[test]
fn when_a_reading_dialog_has_links_it_should_use_the_visible_text_and_restore_the_dialog() {
    let (mut app, _dir, _fixture) = app();
    let mut source: serde_json::Value =
        serde_json::from_str(include_str!("../../../../tests/fixtures/post.json")).unwrap();
    source[0]["data"]["children"][0]["data"]["selftext_html"] =
        json!("<p><a href='https://example.org/about'>about guide</a></p>");
    source[1]["data"]["children"][0]["data"]["body_html"] =
        json!("<p><a href='https://example.org/comment'>comment guide</a></p>");
    let page = crate::reddit::parse::parse_post_page(&source.to_string()).unwrap();
    app.overlay = Some(Overlay::Experience);
    app.experience.screens.push(Screen::Browse {
        pagination: Default::default(),
        query: Query::Community("rust".into()),
        page: Page {
            title: "About".into(),
            text: page.body,
            entries: vec![Entry {
                icon: Default::default(),
                title: "Comment".into(),
                body: page.comments[0].replies[0].body.clone(),
                query: None,
                target: None,
                post: None,
            }],
            after: None,
            url: "/r/rust/about".into(),
        },
        selected: 0,
        scroll: 3,
    });

    sequence(&mut app, "gx");
    let Some(Screen::Menu { choices, .. }) = app.experience.screens.last() else {
        panic!("missing dialog links")
    };
    assert_eq!(choices[0].label, "about guide");
    key(&mut app, KeyCode::Esc);
    let Some(Screen::Browse { query, scroll, .. }) = app.experience.screens.last_mut() else {
        panic!("lost dialog")
    };
    assert_eq!(*scroll, 3);
    *query = Query::Thread("/comments/post".into());
    sequence(&mut app, "gx");
    let Some(Screen::Menu { choices, .. }) = app.experience.screens.last() else {
        panic!("missing selected entry links")
    };
    assert_eq!(choices[0].label, "comment guide");
    assert!(app.experience.pending.is_none());
}
