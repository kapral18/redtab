use super::*;

#[test]
fn search_key_audit_posts_guidance_should_follow_editor_sidebar_and_auth_ownership() {
    for cached in [false, true] {
        let mut app = app(&[]);
        app.open_search(Some(crate::reddit::experience::SearchKind::Posts));
        crate::keys::paste(&mut app, "query");
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        press(&mut app, '/');
        if cached {
            app.tabs[crate::app::SEARCH_TAB].feed.posts = vec![post_with("cached", Media::None)];
        }
        app.on_msg(Msg::Feed {
            tab: app.active_tab().id,
            generation: app.active_tab().feed.generation,
            append: false,
            result: Err("offline".into()),
        });
        let failed = screen(&mut app, 200, 24);
        assert!(
            failed.contains("Esc then r"),
            "retry must leave the editor first: {failed}"
        );
        let generation = app.active_tab().feed.generation;
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        press(&mut app, 'r');
        assert_ne!(app.active_tab().feed.generation, generation);
        assert_eq!(app.search.query.value, "query");
        if !cached {
            app.on_msg(Msg::Feed {
                tab: app.active_tab().id,
                generation: app.active_tab().feed.generation,
                append: false,
                result: Ok(crate::model::Listing {
                    posts: vec![],
                    after: None,
                }),
            });
            app.focus_sidebar();
            let empty = screen(&mut app, 200, 24);
            assert!(empty.contains("No results"), "{empty}");
            assert!(
                !empty.contains("Tab for another kind"),
                "Sidebar Tab switches buffers: {empty}"
            );
            let kind = app.search.kind;
            key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
            assert_ne!(app.active, crate::app::SEARCH_TAB);
            assert_eq!(app.search.kind, kind);
        }
    }
    for view in [ListView::Card, ListView::Compact] {
        let mut app = app(&[]);
        app.open_search(Some(crate::reddit::experience::SearchKind::Posts));
        crate::keys::paste(&mut app, "query");
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        press(&mut app, '/');
        if app.state.list_view() != view {
            app.state.toggle_list_view();
        }
        app.on_msg(Msg::Feed {
            tab: app.active_tab().id,
            generation: app.active_tab().feed.generation,
            append: false,
            result: Ok(crate::model::Listing {
                posts: vec![post_with("existing", Media::None)],
                after: Some("next".into()),
            }),
        });
        app.load_feed(crate::app::SEARCH_TAB, true);
        app.on_msg(Msg::Feed {
            tab: app.active_tab().id,
            generation: app.active_tab().feed.generation,
            append: true,
            result: Err("offline".into()),
        });
        let failed = screen(&mut app, 200, 30);
        assert!(
            failed.contains("Next page failed · Esc then r retries"),
            "the continuation boundary must agree with the query editor: {failed}"
        );
        let generation = app.active_tab().feed.generation;
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        press(&mut app, 'r');
        assert_ne!(app.active_tab().feed.generation, generation);
        assert_eq!(app.search.query.value, "query");
    }
    for preview in [false, true] {
        let mut app = app(&[]);
        app.open_search(Some(crate::reddit::experience::SearchKind::Posts));
        let old = post_with("existing", Media::None);
        let incoming = post_with("incoming", Media::None);
        if preview {
            app.search.discovered.loading = false;
            app.search.preview.target = Some(app.search.selected_row().unwrap().target);
            app.search.preview.posts = vec![old.clone()];
            app.search.preview.update = Some(vec![incoming, old]);
            app.search.preview.loading = false;
        } else {
            crate::keys::paste(&mut app, "query");
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            press(&mut app, '/');
            app.on_msg(Msg::Feed {
                tab: app.active_tab().id,
                generation: app.active_tab().feed.generation,
                append: false,
                result: Ok(crate::model::Listing {
                    posts: vec![old.clone()],
                    after: None,
                }),
            });
            screen(&mut app, 200, 30);
            app.load_feed(crate::app::SEARCH_TAB, false);
            app.on_msg(Msg::Feed {
                tab: app.active_tab().id,
                generation: app.active_tab().feed.generation,
                append: false,
                result: Ok(crate::model::Listing {
                    posts: vec![incoming, old],
                    after: None,
                }),
            });
        }
        let pending = screen(&mut app, 200, 30);
        assert!(
            pending.contains("New posts · Esc then r refresh"),
            "{pending}"
        );
        let query = app.search.query.value.clone();
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        press(&mut app, 'r');
        let refreshed = screen(&mut app, 200, 30);
        assert!(refreshed.contains("incoming"), "{refreshed}");
        assert!(!refreshed.contains("New posts"), "{refreshed}");
        assert_eq!(app.search.query.value, query);
    }
    let mut app = app(&[]);
    app.reddit = Reddit::disconnected(false).unwrap();
    app.health = Health::SignedOut;
    app.open_search(Some(crate::reddit::experience::SearchKind::Posts));
    crate::keys::paste(&mut app, "query");
    let connect = screen(&mut app, 200, 24);
    let brand = connect.lines().nth(2).unwrap();
    assert!(
        brand.contains("list keys") && brand.contains("F1"),
        "{connect}"
    );
    assert!(connect.contains("Press Esc then SPC p a"), "{connect}");
    assert!(
        connect.lines().last().unwrap().contains("Esc then SPC p a"),
        "{connect}"
    );
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(crate::keys::map::available(
        &app,
        crate::keys::map::Action::Accounts
    ));
}

#[test]
fn search_tab_should_say_what_it_shows_and_which_keys_act() {
    let mut app = app(&[]);
    app.open_search(None);
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    let lit = |app: &mut App| {
        let mut terminal = Terminal::new(TestBackend::new(120, 20)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..120).any(|x| {
            buffer[(x, 0)].symbol() == "P" && buffer[(x, 0)].modifier.contains(Modifier::UNDERLINED)
        })
    };

    let explore = screen(&mut app, 120, 20);
    assert!(
        !lit(&mut app) && explore.contains("Search posts (/ to type)"),
        "Explore lights no kind as the shown view: {explore}"
    );
    let footer: Vec<_> = crate::keys::footer_hints(&app)
        .into_iter()
        .map(|(key, label)| format!("{key} {label}"))
        .collect();
    assert_eq!(footer[..2], ["/ type", "Tab kind"]);
    let narrow = screen(&mut app, 70, 30);
    let title = narrow.lines().find(|l| l.contains("Explore")).unwrap();
    assert!(
        !title.contains('…'),
        "the title drops, not cuts, its subtitle: {narrow}"
    );

    press(&mut app, 'h');
    let hints = crate::keys::footer_hints(&app);
    assert_eq!(hints[0], ("j/k".into(), "tabs".into()));
    let kind = app.search.kind;
    key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    assert_ne!(app.active, crate::app::SEARCH_TAB);
    assert_eq!(app.search.kind, kind);

    app.open_search(None);
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    app.resize(200);
    press_keys(&mut app, "\u{17}h");
    assert_eq!(app.focus, Focus::Sidebar);
    assert_eq!(
        crate::keys::footer_hints(&app)[0],
        ("j/k".into(), "tabs".into())
    );
    assert!(
        !screen(&mut app, 200, 24).contains("Enter opens"),
        "the sidebar's Enter only returns focus to the list"
    );
    press(&mut app, 'h');
    assert_eq!(app.focus, Focus::Sidebar);
    press(&mut app, 'l');
    assert!(screen(&mut app, 200, 24).contains("Enter opens"));
    key(&mut app, KeyCode::F(2), KeyModifiers::NONE);
    assert!(matches!(app.overlay, Some(Overlay::Settings)));
    let hints = crate::keys::footer_hints(&app);
    assert_eq!(hints[0], ("j/k".into(), "move".into()));
    assert!(
        !hints.iter().any(|(_, label)| matches!(
            label.as_str(),
            "type" | "kind" | "open feed" | "rows" | "next row"
        )),
        "Settings owns the keys: {hints:?}"
    );
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);

    press(&mut app, '/');
    assert_eq!(
        crate::keys::footer_hints(&app)[0],
        ("↑↓".into(), "choose".into())
    );
    crate::keys::paste(&mut app, "rust");
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(lit(&mut app), "a searched kind is lit");
}
