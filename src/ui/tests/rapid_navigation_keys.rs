use super::*;

#[test]
fn rapid_navigation_should_reuse_loaded_native_media_without_new_preparation_or_uploads() {
    for kind in ["image", "video", "gallery", "link"] {
        let mut app = card_with_image(640, 320);
        let url = "https://i.redd.it/p1.jpeg";
        app.tabs[4].feed.posts[0].media = match kind {
            "video" => Media::Video {
                gif: false,
                url: Some(VIDEO.into()),
                poster: Some(url.into()),
            },
            "gallery" => Media::Gallery {
                thumb: None,
                previews: Vec::new(),
                images: vec![url.into(), "other".into()],
            },
            "link" => Media::Link {
                url: "https://example.com/story".into(),
                thumb: Some(url.into()),
            },
            _ => Media::Image {
                url: url.into(),
                preview: None,
            },
        };
        app.tabs[4]
            .feed
            .posts
            .extend((0..10).map(|i| post_with(&format!("tail{i}"), Media::None)));
        screen(&mut app, 100, 40);
        for delta in [3, 1, -1, -3, 8, -8] {
            app.scroll_card(delta);
            let shown = first_screen(&mut app, 100, 40);
            assert!(
                !shown.contains("preparing image"),
                "warm {kind} must draw immediately"
            );
            assert!(shown.contains('\u{10EEEE}'), "{kind}");
            assert!(
                !shown.contains("_G"),
                "warm {kind} must not upload pixels again"
            );
            assert!(
                !app.card_renderer.pending(),
                "warm {kind} must not queue encoding"
            );
        }
    }
}

#[test]
fn warm_navigation_replay_should_display_loaded_cards_without_media_work() {
    let mut app = card_feed_app(kitty());
    app.tabs[4].feed.posts = (0..12)
        .map(|i| {
            let mut post = post_with(
                &format!("warm{i}"),
                Media::Image {
                    url: format!("https://i.redd.it/warm{i}.png"),
                    preview: None,
                },
            );
            post.preview_size = Some((320, 640));
            post
        })
        .collect();
    for i in 0..12 {
        app.tabs[4].feed.selected = i;
        app.tabs[4].feed.cards = CardScroll::default();
        let url = format!("https://i.redd.it/warm{i}.png");
        app.card_pictures_drawn(&[&url]);
        app.on_msg(Msg::Image {
            url,
            result: Ok(image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(
                320,
                640,
                |x, y| image::Rgb([x as u8, y as u8, i as u8]),
            ))
            .into()),
        });
        screen(&mut app, 140, 50);
    }
    let started = Instant::now();
    let (mut delayed, mut preparation, mut uploads) = (0, 0, 0);
    for delta in std::iter::repeat_n(-1, 11)
        .chain(std::iter::repeat_n(1, 11))
        .cycle()
        .take(66)
    {
        app.move_selection(delta);
        let shown = first_screen(&mut app, 140, 50);
        delayed +=
            usize::from(shown.contains("preparing image") || shown.contains("loading image"));
        preparation += usize::from(app.card_renderer.pending());
        uploads += usize::from(shown.contains("_G"));
    }
    eprintln!(
        "navigation replay: 66 moves in {:?}; delayed frames={delayed}; preparation batches={preparation}; upload frames={uploads}",
        started.elapsed()
    );
    assert_eq!((delayed, preparation, uploads), (0, 0, 0));
}

#[test]
fn rapid_list_navigation_should_defer_cold_graphics_preparation() {
    let mut app = card_with_decoded_image();
    app.move_selection(0);
    app.move_selection(0);
    first_screen(&mut app, 100, 24);
    assert!(
        !app.card_renderer.pending(),
        "list input must not queue cold graphics during a quick run"
    );
}

#[test]
fn when_sidebar_navigation_shows_decoded_card_pixels_it_should_defer_missing_preparation() {
    let mut app = card_with_decoded_image();
    app.focus_sidebar();
    app.switch_tab(1);
    app.switch_tab(4);

    let actual = first_screen(&mut app, 100, 24);

    assert!(actual.contains("preparing image"), "{actual}");
    assert!(
        !app.card_renderer.pending(),
        "preparation must wait for settling"
    );
}

#[test]
fn when_sidebar_navigation_shows_cached_card_graphics_they_should_stay_visible_immediately() {
    let mut app = card_with_decoded_image();
    screen(&mut app, 100, 24);
    app.focus_sidebar();
    app.switch_tab(1);
    app.switch_tab(4);

    let actual = first_screen(&mut app, 100, 24);

    assert!(actual.contains('\u{10EEEE}'), "{actual}");
    assert!(!actual.contains("preparing image"), "{actual}");
    assert!(!app.card_renderer.pending());
}

#[test]
fn when_sidebar_navigation_settles_missing_card_graphics_should_resume_preparation() {
    let mut app = card_with_decoded_image();
    app.focus_sidebar();
    app.switch_tab(1);
    app.switch_tab(4);
    first_screen(&mut app, 100, 24);

    std::thread::sleep(crate::app::NAV_SETTLE);
    assert!(app.tick());
    let actual = screen(&mut app, 100, 24);

    assert!(actual.contains('\u{10EEEE}'), "{actual}");
    assert!(!actual.contains("preparing image"), "{actual}");
}

#[test]
fn when_navigation_defers_a_card_image_its_reserved_space_should_show_loading() {
    let mut app = card_feed_app(kitty());
    app.move_selection(1);
    app.move_selection(1);

    let actual = first_screen(&mut app, 100, 24);

    assert!(actual.contains("loading image"), "{actual}");
    assert!(
        !app.card_images.contains_key("https://i.redd.it/p3.jpeg"),
        "navigation must still defer the request"
    );
}
