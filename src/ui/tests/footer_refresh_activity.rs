use super::*;

#[test]
fn when_jump_covers_the_append_loader_pagination_should_use_the_footer() {
    let mut app = feed_with_next_page(ListView::Compact);
    app.load_feed(4, true);
    app.overlay = Some(Overlay::Picker(crate::app::PickerState::default()));

    let actual = screen(&mut app, 80, 20);

    assert!(!actual.contains("Loading posts"), "{actual}");
    assert!(
        actual.lines().last().unwrap().contains("⠋ more posts"),
        "{actual}"
    );
}

#[test]
fn when_the_append_loader_animates_cached_graphics_should_not_be_prepared_or_uploaded_again() {
    let mut app = card_with_image(640, 320);
    screen(&mut app, 100, 40);
    app.tabs[4].feed.after = Some("next".into());
    app.load_feed(4, true);
    screen(&mut app, 100, 40);
    app.loading_frame = 1;

    let actual = first_screen(&mut app, 100, 40);

    assert!(
        actual.contains("Loading posts") && actual.contains("━"),
        "{actual}"
    );
    assert!(actual.contains('\u{10EEEE}'));
    assert!(
        !actual.contains("_G"),
        "warm graphics should not upload again"
    );
    assert!(
        !app.card_renderer.pending(),
        "warm graphics should not queue encoding"
    );
}

#[test]
fn when_a_new_query_waits_in_a_narrow_pane_its_initial_loader_should_remain_visible() {
    let mut app = feed_with_next_page(ListView::Card);
    app.search("a query long enough to fill the header".into());

    let actual = screen(&mut app, 50, 12);

    assert!(
        actual.contains("⠋ Loading posts") && !actual.lines().last().unwrap().contains("posts"),
        "{actual}"
    );
}

#[test]
fn when_a_page_finishes_it_should_remove_the_loading_boundary() {
    let mut app = feed_with_next_page(ListView::Card);
    app.load_feed(4, true);
    app.on_msg(Msg::Feed {
        tab: app.active_tab().id,
        generation: app.active_tab().feed.generation,
        append: true,
        result: Ok(crate::model::Listing {
            posts: vec![post_with("next", Media::None)],
            after: None,
        }),
    });

    let actual = screen(&mut app, 80, 20);

    assert!(actual.contains("next"), "{actual}");
    assert!(!actual.contains("more posts"), "{actual}");
    assert!(
        !actual.contains("Loading posts") && !actual.contains("━"),
        "{actual}"
    );
}

#[test]
fn when_a_page_fails_it_should_replace_loading_with_the_retry_action() {
    let mut app = feed_with_next_page(ListView::Card);
    app.load_feed(4, true);
    app.on_msg(Msg::Feed {
        tab: app.active_tab().id,
        generation: app.active_tab().feed.generation,
        append: true,
        result: Err("offline".into()),
    });

    let actual = screen(&mut app, 80, 20);

    assert!(actual.contains("Next page failed · r retries"), "{actual}");
    assert!(!actual.contains("more posts"), "{actual}");
    assert!(
        !actual.contains("Loading posts") && !actual.contains("━"),
        "{actual}"
    );
}

#[test]
fn when_cached_posts_are_refreshing_a_single_background_loader_should_keep_activity_visible() {
    let mut app = feed_with_next_page(ListView::Card);
    app.load_feed(4, false);
    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Feed {
            tab: app.active_tab().id,
            generation: app.active_tab().feed.generation,
            append: false,
            result: Ok(crate::model::Listing {
                posts: app.active_tab().feed.posts.clone(),
                after: Some("next".into()),
            }),
        }),
        fetched_at: crate::app::now() - 600,
        refreshing: true,
        source: crate::cache::Source {
            cached: true,
            ..Default::default()
        },
    });
    assert!(!app.active_tab().feed.loading() && app.active_tab().feed.refreshing());

    let actual = screen(&mut app, 80, 20);

    assert!(
        actual.contains("Existing post") && actual.contains("Refreshing posts…"),
        "{actual}"
    );
    assert!(
        actual.contains("⠋ Refreshing posts")
            && !actual.lines().last().unwrap().contains("refresh"),
        "{actual}"
    );
}

#[test]
fn when_compact_thumbnails_are_pending_their_reserved_slots_should_show_activity() {
    let mut app = feed_app(kitty());

    let actual = first_screen(&mut app, 100, 20);

    assert!(actual.contains('⠋'), "{actual}");
}

#[test]
fn when_a_cached_thumbnail_is_refreshing_the_footer_should_show_activity_and_keep_pixels() {
    let mut app = refreshing_thumbnail_app();

    let actual = first_screen(&mut app, 100, 20);

    assert!(actual.contains('\u{10EEEE}'), "{actual}");
    assert!(
        actual.lines().last().unwrap().contains("⠋ images"),
        "{actual}"
    );
}

#[test]
fn when_a_skipped_thumbnail_returns_its_first_frame_should_show_resumed_refresh_activity() {
    let mut app = refreshing_thumbnail_app();
    app.on_msg(Msg::ThumbSkipped {
        url: "https://i.redd.it/p1.jpeg".into(),
    });

    let actual = first_screen(&mut app, 100, 20);

    assert!(actual.contains('\u{10EEEE}'), "{actual}");
    assert!(
        actual.lines().last().unwrap().contains("⠋ images"),
        "{actual}"
    );
}

#[test]
fn when_a_cached_thumbnail_refresh_finishes_the_footer_should_stop_activity() {
    let mut app = refreshing_thumbnail_app();
    app.on_msg(Msg::Thumb {
        url: "https://i.redd.it/p1.jpeg".into(),
        result: Ok(image::DynamicImage::new_rgb8(32, 32)),
    });

    let actual = first_screen(&mut app, 100, 20);

    assert!(actual.contains('\u{10EEEE}'), "{actual}");
    assert!(
        !actual.lines().last().unwrap().contains("⠋ images"),
        "{actual}"
    );
}

#[test]
fn when_a_cached_thumbnail_refresh_fails_the_footer_should_stop_activity_and_keep_pixels() {
    let mut app = refreshing_thumbnail_app();
    app.on_msg(Msg::Thumb {
        url: "https://i.redd.it/p1.jpeg".into(),
        result: Err("offline".into()),
    });

    let actual = first_screen(&mut app, 100, 20);

    assert!(actual.contains('\u{10EEEE}'), "{actual}");
    assert!(
        !actual.lines().last().unwrap().contains("⠋ images"),
        "{actual}"
    );
}

#[test]
fn when_images_are_disabled_thumbnail_refresh_should_not_show_activity() {
    let mut app = refreshing_thumbnail_app();
    app.show_images = false;

    let actual = first_screen(&mut app, 100, 20);

    assert!(!actual.contains('\u{10EEEE}'), "{actual}");
    assert!(
        !actual.lines().last().unwrap().contains("⠋ images"),
        "{actual}"
    );
}

#[test]
fn when_thumbnails_are_unsupported_their_refresh_should_not_show_activity() {
    let mut app = refreshing_thumbnail_app();
    app.image_picker = Picker::halfblocks();

    let actual = first_screen(&mut app, 100, 20);

    assert!(!actual.contains('\u{10EEEE}'), "{actual}");
    assert!(
        !actual.lines().last().unwrap().contains("⠋ images"),
        "{actual}"
    );
}

#[test]
fn when_media_opening_outlasts_its_notification_the_footer_should_keep_showing_activity() {
    use crate::reddit::tests::{Fixture, Reply};
    let fixture = Fixture::new(|request| {
        let body = if request.url.contains("/comments/") {
            serde_json::from_str(include_str!("../../../tests/fixtures/post.json")).unwrap()
        } else {
            serde_json::json!({"data": {"children": [], "after": null}})
        };
        Reply::json(body)
    });
    let mut app = feed_with_next_page_in(app_with_client(&["s"], fixture.reddit()), ListView::Card);
    app.tabs[4].feed.posts[0].media = Media::Video {
        gif: false,
        url: None,
        poster: None,
    };
    app.open_media();
    app.status.as_mut().unwrap().at = Instant::now() - std::time::Duration::from_secs(7);
    app.tick();
    assert!(app.status.is_none());

    let actual = screen(&mut app, 80, 20);

    assert!(
        actual.lines().last().unwrap().contains("⠋ media"),
        "{actual}"
    );
}

#[test]
fn when_media_loading_fails_the_footer_should_show_the_error_and_stop_activity() {
    let mut app = feed_with_next_page(ListView::Card);
    app.tabs[4].feed.posts[0].media = Media::Video {
        gif: false,
        url: None,
        poster: None,
    };
    app.open_media();
    app.on_msg(Msg::MediaResolved {
        permalink: app.tabs[4].feed.posts[0].permalink.clone(),
        result: Err("offline".into()),
    });

    let actual = screen(&mut app, 80, 20);

    let footer = actual.lines().last().unwrap();
    assert!(
        footer.contains("media: offline") && !footer.contains("⠋ media"),
        "{actual}"
    );
}

#[test]
fn video_startup_should_emit_loading_text_without_losing_image_uploads() {
    for protocol in [
        ProtocolType::Kitty,
        ProtocolType::Sixel,
        ProtocolType::Iterm2,
    ] {
        let mut app = video_card_with_poster(image::DynamicImage::new_rgb8(320, 180));
        app.image_picker.set_protocol_type(protocol);
        app.config.media.video_autoplay = true;
        let size = if protocol == ProtocolType::Kitty {
            Size::new(10, 1)
        } else {
            Size::new(40, 8)
        };
        let picture = CardPicture {
            url: Some("https://i.redd.it/p1.jpeg".into()),
            area: Rect::new(0, 0, size.width, size.height),
            size,
            skipped: 0,
            pixels: (320, 180),
            indicator: Some(Indicator::Video {
                gif: false,
                play: true,
            }),
            selected: true,
            detail: false,
            post_id: Some("p1".into()),
            video_url: Some("https://v.redd.it/example/HLSPlaylist.m3u8".into()),
            video_frame: false,
        };
        let draw = |app: &mut App| {
            let mut terminal = Terminal::new(TestBackend::new(size.width, size.height)).unwrap();
            loop {
                terminal
                    .draw(|f| draw_card_pictures(f, app, std::slice::from_ref(&picture), None))
                    .unwrap();
                if !app.card_renderer.pending() {
                    break;
                }
                app.card_renderer.wait_for_completion();
                app.card_renderer.poll();
            }
            terminal.backend().buffer().clone()
        };

        let startup = draw(&mut app);
        let empty = ratatui::buffer::Buffer::empty(startup.area);
        let updates = empty.diff(&startup);
        assert!(
            updates
                .iter()
                .any(|(_, _, cell)| cell.symbol() == loading::glyph(0)),
            "{protocol:?}: loading must reach terminal updates"
        );

        app.config.media.video_autoplay = false;
        let idle = draw(&mut app);
        if protocol == ProtocolType::Kitty {
            assert!(
                buffer_text(&idle).contains("_Ga=T"),
                "the image upload must survive a one-row loading slot"
            );
        }
    }
}

#[test]
fn video_startup_should_stop_loading_when_the_initial_thread_request_fails() {
    let mut app = video_card_with_poster(image::DynamicImage::new_rgb8(320, 180));
    app.config.media.video_autoplay = true;
    app.open_current_post();
    screen(&mut app, 100, 30);
    let generation = app.post_view.as_ref().unwrap().generation;
    assert_eq!(
        loading_activity(&app, &VisibleLoaders::default()),
        Some("video")
    );

    app.on_msg(Msg::Post {
        generation,
        result: Err("fixture request failed".into()),
    });
    let actual = screen(&mut app, 100, 30);

    assert!(actual.contains("Could not load the post"), "{actual}");
    assert_ne!(
        loading_activity(&app, &VisibleLoaders::default()),
        Some("video")
    );
    assert!(!app.loading_active());
}

#[test]
fn when_a_short_card_view_loads_pagination_it_should_keep_the_footer_status_visible() {
    let mut app = feed_with_next_page(ListView::Card);
    app.load_feed(4, true);

    let actual = screen(&mut app, 50, 5);

    assert!(actual.lines().last().unwrap().contains('⠋'), "{actual}");
    assert!(actual.contains("Existing"), "{actual}");
}

#[test]
fn when_a_skipped_thumbnail_stays_offscreen_the_footer_should_not_show_refresh_activity() {
    let mut app = refreshing_thumbnail_app();
    app.on_msg(Msg::ThumbSkipped {
        url: "https://i.redd.it/p1.jpeg".into(),
    });
    app.tabs[4].feed.posts.clear();

    let actual = first_screen(&mut app, 100, 20);

    assert!(!actual.contains('\u{10EEEE}'), "{actual}");
    assert!(
        !actual.lines().last().unwrap().contains("⠋ images"),
        "{actual}"
    );
}
