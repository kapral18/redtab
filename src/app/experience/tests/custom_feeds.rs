use super::*;

#[test]
fn when_a_custom_feed_is_deleted_it_should_reload_the_custom_feed_index() {
    let (mut app, _dir, _fixture) = app();
    app.experience.screens.push(Screen::Browse {
        pagination: Default::default(),
        query: Query::Multi {
            owner: "reader".into(),
            name: "tools".into(),
        },
        page: Page {
            title: "tools".into(),
            text: String::new(),
            entries: Vec::new(),
            after: None,
            url: "/user/reader/m/tools".into(),
        },
        selected: 0,
        scroll: 0,
    });
    let action = Action::DeleteMulti {
        owner: "reader".into(),
        name: "tools".into(),
    };
    app.experience.screens.push(Screen::Confirm {
        label: "Delete tools".into(),
        action: action.clone(),
        native: "/user/reader/m/tools".into(),
    });
    app.experience.pending = Some((91, Request::Write(action)));
    app.on_experience(
        91,
        Ok(Response::Written { url: None }),
        crate::cache::Freshness {
            fetched_at: crate::app::now(),
            source: Default::default(),
        },
    );
    assert!(matches!(
        &app.experience.pending,
        Some((
            _,
            Request::Read {
                query: Query::Multis,
                after: None
            }
        ))
    ));
}

#[test]
fn custom_feed_keys_should_manage_communities_then_allow_post_actions_after_search() {
    for refreshing in [false, true] {
        let (mut app, _dir, _fixture) = app();
        let target = Target::from_post(&post_page().post);
        app.overlay = Some(Overlay::Experience);
        app.experience.screens = vec![Screen::Browse {
            pagination: Default::default(),
            query: Query::Multi {
                owner: "reader".into(),
                name: "feed".into(),
            },
            page: Page {
                title: "feed".into(),
                text: String::new(),
                entries: vec![Entry {
                    icon: Default::default(),
                    title: "post".into(),
                    body: String::new(),
                    query: None,
                    target: Some(target),
                    post: None,
                }],
                after: None,
                url: "/".into(),
            },
            selected: 0,
            scroll: 0,
        }];
        key(&mut app, KeyCode::Char('d'));
        assert!(app.experience.pending.is_none());
        assert!(matches!(
            app.experience.screens.last(),
            Some(Screen::Input(Form {
                kind: FormKind::MultiCommunity { add: false, .. },
                ..
            }))
        ));
        key(&mut app, KeyCode::Esc);
        let old_generation = app.generation();
        if refreshing {
            app.experience.refreshing = true;
            app.experience.pending = Some((
                old_generation,
                Request::Read {
                    query: Query::Multi {
                        owner: "reader".into(),
                        name: "feed".into(),
                    },
                    after: None,
                },
            ));
        }
        sequence(&mut app, " sp");
        crate::keys::paste(&mut app, "rust");
        key(&mut app, KeyCode::Enter);
        let post = post_page().post;
        let target = Target::from_post(&post);
        app.on_msg(Msg::Feed {
            tab: app.active_tab().id,
            generation: app.active_tab().feed.generation,
            append: false,
            result: Ok(crate::model::Listing {
                posts: vec![post],
                after: None,
            }),
        });
        key(&mut app, KeyCode::Esc);
        assert!(app.overlay.is_none());
        key(&mut app, KeyCode::Char('d'));
        assert!(matches!(&app.experience.pending,
        Some((_, Request::Read { query: Query::Inspect(actual), .. })) if actual == &target));
        app.on_msg(Msg::Experience {
            generation: old_generation,
            result: Ok(Response::Page(Page {
                title: "Old custom feed".into(),
                text: String::new(),
                entries: Vec::new(),
                after: None,
                url: "/".into(),
            })),
        });
        assert!(matches!(&app.experience.pending,
            Some((_, Request::Read { query: Query::Inspect(actual), .. })) if actual == &target));
    }
}
