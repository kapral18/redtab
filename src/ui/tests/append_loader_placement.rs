use super::*;

#[test]
fn when_a_collection_loads_more_it_should_use_the_shared_append_loader() {
    use crate::app::experience::Screen;
    use crate::reddit::experience::{Entry, Page, Query, Request};
    let mut app = app(&[]);
    app.overlay = Some(Overlay::Experience);
    app.experience.screens.push(Screen::Browse {
        pagination: Default::default(),
        query: Query::Multis,
        page: Page {
            title: "Custom feeds".into(),
            text: String::new(),
            entries: vec![Entry {
                icon: Default::default(),
                title: "Existing feed".into(),
                body: String::new(),
                target: None,
                query: None,
                post: None,
            }],
            after: Some("next".into()),
            url: "/api/multi/mine".into(),
        },
        selected: 0,
        scroll: 0,
    });
    app.experience.pending = Some((
        91,
        Request::Read {
            query: Query::Multis,
            after: Some("next".into()),
        },
    ));

    let actual = screen(&mut app, 100, 30);

    assert!(actual.contains("Existing feed"), "{actual}");
    assert!(
        actual.contains("━") && actual.contains("Loading results"),
        "{actual}"
    );
    assert!(matches!(
        app.experience.screens.last(),
        Some(Screen::Browse {
            selected: 0,
            scroll: 0,
            ..
        })
    ));
    app.on_experience(
        91,
        Err("offline".into()),
        crate::cache::Freshness {
            fetched_at: crate::app::now(),
            source: Default::default(),
        },
    );
    for (width, height) in [(24, 10), (40, 12), (100, 30)] {
        let actual = screen(&mut app, width, height);
        assert!(actual.contains("Existing feed"), "{actual}");
        assert!(actual.contains("r retry"), "{actual}");
    }
    press(&mut app, 'r');
    assert!(
        matches!(&app.experience.pending, Some((_, Request::Read { after: Some(cursor), .. })) if cursor == "next")
    );
}

#[test]
fn when_replies_load_more_the_compact_loader_should_keep_the_comment_rows_stable() {
    let (mut app, _) = open_long_post(|_| {});
    let view = app.post_view.as_mut().unwrap();
    let page = view.page.as_mut().unwrap();
    page.body.clear();
    page.comments[0].more = Some(crate::model::MoreReplies {
        count: Some(2),
        cursor: "next".into(),
    });
    screen(&mut app, 100, 30);
    let rows = app.post_view.as_ref().unwrap().rendered_document.len();
    app.post_view
        .as_mut()
        .unwrap()
        .loading_more
        .insert("c".into());

    let actual = screen(&mut app, 100, 30);

    assert!(
        actual.contains("Loading replies") && actual.contains("━"),
        "{actual}"
    );
    assert_eq!(
        app.post_view.as_ref().unwrap().rendered_document.len(),
        rows
    );
}

#[test]
fn when_images_are_pending_cards_and_embedded_pictures_should_share_the_media_spinner() {
    let mut card = card_feed_app(kitty());
    let mut post = picture_comment("https://preview.redd.it/pending.png");

    let card_view = first_screen(&mut card, 100, 30);
    let post_view = screen(&mut post, 100, 30);

    assert!(
        card_view.contains('⠋') && card_view.contains("loading image"),
        "{card_view}"
    );
    assert!(
        post_view.contains('⠋') && post_view.contains("loading image"),
        "{post_view}"
    );
}

#[test]
fn when_initial_or_append_loading_waits_for_cooldown_the_loader_should_explain_the_wait() {
    use crate::reddit::tests::{Fixture, Reply};
    use std::time::{Duration, Instant};
    for append in [false, true] {
        let fixture = Fixture::new(|_| Reply {
            status: 429,
            body: r#"{"kind":"Listing","data":{"children":[],"after":null}}"#.into(),
            headers: vec![("Retry-After".into(), "30".into())],
        });
        let reddit = fixture.reddit();
        assert!(reddit.search("s", "first", None).is_err());
        let mut app = app_with_client(&["s"], reddit.clone());
        app.switch_tab(4);
        if append {
            app.on_msg(Msg::Feed {
                tab: app.active_tab().id,
                generation: app.active_tab().feed.generation,
                append: false,
                result: Ok(crate::model::Listing {
                    posts: vec![post_with("existing", Media::None)],
                    after: Some("next".into()),
                }),
            });
            app.load_feed(4, true);
        }
        app.status = None;
        let deadline = Instant::now() + Duration::from_secs(1);
        while reddit.pause().is_none() {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        app.tick();

        let actual = screen(&mut app, 100, 24);
        reddit.retire();

        assert!(actual.contains("Waiting for Reddit"), "{actual}");
        assert!(actual.contains("HTTP 429"), "{actual}");
    }
}

#[test]
fn when_an_initial_page_streams_card_and_compact_views_should_keep_the_append_loader() {
    for view in [ListView::Card, ListView::Compact] {
        let mut app = app(&["s"]);
        app.switch_tab(4);
        if app.state.list_view() != view {
            app.state.toggle_list_view();
        }
        let mut post = post_with("arrived", Media::None);
        post.title = "Arrived post".into();
        app.on_msg(Msg::FeedProgress {
            tab: app.active_tab().id,
            generation: app.active_tab().feed.generation,
            post: Box::new(post),
            source: crate::cache::Source::default(),
        });

        let actual = screen(&mut app, 100, 24);

        assert!(
            actual.contains("Arrived post")
                && actual.contains("Loading posts")
                && actual.contains('━'),
            "{actual}"
        );
        assert!(app.active_tab().feed.loading());
        assert!(actual.find("Arrived post").unwrap() < actual.find("Loading posts").unwrap());
    }
}

#[test]
fn scrolling_should_defer_pagination_until_settled_then_show_the_append_loader() {
    let mut app = feed_with_next_page(ListView::Card);
    screen(&mut app, 80, 20);
    assert!(!app.active_tab().feed.loading_more());
    app.scroll_card(1);

    screen(&mut app, 80, 20);
    assert!(!app.active_tab().feed.loading_more());
    app.tick_at(std::time::Instant::now() + crate::app::NAV_SETTLE);
    let actual = screen(&mut app, 80, 20);

    assert!(app.active_tab().feed.loading_more());
    assert!(
        actual.contains("Loading posts") && actual.contains("━"),
        "{actual}"
    );
    assert!(
        !actual.lines().last().unwrap().contains("more posts"),
        "{actual}"
    );
}

#[test]
fn when_a_long_notification_is_visible_the_append_loader_should_remain_visible() {
    let mut app = feed_with_next_page(ListView::Compact);
    app.load_feed(4, true);
    app.info("Checking membership and keeping the current reading position while posts load");

    let actual = screen(&mut app, 80, 20);

    let footer = actual.lines().last().unwrap();
    assert!(footer.contains("Checking"), "{actual}");
    assert!(
        actual.contains("Loading posts") && actual.contains("━"),
        "{actual}"
    );
    assert!(!footer.contains("more posts"), "{actual}");
    assert!(footer.contains("F1"), "{actual}");
}

#[test]
fn when_a_post_with_an_overflow_title_is_pending_its_loader_should_not_overwrite_the_title() {
    for (width, focus) in [(50, Focus::Post), (158, Focus::List)] {
        let (mut app, _) = open_long_post(|_| {});
        app.show_sidebar = false;
        app.focus = focus;
        let view = app.post_view.as_mut().unwrap();
        view.post.title = (0..200).map(|i| format!("Segment{i:02} ")).collect();
        view.page = None.into();
        view.loading = true;
        let actual = screen(&mut app, width, 20);
        let view = app.post_view.as_ref().unwrap();
        for line in view
            .rendered_document
            .iter()
            .skip(view.scroll)
            .take(view.page_height)
        {
            let text = line.to_string();
            if !text.trim().is_empty() {
                let text: String = text.chars().take(40).collect();
                assert!(
                    actual.contains(text.trim_end()),
                    "Missing {text:?}:\n{actual}"
                );
            }
        }
        assert!(
            actual.lines().last().unwrap().contains("⠋ post"),
            "{actual}"
        );
    }
}

#[test]
fn when_a_notification_is_visible_the_card_append_loader_should_remain_visible() {
    let mut app = feed_with_next_page(ListView::Card);
    app.load_feed(4, true);
    app.info("Still browsing");

    let actual = screen(&mut app, 80, 20);

    let footer = actual.lines().last().unwrap();
    assert!(
        actual.contains("Loading posts")
            && !footer.contains("more posts")
            && footer.contains("Still browsing"),
        "{actual}"
    );
}

#[test]
fn when_the_append_position_is_offscreen_pagination_should_use_the_footer() {
    let mut app = feed_with_next_page(ListView::Card);
    app.tabs[4]
        .feed
        .posts
        .extend((0..8).map(|i| post_with(&format!("post{i}"), Media::None)));
    app.load_feed(4, true);

    let actual = screen(&mut app, 80, 20);

    assert!(
        !actual.contains("Loading posts") && !actual.contains("━"),
        "{actual}"
    );
    assert!(
        actual.lines().last().unwrap().contains("⠋ more posts"),
        "{actual}"
    );
}

#[test]
fn when_a_loaded_split_post_has_focus_pagination_should_stay_visible() {
    let mut app = feed_with_next_page(ListView::Compact);
    app.open_current_post();
    let view = app.post_view.as_ref().unwrap();
    app.on_msg(Msg::Post {
        generation: view.generation,
        result: Ok(Box::new(PostPage {
            post: view.post.clone(),
            body: "Loaded detail".into(),
            comments: vec![],
        })),
    });
    app.load_feed(4, true);

    let actual = screen(&mut app, 158, 20);

    assert!(actual.contains("Loaded detail"), "{actual}");
    assert!(app.post_focused());
    assert_eq!(actual.matches("Loading posts").count(), 1, "{actual}");
    assert!(
        !actual.lines().last().unwrap().contains("more posts"),
        "{actual}"
    );
}
