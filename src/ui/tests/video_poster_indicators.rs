use super::*;

#[test]
fn video_startup_should_stay_visible_over_posters_and_retained_frames_until_playing() {
    let frame = video::Frame {
        width: 32,
        height: 18,
        pixels: video::Pixels::Raw(std::sync::Arc::new(image::DynamicImage::new_rgb8(32, 18))),
        same_as_first: false,
    };
    for detail in [false, true] {
        let mut app = if detail {
            open_video_post()
        } else {
            video_card_with_poster(image::DynamicImage::new_rgb8(320, 180))
        };
        app.config.media.video_autoplay = true;
        let url = app
            .playback_url(if detail {
                VIDEO
            } else {
                "https://v.redd.it/example/HLSPlaylist.m3u8"
            })
            .unwrap();
        let height = if detail { 30 } else { 20 };

        let actual = screen(&mut app, 100, height);
        assert!(
            actual
                .lines()
                .take(height as usize - 1)
                .any(|line| line.contains("loading video")),
            "{actual}"
        );
        assert!(app.loading_active());

        for resolving in [true, false] {
            let mut player = if resolving {
                video::Player::pending(&url, Size::new(32, 18))
            } else {
                video::Player::silent(&url, Size::new(32, 18))
            };
            player.retain_frame(Some(frame.clone()));
            app.video = Some(player);

            let actual = screen(&mut app, 100, height);
            assert!(
                actual
                    .lines()
                    .take(height as usize - 1)
                    .any(|line| line.contains("loading video")),
                "{actual}"
            );
            assert!(app.loading_active());
            assert_eq!(
                loading_activity(&app, &VisibleLoaders::default()),
                Some("video")
            );
        }

        app.video.as_mut().unwrap().show(frame.clone());
        let actual = screen(&mut app, 100, height);
        assert!(!actual.contains("loading video"), "{actual}");
        assert!(!app.loading_active());

        app.video = Some(video::Player::unavailable(&url, Size::new(32, 18)));
        let actual = screen(&mut app, 100, height);
        assert!(!actual.contains("loading video"), "{actual}");
        assert!(!app.loading_active());

        app.video = None;
        app.config.media.video_autoplay = false;
        let actual = screen(&mut app, 100, height);
        assert!(!actual.contains("loading video"), "{actual}");
        assert!(app.video_loading().is_none());
        assert_ne!(
            loading_activity(&app, &VisibleLoaders::default()),
            Some("video")
        );
    }
}

#[test]
fn when_a_video_poster_is_solid_dark_its_card_should_keep_its_play_indicator() {
    let mut app = video_card_with_poster(image::DynamicImage::ImageRgba8(
        image::RgbaImage::from_pixel(404, 719, image::Rgba([2, 0, 3, 255])),
    ));
    let reserved_height = app.active_tab().feed.cards.height;

    let actual = screen(&mut app, 100, 20);

    assert!(
        actual.contains("video") || actual.contains("gif"),
        "{actual}"
    );
    assert!(
        actual.contains('\u{10EEEE}')
            || app.image_picker.protocol_type() == ProtocolType::Halfblocks,
        "{actual}"
    );
    assert_eq!(app.active_tab().feed.cards.height, reserved_height);
    assert!(!app.card_renderer.pending());
}

#[test]
fn when_a_video_poster_is_transparent_its_card_should_keep_its_play_indicator() {
    let mut app = video_card_with_poster(image::DynamicImage::ImageRgba8(
        image::RgbaImage::from_fn(2, 2, |x, y| image::Rgba([x as u8, y as u8, 255, 0])),
    ));
    app.image_picker = Picker::halfblocks();

    let actual = screen(&mut app, 100, 20);

    assert!(
        actual.contains("video") || actual.contains("gif"),
        "{actual}"
    );
    assert!(
        actual.contains('\u{10EEEE}')
            || app.image_picker.protocol_type() == ProtocolType::Halfblocks,
        "{actual}"
    );
    assert!(!app.card_renderer.pending());
}

#[test]
fn when_a_video_poster_has_visible_content_its_card_should_render_the_pixels() {
    let mut app = video_card_with_poster(image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(
        320,
        180,
        |x, _| image::Rgb([x as u8, 128, 255]),
    )));

    let actual = screen(&mut app, 100, 20);

    assert!(actual.contains('\u{10EEEE}'));
    assert!(!actual.contains("▶ video · open post"));
}

#[test]
fn when_a_flat_poster_url_is_used_by_an_image_post_it_should_still_render_the_pixels() {
    let mut app = video_card_with_poster(image::DynamicImage::new_rgb8(320, 180));
    app.tabs[4].feed.posts[0].media = Media::Image {
        url: "https://i.redd.it/p1.jpeg".into(),
        preview: None,
    };

    let actual = screen(&mut app, 100, 20);

    assert!(actual.contains('\u{10EEEE}'));
    assert!(!actual.contains("▶ video · open post"));
}

#[test]
fn when_a_gif_card_has_a_flat_first_frame_it_should_keep_its_play_indicator() {
    let mut app = card_feed_app(kitty());
    app.config.media.video_autoplay = false;
    app.tabs[4].feed.posts.truncate(1);
    app.tabs[4].feed.posts[0].media = Media::Image {
        url: "https://i.redd.it/animation.gif".into(),
        preview: None,
    };
    first_screen(&mut app, 100, 20);
    app.on_msg(Msg::Image {
        url: "https://i.redd.it/animation.gif".into(),
        result: Ok(image::DynamicImage::new_rgba8(320, 180).into()),
    });

    let actual = screen(&mut app, 100, 20);

    assert!(
        actual.contains("video") || actual.contains("gif"),
        "{actual}"
    );
    assert!(
        actual.contains('\u{10EEEE}')
            || app.image_picker.protocol_type() == ProtocolType::Halfblocks,
        "{actual}"
    );
}

#[test]
fn when_a_flat_video_preview_returns_after_eviction_its_indicator_should_stay_visible() {
    let mut app = video_card_with_poster(image::DynamicImage::new_rgb8(320, 180));
    screen(&mut app, 100, 20);
    app.card_images.clear();

    let actual = screen(&mut app, 100, 20);

    assert!(
        actual.contains("video") || actual.contains("gif"),
        "{actual}"
    );
    assert!(
        actual.contains('\u{10EEEE}')
            || app.image_picker.protocol_type() == ProtocolType::Halfblocks,
        "{actual}"
    );
    assert!(!actual.contains("loading image"));
}

#[test]
fn when_a_retained_flat_video_preview_is_upgraded_it_should_render_the_new_pixels() {
    let mut app = video_card_with_poster(image::DynamicImage::new_rgb8(320, 180));
    screen(&mut app, 100, 20);
    app.card_images.clear();
    first_screen(&mut app, 100, 20);
    app.on_msg(Msg::Image {
        url: "https://i.redd.it/p1.jpeg".into(),
        result: Ok(
            image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(320, 180, |x, _| {
                image::Rgb([x as u8, 128, 255])
            }))
            .into(),
        ),
    });

    let actual = screen(&mut app, 100, 20);

    assert!(actual.contains('\u{10EEEE}'));
    assert!(!actual.contains("▶ video · open post"));
}

#[test]
fn when_a_retained_video_preview_is_upgraded_to_a_flat_frame_it_should_keep_the_play_indicator() {
    let mut app = video_card_with_poster(image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(
        320,
        180,
        |x, _| image::Rgb([x as u8, 128, 255]),
    )));
    screen(&mut app, 100, 20);
    app.card_images.clear();
    first_screen(&mut app, 100, 20);
    app.on_msg(Msg::Image {
        url: "https://i.redd.it/p1.jpeg".into(),
        result: Ok(image::DynamicImage::new_rgb8(320, 180).into()),
    });

    let actual = screen(&mut app, 100, 20);

    assert!(
        actual.contains("video") || actual.contains("gif"),
        "{actual}"
    );
    assert!(
        actual.contains('\u{10EEEE}')
            || app.image_picker.protocol_type() == ProtocolType::Halfblocks,
        "{actual}"
    );
}

#[test]
fn when_a_youtube_card_waits_for_playback_it_should_show_the_provider_poster_and_play_overlay() {
    let mut app = card_feed_app(kitty());
    app.config.media.video_autoplay = false;
    app.tabs[4].feed.posts.truncate(1);
    app.tabs[4].feed.posts[0].media = Media::Link {
        url: "https://www.youtube.com/shorts/dQw4w9WgXcQ".into(),
        thumb: Some("https://preview.redd.it/youtube.jpg".into()),
    };
    let mut pictures = ListPictures::default();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();

    terminal
        .draw(|f| {
            draw_feed(f, &mut app, f.area(), &mut pictures);
        })
        .unwrap();

    assert_eq!(
        pictures.cards[0].url.as_deref(),
        Some("https://preview.redd.it/youtube.jpg")
    );
    assert!(matches!(
        pictures.cards[0].indicator,
        Some(Indicator::Video {
            gif: false,
            play: true
        })
    ));
    let actual = buffer_text(terminal.backend().buffer());
    assert!(actual.contains("▶ YouTube"), "{actual}");
}

#[test]
fn when_a_youtube_detail_loads_it_should_keep_its_poster_and_provider() {
    let mut app = open_video_post();
    app.config.media.video_autoplay = false;
    let media = Media::Link {
        url: "https://www.youtube.com/watch?v=dQw4w9WgXcQ".into(),
        thumb: Some("https://preview.redd.it/youtube.jpg".into()),
    };
    let view = app.post_view.as_mut().unwrap();
    view.post.media = media.clone();
    view.page.as_mut().unwrap().post.media = media;
    let mut pictures = ListPictures::default();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();

    terminal
        .draw(|f| {
            draw_post(f, &mut app, f.area(), &mut pictures);
        })
        .unwrap();

    assert_eq!(
        pictures.cards[0].url.as_deref(),
        Some("https://preview.redd.it/youtube.jpg")
    );
    assert!(buffer_text(terminal.backend().buffer()).contains("▶ YouTube"));
}
