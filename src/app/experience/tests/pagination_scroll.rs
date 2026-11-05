use super::*;

#[test]
fn pagination_scroll_should_append_retry_and_stop_cursor_cycles_without_losing_position() {
    let (mut app, _dir, _fixture) = app();
    let entries: Vec<_> = (0..8)
        .map(|i| Entry {
            icon: Default::default(),
            title: format!("community {i}"),
            body: "Description\n".repeat(30),
            target: None,
            query: Some(Query::Community(format!("s{i}"))),
            post: None,
        })
        .collect();
    app.overlay = Some(Overlay::Experience);
    app.experience.screens.push(Screen::Browse {
        pagination: Default::default(),
        query: Query::Communities,
        page: Page {
            title: "Communities".into(),
            text: String::new(),
            entries,
            after: Some("a".into()),
            url: "/subreddits/popular".into(),
        },
        selected: 2,
        scroll: 0,
    });

    key(&mut app, KeyCode::Char('j'));
    assert!(app.experience.pending.is_none());
    key(&mut app, KeyCode::Char('k'));
    app.load_settled(Instant::now() + NAV_SETTLE);
    assert!(
        app.experience.pending.is_none(),
        "returning above the margin cancels pagination"
    );
    key(&mut app, KeyCode::Char('j'));
    assert!(!app.load_settled(Instant::now()));
    app.load_settled(Instant::now() + NAV_SETTLE);
    let generation = app
        .experience
        .pending
        .as_ref()
        .expect("scrolling should fetch more")
        .0;
    assert!(
        matches!(&app.experience.pending, Some((_, Request::Read { after: Some(cursor), .. })) if cursor == "a")
    );
    key(&mut app, KeyCode::Char('j'));
    key(&mut app, KeyCode::PageDown);
    key(&mut app, KeyCode::Enter);
    assert!(matches!(
        app.experience.screens.last(),
        Some(Screen::Browse {
            selected: 4,
            scroll: 10,
            ..
        })
    ));
    assert_eq!(app.experience.pending.as_ref().unwrap().0, generation);
    app.on_experience(
        generation,
        Err("offline".into()),
        crate::cache::Freshness {
            fetched_at: crate::app::now(),
            source: Default::default(),
        },
    );
    key(&mut app, KeyCode::Char('G'));
    assert!(
        app.experience.pending.is_none(),
        "scrolling must not retry failures"
    );
    app.experience.screens.push(Screen::Menu {
        title: "Nested menu".into(),
        choices: Vec::new(),
        selected: 0,
        native: "/".into(),
    });
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Char('r'));
    let generation = app
        .experience
        .pending
        .as_ref()
        .expect("r must retry the failed continuation")
        .0;
    assert!(
        matches!(&app.experience.pending, Some((_, Request::Read { after: Some(cursor), .. })) if cursor == "a")
    );
    assert!(
        matches!(app.experience.screens.last(), Some(Screen::Browse { page, selected: 7, .. }) if page.entries.len() == 8)
    );
    let item = Entry {
        icon: Default::default(),
        title: "new community".into(),
        body: "New description".into(),
        target: None,
        query: Some(Query::Community("new".into())),
        post: None,
    };
    app.on_experience(
        generation,
        Ok(Response::Page(Page {
            title: "Communities".into(),
            text: String::new(),
            entries: vec![item.clone(), item],
            after: Some("b".into()),
            url: "/".into(),
        })),
        crate::cache::Freshness {
            fetched_at: crate::app::now(),
            source: Default::default(),
        },
    );
    assert!(
        matches!(app.experience.screens.last(), Some(Screen::Browse { page, selected: 7, .. }) if page.entries.len() == 9)
    );
    key(&mut app, KeyCode::Char('G'));
    app.load_settled(Instant::now() + NAV_SETTLE);
    let generation = app
        .experience
        .pending
        .as_ref()
        .expect("scrolling should fetch the next cursor")
        .0;
    app.on_experience(
        generation,
        Ok(Response::Page(Page {
            title: "Communities".into(),
            text: String::new(),
            entries: Vec::new(),
            after: Some("a".into()),
            url: "/".into(),
        })),
        crate::cache::Freshness {
            fetched_at: crate::app::now(),
            source: Default::default(),
        },
    );
    key(&mut app, KeyCode::Char('j'));
    app.load_settled(Instant::now() + NAV_SETTLE);
    assert!(
        app.experience.pending.is_none(),
        "a longer cursor cycle must stop"
    );
    assert!(
        matches!(app.experience.screens.last(), Some(Screen::Browse { page, selected: 8, .. }) if page.entries.len() == 9 && page.after.is_none())
    );
}

#[test]
fn pagination_scroll_should_advance_empty_collections_and_ignore_abandoned_replies() {
    let (mut app, _dir, _fixture) = app();
    app.overlay = Some(Overlay::Experience);
    app.experience.screens.push(Screen::Browse {
        pagination: Default::default(),
        query: Query::Communities,
        page: Page {
            title: "Communities".into(),
            text: String::new(),
            entries: Vec::new(),
            after: Some("a".into()),
            url: "/subreddits/popular".into(),
        },
        selected: 0,
        scroll: 0,
    });
    key(&mut app, KeyCode::PageDown);
    assert!(app.experience.pending.is_none());
    app.load_settled(Instant::now() + NAV_SETTLE);
    let generation = app
        .experience
        .pending
        .as_ref()
        .expect("scrolling an empty result should fetch more")
        .0;
    app.on_experience(
        generation,
        Ok(Response::Page(Page {
            title: "Communities".into(),
            text: String::new(),
            entries: Vec::new(),
            after: Some("b".into()),
            url: "/".into(),
        })),
        crate::cache::Freshness {
            fetched_at: crate::app::now(),
            source: Default::default(),
        },
    );
    assert!(
        app.experience.pending.is_none(),
        "one gesture must not spin through empty pages"
    );
    key(&mut app, KeyCode::Char('j'));
    app.load_settled(Instant::now() + NAV_SETTLE);
    let generation = app
        .experience
        .pending
        .as_ref()
        .expect("the next scroll should continue")
        .0;
    assert!(
        matches!(&app.experience.pending, Some((_, Request::Read { after: Some(cursor), .. })) if cursor == "b")
    );
    key(&mut app, KeyCode::Esc);
    app.on_experience(
        generation,
        Ok(Response::Page(Page {
            title: "Abandoned result".into(),
            text: String::new(),
            entries: Vec::new(),
            after: None,
            url: "/".into(),
        })),
        crate::cache::Freshness {
            fetched_at: crate::app::now(),
            source: Default::default(),
        },
    );
    assert!(app.overlay.is_none());
    assert!(app.experience.screens.is_empty());
}

#[test]
fn when_pagination_repeats_items_and_its_cursor_it_should_deduplicate_and_stop() {
    let (mut app, _dir, _fixture) = app();
    let entry = Entry {
        icon: Default::default(),
        title: "r/rust".into(),
        body: String::new(),
        target: None,
        query: Some(Query::Community("rust".into())),
        post: None,
    };
    app.overlay = Some(Overlay::Experience);
    app.experience.screens.push(Screen::Browse {
        pagination: Default::default(),
        query: Query::Communities,
        page: Page {
            title: "Communities".into(),
            text: String::new(),
            entries: vec![entry.clone()],
            after: Some("loop".into()),
            url: "/".into(),
        },
        selected: 0,
        scroll: 0,
    });
    app.experience.pending = Some((
        91,
        Request::Read {
            query: Query::Communities,
            after: Some("loop".into()),
        },
    ));
    app.on_experience(
        91,
        Ok(Response::Page(Page {
            title: "Communities".into(),
            text: String::new(),
            entries: vec![entry],
            after: Some("loop".into()),
            url: "/".into(),
        })),
        crate::cache::Freshness {
            fetched_at: crate::app::now(),
            source: Default::default(),
        },
    );
    let Some(Screen::Browse { page, .. }) = app.experience.screens.last() else {
        panic!("Expected browser");
    };
    assert_eq!(page.entries.len(), 1);
    assert!(page.after.is_none());
}
