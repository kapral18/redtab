use super::*;

#[test]
fn when_card_images_are_switched_off_their_previews_should_be_released() {
    let mut app = card_feed_app(kitty());
    screen(&mut app, 100, 20);
    assert!(!app.card_images.is_empty());

    press_keys(&mut app, "zi");
    screen(&mut app, 100, 20);

    assert!(app.card_images.is_empty());
}

#[test]
fn when_a_card_is_a_gallery_it_should_preview_the_first_image_instead_of_its_thumbnail() {
    let mut app = card_feed_app(kitty());
    app.tabs[4].feed.posts[0].media = Media::Gallery {
        images: vec!["https://i.redd.it/full.jpeg".into()],
        previews: Vec::new(),
        thumb: Some("https://i.redd.it/small.jpeg".into()),
    };

    screen(&mut app, 100, 20);

    assert!(app.card_images.contains_key("https://i.redd.it/full.jpeg"));
    assert!(!app.card_images.contains_key("https://i.redd.it/small.jpeg"));
}

#[test]
fn when_cached_pixels_are_decoded_again_the_warm_card_should_not_reencode() {
    let mut app = card_with_image(640, 320);
    screen(&mut app, 201, 57);
    let url = "https://i.redd.it/p1.jpeg";
    app.card_pictures_drawn(&["https://i.redd.it/large.png"]);
    app.on_msg(Msg::Image {
        url: "https://i.redd.it/large.png".into(),
        result: Ok(image::DynamicImage::new_rgba8(4096, 4096).into()),
    });
    app.card_pictures_drawn(&[]);
    assert!(
        !app.card_images.contains_key(url),
        "exercise the decoded-pixel cache limit"
    );
    screen(&mut app, 201, 57);

    app.on_msg(Msg::Image {
        url: url.into(),
        result: Ok(
            image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                640,
                320,
                image::Rgba([0, 0, 0, 255]),
            ))
            .into(),
        ),
    });
    let shown = first_screen(&mut app, 201, 57);

    assert!(
        !app.card_renderer.pending(),
        "identical decoded content must reuse the warm encoding"
    );
    assert!(!shown.contains("_G") && !shown.contains("preparing image"));
}

#[test]
fn the_same_loaded_image_on_another_post_should_reuse_its_native_encoding() {
    let mut app = card_with_image(640, 320);
    screen(&mut app, 100, 40);
    app.tabs[4].feed.posts[0].id = "crosspost".into();
    let shown = first_screen(&mut app, 100, 40);
    assert!(!shown.contains("preparing image"));
    assert!(!shown.contains("_G"));
    assert!(!app.card_renderer.pending());
}

#[test]
fn when_card_graphics_finish_they_should_replace_the_preparation_indicator() {
    let mut app = card_with_decoded_image();

    let actual = screen(&mut app, 100, 24);

    assert!(actual.contains('\u{10EEEE}'), "{actual}");
    assert!(!actual.contains("preparing image"), "{actual}");
}

#[test]
fn when_cards_show_media_they_should_load_only_visible_full_size_previews() {
    let mut app = card_feed_app(kitty());

    screen(&mut app, 100, 20);
    app.on_msg(Msg::Image {
        url: "https://i.redd.it/p1.jpeg".into(),
        result: Ok(image::DynamicImage::new_rgb8(640, 320).into()),
    });
    let cells = placeholder_cells(&mut app, 100, 20);

    assert_eq!(app.card_images.len(), 1);
    assert!(matches!(
        app.card_images.get("https://i.redd.it/p1.jpeg"),
        Some(CardImageSlot::Ready { image, .. }) if (image.width(), image.height()) == (640, 320)
    ));
    assert!(app.thumbs.is_empty());
    assert!(
        cells.len() > 8,
        "card preview should exceed the compact 4×2 thumbnail"
    );
}

#[test]
fn when_a_large_card_image_arrives_it_should_leave_the_ui_thread_responsive() {
    let mut app = card_feed_app(kitty());
    app.tabs[4].feed.posts.truncate(1);
    screen(&mut app, 100, 20);
    let message = Msg::Image {
        url: "https://i.redd.it/p1.jpeg".into(),
        result: Ok(image::DynamicImage::new_rgb8(1600, 1600).into()),
    };
    let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();

    let started = Instant::now();
    app.on_msg(message);
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
    app.scroll_card(1);
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();

    assert!(
        started.elapsed() < std::time::Duration::from_millis(200),
        "image preparation blocked input for {:?}",
        started.elapsed()
    );
    assert_eq!(app.active_tab().feed.cards.line, 0);
}

#[test]
fn when_neighboring_images_load_navigating_back_should_keep_the_same_card_positions() {
    let mut app = card_feed_app(kitty());
    app.tabs[4].feed.posts.iter_mut().for_each(|post| {
        post.title = post.id.clone();
        post.preview_size = Some((640, 320));
    });
    screen(&mut app, 100, 60);
    app.move_selection(1);
    let before = screen(&mut app, 100, 60);
    let row = |screen: &str| screen.lines().position(|line| line.contains("p2"));
    assert!(matches!(
        app.card_images.get("https://i.redd.it/p1.jpeg"),
        Some(CardImageSlot::Loading)
    ));
    app.on_msg(Msg::Image {
        url: "https://i.redd.it/p1.jpeg".into(),
        result: Ok(image::DynamicImage::new_rgb8(640, 320).into()),
    });
    screen(&mut app, 100, 60);
    app.move_selection(-1);
    screen(&mut app, 100, 60);
    app.move_selection(1);

    let actual = screen(&mut app, 100, 60);

    assert!(matches!(
        app.card_images.get("https://i.redd.it/p1.jpeg"),
        Some(CardImageSlot::Ready { .. })
    ));
    assert_eq!(row(&before), row(&actual));
    assert!(row(&actual).is_some());
}

#[test]
fn no_color_with_half_blocks_says_why_there_are_no_images() {
    let mut app = feed_app(Picker::halfblocks());
    app.no_color = true;

    press_keys(&mut app, "zi");
    let s = screen(&mut app, 100, 20);

    assert!(app.show_images, "i changes nothing it cannot show");
    assert!(
        app.status
            .as_ref()
            .is_some_and(|st| st.text.contains("NO_COLOR")),
        "{:?}",
        app.status.as_ref().map(|st| &st.text)
    );
    assert!(s.contains("img off"), "{s}");
}
