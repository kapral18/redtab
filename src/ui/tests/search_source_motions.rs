use super::*;

#[test]
fn search_source_motions_should_lift_details_to_the_parent_rows() {
    use crate::reddit::experience::{Entry, Page, Query, Response};
    use crate::worker::FeedTarget;
    for width in [100, 200] {
        for (command, expected) in [("]s", 4), ("[s", 2)] {
            let mut app = app(&["sidebar_only"]);
            app.open_search(None);
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            app.on_msg(Msg::Search {
                generation: app.search.discovered_generation(),
                result: Ok(Response::Page(Page {
                    title: "Discover".into(),
                    text: String::new(),
                    url: "/".into(),
                    after: None,
                    entries: ["alpha", "beta", "gamma"]
                        .into_iter()
                        .map(|name| Entry {
                            title: format!("r/{name}"),
                            body: String::new(),
                            query: Some(Query::Community(name.into())),
                            icon: Default::default(),
                            target: None,
                            post: None,
                        })
                        .collect(),
                })),
            });
            app.search.selected = 3;
            screen(&mut app, width, 24);
            press(&mut app, 'l');
            let source = app.active;
            let mut post = post_with("nested", Media::None);
            post.subreddit = "beta".into();
            app.on_msg(Msg::Feed {
                tab: app.active_tab().id,
                generation: app.active_tab().feed.generation,
                append: false,
                result: Ok(crate::model::Listing {
                    posts: vec![post.clone()],
                    after: None,
                }),
            });
            press(&mut app, 'l');
            let mut root = reply("root".into());
            root.replies.push(reply("deep".into()));
            app.on_msg(Msg::Post {
                generation: app.post_view.as_ref().unwrap().generation,
                result: Ok(Box::new(PostPage {
                    post,
                    body: "A long passage before the replies.\n".repeat(35),
                    comments: vec![root],
                })),
            });
            screen(&mut app, width, 24);
            press(&mut app, 'G');
            screen(&mut app, width, 24);
            assert!(app.post_view.as_ref().unwrap().scroll > 0);
            if app.detail_fits() && command == "[s" {
                press(&mut app, 'h');
                assert_eq!(app.focus, Focus::List);
                assert!(app.post_view.is_some());
            }

            press_keys(&mut app, command);

            assert_eq!(app.active, crate::app::SEARCH_TAB, "{width}: {command}");
            assert_eq!(app.search.selected, expected);
            assert_eq!(app.focus, Focus::List);
            assert!(!app.search_editing());
            assert!(app.post_view.is_none());
            assert!(!app.ancestor_following());
            assert_eq!(app.tabs[source].feed.posts[0].id, "nested");
            let destination = app.search.selected_row().unwrap().target;
            press(&mut app, 'l');
            assert!(app.search_feed_open());
            let FeedTarget::Subreddit(name) = destination else {
                unreachable!()
            };
            assert_eq!(app.active_tab().subreddit(), Some(name.as_str()));
            if app.detail_fits() {
                key(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL);
                press(&mut app, 'h');
            }
            press_keys(&mut app, command);
            assert_eq!(app.active, crate::app::SEARCH_TAB);
            assert_eq!(app.search.selected, if command == "]s" { 4 } else { 1 });
        }
    }
}

#[test]
fn search_source_motions_should_use_shown_sources_without_opened_subreddit_tabs() {
    use crate::keys::map::{self, Action};
    use crate::reddit::experience::SearchKind;
    use crate::worker::FeedTarget;
    let mut explore = app(&[]);
    explore.open_search(None);
    key(&mut explore, KeyCode::Esc, KeyModifiers::NONE);
    for action in [Action::NextCommunity, Action::PreviousCommunity] {
        assert!(map::available(&explore, action));
    }
    press_keys(&mut explore, "]s");
    assert_eq!(explore.search.selected, 1);
    press_keys(&mut explore, "[s");
    assert_eq!(explore.search.selected, 0);
    press_keys(&mut explore, "[s");
    assert_eq!(explore.search.selected, 0);

    for kind in [SearchKind::Communities, SearchKind::People] {
        let mut app = app(&["unrelated"]);
        app.open_search(Some(kind));
        app.search.query.insert("topic");
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        app.search.results.entries = ["first", "second"]
            .into_iter()
            .map(|name| crate::app::search::Row {
                profile: None,
                comment: None,
                label: name.into(),
                detail: String::new(),
                target: if kind == SearchKind::People {
                    FeedTarget::User(name.into())
                } else {
                    FeedTarget::Subreddit(name.into())
                },
            })
            .collect();
        let help = help::entries(&app);
        assert!(
            help.iter()
                .any(|(keys, text)| keys == "]s / [s" && text.contains("Search source"))
        );
        press(&mut app, 'l');
        press_keys(&mut app, "]s");
        assert_eq!(app.active, crate::app::SEARCH_TAB);
        assert_eq!(app.search.selected, 1);
        press(&mut app, 'l');
        press_keys(&mut app, "]s");
        assert_eq!(app.active, crate::app::SEARCH_TAB);
        assert_eq!(app.search.selected, 1, "lift even at the last source");
        press_keys(&mut app, "[s");
        assert_eq!(app.search.selected, 0);
        assert_eq!(app.search.query.value, "topic");
        assert_eq!(app.search.kind, kind);
        press(&mut app, '/');
        assert!(!map::available(&app, Action::FocusLeft));
        assert!(!map::available(&app, Action::FocusRight));
        press_keys(&mut app, "]s[s");
        assert_eq!(app.search.query.value, "topic]s[s");
        assert_eq!(app.search.selected, 0);
        app.show_auth();
        assert!(!map::available(&app, Action::NextCommunity));
        assert!(!map::available(&app, Action::PreviousCommunity));
    }
}

#[test]
fn search_source_motions_should_hide_unreachable_post_and_pane_branches() {
    use crate::keys::map::{self, Action};
    let mut app = app(&[]);
    app.open_search(None);
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    screen(&mut app, 100, 24);
    assert!(!map::available(&app, Action::FocusRight));
    assert!(
        !help::entries(&app)
            .iter()
            .any(|(keys, _)| keys.contains("C-w l"))
    );
    screen(&mut app, 200, 24);
    assert!(map::available(&app, Action::FocusRight));
    press_keys(&mut app, "\u{17}l");
    assert!(app.search_feed_open());
    let post = post_with("parent", Media::None);
    app.on_msg(Msg::Feed {
        tab: app.active_tab().id,
        generation: app.active_tab().feed.generation,
        append: false,
        result: Ok(crate::model::Listing {
            posts: vec![post],
            after: None,
        }),
    });
    press(&mut app, 'l');
    assert!(map::available(&app, Action::NextPost));
    assert!(map::available(&app, Action::PreviousPost));
    assert!(!map::available(&app, Action::FocusRight));
    assert!(map::available(&app, Action::FocusLeft));
    app.tabs[app.active].feed.posts.clear();
    assert!(!map::available(&app, Action::NextPost));
    assert!(!map::available(&app, Action::PreviousPost));
    assert!(
        !help::entries(&app)
            .iter()
            .any(|(keys, _)| keys.contains("]p"))
    );
    press_keys(&mut app, "[s");
    assert_eq!(app.active, crate::app::SEARCH_TAB);
    assert_eq!(app.search.selected, 0);
    app.focus_sidebar();
    assert!(!map::available(&app, Action::FocusLeft));
    assert!(map::available(&app, Action::FocusRight));
    assert!(!map::available(&app, Action::NextCommunity));
    press_keys(&mut app, "\u{17}l");
    assert_eq!(app.focus, Focus::List);

    app.open_search(Some(crate::reddit::experience::SearchKind::People));
    crate::keys::paste(&mut app, "missing");
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    screen(&mut app, 200, 24);
    assert!(app.search.rows_loading());
    assert!(!map::available(&app, Action::FocusRight));
    press_keys(&mut app, "\u{17}l");
    assert_eq!(app.active, crate::app::SEARCH_TAB);
    app.on_msg(Msg::Search {
        generation: app.search.results_generation(),
        result: Err("offline".into()),
    });
    screen(&mut app, 200, 24);
    assert!(app.search.rows_error().is_some());
    assert!(!map::available(&app, Action::FocusRight));
    press(&mut app, 'r');
    app.on_msg(Msg::Search {
        generation: app.search.results_generation(),
        result: Ok(crate::reddit::experience::Response::Page(
            crate::reddit::experience::Page {
                title: "People".into(),
                text: String::new(),
                entries: vec![],
                after: None,
                url: "/search".into(),
            },
        )),
    });
    screen(&mut app, 200, 24);
    assert!(!map::available(&app, Action::FocusRight));
    assert!(
        !help::entries(&app)
            .iter()
            .any(|(keys, _)| keys.contains("C-w l"))
    );
    press(&mut app, 'r');
    app.on_msg(Msg::Search {
        generation: app.search.results_generation(),
        result: Ok(crate::reddit::experience::Response::Page(
            crate::reddit::experience::Page {
                title: "People".into(),
                text: String::new(),
                after: None,
                url: "/search".into(),
                entries: vec![crate::reddit::experience::Entry {
                    title: "writer".into(),
                    body: String::new(),
                    icon: Default::default(),
                    target: None,
                    post: None,
                    query: Some(crate::reddit::experience::Query::Profile("writer".into())),
                }],
            },
        )),
    });
    screen(&mut app, 200, 24);
    assert!(map::available(&app, Action::FocusRight));
    press_keys(&mut app, "\u{17}l");
    assert!(app.search_feed_open());
    assert!(matches!(&app.active_tab().kind, TabKind::User { name, .. } if name == "writer"));
}
