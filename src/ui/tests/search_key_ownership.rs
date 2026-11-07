use super::*;

#[test]
fn search_key_audit_ownership_sidebar_should_keep_the_active_nested_feed() {
    let mut app = app(&["rust"]);
    app.config.show_nsfw = true;
    app.open_search(None);
    app.search.discovered.loading = false;
    app.search.discovered.entries.push(crate::app::search::Row {
        profile: None,
        comment: None,
        label: "r/rust".into(),
        detail: String::new(),
        target: crate::worker::FeedTarget::Subreddit("rust".into()),
    });
    app.search.selected = 2;
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    press(&mut app, 'l');
    app.on_msg(Msg::Feed {
        tab: app.active_tab().id,
        generation: app.active_tab().feed.generation,
        append: false,
        result: Ok(crate::model::Listing {
            posts: vec![post_with("readable", Media::None)],
            after: None,
        }),
    });
    app.load_feed(app.active, false);
    app.on_msg(Msg::Feed {
        tab: app.active_tab().id,
        generation: app.active_tab().feed.generation,
        append: false,
        result: Err("offline".into()),
    });
    screen(&mut app, 80, 30);
    key(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL);
    press(&mut app, 'h');
    assert_eq!(app.focus, Focus::Sidebar);
    let child = app.active;
    let failed = screen(&mut app, 200, 30);
    assert!(app.search_feed_open() && app.post_view.is_none());
    assert!(
        failed.contains("r retry") && !failed.contains("C-w l then r"),
        "{failed}"
    );
    let generation = app.active_tab().feed.generation;
    press(&mut app, 'r');
    assert_eq!(app.active, child);
    assert_ne!(app.active_tab().feed.generation, generation);
    app.search("empty".into());
    app.on_msg(Msg::Feed {
        tab: app.active_tab().id,
        generation: app.active_tab().feed.generation,
        append: false,
        result: Ok(crate::model::Listing {
            posts: vec![],
            after: None,
        }),
    });
    let empty = screen(&mut app, 200, 30);
    assert!(empty.contains("Esc then Esc clears it"), "{empty}");
    press(&mut app, '/');
    assert!(matches!(app.overlay, Some(Overlay::Search { .. })));
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert_eq!(app.focus, Focus::List);
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert_eq!(app.active, child);
    assert!(app.active_tab().feed.search.is_none() && app.search_feed_open());
}

#[test]
fn search_key_audit_ownership_empty_feeds_should_focus_list_for_local_controls() {
    let mut app = app(&["rust"]);
    app.config.show_nsfw = true;
    app.open_search(None);
    app.search.discovered.loading = false;
    app.search.discovered.entries.push(crate::app::search::Row {
        profile: None,
        comment: None,
        label: "r/rust".into(),
        detail: String::new(),
        target: crate::worker::FeedTarget::Subreddit("rust".into()),
    });
    app.search.selected = 2;
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    press(&mut app, 'l');
    let post = post_with("retained", Media::None);
    app.on_msg(Msg::Feed {
        tab: app.active_tab().id,
        generation: app.active_tab().feed.generation,
        append: false,
        result: Ok(crate::model::Listing {
            posts: vec![post.clone()],
            after: None,
        }),
    });
    screen(&mut app, 200, 30);
    press(&mut app, 'l');
    let detail = app.post_view.as_ref().unwrap().generation;
    app.on_msg(Msg::Post {
        generation: detail,
        result: Ok(Box::new(PostPage {
            post,
            body: "Retained detail".into(),
            comments: vec![],
        })),
    });
    key(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL);
    press(&mut app, 'h');
    app.search("empty".into());
    app.on_msg(Msg::Feed {
        tab: app.active_tab().id,
        generation: app.active_tab().feed.generation,
        append: false,
        result: Ok(crate::model::Listing {
            posts: vec![],
            after: None,
        }),
    });
    key(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL);
    press(&mut app, 'l');
    let empty = screen(&mut app, 200, 30);
    assert!(
        empty.contains("C-w h then / edits") && empty.contains("C-w h then Esc clears"),
        "{empty}"
    );
    key(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL);
    press(&mut app, 'h');
    press(&mut app, '/');
    assert!(matches!(app.overlay, Some(Overlay::Search { .. })));
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(app.active_tab().feed.search.is_none());
    assert_eq!(app.post_view.as_ref().unwrap().generation, detail);
    app.on_msg(Msg::Feed {
        tab: app.active_tab().id,
        generation: app.active_tab().feed.generation,
        append: false,
        result: Ok(crate::model::Listing {
            posts: vec![],
            after: None,
        }),
    });
    key(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL);
    press(&mut app, 'l');
    let empty = screen(&mut app, 200, 30);
    assert!(empty.contains("C-w h then SPC f s"), "{empty}");
    key(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL);
    press(&mut app, 'h');
    press_keys(&mut app, " fs");
    assert!(matches!(app.overlay, Some(Overlay::Sort { .. })));
}

#[test]
fn search_key_audit_ownership_retained_detail_should_update_its_cached_controls() {
    for kind in [
        "error",
        "spoiler",
        "more",
        "gallery",
        "media",
        "spoiler_gallery",
    ] {
        let (mut app, _) = open_long_post(|_| {});
        let mut page = app
            .post_view
            .as_ref()
            .unwrap()
            .page
            .as_ref()
            .unwrap()
            .clone();
        page.body = "Reading the retained post".into();
        page.comments[0].body = "Reading its comment".into();
        page.comments[0].more = Some(crate::model::MoreReplies {
            cursor: "/next".into(),
            count: Some(2),
        });
        page.post.spoiler = matches!(kind, "spoiler" | "spoiler_gallery");
        page.post.media = match kind {
            "gallery" | "spoiler_gallery" => Media::Gallery {
                images: vec![
                    "https://i.redd.it/one.jpg".into(),
                    "https://i.redd.it/two.jpg".into(),
                ],
                previews: vec![],
                thumb: None,
            },
            "media" => Media::Image {
                url: "https://i.redd.it/one.jpg".into(),
                preview: None,
            },
            _ => Media::None,
        };
        app.open_search(None);
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        press(&mut app, 'l');
        app.on_msg(Msg::Feed {
            tab: app.active_tab().id,
            generation: app.active_tab().feed.generation,
            append: false,
            result: Ok(crate::model::Listing {
                posts: vec![page.post.clone()],
                after: None,
            }),
        });
        screen(&mut app, 200, 40);
        press(&mut app, 'l');
        let generation = app.post_view.as_ref().unwrap().generation;
        app.on_msg(Msg::Post {
            generation,
            result: if kind == "error" {
                Err("offline".into())
            } else {
                Ok(Box::new(page))
            },
        });
        let cue = match kind {
            "error" => "press r to retry",
            "spoiler" | "spoiler_gallery" => "zs reveals the post",
            "more" => "more replies · Enter",
            "gallery" => "[i ]i browse",
            _ => "O opens",
        };
        let focused = screen(&mut app, 200, 40);
        assert!(focused.contains(cue), "{kind}: {focused}");
        if kind == "spoiler_gallery" {
            assert!(!focused.contains("[i ]i browse"), "{focused}");
            press_keys(&mut app, "]i");
            assert_eq!(app.post_view.as_ref().unwrap().gallery_index, 0);
            press_keys(&mut app, "zs");
            let revealed = screen(&mut app, 200, 40);
            assert!(revealed.contains("[i ]i browse"), "{revealed}");
            press_keys(&mut app, "]i");
            assert_eq!(app.post_view.as_ref().unwrap().gallery_index, 1);
            press_keys(&mut app, "zs");
            assert!(!screen(&mut app, 200, 40).contains("[i ]i browse"));
        }
        for sidebar in [false, true] {
            key(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL);
            press(&mut app, 'h');
            if sidebar {
                key(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL);
                press(&mut app, 'h');
            }
            let background = screen(&mut app, 200, 40);
            assert!(
                !background.contains(cue),
                "{kind}: another pane owns these keys: {background}"
            );
            if sidebar {
                key(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL);
                press(&mut app, 'l');
            }
            key(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL);
            press(&mut app, 'l');
            let restored = screen(&mut app, 200, 40);
            assert!(
                restored.contains(cue),
                "{kind}: cached controls must return with focus: {restored}"
            );
            assert_eq!(app.post_view.as_ref().unwrap().generation, generation);
        }
    }
}

#[test]
fn search_key_audit_ownership_shared_island_keys_should_name_the_selected_group() {
    let mut app = app(&["rust", "vim"]);
    app.switch_tab(crate::app::ALL_SUBS_TAB);
    for group in &mut app.all_subs.groups {
        group.error = Some("offline".into());
        group.posts = vec![post_with("readable", Media::None)];
    }
    app.focus_main();
    let rows = screen(&mut app, 200, 30);
    assert_eq!(rows.matches("✗ r retry").count(), 1, "{rows}");
    press(&mut app, 'l');
    let detail = screen(&mut app, 200, 30);
    assert!(
        !detail.contains("✗ r retry") && !detail.contains("zc Card"),
        "{detail}"
    );
}

#[test]
fn search_key_audit_feed_retry_should_leave_focused_detail_before_reloading() {
    for nested in [false, true] {
        for append in [false, true] {
            let mut app = app(&[]);
            app.open_search(Some(crate::reddit::experience::SearchKind::Posts));
            if !nested {
                crate::keys::paste(&mut app, "query");
                key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            }
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            if nested {
                press(&mut app, 'l');
            }
            app.on_msg(Msg::Feed {
                tab: app.active_tab().id,
                generation: app.active_tab().feed.generation,
                append: false,
                result: Ok(crate::model::Listing {
                    posts: vec![post_with("readable", Media::None)],
                    after: Some("next".into()),
                }),
            });
            screen(&mut app, 200, 30);
            app.load_feed(app.active, append);
            app.on_msg(Msg::Feed {
                tab: app.active_tab().id,
                generation: app.active_tab().feed.generation,
                append,
                result: Err("offline".into()),
            });
            press(&mut app, 'l');
            assert_eq!(app.focus, Focus::Post);
            let failed = screen(&mut app, 200, 30);
            assert!(
                failed.contains("C-w h then r retry"),
                "feed retry must first focus the visible list: {failed}"
            );
            if append {
                assert!(
                    failed.contains("Next page failed · C-w h then r retries"),
                    "{failed}"
                );
            }
            let feed = app.active_tab().feed.generation;
            let detail = app.post_view.as_ref().unwrap().generation;
            key(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL);
            press(&mut app, 'h');
            assert_eq!(app.focus, Focus::List);
            assert!(app.post_view.is_some());
            press(&mut app, 'r');
            assert_ne!(app.active_tab().feed.generation, feed);
            assert_eq!(app.post_view.as_ref().unwrap().generation, detail);
        }
    }
}

#[test]
fn search_key_audit_query_should_share_editor_modifier_filtering() {
    let mut app = app(&[]);
    app.open_search(None);
    crate::keys::paste(&mut app, "alpha rust");
    app.search.selected = 1;
    for (code, modifiers) in [
        (KeyCode::Enter, KeyModifiers::CONTROL),
        (KeyCode::Tab, KeyModifiers::CONTROL),
        (KeyCode::Esc, KeyModifiers::ALT),
        (KeyCode::Left, KeyModifiers::ALT),
        (KeyCode::Delete, KeyModifiers::CONTROL),
        (KeyCode::Down, KeyModifiers::ALT),
    ] {
        let expected = (
            app.active,
            app.active_tab().feed.generation,
            app.search.kind,
            app.search.selected,
            app.search.query.value.clone(),
            app.search.query.cursor,
            app.search.editing,
        );
        key(&mut app, code, modifiers);
        assert_eq!(
            (
                app.active,
                app.active_tab().feed.generation,
                app.search.kind,
                app.search.selected,
                app.search.query.value.clone(),
                app.search.query.cursor,
                app.search.editing
            ),
            expected,
            "unsupported editor chord {modifiers:?}+{code:?}"
        );
    }
    key(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL);
    assert_eq!(app.search.query.cursor, 0);
    key(&mut app, KeyCode::Char('e'), KeyModifiers::CONTROL);
    key(&mut app, KeyCode::Backspace, KeyModifiers::ALT);
    assert_eq!(app.search.query.value, "alpha ");
    key(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
    assert!(app.search.query.value.is_empty());
}

#[test]
fn keys_in_a_feed_opened_from_search_should_stay_in_search() {
    let opened = || {
        let mut app = app(&["rust", "other"]);
        app.open_search(None);
        app.search.discovered.loading = false;
        app.search.discovered.entries.push(crate::app::search::Row {
            profile: None,
            comment: None,
            label: "r/rust".into(),
            detail: String::new(),
            target: crate::worker::FeedTarget::Subreddit("rust".into()),
        });
        app.search.selected = 2;
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        screen(&mut app, 200, 30);
        press(&mut app, 'l');
        assert!(app.search_feed_open() && app.active != crate::app::SEARCH_TAB);
        app
    };
    let mut app = opened();
    let kind = app.search.kind;
    key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    assert_eq!(app.active, crate::app::SEARCH_TAB, "Tab left Search");
    assert_ne!(app.search.kind, kind);

    let mut app = opened();
    press(&mut app, 'q');
    assert!(!app.quit, "q quit from the feed opened beside Search");
    assert_eq!(app.active, crate::app::SEARCH_TAB);
}
