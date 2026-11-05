use super::*;

#[test]
fn live_reorders_keep_the_selected_card_selected_and_visible() {
    let mut app = app(&["rust"]);
    if app.state.list_view() != crate::state::ListView::Card {
        app.state.toggle_list_view();
    }
    app.toggle_sidebar();
    app.switch_tab(4);
    let tab = app.active_tab().id;
    let posts: Vec<_> = (0..8)
        .map(|n| {
            let mut post = post_with(&format!("p{n}"), Media::None);
            post.title = format!("Post number {n}");
            post
        })
        .collect();
    let answer = |generation, posts| Msg::Feed {
        tab,
        generation,
        append: false,
        result: Ok(crate::model::Listing { posts, after: None }),
    };
    app.on_msg(answer(app.active_tab().feed.generation, posts.clone()));
    screen(&mut app, 100, 20);
    app.move_selection(1);
    screen(&mut app, 100, 20);
    assert_eq!(app.current_post().unwrap().id, "p1");

    let mut reordered = posts.clone();
    let selected = reordered.remove(1);
    reordered.push(selected);
    app.load_feed(app.active, false);
    app.on_msg(answer(app.active_tab().feed.generation, reordered));
    let shown = screen(&mut app, 100, 20);

    assert_eq!(app.current_post().unwrap().id, "p1");
    assert!(shown.contains("Post number 1"), "{shown}");
}

#[test]
fn a_card_entering_or_leaving_above_the_selection_keeps_the_selected_card_on_its_row() {
    let mut app = app(&["rust"]);
    if app.state.list_view() != crate::state::ListView::Card {
        app.state.toggle_list_view();
    }
    app.toggle_sidebar();
    app.switch_tab(4);
    let tab = app.active_tab().id;
    let post = |id: &str| {
        let mut post = post_with(id, Media::None);
        post.title = format!("Post {id}");
        post
    };
    let answer = |generation, ids: &[&str]| Msg::Feed {
        tab,
        generation,
        append: false,
        result: Ok(crate::model::Listing {
            posts: ids.iter().map(|id| post(id)).collect(),
            after: None,
        }),
    };
    let row = |shown: &str| shown.lines().position(|line| line.contains("Post p2"));
    app.on_msg(answer(
        app.active_tab().feed.generation,
        &["p0", "p1", "p2", "p3", "p4", "p5", "p6", "p7", "p8", "p9"],
    ));
    screen(&mut app, 100, 30);
    app.move_selection(2);
    let before = row(&screen(&mut app, 100, 30));
    assert!(before.is_some());

    for ids in [
        [
            "p0", "m", "p1", "p2", "p3", "p4", "p5", "p6", "p7", "p8", "p9",
        ]
        .as_slice(),
        &["p0", "p1", "p2", "p3", "p4", "p5", "p6", "p7", "p8", "p9"],
    ] {
        app.load_feed(app.active, false);
        app.on_msg(answer(app.active_tab().feed.generation, ids));
        let shown = screen(&mut app, 100, 30);

        assert_eq!(app.current_post().unwrap().id, "p2");
        assert_eq!(row(&shown), before, "{shown}");
    }
}

#[test]
fn shrinking_the_terminal_keeps_a_low_selected_card_selected_and_visible() {
    let mut app = app(&["rust"]);
    if app.state.list_view() != crate::state::ListView::Card {
        app.state.toggle_list_view();
    }
    app.toggle_sidebar();
    app.switch_tab(4);
    let tab = app.active_tab().id;
    app.on_msg(Msg::Feed {
        tab,
        generation: app.active_tab().feed.generation,
        append: false,
        result: Ok(crate::model::Listing {
            posts: (0..10)
                .map(|n| {
                    let mut post = post_with(&format!("p{n}"), Media::None);
                    post.title = format!("Post p{n}");
                    post
                })
                .collect(),
            after: None,
        }),
    });
    screen(&mut app, 100, 40);
    app.move_selection(6);

    for height in [29, 14] {
        assert!(screen(&mut app, 100, 40).contains("Post p6"));
        let shown = screen(&mut app, 100, height);

        assert_eq!(app.current_post().unwrap().id, "p6");
        let lines: Vec<_> = shown.lines().collect();
        let title = lines.iter().position(|line| line.contains("Post p6"));
        assert!(
            title.is_some_and(|title| lines[title + 1].contains("comments")),
            "{height}: {shown}"
        );
    }
}

#[test]
fn a_redraw_without_input_leaves_the_card_list_in_place() {
    let mut app = app(&["rust"]);
    if app.state.list_view() != crate::state::ListView::Card {
        app.state.toggle_list_view();
    }
    app.toggle_sidebar();
    app.switch_tab(4);
    let tab = app.active_tab().id;
    app.on_msg(Msg::Feed {
        tab,
        generation: app.active_tab().feed.generation,
        append: false,
        result: Ok(crate::model::Listing {
            posts: (0..10)
                .map(|n| {
                    let mut post = post_with(&format!("p{n}"), Media::None);
                    post.title = format!("Post p{n}");
                    post
                })
                .collect(),
            after: None,
        }),
    });
    screen(&mut app, 100, 20);
    app.move_selection(5);
    screen(&mut app, 100, 20);
    app.scroll_card(-2);
    let shown = screen(&mut app, 100, 20);

    assert_eq!(screen(&mut app, 100, 20), shown);
}

#[test]
fn narrowing_the_list_keeps_a_rewrapped_selected_card_whole() {
    let mut app = app(&["rust"]);
    if app.state.list_view() != crate::state::ListView::Card {
        app.state.toggle_list_view();
    }
    app.toggle_sidebar();
    app.switch_tab(4);
    let tab = app.active_tab().id;
    app.on_msg(Msg::Feed {
        tab,
        generation: app.active_tab().feed.generation,
        append: false,
        result: Ok(crate::model::Listing {
            posts: (0..10)
                .map(|n| {
                    let mut post = post_with(&format!("p{n}"), Media::None);
                    post.title = format!(
                        "Post p{n} alpha beta gamma delta epsilon zeta eta theta iota kappa end{n}"
                    );
                    post
                })
                .collect(),
            after: None,
        }),
    });
    screen(&mut app, 160, 30);
    app.move_selection(6);
    screen(&mut app, 160, 30);
    app.scroll_card(-3);
    assert!(screen(&mut app, 160, 30).contains("end6"));

    let shown = screen(&mut app, 40, 30);

    assert_eq!(app.current_post().unwrap().id, "p6");
    let lines: Vec<_> = shown.lines().collect();
    let title = lines.iter().position(|line| line.contains("end6"));
    assert!(
        title.is_some_and(|title| lines[title + 1].contains("comments")),
        "{shown}"
    );
}

#[test]
fn when_view_is_toggled_it_should_keep_the_selected_post_and_restore_cards() {
    let mut app = card_feed_app(Picker::halfblocks());
    app.move_selection(4);
    let state_path = app.state_path().to_path_buf();
    let before = app.current_post().unwrap().id;

    press_keys(&mut app, "zc");
    let compact = screen(&mut app, 100, 20);
    let (saved, warning) = crate::state::State::load(&state_path);
    assert!(warning.is_none());
    assert_eq!(saved.list_view(), ListView::Compact);
    assert!(compact.contains("zc Compact"), "{compact}");
    assert_eq!(app.current_post().unwrap().id, before);

    press_keys(&mut app, "zc");
    let cards = screen(&mut app, 100, 20);
    assert!(cards.contains("zc Card"), "{cards}");
    assert!(cards.contains("p5"), "{cards}");
    assert!(!cards.contains("……"), "{cards}");
    assert_eq!(app.current_post().unwrap().id, before);
}
