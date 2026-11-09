use super::*;

#[test]
fn search_tab_post_results_should_name_each_posts_community() {
    let mut app = app(&[]);
    app.open_search(None);
    crate::keys::paste(&mut app, "editor");
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    let mut post = post_with("found", Media::None);
    post.subreddit = "vim".into();
    app.on_msg(Msg::Feed {
        tab: app.active_tab().id,
        generation: app.active_tab().feed.generation,
        append: false,
        result: Ok(crate::model::Listing {
            posts: vec![post],
            after: None,
        }),
    });

    let shown = screen(&mut app, 100, 20);

    assert!(shown.contains("r/vim"), "{shown}");
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(
        help::entries(&app)
            .iter()
            .any(|(key, text)| key == "Tab / S-Tab"
                && text == "Switch Posts / Communities / Comments / Media / People"),
        "list help names the kind keys"
    );
    app.tabs[crate::app::SEARCH_TAB].feed.after = Some("next".into());
    app.load_feed(crate::app::SEARCH_TAB, true);
    app.overlay = Some(Overlay::Picker(crate::app::PickerState::default()));
    let covered = screen(&mut app, 100, 20);
    assert!(
        covered.lines().last().unwrap().contains("more posts"),
        "a covered results loader moves to the footer: {covered}"
    );
    app.overlay = None;
    assert!(shown.contains("Posts  matching “editor”"), "{shown}");
    assert_eq!(
        shown.matches("editor").count(),
        2,
        "query bar and title only"
    );
}

#[test]
fn long_explore_descriptions_should_show_visible_summaries_and_later_entries() {
    let mut app = app(&[]);
    app.open_search(None);
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    app.search.discovered.fetch = Default::default();
    app.search.discovered.entries = vec![
        crate::app::search::Row {
            profile: None,
            comment: None,
            label: "r/long_description".into(),
            detail: "A descriptive word ".repeat(60),
            target: crate::worker::FeedTarget::Subreddit("long_description".into()),
        },
        crate::app::search::Row {
            profile: None,
            comment: None,
            label: "r/second".into(),
            detail: "A short description".into(),
            target: crate::worker::FeedTarget::Subreddit("second".into()),
        },
    ];
    app.search.selected = 2;
    let actual = screen(&mut app, 70, 20);
    assert!(
        actual.contains("r/second"),
        "long summaries must leave room for later entries: {actual}"
    );
    assert!(
        actual
            .lines()
            .any(|line| line.contains("descriptive") && line.contains('…')),
        "a shortened description needs a visible ellipsis: {actual}"
    );
}

#[test]
fn search_revision_should_prefetch_from_the_visible_window_before_the_last_rows() {
    use crate::reddit::experience::{Entry, Page, Query, Response};
    let mut app = app(&[]);
    app.open_search(None);
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    app.on_msg(Msg::Search {
        generation: app.search.discovered_generation(),
        result: Ok(Response::Page(Page {
            title: "Discover".into(),
            text: String::new(),
            url: "/".into(),
            after: Some("next".into()),
            entries: (0..50)
                .map(|i| Entry {
                    title: format!("r/community{i}"),
                    body: String::new(),
                    query: Some(Query::Community(format!("community{i}"))),
                    icon: Default::default(),
                    target: None,
                    post: None,
                })
                .collect(),
        })),
    });
    app.search.selected = 0;
    screen(&mut app, 160, 30);
    assert!(
        app.search.discovered.loading(),
        "keep another API page ready while the first page is being read"
    );
    let generation = app.search.discovered_generation();
    screen(&mut app, 160, 30);
    assert_eq!(
        app.search.discovered_generation(),
        generation,
        "one continuation at a time"
    );
}

#[test]
fn search_revision_should_keep_incremental_panes_and_restore_parent_lists() {
    for width in [100, 160, 200] {
        let mut app = app(&[]);
        app.open_search(None);
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        app.search.discovered.fetch = Default::default();
        app.search.discovered.entries.push(crate::app::search::Row {
            profile: None,
            comment: None,
            label: "r/zig".into(),
            detail: "Community summary marker".into(),
            target: crate::worker::FeedTarget::Subreddit("zig".into()),
        });
        app.search.selected = 2;
        screen(&mut app, width, 30);
        let split = app.detail_fits();
        press(&mut app, 'l');
        let mut first = post_with("nested_first", Media::None);
        first.subreddit = "zig".into();
        let mut second = post_with("nested_second", Media::None);
        second.subreddit = "zig".into();
        app.on_msg(Msg::Feed {
            tab: app.active_tab().id,
            generation: app.active_tab().feed.generation,
            append: false,
            result: Ok(crate::model::Listing {
                posts: vec![first, second],
                after: None,
            }),
        });
        press(&mut app, 'j');
        let feed = screen(&mut app, width, 30);
        assert_eq!(
            feed.contains("Community summary marker"),
            split,
            "community rows should remain beside the active feed: {feed}"
        );
        assert_eq!(app.current_post().unwrap().id, "nested_second");
        press(&mut app, 'l');
        let post = screen(&mut app, width, 30);
        assert!(app.post_view.is_some());
        assert!(
            !post.contains("Community summary marker"),
            "the feed replaces communities beside post detail: {post}"
        );
        press(&mut app, 'h');
        let back = screen(&mut app, width, 30);
        assert_eq!(app.focus, crate::app::Focus::List);
        if split {
            let generation = app
                .post_view
                .as_ref()
                .expect("first h keeps detail")
                .generation;
            assert!(
                !back.contains("Community summary marker"),
                "first h focuses posts without shifting the panes: {back}"
            );
            press(&mut app, 'l');
            assert_eq!(app.focus, crate::app::Focus::Post);
            assert_eq!(app.post_view.as_ref().unwrap().generation, generation);
            press(&mut app, 'h');
            press(&mut app, 'h');
        }
        let back = screen(&mut app, width, 30);
        assert!(
            app.post_view.is_none(),
            "leaving the post list restores the community/feed pair"
        );
        assert_eq!(app.focus, crate::app::Focus::List);
        assert_eq!(back.contains("Community summary marker"), split, "{back}");
        assert_eq!(app.current_post().unwrap().id, "nested_second");
        press(&mut app, 'h');
        assert_eq!(app.active, crate::app::SEARCH_TAB);
        assert_eq!(app.search.selected, 2);
    }
}

#[test]
fn search_revision_icons_should_leave_a_gap_before_the_title() {
    use crate::reddit::experience::{Entry, Page, Query, Response};
    let mut app = app(&[]);
    app.image_picker = kitty();
    app.show_images = true;
    app.open_search(None);
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    app.on_msg(Msg::Search {
        generation: app.search.discovered_generation(),
        result: Ok(Response::Page(Page {
            title: "Discover".into(),
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
    });
    app.on_msg(Msg::Icon {
        subreddit: "zig".into(),
        result: Ok(Some("/icon/zig.png".into())),
    });
    let actual = screen(&mut app, 120, 24);
    let row = actual.lines().find(|line| line.contains("r/zig")).unwrap();
    let label = row[..row.find("r/zig").unwrap()].width();
    let icon_right = usize::from(app.sidebar_width() + PANE_MARGIN.horizontal + 2 + ICON_COLS);
    assert_eq!(
        label.saturating_sub(icon_right),
        1,
        "icon and title need a clear gap: {actual}"
    );
    assert!(
        app.thumbs.contains_key("/icon/zig.png"),
        "a community row draws its icon"
    );
}

#[test]
fn reported_explore_should_fill_its_pane_and_wrap_descriptions() {
    use crate::reddit::experience::{Entry, Page, Query, Response};
    let mut app = app(&[]);
    app.open_search(None);
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    app.on_msg(Msg::Search {
        generation: app.search.discovered_generation(),
        result: Ok(Response::Page(Page {
            title: "Discover".into(),
            text: String::new(),
            url: "/".into(),
            entries: vec![Entry {
                title: "r/community_with_a_long_name".into(),
                body:
                    "A description with enough words to wrap across several lines ending in papaya"
                        .into(),
                query: Some(Query::Community("community_with_a_long_name".into())),
                target: None,
                post: None,
                icon: Default::default(),
            }],
            after: Some("next".into()),
        })),
    });
    let actual = screen(&mut app, 62, 36);
    assert!(
        app.search.discovered.loading(),
        "an underfilled Explore pane should load the next page"
    );
    assert!(
        actual.contains("papaya"),
        "the description must fit by wrapping: {actual}"
    );
    let generation = app.search.discovered_generation();
    screen(&mut app, 62, 36);
    assert_eq!(
        app.search.discovered_generation(),
        generation,
        "one page at a time"
    );
    app.on_msg(Msg::Search {
        generation,
        result: Ok(Response::Page(Page {
            title: "Discover".into(),
            text: String::new(),
            url: "/".into(),
            entries: Vec::new(),
            after: Some("next".into()),
        })),
    });
    screen(&mut app, 62, 36);
    assert!(
        !app.search.discovered.loading(),
        "a repeated cursor must stop automatic paging"
    );
}

#[test]
fn an_empty_search_answer_should_be_asked_again_before_it_counts_as_no_matches() {
    use crate::reddit::experience::{Entry, Page, Query, Response};
    let answer = |app: &mut App, names: &[&str]| {
        app.on_msg(Msg::Search {
            generation: app.search.results_generation(),
            result: Ok(Response::Page(Page {
                title: "Communities".into(),
                text: String::new(),
                url: "/".into(),
                after: None,
                entries: names
                    .iter()
                    .map(|name| Entry {
                        title: format!("r/{name}"),
                        body: String::new(),
                        query: Some(Query::Community((*name).into())),
                        icon: Default::default(),
                        target: None,
                        post: None,
                    })
                    .collect(),
            })),
        });
    };
    let retried = |app: &mut App, wait: u64| {
        let generation = app.search.results_generation();
        app.tick_at(std::time::Instant::now() + std::time::Duration::from_millis(wait));
        assert_ne!(app.search.results_generation(), generation, "asked again");
    };
    let searched = || {
        let mut app = app(&[]);
        app.open_search(Some(crate::reddit::experience::SearchKind::Communities));
        crate::keys::paste(&mut app, "jon");
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        app.tick_at(std::time::Instant::now() + crate::app::search::DEBOUNCE);
        app
    };

    let mut app = searched();
    answer(&mut app, &[]);
    let waiting = screen(&mut app, 200, 24);
    assert!(waiting.contains("asking again (1 of 2)"), "{waiting}");
    assert!(!waiting.contains("No matches"), "{waiting}");
    retried(&mut app, 1050);
    answer(&mut app, &["jon"]);
    assert!(screen(&mut app, 200, 24).contains("r/jon"));

    let mut app = searched();
    answer(&mut app, &[]);
    retried(&mut app, 1050);
    answer(&mut app, &[]);
    assert!(screen(&mut app, 200, 24).contains("asking again (2 of 2)"));
    retried(&mut app, 3050);
    answer(&mut app, &[]);
    assert!(screen(&mut app, 200, 24).contains("No matches"));

    // Typing another query drops the old query's ask-again.
    let mut app = searched();
    answer(&mut app, &[]);
    press(&mut app, '/');
    crate::keys::paste(&mut app, "x");
    assert!(app.search.empty_retry().is_none());

    // Away from Search the ask-again waits, and runs again on return.
    let mut app = searched();
    answer(&mut app, &[]);
    app.switch_tab(0);
    let away = app.search.results_generation();
    app.tick_at(std::time::Instant::now() + std::time::Duration::from_millis(1050));
    assert_eq!(app.search.results_generation(), away, "asked while away");
    app.switch_tab(crate::app::SEARCH_TAB);
    app.tick_at(std::time::Instant::now() + crate::app::NAV_SETTLE);
    assert_ne!(
        app.search.results_generation(),
        away,
        "asked again on return"
    );
}
