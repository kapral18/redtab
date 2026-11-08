use super::*;

#[test]
fn a_notification_read_write_keeps_unrelated_reader_state() {
    let action = Action::Read {
        id: "t4_message".into(),
        read: true,
    };
    let (mut app, _dir, _fixture) = app();
    let page = post_page();
    app.show_post(page.post.clone(), &page.post.permalink);
    let post_generation = app.post_view.as_ref().unwrap().generation;
    app.tabs[0].feed.loaded = true;
    let feed_generation = app.tabs[0].feed.generation;
    app.write_experience(action.clone());
    app.after_experience_write(&action);
    assert!(app.tabs[0].feed.loaded);
    assert_eq!(app.tabs[0].feed.generation, feed_generation);
    assert_eq!(app.post_view.as_ref().unwrap().generation, post_generation);
}

#[test]
fn navigation_cache_reopens_collections_without_another_request_but_r_refetches() {
    let (mut app, _dir, _fixture) = app();
    app.read_experience(Query::Communities, None);
    let generation = app.experience.pending.as_ref().unwrap().0;
    app.on_msg(Msg::Experience {
        generation,
        result: Ok(Response::Page(Page {
            title: "Communities".into(),
            text: "Saved page".into(),
            entries: Vec::new(),
            after: None,
            url: "/subreddits/popular".into(),
        })),
    });
    app.experience = Experience::default();

    app.read_experience(Query::Communities, None);

    assert!(app.experience.pending.is_none());
    assert!(
        matches!(app.experience.screens.last(), Some(Screen::Browse { page, .. }) if page.text == "Saved page")
    );
    app.collection_action(crate::keys::map::Action::Reload);
    assert!(app.experience.pending.is_some());
}

#[test]
fn navigation_cache_thread_links_keep_the_snapshot_age_and_cached_status() {
    let (mut app, _dir, _fixture) = app();
    let page = post_page();
    app.read_experience(Query::Thread(page.post.permalink.clone()), None);
    let generation = app.experience.pending.as_ref().unwrap().0;
    let fetched_at = crate::app::now() - 120;

    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Experience {
            generation,
            result: Ok(Response::Thread(Box::new(page))),
        }),
        fetched_at,
        source: Source {
            cached: true,
            ..Source::default()
        },
        refreshing: false,
    });

    let view = app.post_view.as_ref().unwrap();
    assert_eq!(view.freshness.unwrap().fetched_at, fetched_at);
    assert!(view.freshness.unwrap().source.cached);
}

#[test]
fn navigation_cache_stale_collections_remain_readable_and_keep_the_selected_entry() {
    let (mut app, _dir, _fixture) = app();
    let page = |titles: &[&str]| Page {
        title: "Communities".into(),
        text: String::new(),
        entries: titles
            .iter()
            .map(|title| Entry {
                icon: Default::default(),
                title: (*title).into(),
                body: String::new(),
                target: None,
                query: Some(Query::Community((*title).into())),
                post: None,
            })
            .collect(),
        after: None,
        url: "/subreddits/popular".into(),
    };
    app.read_experience(Query::Communities, None);
    let generation = app.experience.pending.as_ref().unwrap().0;
    app.on_msg(Msg::Experience {
        generation,
        result: Ok(Response::Page(page(&["a", "b"]))),
    });
    app.collections[0].fetched = Instant::now() - Duration::from_secs(600);
    app.experience = Experience::default();

    app.read_experience(Query::Communities, None);
    key(&mut app, KeyCode::Char('j'));
    assert!(matches!(
        app.experience.screens.last(),
        Some(Screen::Browse { selected: 1, .. })
    ));
    if let Some(Screen::Browse { scroll, .. }) = app.experience.screens.last_mut() {
        *scroll = 4;
    }
    let generation = app.experience.pending.as_ref().unwrap().0;
    app.on_msg(Msg::Experience {
        generation,
        result: Ok(Response::Page(page(&["new", "a", "b"]))),
    });

    assert!(matches!(
        app.experience.screens.last(),
        Some(Screen::Browse {
            selected: 1,
            scroll: 4,
            ..
        })
    ));
    assert!(!app.experience.refreshing);
    if let Some(Screen::Browse {
        page, pagination, ..
    }) = app.experience.screens.last()
    {
        assert_eq!(
            page.entries
                .iter()
                .map(|entry| entry.title.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert!(pagination.update.is_some());
    }
    app.collection_action(crate::keys::map::Action::Reload);
    assert!(app.experience.pending.is_none());
    assert!(matches!(
        app.experience.screens.last(),
        Some(Screen::Browse { selected: 2, .. })
    ));
    app.experience = Experience::default();
    app.read_experience(Query::Communities, None);
    assert!(app.experience.pending.is_none());
    app.write_experience(Action::Read {
        id: "t4_message".into(),
        read: true,
    });
    assert!(app.collections.is_empty());
    app.reset_pending_loads();
    app.read_experience(Query::Communities, None);
    assert!(app.experience.pending.is_some());
}

#[test]
fn rereading_a_list_after_a_write_or_r_keeps_the_selected_entry() {
    let (mut app, _dir, _fixture) = app();
    let page = |skip: usize| Page {
        title: "Communities".into(),
        text: String::new(),
        entries: (0..8)
            .filter(|i| *i != skip)
            .map(|i| Entry {
                icon: Default::default(),
                title: format!("community {i}"),
                body: String::new(),
                target: None,
                query: Some(Query::Community(format!("s{i}"))),
                post: None,
            })
            .collect(),
        after: None,
        url: "/subreddits/mine".into(),
    };
    app.overlay = Some(Overlay::Experience);
    app.experience.screens.push(Screen::Browse {
        pagination: Default::default(),
        query: Query::Communities,
        page: page(usize::MAX),
        selected: 4,
        scroll: 2,
    });
    app.experience.screens.push(Screen::Menu {
        title: "community 4".into(),
        choices: Vec::new(),
        selected: 0,
        native: String::new(),
    });
    app.experience.pending = Some((
        91,
        Request::Write(Action::Save {
            id: "t3_x".into(),
            save: false,
        }),
    ));
    app.on_experience(
        91,
        Ok(Response::Written { url: None }),
        crate::cache::Freshness {
            fetched_at: crate::app::now(),
            source: Default::default(),
        },
    );
    // The re-read fails once; r on its Loading screen retries it.
    let failed = app.experience.pending.as_ref().unwrap().0;
    app.on_experience(
        failed,
        Err("offline".into()),
        crate::cache::Freshness {
            fetched_at: crate::app::now(),
            source: Default::default(),
        },
    );
    app.experience_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
    let generation = app.experience.pending.as_ref().unwrap().0;
    assert_ne!(generation, failed);

    app.on_experience(
        generation,
        Ok(Response::Page(page(1))),
        crate::cache::Freshness {
            fetched_at: crate::app::now(),
            source: Default::default(),
        },
    );

    assert!(matches!(
        app.experience.screens.last(),
        Some(Screen::Browse { page, selected: 3, scroll: 2, .. })
            if page.entries[3].title == "community 4"
    ));

    // r reads the list again the same way.
    crate::app::App::collection_action(&mut app, crate::keys::map::Action::Reload);
    let generation = app.experience.pending.as_ref().unwrap().0;
    app.on_experience(
        generation,
        Ok(Response::Page(page(usize::MAX))),
        crate::cache::Freshness {
            fetched_at: crate::app::now(),
            source: Default::default(),
        },
    );
    assert!(matches!(
        app.experience.screens.last(),
        Some(Screen::Browse { page, selected: 4, scroll: 2, .. })
            if page.entries[4].title == "community 4"
    ));

    // A re-read abandoned with Esc leaves a later open of the list at the top.
    app.experience.pending = Some((
        92,
        Request::Write(Action::Save {
            id: "t3_x".into(),
            save: false,
        }),
    ));
    app.experience.screens.push(Screen::Menu {
        title: "community 4".into(),
        choices: Vec::new(),
        selected: 0,
        native: String::new(),
    });
    app.on_experience(
        92,
        Ok(Response::Written { url: None }),
        crate::cache::Freshness {
            fetched_at: crate::app::now(),
            source: Default::default(),
        },
    );
    app.experience_back();
    app.read_experience(Query::Communities, None);
    let failed = app.experience.pending.as_ref().unwrap().0;
    app.on_experience(
        failed,
        Err("offline".into()),
        crate::cache::Freshness {
            fetched_at: crate::app::now(),
            source: Default::default(),
        },
    );
    app.experience_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
    let generation = app.experience.pending.as_ref().unwrap().0;
    app.on_experience(
        generation,
        Ok(Response::Page(page(1))),
        crate::cache::Freshness {
            fetched_at: crate::app::now(),
            source: Default::default(),
        },
    );

    assert!(matches!(
        app.experience.screens.last(),
        Some(Screen::Browse {
            selected: 0,
            scroll: 0,
            ..
        })
    ));
}
