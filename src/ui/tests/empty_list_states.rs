use super::*;

#[test]
fn ui_audit_empty_jump_results_should_explain_the_empty_list() {
    let mut app = app(&[]);
    app.overlay = Some(Overlay::Picker(crate::app::PickerState {
        input: "not a reddit name!".into(),
        selected: 0,
    }));
    let actual = screen(&mut app, 80, 20);
    assert!(actual.contains("No matching"), "{actual}");
}

#[test]
fn unclassified_request_denial_should_keep_cached_posts_and_offer_retry_when_empty() {
    let mut app = denied_community(AccessDenied::Forbidden);
    let actual = screen(&mut app, 100, 24);
    assert!(app.current_post().is_some(), "{actual}");
    assert!(
        !actual.contains("Access restricted") && !actual.contains("Private community"),
        "{actual}"
    );
    let current = app.active;
    app.tabs[current].feed.posts.clear();
    let actual = screen(&mut app, 100, 24);
    assert!(
        actual.contains("Could not load posts") && actual.contains("retry"),
        "{actual}"
    );
    assert!(
        !actual.contains("Access restricted") && !actual.contains("Private community"),
        "{actual}"
    );
}

#[test]
fn when_an_empty_feed_has_a_failed_next_page_it_should_offer_the_cursor_retry_key() {
    let mut app = app(&["s"]);
    app.switch_tab(4);
    let tab = app.active_tab().id;
    app.on_msg(Msg::Feed {
        tab,
        generation: app.active_tab().feed.generation,
        append: false,
        result: Ok(crate::model::Listing {
            posts: vec![],
            after: Some("next".into()),
        }),
    });
    app.load_feed(4, true);
    app.on_msg(Msg::Feed {
        tab,
        generation: app.active_tab().feed.generation,
        append: true,
        result: Err("failed continuation".into()),
    });

    let actual = screen(&mut app, 100, 24);

    assert!(
        actual.contains("Could not load more posts") && actual.contains("press r to retry"),
        "{actual}"
    );
}

#[test]
fn an_empty_feed_says_whether_the_config_hid_the_posts_or_a_search_found_none() {
    let mut app = feed_app(Picker::halfblocks());
    let answer = |app: &mut App, posts: Vec<Post>| {
        let tab = app.tabs[4].id;
        let generation = app.tabs[4].feed.generation;
        app.on_msg(Msg::Feed {
            tab,
            generation,
            append: false,
            result: Ok(crate::model::Listing { posts, after: None }),
        });
    };

    app.search("first".into());
    answer(&mut app, Vec::new());
    let no_results = screen(&mut app, 100, 20);
    app.search("second".into());
    let waiting = screen(&mut app, 100, 20);
    let mut nsfw = post_with("n1", Media::None);
    nsfw.nsfw = true;
    answer(&mut app, vec![nsfw]);
    let hidden = screen(&mut app, 100, 20);

    assert!(no_results.contains("No results"), "{no_results}");
    assert!(!no_results.contains("try another sort"), "{no_results}");
    // The first search's empty answer says nothing about the second one.
    assert!(waiting.contains("Loading"), "{waiting}");
    assert!(!waiting.contains("No results"), "{waiting}");
    assert!(hidden.contains("Every loaded post is hidden"), "{hidden}");
}
