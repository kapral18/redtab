use super::*;

#[test]
fn existing_feed_updates_apply_while_only_new_posts_require_refresh() {
    for view in [
        crate::state::ListView::Card,
        crate::state::ListView::Compact,
    ] {
        let mut app = app(&["AcademicUAP"]);
        if app.state.list_view() != view {
            app.state.toggle_list_view();
        }
        app.toggle_sidebar();
        app.switch_tab(4);
        app.config.filters.authors.push("blocked".into());
        let tab = app.active_tab().id;
        let answer = |generation, posts| Msg::Feed {
            tab,
            generation,
            append: false,
            result: Ok(crate::model::Listing { posts, after: None }),
        };
        let mut first = post_with(
            "first",
            Media::Video {
                url: Some("https://v.redd.it/video/HLSPlaylist.m3u8?a=old".into()),
                poster: None,
                gif: false,
            },
        );
        first.title = "Existing post".into();
        let second = post_with("second", Media::None);
        let mut hidden = post_with("known-hidden", Media::None);
        hidden.author = "blocked".into();
        app.on_msg(answer(
            app.active_tab().feed.generation,
            vec![first.clone(), second.clone(), hidden.clone()],
        ));
        screen(&mut app, 100, 20);

        first.title = "Updated existing post".into();
        first.score = Some(42);
        first.comments = 12;
        first.media = Media::Video {
            url: Some("https://v.redd.it/video/HLSPlaylist.m3u8?a=new".into()),
            poster: None,
            gif: false,
        };
        hidden.author = "visible".into();
        let mut filtered_new = post_with("filtered-new", Media::None);
        filtered_new.author = "blocked".into();
        app.load_feed(4, false);
        app.on_msg(answer(
            app.active_tab().feed.generation,
            vec![
                second.clone(),
                first.clone(),
                hidden.clone(),
                filtered_new.clone(),
            ],
        ));
        let updated = screen(&mut app, 100, 20);

        assert!(updated.contains("Updated existing post"), "{updated}");
        assert!(!updated.contains("r refresh"), "{updated}");
        // The untouched top stays the top as the list reorders.
        assert_eq!(app.current_post().unwrap().id, "second");
        assert_eq!(app.active_tab().feed.posts[0].id, "second");
        let updated_first = &app.active_tab().feed.posts[1];
        assert_eq!(updated_first.id, "first");
        assert_eq!(updated_first.score, Some(42));
        assert_eq!(updated_first.comments, 12);
        assert_eq!(updated_first.media, first.media);
        assert!(
            app.config
                .visible(&app.active_tab().feed.posts)
                .any(|post| post.id == "known-hidden")
        );

        first.title = "Live existing edit".into();
        first.score = Some(84);
        let mut incoming = post_with("new", Media::None);
        incoming.title = "Incoming post".into();
        app.load_feed(4, false);
        app.on_msg(answer(
            app.active_tab().feed.generation,
            vec![
                incoming.clone(),
                second.clone(),
                first.clone(),
                hidden.clone(),
                filtered_new,
            ],
        ));
        let pending = screen(&mut app, 100, 20);

        assert!(pending.contains("New posts · r refresh"), "{pending}");
        assert!(pending.contains("Live existing edit"), "{pending}");
        assert!(!pending.contains("Incoming post"), "{pending}");
        assert_eq!(app.current_post().unwrap().id, "second");
        assert!(
            app.active_tab()
                .feed
                .posts
                .iter()
                .any(|post| post.id == "first" && post.score == Some(84))
        );

        let mut filtered_next = post_with("filtered-next", Media::None);
        filtered_next.author = "blocked".into();
        app.load_feed(4, false);
        app.on_msg(answer(
            app.active_tab().feed.generation,
            vec![incoming, second, first.clone(), hidden, filtered_next],
        ));
        assert!(
            !app.active_tab()
                .feed
                .posts
                .iter()
                .any(|post| post.id == "filtered-new")
        );

        press_keys(&mut app, "r");
        let refreshed = screen(&mut app, 100, 20);
        assert!(!refreshed.contains("r refresh"), "{refreshed}");
        assert!(refreshed.contains("Incoming post"), "{refreshed}");
        assert_eq!(app.current_post().unwrap().id, "new");

        app.load_feed(4, false);
        app.on_msg(answer(app.active_tab().feed.generation, vec![first]));
        let removed = screen(&mut app, 100, 20);
        assert!(!removed.contains("r refresh"), "{removed}");
        assert_eq!(app.current_post().unwrap().id, "first");
        assert_eq!(app.active_tab().feed.posts.len(), 1);
    }
}

#[test]
fn buffered_feed_refresh_shows_the_list_top_without_reordering_pinned_posts() {
    for (view, index) in [
        (crate::state::ListView::Card, 0),
        (crate::state::ListView::Compact, 0),
        (crate::state::ListView::Card, 4),
        (crate::state::ListView::Compact, 4),
    ] {
        for width in [40, 100] {
            let mut app = app(&["rust"]);
            if app.state.list_view() != view {
                app.state.toggle_list_view();
            }
            app.toggle_sidebar();
            app.switch_tab(index);
            let tab = app.active_tab().id;
            let mut first = post_with("old", Media::None);
            first.title = "Existing post".into();
            first.stickied = true;
            let second = post_with("second", Media::None);
            let answer = |generation, posts| Msg::Feed {
                tab,
                generation,
                append: false,
                result: Ok(crate::model::Listing { posts, after: None }),
            };
            app.on_msg(answer(
                app.active_tab().feed.generation,
                vec![first.clone(), second.clone()],
            ));
            let before = screen(&mut app, width, 20);
            let row = before
                .lines()
                .position(|line| line.contains(&first.title))
                .unwrap();
            app.load_feed(index, false);
            let mut next = post_with("new", Media::None);
            next.title = "Incoming post".into();

            app.on_msg(answer(
                app.active_tab().feed.generation,
                vec![first, next.clone(), second],
            ));
            let pending = screen(&mut app, width, 20);

            assert!(pending.contains("New posts · r refresh"), "{pending}");
            assert!(!pending.contains(&next.title), "{pending}");
            assert_eq!(
                pending
                    .lines()
                    .position(|line| line.contains("Existing post")),
                Some(row)
            );
            crate::keys::handle(
                &mut app,
                crossterm::event::KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
            );
            let refreshed = screen(&mut app, width, 20);
            assert!(!refreshed.contains("r refresh"), "{refreshed}");
            assert_eq!(
                app.active_tab()
                    .feed
                    .posts
                    .iter()
                    .map(|post| post.id.as_str())
                    .collect::<Vec<_>>(),
                ["old", "new", "second"]
            );
            assert_eq!(app.current_post().unwrap().id, "old");
            assert!(refreshed.contains("Existing post"), "{refreshed}");
            assert!(refreshed.contains(&next.title), "{refreshed}");
        }
    }
}

#[test]
fn manual_refresh_reveals_arrival_headers_after_fetch_or_buffered_updates() {
    let mut app = two_tall_media_cards();
    let initial = screen(&mut app, 100, 20);
    let title_row = initial
        .lines()
        .position(|line| line.contains("Full width image"))
        .unwrap();
    app.scroll_card(8);
    screen(&mut app, 100, 20);
    let mut posts = app.tabs[4].feed.posts.clone();
    let mut fetched = post_with(
        "inserted",
        Media::Image {
            url: "https://i.redd.it/fetched.jpeg".into(),
            preview: None,
        },
    );
    fetched.title = "Fetched arrival".into();
    posts.insert(0, fetched);
    app.reload();

    app.on_msg(Msg::Feed {
        tab: app.active_tab().id,
        generation: app.active_tab().feed.generation,
        append: false,
        result: Ok(crate::model::Listing { posts, after: None }),
    });
    let refreshed = screen(&mut app, 100, 20);

    assert_eq!(app.current_post().unwrap().id, "inserted");
    assert_eq!(
        app.active_tab().feed.cards.post.as_deref(),
        Some("inserted")
    );
    assert_eq!(app.active_tab().feed.cards.line, 0);
    assert_eq!(
        refreshed
            .lines()
            .position(|line| line.contains("Fetched arrival")),
        Some(title_row)
    );
    app.on_msg(Msg::Image {
        url: "https://i.redd.it/fetched.jpeg".into(),
        result: Ok(image::DynamicImage::new_rgb8(320, 640).into()),
    });
    let loaded = screen(&mut app, 100, 20);
    assert_eq!(app.active_tab().feed.cards.line, 0);
    assert_eq!(
        loaded
            .lines()
            .position(|line| line.contains("Fetched arrival")),
        Some(title_row)
    );

    app.move_selection(1);
    screen(&mut app, 100, 20);
    app.scroll_card(8);
    screen(&mut app, 100, 20);

    let mut posts = app.active_tab().feed.posts.clone();
    let mut incoming = post_with("newer", Media::None);
    incoming.title = "Newest post".into();
    posts.insert(0, incoming);
    app.load_feed(app.active, false);
    app.on_msg(Msg::Feed {
        tab: app.active_tab().id,
        generation: app.active_tab().feed.generation,
        append: false,
        result: Ok(crate::model::Listing { posts, after: None }),
    });
    screen(&mut app, 100, 20);
    assert_eq!(app.active_tab().feed.cards.post.as_deref(), Some("p1"));
    assert_eq!(app.active_tab().feed.cards.line, 8);

    press_keys(&mut app, "r");
    let refreshed = screen(&mut app, 100, 20);

    assert_eq!(app.current_post().unwrap().id, "newer");
    assert_eq!(app.active_tab().feed.cards.post.as_deref(), Some("newer"));
    assert_eq!(app.active_tab().feed.cards.line, 0);
    assert_eq!(
        refreshed
            .lines()
            .position(|line| line.contains("Newest post")),
        Some(title_row)
    );
}
