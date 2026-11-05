use super::*;

#[test]
fn search_tab_rows_should_keep_the_preview_beside_them_at_common_widths() {
    let mut app = app(&[]);
    app.open_search(None);
    app.resize(160);
    assert!(
        help::entries(&app)
            .iter()
            .any(|(_, text)| text == "Move the cursor"),
        "typing help lists cursor keys"
    );
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);

    let shown = screen(&mut app, 160, 20);

    assert!(shown.contains("Enter opens"), "{shown}");
    let bar = shown
        .lines()
        .find(|line| line.contains("Search posts"))
        .unwrap();
    assert!(
        bar.contains("Posts │ Communities │ People"),
        "query and kinds share one bar above the list and preview: {shown}"
    );

    let target = app.search.selected_row().unwrap().target;
    app.search.preview.target = Some(target);
    app.search.preview.loading = false;
    let mut hidden = post_with("hidden", Media::None);
    hidden.title = "spoilers ahead".into();
    app.config.filters.keywords = vec!["spoilers".into()];
    app.search.preview.posts = vec![hidden, post_with("kept", Media::None)];
    app.search.preview.error = Some("offline".into());
    let shown = screen(&mut app, 160, 20);
    assert!(
        shown.contains("refresh failed") && shown.contains("kept"),
        "a failed refresh keeps the cached posts: {shown}"
    );

    press(&mut app, '/');
    key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    crate::keys::paste(&mut app, "zzzz");
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    let searching = screen(&mut app, 160, 20);
    assert!(
        searching.contains("Searching Reddit") && !searching.contains("j/k choose"),
        "no rows, no row hint beside them: {searching}"
    );
    assert!(
        !shown.contains("spoilers ahead"),
        "hide filters apply to the preview: {shown}"
    );
}

#[test]
fn ui_audit_a_narrow_search_tab_should_keep_the_query_above_its_kinds() {
    let mut app = form_app("Search posts across Reddit");
    assert_eq!(app.active, crate::app::SEARCH_TAB);
    let actual = screen(&mut app, 32, 12);
    let field = actual.find("Search posts").expect("query field");
    assert!(field < actual.find("Posts").expect("kinds"), "{actual}");
}

#[test]
fn search_tab_reload_should_show_loading_and_failures() {
    use crate::reddit::experience::{Entry, Page, Query, Response, SearchKind};
    let page = |entries| {
        Ok(Response::Page(Page {
            title: "Communities".into(),
            text: String::new(),
            entries,
            after: None,
            url: "/".into(),
        }))
    };
    let mut app = app(&[]);
    app.resize(130);
    app.open_search(Some(SearchKind::Communities));
    for editing in [false, true] {
        if editing {
            app.edit_search();
        } else {
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        }
        app.on_msg(Msg::Search {
            generation: app.search.discovered_generation(),
            result: Err("Reddit request timed out; retry when ready".into()),
        });
        for width in [70, 130] {
            let failed = screen(&mut app, width, 24);
            assert!(failed.contains("timed out"), "{failed}");
            assert!(failed.contains("r retries"), "{failed}");
            if editing {
                assert!(failed.contains("Esc then r"), "{failed}");
            }
        }
        if editing {
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        }
        let failed = app.search.discovered_generation();
        press(&mut app, 'r');
        assert!(app.search.discovered.loading);
        assert_ne!(app.search.discovered_generation(), failed);
        assert!(
            app.search.text().is_empty(),
            "retry must not type into the query"
        );
    }
    app.edit_search();
    crate::keys::paste(&mut app, "rust");
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    let entries = vec![Entry {
        icon: Default::default(),
        title: "r/rust".into(),
        body: String::new(),
        query: Some(Query::Community("rust".into())),
        target: None,
        post: None,
    }];
    app.on_msg(Msg::Search {
        generation: app.search.results_generation(),
        result: page(entries.clone()),
    });
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(screen(&mut app, 130, 24).contains("r/rust"));

    press(&mut app, 'r');
    let loading = screen(&mut app, 130, 24);
    assert!(loading.contains("Searching Reddit"), "{loading}");
    app.on_msg(Msg::Search {
        generation: app.search.results_generation(),
        result: Err("offline".into()),
    });
    let failed = screen(&mut app, 130, 24);
    assert!(
        failed.contains("Search failed") && failed.contains("r to retry"),
        "{failed}"
    );

    press(&mut app, 'r');
    app.on_msg(Msg::Search {
        generation: app.search.results_generation(),
        result: page(entries),
    });
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    let rows = screen(&mut app, 130, 24);
    assert!(
        rows.contains("r/rust") && !rows.contains("Enter opens"),
        "{rows}"
    );
}

#[test]
fn search_tab_preview_should_draw_feed_cards_with_pictures() {
    let mut app = app(&[]);
    app.image_picker = kitty();
    app.show_images = true;
    app.open_search(None);
    app.resize(160);
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    app.search.preview.target = Some(app.search.selected_row().unwrap().target);
    app.search.preview.loading = false;
    app.search.preview.posts = vec![post_with(
        "pic",
        Media::Image {
            url: "https://i.redd.it/pic.jpeg".into(),
            preview: None,
        },
    )];

    let shown = screen(&mut app, 160, 24);

    assert!(
        app.card_images.contains_key("https://i.redd.it/pic.jpeg"),
        "the preview loads the card's picture"
    );
    assert!(shown.contains("comments"), "{shown}");
    assert_eq!(
        shown.matches('▌').count(),
        1,
        "only the rows mark a selection, not the preview's cards: {shown}"
    );
}
