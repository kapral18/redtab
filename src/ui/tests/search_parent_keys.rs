use super::*;

#[test]
fn search_key_audit_source_links_should_include_explore_feeds() {
    use crate::keys::map::{self, Action};
    use crate::worker::FeedTarget;
    for (target, path, membership) in [
        (
            FeedTarget::Subreddit("popular".into()),
            "/r/popular/",
            false,
        ),
        (FeedTarget::Subreddit("all".into()), "/r/all/", false),
        (FeedTarget::Subreddit("rust".into()), "/r/rust/", true),
        (FeedTarget::User("writer".into()), "/user/writer/", true),
    ] {
        let mut app = app(&[]);
        app.open_search(None);
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        app.search.discovered.entries.push(crate::app::search::Row {
            profile: None,
            comment: None,
            label: "selected source".into(),
            detail: String::new(),
            target,
        });
        app.search.selected = 2;
        assert!(
            map::available(&app, Action::CopyLink),
            "selected row has a Reddit URL"
        );
        assert!(map::available(&app, Action::CommunityFeed));
        assert!(map::available(&app, Action::SourceBrowser));
        assert_eq!(map::available(&app, Action::Membership), membership);
        assert_eq!(
            app.copy_link_target().unwrap(),
            (format!("https://www.reddit.com{path}"), false)
        );
        press(&mut app, 'y');
        assert!(map::hints(&app).iter().any(|(key, _)| key == "y"));
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("browser.sh");
        std::fs::write(&script, "printf '%s' \"$2\" > \"$1\"\n").unwrap();
        for (index, command) in ["o", " oc"].into_iter().enumerate() {
            let output = dir.path().join(format!("url{index}"));
            app.config.commands.browser = vec![
                "/bin/sh".into(),
                script.to_string_lossy().into(),
                output.to_string_lossy().into(),
            ];
            press_keys(&mut app, command);
            let expected = format!("https://www.reddit.com{path}");
            let deadline = Instant::now() + std::time::Duration::from_secs(2);
            while std::fs::read_to_string(&output).ok().as_deref() != Some(&expected) {
                assert!(
                    Instant::now() < deadline,
                    "{command:?} did not open {expected}"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    }
}

#[test]
fn search_key_audit_focus_and_back_should_follow_visible_parent_panes() {
    for width in [100, 200] {
        let mut app = app(&[]);
        app.open_search(None);
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        screen(&mut app, width, 24);
        press_keys(&mut app, "\u{17}l");
        if app.detail_fits() {
            assert!(app.search_feed_open(), "focus the visible post preview");
            press_keys(&mut app, "\u{17}h");
            assert_eq!(app.active, crate::app::SEARCH_TAB);
        } else {
            assert_eq!(app.active, crate::app::SEARCH_TAB, "there is no right pane");
        }
        press(&mut app, 'l');
        let feed = app.active;
        app.focus_sidebar();
        key(&mut app, KeyCode::Right, KeyModifiers::NONE);
        if app.detail_fits() {
            assert_eq!(
                app.active,
                crate::app::SEARCH_TAB,
                "the visible middle pane is Search"
            );
            press(&mut app, 'l');
        } else {
            assert_eq!(app.active, feed);
        }
        app.search("local filter".into());
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(
            app.active, feed,
            "clear the local filter before leaving its feed"
        );
        assert!(app.active_tab().feed.search.is_none());
        key(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(app.active, crate::app::SEARCH_TAB);
        assert_eq!(app.focus, Focus::List);
    }
}

#[test]
fn search_key_audit_parent_retry_guidance_should_not_target_the_child_feed() {
    let mut app = app(&[]);
    app.open_search(None);
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    app.search.discovered.loading = false;
    app.search.discovered.error = Some("parent failed".into());
    screen(&mut app, 200, 24);
    press(&mut app, 'l');
    let actual = screen(&mut app, 200, 24);
    assert!(actual.contains("parent failed"), "{actual}");
    assert!(
        !actual.contains("r retries"),
        "r reloads the focused child feed: {actual}"
    );
}

#[test]
fn search_key_audit_kind_hints_should_follow_list_focus_and_ancestor_navigation() {
    let mut app = app(&[]);
    app.open_search(None);
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    app.search.query.value = "query".into();
    app.tabs[crate::app::SEARCH_TAB].feed.search = Some("query".into());
    app.tabs[crate::app::SEARCH_TAB].feed.posts = vec![post_with("result", Media::None)];
    screen(&mut app, 200, 24);
    press(&mut app, 'l');
    press(&mut app, 'h');
    assert!(app.post_view.is_some());
    assert!(
        crate::keys::footer_hints(&app)
            .iter()
            .any(|(key, label)| key == "Tab" && label == "kind")
    );
    let before = app.search.kind;
    key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    assert_ne!(app.search.kind, before);

    let mut app = self::app(&[]);
    app.switch_tab(0);
    app.tabs[0].feed.posts = vec![post_with("origin", Media::None)];
    app.open_current_post();
    press_keys(&mut app, "gtgtgt");
    assert_eq!(app.active, crate::app::SEARCH_TAB);
    assert!(app.ancestor_following());
    assert!(
        !help::entries(&app)
            .iter()
            .any(|(key, description)| key.contains("Tab") && description.contains("Communities")),
        "Tab keeps navigating buffers while following post depth"
    );
    let before = app.search.kind;
    key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    assert_ne!(app.active, crate::app::SEARCH_TAB);
    assert_eq!(app.search.kind, before);
}

#[test]
fn search_key_audit_other_row_views_should_keep_motion_parity() {
    for all_subs in [false, true] {
        let mut app = app(&["rust", "zig"]);
        let mut posts = (0..20)
            .map(|index| post_with(&format!("item{index}"), Media::None))
            .collect::<Vec<_>>();
        if all_subs {
            app.switch_tab(crate::app::ALL_SUBS_TAB);
            app.config.all_subs_posts_per_group = 10;
            app.all_subs.groups[1].posts = posts.split_off(10);
            app.all_subs.groups[0].posts = posts;
        } else {
            app.switch_tab(0);
            if app.state.list_view() != ListView::Compact {
                app.state.toggle_list_view();
            }
            app.tabs[0].feed.posts = posts;
        }
        app.focus_main();
        assert_eq!(app.current_post().unwrap().id, "item0");
        key(&mut app, KeyCode::Char('e'), KeyModifiers::CONTROL);
        assert_eq!(app.current_post().unwrap().id, "item1");
        key(&mut app, KeyCode::Char('y'), KeyModifiers::CONTROL);
        assert_eq!(app.current_post().unwrap().id, "item0");
        key(&mut app, KeyCode::Char('d'), KeyModifiers::CONTROL);
        assert_eq!(app.current_post().unwrap().id, "item5");
        key(&mut app, KeyCode::Char('f'), KeyModifiers::CONTROL);
        assert_eq!(app.current_post().unwrap().id, "item15");
        key(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
        assert_eq!(app.current_post().unwrap().id, "item10");
        key(&mut app, KeyCode::Char('b'), KeyModifiers::CONTROL);
        assert_eq!(app.current_post().unwrap().id, "item0");
    }
}

#[test]
fn reported_search_reverse_navigation_should_keep_the_viewport() {
    let mut app = app(&[]);
    app.open_search(None);
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    app.search.discovered.loading = false;
    app.search.discovered.entries = (0..60)
        .map(|i| crate::app::search::Row {
            profile: None,
            comment: None,
            label: format!("r/community{i:02}"),
            detail: String::new(),
            target: crate::worker::FeedTarget::Subreddit(format!("community{i:02}")),
        })
        .collect();
    for _ in 0..35 {
        press(&mut app, 'j');
        screen(&mut app, 100, 24);
    }
    let before = screen(&mut app, 100, 24);
    let first = before
        .lines()
        .find(|line| line.contains("r/community"))
        .unwrap()
        .trim()
        .to_owned();
    press(&mut app, 'k');
    let after = screen(&mut app, 100, 24);
    assert!(
        after.lines().any(|line| line.trim() == first),
        "reverse navigation moved the viewport: {after}"
    );
}

#[test]
fn search_key_audit_rows_should_use_row_motions_after_post_cards() {
    for viewport in [0, 35] {
        let mut app = app(&[]);
        app.open_search(None);
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        app.search.discovered.entries = (0..30)
            .map(|i| crate::app::search::Row {
                profile: None,
                comment: None,
                label: format!("r/source{i}"),
                detail: String::new(),
                target: crate::worker::FeedTarget::Subreddit(format!("source{i}")),
            })
            .collect();
        app.tabs[crate::app::SEARCH_TAB].feed.cards.viewport = viewport;
        for (code, modifiers, selected) in [
            (KeyCode::Char('e'), KeyModifiers::CONTROL, 1),
            (KeyCode::Char('y'), KeyModifiers::CONTROL, 0),
            (KeyCode::Char('f'), KeyModifiers::CONTROL, 10),
            (KeyCode::Char('d'), KeyModifiers::CONTROL, 15),
            (KeyCode::Char('u'), KeyModifiers::CONTROL, 10),
            (KeyCode::Char('b'), KeyModifiers::CONTROL, 0),
            (KeyCode::PageDown, KeyModifiers::NONE, 10),
            (KeyCode::PageUp, KeyModifiers::NONE, 0),
        ] {
            key(&mut app, code, modifiers);
            assert_eq!(
                app.search.selected, selected,
                "{code:?} with card viewport{viewport}"
            );
        }
        press(&mut app, 'G');
        assert_eq!(app.search.selected, 31);
        press_keys(&mut app, "gg");
        assert_eq!(app.search.selected, 0);
    }
}

#[test]
fn search_key_audit_source_commands_should_keep_the_parent() {
    use crate::worker::FeedTarget;
    for target in [
        FeedTarget::Subreddit("rust".into()),
        FeedTarget::User("writer".into()),
    ] {
        for command in ["l", "gc", " cf"] {
            let mut app = app(&[]);
            app.open_search(None);
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            app.search.discovered.entries.push(crate::app::search::Row {
                profile: None,
                comment: None,
                label: "selected source".into(),
                detail: String::new(),
                target: target.clone(),
            });
            app.search.selected = 2;
            screen(&mut app, 200, 24);
            press_keys(&mut app, command);
            match &target {
                FeedTarget::Subreddit(name) => {
                    assert_eq!(app.active_tab().subreddit(), Some(name.as_str()))
                }
                FeedTarget::User(name) => assert!(
                    matches!(&app.active_tab().kind, TabKind::User { name: actual, .. } if actual == name)
                ),
                FeedTarget::Home => unreachable!(),
            }
            assert!(
                app.search_feed_open(),
                "{command:?} should keep the same parent as Enter"
            );
            press(&mut app, 'h');
            assert_eq!(app.active, crate::app::SEARCH_TAB);
            assert_eq!(app.search.selected, 2);
        }
    }
}

#[test]
fn reported_q_should_close_the_right_pane_beside_search_results_until_a_row_is_opened() {
    use crate::reddit::experience::{Entry, Page, Query, Response};
    let mut app = app(&[]);
    app.open_search(None);
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    let pane = |app: &mut App| screen(app, 200, 24).contains("Enter opens");
    assert!(pane(&mut app), "the preview shows beside the rows");

    press(&mut app, 'q');
    assert!(!app.quit && !pane(&mut app), "q closes the preview first");
    press(&mut app, 'j');
    assert!(!pane(&mut app), "moving keeps the pane closed");
    // A typed query, its rows and a kind switch keep it closed.
    press(&mut app, '/');
    press_keys(&mut app, "rust");
    key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    app.on_msg(Msg::Search {
        generation: app.search.results_generation(),
        result: Ok(Response::Page(Page {
            title: String::new(),
            text: String::new(),
            url: "/".into(),
            after: None,
            entries: ["rust", "zig"]
                .map(|name| Entry {
                    title: format!("r/{name}"),
                    body: String::new(),
                    icon: Default::default(),
                    query: Some(Query::Community(name.into())),
                    target: None,
                    post: None,
                })
                .to_vec(),
        })),
    });

    assert!(
        app.search.rows().len() == 2 && !pane(&mut app),
        "new rows keep it closed"
    );
    press(&mut app, 'j');
    assert!(!pane(&mut app), "moving through new rows keeps it closed");
    // Leaving Search and coming back keeps it closed.
    app.switch_tab(0);
    app.switch_tab(crate::app::SEARCH_TAB);
    assert!(!pane(&mut app), "returning to Search keeps it closed");

    for open in ["\r", "\u{17}l"] {
        press_keys(&mut app, open);
        assert!(
            app.search_feed_open(),
            "{open:?} opens the row beside the results"
        );
        press(&mut app, 'q');
        assert_eq!(app.active, crate::app::SEARCH_TAB);
        assert!(
            !pane(&mut app),
            "q closes the opened feed and leaves no preview"
        );
    }
    press(&mut app, 'q');
    assert!(app.quit, "with no right pane, q quits");
}
