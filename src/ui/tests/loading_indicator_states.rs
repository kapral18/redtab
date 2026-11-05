use super::*;

#[test]
fn when_disconnected_the_empty_feed_should_invite_connecting_without_loading_activity() {
    let mut app = app(&[]);
    app.reddit = Reddit::disconnected(false).unwrap();
    app.health = Health::SignedOut;
    app.active = 0;
    let actual = screen(&mut app, 90, 24);
    assert!(actual.contains("Connect Reddit"), "{actual}");
    assert!(
        actual.contains("Press SPC p a to connect your account"),
        "{actual}"
    );
    assert!(!actual.contains("Loading"), "{actual}");
    assert!(loading_activity(&app, &VisibleLoaders::default()).is_none());
}

#[test]
fn when_account_loading_is_pending_it_should_show_a_loading_indicator() {
    let mut app = app(&[]);
    app.auth.phase = crate::auth::Phase::LoadingAccounts;
    app.overlay = Some(Overlay::Auth);
    let actual = screen(&mut app, 60, 12);
    assert!(actual.contains("Loading saved accounts"), "{actual}");
    assert!(actual.contains("Esc cancels"), "{actual}");
}

#[test]
fn when_connection_loading_has_no_active_account_it_should_not_offer_disconnect() {
    for phase in [
        crate::auth::Phase::Discovering,
        crate::auth::Phase::Importing,
    ] {
        for connected in [false, true] {
            let mut app = app(&[]);
            app.auth.phase = phase;
            app.auth.active_id = connected.then(|| "current".into());
            app.overlay = Some(Overlay::Auth);

            let actual = screen(&mut app, 76, 22);

            assert_eq!(
                actual.contains("d Remove from redtab"),
                connected,
                "{actual}"
            );
            assert!(actual.contains("Esc cancels"), "{actual}");
        }
    }
}

#[test]
fn islands_loading_or_joining_above_keep_the_selected_island_in_place() {
    let mut app = app(&["go", "rust", "vim"]);
    app.switch_tab(crate::app::ALL_SUBS_TAB);
    let titled = |id: &str, sub: &str| {
        let mut post = post_with(id, Media::None);
        post.subreddit = sub.into();
        post.title = format!("Title {id}");
        post
    };
    let answer = |app: &App, sub: &str, ids: &[&str]| Msg::Group {
        generation: app
            .all_subs
            .groups
            .iter()
            .find(|group| group.subreddit == sub)
            .unwrap()
            .generation,
        subreddit: sub.into(),
        result: Ok(crate::model::Listing {
            posts: ids.iter().map(|id| titled(id, sub)).collect(),
            after: None,
        }),
    };
    let row = |shown: &str| shown.lines().position(|line| line.contains("Title v1"));
    app.on_msg(answer(&app, "go", &["g1"]));
    app.on_msg(answer(&app, "rust", &["r1"]));
    app.on_msg(answer(&app, "vim", &["v1", "v2", "v3"]));
    screen(&mut app, 100, 60);
    app.move_selection(2);
    assert_eq!(app.current_post().unwrap().id, "v1");
    let before = row(&screen(&mut app, 100, 60));
    assert!(before.is_some());

    app.on_msg(answer(&app, "go", &["g1", "g2", "g3"]));
    assert_eq!(row(&screen(&mut app, 100, 60)), before);

    app.on_msg(Msg::Subscriptions {
        generation: 0,
        result: Ok(["ada", "go", "rust", "vim"]
            .into_iter()
            .map(|name| crate::model::Subscription::Community {
                name: name.into(),
                icon: crate::model::SubscriptionIcon::Unknown,
            })
            .collect()),
    });
    let shown = screen(&mut app, 100, 60);

    assert_eq!(app.current_post().unwrap().id, "v1");
    assert_eq!(row(&shown), before, "{shown}");
}

#[test]
fn when_subscriptions_are_loading_empty_or_failed_all_subs_should_explain_the_state() {
    let mut app = app(&[]);
    app.all_subs.loading = true;
    assert!(screen(&mut app, 80, 24).contains("Loading subscriptions"));
    app.all_subs.loading = false;
    assert!(screen(&mut app, 80, 24).contains("No joined communities"));
    app.all_subs.error = Some("offline".into());
    let failed = screen(&mut app, 80, 24);
    assert!(
        failed.contains("Could not load subscriptions") && failed.contains("press r"),
        "{failed}"
    );
}

#[test]
fn when_subscriptions_first_load_the_initial_loader_should_use_the_shared_spinner() {
    let mut app = app(&[]);
    app.all_subs.loading = true;

    let actual = screen(&mut app, 100, 30);

    assert!(
        actual.contains("Loading subscriptions") && actual.contains('⠋'),
        "{actual}"
    );
}

#[test]
fn when_a_card_refreshes_its_image_it_should_show_activity_while_preserving_pixels() {
    let mut app = card_with_decoded_image();
    screen(&mut app, 100, 24);
    let url = "https://i.redd.it/p1.jpeg";
    let Some(CardImageSlot::Ready { image, protocol }) = app.card_images.remove(url) else {
        panic!("loaded card image")
    };
    app.card_images.insert(
        url.into(),
        CardImageSlot::Cached {
            image,
            protocol,
            refreshing: true,
        },
    );

    let actual = first_screen(&mut app, 100, 24);

    assert!(actual.contains('\u{10EEEE}'), "{actual}");
    assert!(!actual.contains("preparing image"), "{actual}");
    assert!(
        actual.lines().last().unwrap().contains("⠋ images"),
        "{actual}"
    );
}

#[test]
fn video_frame_preparation_should_not_show_image_loading_activity() {
    let mut app = open_video_post();
    let url = app.reddit.media_url(VIDEO).unwrap();
    let frame = || video::Frame {
        width: 16,
        height: 9,
        pixels: video::Pixels::Raw(std::sync::Arc::new(image::DynamicImage::new_rgb8(16, 9))),
        same_as_first: false,
    };
    app.video = Some(video::Player::showing(&url, frame()));
    for _ in 0..2 {
        let actual = first_screen(&mut app, 100, 40);
        assert!(app.card_renderer.pending(), "keep graphics backpressure");
        assert!(
            !app.images_loading(),
            "normal video preparation is playback"
        );
        assert!(
            !actual.lines().last().unwrap().contains("images"),
            "{actual}"
        );
        app.card_renderer.wait_for_completion();
        assert!(
            app.card_renderer.pending(),
            "unpublished tiles still need polling"
        );
        assert!(
            !app.images_loading(),
            "completed video tiles are playback too"
        );
        assert!(app.card_renderer.poll());
        assert!(!app.card_renderer.pending());
        app.video.as_mut().unwrap().show(frame());
    }
}

#[test]
fn when_split_views_load_independently_each_should_keep_its_own_progress() {
    for replies in [false, true] {
        let mut app = feed_with_next_page(ListView::Compact);
        app.open_current_post();
        let view = app.post_view.as_ref().unwrap();
        app.on_msg(Msg::Post {
            generation: view.generation,
            result: Ok(Box::new(PostPage {
                post: view.post.clone(),
                body: paragraphs("paragraph", 1..=60),
                comments: vec![reply("c".into())],
            })),
        });
        app.load_feed(4, true);
        if replies {
            let view = app.post_view.as_mut().unwrap();
            view.page.as_mut().unwrap().comments[0].more = Some(crate::model::MoreReplies {
                count: Some(2),
                cursor: "next".into(),
            });
            view.loading_more.insert("c".into());
        } else {
            app.reload();
        }
        let actual = screen(&mut app, 158, 20);
        assert!(actual.contains("Loading posts"), "{actual}");
        let activity = if replies {
            "⠋ replies"
        } else {
            "⠋ refresh"
        };
        assert!(
            actual.lines().last().unwrap().contains(activity),
            "{actual}"
        );
    }

    let mut app = feed_with_next_page(ListView::Card);
    app.tabs[4]
        .feed
        .posts
        .extend((0..8).map(|i| post_with(&format!("post{i}"), Media::None)));
    app.open_current_post();
    app.load_feed(4, true);
    let actual = screen(&mut app, 158, 20);
    assert!(actual.contains("Loading post"), "{actual}");
    assert!(
        actual
            .lines()
            .last()
            .unwrap()
            .contains("⠋ Loading more posts"),
        "{actual}"
    );
    let view = app.post_view.as_ref().unwrap();
    app.on_msg(Msg::Post {
        generation: view.generation,
        result: Ok(Box::new(PostPage {
            post: view.post.clone(),
            body: "Retained detail".into(),
            comments: vec![],
        })),
    });
    app.reload();
    for (focus, activity) in [
        (Focus::List, "⠋ Loading more posts"),
        (Focus::Post, "⠋ refresh"),
    ] {
        app.focus = focus;
        let actual = screen(&mut app, 158, 20);
        assert!(
            actual.lines().last().unwrap().contains(activity),
            "{actual}"
        );
    }
}
