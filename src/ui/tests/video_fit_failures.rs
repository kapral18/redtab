use super::*;

#[test]
fn in_a_pane_shorter_than_a_picture_it_should_still_come_wholly_into_view() {
    let url = "https://preview.redd.it/c.png";
    let mut app = picture_comment(url);
    let slot = ready(&app, 400, 200);
    app.images.insert(url.into(), slot);

    // About 10 rows of text: shorter than the 12 a picture may take.
    let drawn = (0..20).any(|_| {
        let shown = screen(&mut app, 100, 18).contains('\u{10EEEE}');
        app.scroll_post(1);
        shown
    });

    assert!(drawn);
}

#[test]
fn video_width_regression_portrait_detail_should_fit_height_and_scroll_with_content() {
    let mut app = open_video_post();
    app.post_view
        .as_mut()
        .unwrap()
        .page
        .as_mut()
        .unwrap()
        .post
        .preview_size = Some((1080, 1920));

    let first = detail_pictures(&mut app, 80, 30);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].size, Size::new(17, 15));
    assert_eq!(first[0].area.width, 17);

    app.scroll_post(10);
    let scrolled = detail_pictures(&mut app, 80, 30);
    assert_eq!(scrolled[0].skipped, 9);
    assert_eq!(scrolled[0].size, first[0].size);
    assert!(app.post_view.as_ref().unwrap().image_shown);
}

#[test]
fn video_width_regression_unknown_portrait_should_use_the_first_frame_shape() {
    let mut app = open_video_post();
    let url = app.reddit.media_url(VIDEO).unwrap();
    app.video = Some(video::Player::showing(
        &url,
        video::Frame {
            width: 180,
            height: 320,
            pixels: video::Pixels::Raw(std::sync::Arc::new(image::DynamicImage::new_rgb8(
                180, 320,
            ))),
            same_as_first: false,
        },
    ));

    let actual = detail_pictures(&mut app, 80, 30);

    assert_eq!(actual[0].size, Size::new(17, 15));
    assert_eq!(actual[0].pixels, (180, 320));
}

#[test]
fn media_failure_without_a_poster_or_frame_should_not_reserve_a_blank_picture() {
    let mut app = open_video_post();
    let url = app.reddit.media_url(VIDEO).unwrap();
    app.video = Some(video::Player::unavailable(&url, Size::new(70, 63)));

    let pictures = detail_pictures(&mut app, 100, 50);
    assert!(pictures.iter().all(|p| p.size.height == 0));
    assert!(app.video_area.is_none());
    let actual = screen(&mut app, 100, 50);
    assert!(
        actual.lines().last().unwrap().contains("p retry"),
        "{actual}"
    );
}

#[test]
fn media_failure_from_an_old_source_should_not_hide_the_current_video() {
    let mut app = open_video_post();
    let url = app.reddit.media_url(VIDEO).unwrap();
    app.video = Some(video::Player::unavailable(&url, Size::new(70, 63)));
    let media = &mut app
        .post_view
        .as_mut()
        .unwrap()
        .page
        .as_mut()
        .unwrap()
        .post
        .media;
    if let Media::Video { url, .. } = media {
        *url = Some("https://v.redd.it/current/video.mp4".into());
    }

    let pictures = detail_pictures(&mut app, 100, 50);

    assert!(pictures.iter().any(|p| p.size.height > 0));
    assert!(app.video_area.is_some());
}

#[test]
fn video_width_regression_split_detail_should_leave_the_list_poster_static() {
    let mut picker = kitty();
    picker.set_protocol_type(ProtocolType::Iterm2);
    let mut app = card_feed_app(picker);
    let poster = "https://i.redd.it/p1.jpeg";
    app.tabs[4].feed.posts.truncate(1);
    app.tabs[4].feed.posts[0].media = Media::Video {
        gif: false,
        url: Some(VIDEO.into()),
        poster: Some(poster.into()),
    };
    app.tabs[4].feed.posts[0].preview_size = Some((160, 90));
    app.card_images.insert(
        poster.into(),
        CardImageSlot::Ready {
            image: std::sync::Arc::new(image::DynamicImage::ImageRgb8(
                image::RgbImage::from_pixel(160, 90, image::Rgb([0, 0, 255])),
            )),
            protocol: None,
        },
    );
    screen(&mut app, 200, 40);
    app.open_current_post();
    let post = app.post_view.as_ref().unwrap().post.clone();
    app.post_view.as_mut().unwrap().page = Some(PostPage {
        post,
        body: String::new(),
        comments: vec![],
    })
    .into();
    let url = app.reddit.media_url(VIDEO).unwrap();
    app.video = Some(video::Player::showing(
        &url,
        video::Frame {
            width: 160,
            height: 90,
            pixels: video::Pixels::Raw(std::sync::Arc::new(image::DynamicImage::ImageRgb8(
                image::RgbImage::from_pixel(160, 90, image::Rgb([255, 0, 0])),
            ))),
            same_as_first: false,
        },
    ));

    let actual = screen(&mut app, 200, 40);

    use base64::Engine;
    let images: Vec<_> = actual
        .split("doNotMoveCursor=1:")
        .skip(1)
        .map(|chunk| {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(chunk.split('\u{7}').next().unwrap())
                .unwrap();
            image::load_from_memory(&bytes).unwrap().to_rgb8()
        })
        .collect();
    assert!(
        images
            .iter()
            .any(|i| i.pixels().any(|p| p.0 == [0, 0, 255])),
        "list poster must stay blue"
    );
    assert!(
        images
            .iter()
            .any(|i| i.pixels().any(|p| p.0 == [255, 0, 0])),
        "detail video must be red"
    );
}

#[test]
fn when_a_video_frame_is_decoded_smaller_it_should_still_be_drawn_at_full_size() {
    let placement = |w, h| {
        let mut app = open_video_post();
        let frame = video::Frame {
            width: w,
            height: h,
            pixels: video::Pixels::Raw(std::sync::Arc::new(image::DynamicImage::new_rgb8(w, h))),
            same_as_first: false,
        };
        let url = app.reddit.media_url(VIDEO).unwrap();
        app.video = Some(video::Player::showing(&url, frame));
        detail_pictures(&mut app, 100, 40)[0].size
    };

    assert_eq!(placement(160, 90), Size::new(75, 21));
    assert_eq!(placement(1280, 720), Size::new(75, 21));
}

#[test]
fn a_wide_video_should_fit_the_height_when_width_would_overflow() {
    let mut app = open_video_post();
    app.post_view
        .as_mut()
        .unwrap()
        .page
        .as_mut()
        .unwrap()
        .post
        .preview_size = Some((800, 600));
    let actual = detail_pictures(&mut app, 200, 40);
    assert_eq!(actual[0].size, Size::new(56, 21));
    assert_eq!(actual[0].area.width, 56);
    assert_eq!(actual[0].area.height, actual[0].size.height);
    assert_eq!(actual[0].area.x, 71);
}

#[test]
fn when_card_pixels_wait_for_graphics_preparation_the_first_frame_should_explain_the_wait() {
    let mut app = card_with_decoded_image();

    let actual = first_screen(&mut app, 100, 24);

    assert!(actual.contains("preparing image"), "{actual}");
    assert!(
        app.images_loading(),
        "still graphics preparation remains visible"
    );
    app.card_renderer.wait_for_completion();
    assert!(
        app.images_loading(),
        "unpublished still graphics remain visible"
    );
    assert!(app.card_renderer.poll());
    assert!(!app.images_loading());
}

#[test]
fn encoding_failures_should_show_an_error_in_the_first_failed_frame() {
    let mut app = card_with_image(320, 180);
    screen(&mut app, 100, 20);
    app.card_renderer.fail_cached_tiles("encoder failed");

    let actual = first_screen(&mut app, 100, 20);

    assert!(actual.contains("image unavailable:"), "{actual}");
    assert!(matches!(
        app.card_images.get("https://i.redd.it/p1.jpeg"),
        Some(CardImageSlot::Failed(error)) if error == "encoder failed"
    ));
}

#[test]
fn fitted_media_status_should_wrap_in_a_narrow_portrait_box() {
    let mut terminal = Terminal::new(TestBackend::new(16, 8)).unwrap();
    terminal
        .draw(|f| draw_picture_status(f, Rect::new(1, 1, 12, 6), "image unavailable: offline"))
        .unwrap();
    let text = buffer_text(terminal.backend().buffer());
    assert!(text.contains("offline"), "{text}");
    assert!(text.contains("unavailable"), "{text}");
}

#[test]
fn video_width_regression_tall_detail_should_keep_controls_in_the_footer() {
    let mut app = open_video_post();
    app.post_view
        .as_mut()
        .unwrap()
        .page
        .as_mut()
        .unwrap()
        .post
        .preview_size = Some((1080, 1920));
    let url = app.reddit.media_url(VIDEO).unwrap();
    let mut player = video::Player::showing(
        &url,
        video::Frame {
            width: 180,
            height: 320,
            pixels: video::Pixels::Raw(std::sync::Arc::new(image::DynamicImage::new_rgb8(
                180, 320,
            ))),
            same_as_first: false,
        },
    );
    if !player.paused() {
        player.toggle_pause(Instant::now());
    }
    app.video = Some(player);

    let actual = screen(&mut app, 100, 20);

    let footer = actual
        .lines()
        .last()
        .unwrap()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(footer.contains("p play"), "{actual}");
    assert!(footer.contains("m sound"), "{actual}");
}
