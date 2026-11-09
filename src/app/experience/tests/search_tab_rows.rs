use super::*;

#[test]
fn search_tab_rows_should_open_feeds_with_post_navigation_at_every_width() {
    use crate::reddit::experience::{Entry, Page, Query, Response, SearchKind};
    use crate::worker::FeedTarget;
    for width in [90, 200] {
        for (kind, query, target) in [
            (
                SearchKind::Communities,
                Query::Community("rust".into()),
                FeedTarget::Subreddit("rust".into()),
            ),
            (
                SearchKind::People,
                Query::Profile("writer".into()),
                FeedTarget::User("writer".into()),
            ),
        ] {
            let (mut app, _dir, _fixture) = app();
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 24)).unwrap();
            app.resize(width);
            app.open_search(Some(kind));
            crate::keys::paste(&mut app, "found");
            key(&mut app, KeyCode::Enter);
            app.on_msg(Msg::Search {
                generation: app.search.results_generation(),
                result: Ok(Response::Page(Page {
                    title: "Matches".into(),
                    text: String::new(),
                    entries: vec![
                        Entry {
                            icon: Default::default(),
                            title: "Other".into(),
                            body: String::new(),
                            query: Some(match kind {
                                SearchKind::Communities => Query::Community("other".into()),
                                _ => Query::Profile("other".into()),
                            }),
                            target: None,
                            post: None,
                        },
                        Entry {
                            icon: Default::default(),
                            title: "Match".into(),
                            body: String::new(),
                            query: Some(query),
                            target: None,
                            post: None,
                        },
                    ],
                    after: None,
                    url: "/".into(),
                })),
            });
            key(&mut app, KeyCode::Down);
            key(&mut app, KeyCode::Esc);
            terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
            key(&mut app, KeyCode::Char('l'));
            assert_eq!(app.active_tab().feed_target(), Some(target.clone()));
            app.on_msg(Msg::Feed {
                tab: app.active_tab().id,
                generation: app.active_tab().feed.generation,
                append: false,
                result: Ok(crate::model::Listing {
                    posts: ["first", "second", "third"]
                        .map(|id| {
                            let mut post = post_page().post;
                            post.id = id.into();
                            let (source, prefix) = match &target {
                                FeedTarget::Subreddit(name) => (name.clone(), format!("/r/{name}")),
                                FeedTarget::User(name) => {
                                    (format!("u_{name}"), format!("/user/{name}"))
                                }
                                _ => unreachable!(),
                            };
                            post.subreddit = source;
                            post.permalink = format!("{prefix}/comments/{id}/title/");
                            post
                        })
                        .into(),
                    after: Some("page_two".into()),
                }),
            });
            terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
            assert_eq!(app.current_post().unwrap().id, "first");
            key(&mut app, KeyCode::Char('j'));
            assert_eq!(app.current_post().unwrap().id, "second");
            key(&mut app, KeyCode::Char('k'));
            assert_eq!(app.current_post().unwrap().id, "first");
            key(&mut app, KeyCode::Char('G'));
            let _ = app.tick_at(Instant::now() + std::time::Duration::from_secs(1));
            assert!(app.active_tab().feed.loading_more());
            assert_eq!(app.current_post().unwrap().id, "third");
            assert_eq!(app.active_tab().feed_target(), Some(target));
            assert_eq!(app.search.selected, 1);
            key(&mut app, KeyCode::Char('l'));
            assert_eq!(app.post_view.as_ref().unwrap().post.id, "third");
            let feed_id = app.active_tab().id;
            key(&mut app, KeyCode::Char('h'));
            assert_eq!(app.active_tab().id, feed_id);
            assert_eq!(app.focus, Focus::List);
            if width == 200 {
                assert!(app.post_view.is_some(), "first h only changes focus");
                key(&mut app, KeyCode::Left);
                assert_eq!(app.active_tab().id, feed_id);
                assert_eq!(app.focus, Focus::List);
            }
            assert!(app.post_view.is_none());
            key(
                &mut app,
                if width == 90 {
                    KeyCode::Char('h')
                } else {
                    KeyCode::Left
                },
            );
            assert_eq!(app.active, crate::app::SEARCH_TAB);
            assert_eq!(app.focus, Focus::List);
            assert!(!app.search_editing());
            assert_eq!(app.search.kind, kind);
            assert_eq!(app.search.selected, 1);
            assert_eq!(app.search.query.value, "found");
            key(&mut app, KeyCode::Char('h'));
            assert_eq!(app.focus, Focus::Sidebar);
            key(&mut app, KeyCode::Char('l'));
            key(&mut app, KeyCode::Char('l'));
            assert_eq!(app.active_tab().id, feed_id);
            sequence(&mut app, "gtgT");
            assert_eq!(app.active_tab().id, feed_id);
            key(&mut app, KeyCode::Char('h'));
            assert_eq!(app.active_tab().id, feed_id);
            assert_eq!(app.focus, Focus::Sidebar);
            key(&mut app, KeyCode::F(3));
            assert!(app.search_editing());
            assert_eq!(app.search.query.value, "found");
        }
    }
}

#[test]
fn search_tab_should_search_as_you_type_preview_rows_and_open_them() {
    use crate::app::search::{DEBOUNCE, PREVIEW_SETTLE};
    use crate::worker::FeedTarget;
    let (mut app, _dir, _fixture) = app();
    app.resize(200);
    key(&mut app, KeyCode::F(3));
    assert_eq!(app.active, crate::app::SEARCH_TAB);
    assert!(app.search_editing());
    assert!(app.search.exploring());
    assert_eq!(
        app.search.rows()[0].target,
        FeedTarget::Subreddit("popular".into())
    );

    sequence(&mut app, "rust");
    assert_eq!(app.search.query.value, "rust");
    assert!(
        app.active_tab().feed.search.is_none(),
        "waits for the pause"
    );
    let _ = app.tick_at(Instant::now() + DEBOUNCE);
    assert_eq!(app.active_tab().feed.search.as_deref(), Some("rust"));
    assert!(app.active_tab().feed.loading());

    key(&mut app, KeyCode::Tab);
    assert_eq!(app.search.kind, SearchKind::Communities);
    let _ = app.tick_at(Instant::now() + DEBOUNCE);
    assert!(app.search.results.loading());
    let generation = app.search.results_generation();
    app.on_search(
        generation,
        Ok(Response::Page(Page {
            title: "Communities".into(),
            text: String::new(),
            entries: vec![
                Entry {
                    icon: Default::default(),
                    title: "r/rust".into(),
                    body: format!(
                        "\n{}{}https://www.rust-lang.org{}The Rust{} language\n",
                        crate::model::TEXT_LINK,
                        crate::model::LINK_TARGET,
                        crate::model::LINK_TARGET_END,
                        crate::model::TEXT_END
                    ),
                    query: Some(Query::Community("rust".into())),
                    target: None,
                    post: None,
                },
                Entry {
                    icon: Default::default(),
                    title: "r/vim".into(),
                    body: String::new(),
                    query: Some(Query::Community("vim".into())),
                    target: None,
                    post: None,
                },
            ],
            after: Some("t5_vim".into()),
            url: "/".into(),
        })),
    );
    assert_eq!(app.search.rows()[0].label, "r/rust");
    assert_eq!(app.search.rows()[0].detail, "The Rust language");

    key(&mut app, KeyCode::Down);
    assert_eq!(app.search.selected, 1, "arrows choose while typing");
    assert!(
        app.search.results.loading() && app.search.rows().len() == 2,
        "the end of the rows asks for the next page and keeps them"
    );
    app.on_search(
        app.search.results_generation(),
        Ok(Response::Page(Page {
            title: "Communities".into(),
            text: String::new(),
            entries: vec![Entry {
                icon: Default::default(),
                title: "r/zig".into(),
                body: String::new(),
                query: Some(Query::Community("zig".into())),
                target: None,
                post: None,
            }],
            after: None,
            url: "/".into(),
        })),
    );
    assert_eq!(app.search.rows()[2].label, "r/zig");
    crate::keys::handle(
        &mut app,
        crossterm::event::KeyEvent::new(
            KeyCode::Char('p'),
            crossterm::event::KeyModifiers::CONTROL,
        ),
    );
    assert!(
        app.search_editing() && app.search.selected == 0,
        "C-p chooses while typing"
    );
    app.search_preview_drawn(false);
    let _ = app.tick_at(Instant::now() + PREVIEW_SETTLE);
    assert_eq!(
        app.search.preview.target, None,
        "a hidden pane loads nothing"
    );
    app.search_preview_drawn(true);
    let _ = app.tick_at(Instant::now() + PREVIEW_SETTLE);
    assert_eq!(
        app.search.preview.target,
        Some(FeedTarget::Subreddit("rust".into()))
    );
    assert!(app.search.preview.loading);
    let named = |id: &str| {
        let mut post = post_page().post;
        post.id = id.into();
        post
    };
    let answer = |posts, refreshing| Msg::Snapshot {
        message: Box::new(Msg::Feed {
            tab: crate::worker::PREVIEW_FEED,
            generation: app.search.preview.generation,
            append: false,
            result: Ok(crate::model::Listing { posts, after: None }),
        }),
        fetched_at: 1,
        refreshing,
        source: crate::cache::Source::default(),
    };
    let cached = answer(vec![named("old")], true);
    let live = answer(vec![named("hot")], false);
    app.on_msg(cached);
    assert_eq!(
        app.search.preview.posts[0].id, "old",
        "the cached copy shows at once"
    );
    assert!(
        app.search.preview.loading,
        "and Reddit's answer is still wanted"
    );
    app.on_msg(live);
    assert!(!app.search.preview.loading);
    assert_eq!(app.search.preview.posts[0].id, "old");
    assert_eq!(app.search.preview.update.as_ref().unwrap()[0].id, "hot");

    key(&mut app, KeyCode::Esc);
    assert_eq!(
        app.membership_target(),
        Some(crate::model::MembershipTarget::Community("rust".into())),
        "SPC c and SPC m act on the selected row"
    );
    sequence(&mut app, "r");
    assert_eq!(app.search.preview.posts[0].id, "hot");
    assert!(app.search.preview.update.is_none());
    sequence(&mut app, "r");
    assert!(
        app.search.results.loading() && app.search.preview_forced(),
        "r asks again and keeps the preview's skip-the-cache request"
    );
    app.on_search(
        app.search.results_generation(),
        Ok(Response::Page(Page {
            title: "Communities".into(),
            text: String::new(),
            entries: vec![Entry {
                icon: Default::default(),
                title: "r/rust".into(),
                body: String::new(),
                query: Some(Query::Community("rust".into())),
                target: None,
                post: None,
            }],
            after: None,
            url: "/".into(),
        })),
    );
    let _ = app.tick_at(Instant::now() + PREVIEW_SETTLE);
    assert!(app.search.preview.loading && !app.search.preview_forced());

    key(&mut app, KeyCode::Char('/'));
    assert!(app.search_editing());
    key(&mut app, KeyCode::Char('x'));
    key(&mut app, KeyCode::Backspace);
    key(&mut app, KeyCode::Enter);
    assert!(
        !app.search_editing() && app.active == crate::app::SEARCH_TAB,
        "Enter while typing leaves the query for the results"
    );
    key(&mut app, KeyCode::Enter);
    assert_eq!(
        app.active_tab().subreddit(),
        Some("rust"),
        "Enter on the results opens the chosen row"
    );

    sequence(&mut app, " su");
    assert_eq!(app.active, crate::app::SEARCH_TAB);
    assert_eq!(app.search.kind, SearchKind::People);
    assert!(app.search_editing());
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Tab);
    assert_eq!(
        (app.active, app.search.kind),
        (crate::app::SEARCH_TAB, SearchKind::Posts),
        "Tab in the results switches the kind, not the tab"
    );
    key(&mut app, KeyCode::Esc);
    assert!(
        !app.search_editing(),
        "Esc in the results stays out of the query"
    );
    app.experience.pending = None;
    for query in [
        Query::Community("rust".into()),
        Query::Profile("spez".into()),
    ] {
        app.overlay = Some(Overlay::Experience);
        app.experience.screens = vec![Screen::Browse {
            pagination: Default::default(),
            query: Query::Communities,
            page: Page {
                title: "Results".into(),
                text: String::new(),
                entries: vec![Entry {
                    icon: Default::default(),
                    title: "Result".into(),
                    body: String::new(),
                    query: Some(query.clone()),
                    target: None,
                    post: None,
                }],
                after: None,
                url: "/".into(),
            },
            selected: 0,
            scroll: 0,
        }];
        key(&mut app, KeyCode::Enter);
        assert!(app.overlay.is_none());
        match query {
            Query::Community(_) => assert!(
                matches!(&app.active_tab().kind, TabKind::Subreddit { name, .. } if name == "rust")
            ),
            _ => assert!(
                matches!(&app.active_tab().kind, TabKind::User { name, .. } if name == "spez")
            ),
        }
    }
}
