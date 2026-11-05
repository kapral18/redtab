use super::*;

#[test]
fn when_revisiting_a_card_after_pixel_eviction_its_geometry_should_stay_reserved() {
    let mut app = card_with_image(320, 640);
    app.tabs[4].feed.posts.push(post_with("p2", Media::None));
    screen(&mut app, 100, 20);
    app.card_images.clear();
    app.move_selection(1);
    screen(&mut app, 100, 20);
    app.move_selection(-1);

    let actual = screen(&mut app, 100, 20);

    assert_eq!(app.active_tab().feed.cards.height, 11, "{actual}");
    assert!(actual.contains("p2"), "{actual}");
}

#[test]
fn when_paging_a_bookmark_before_its_image_loads_it_should_keep_all_reserved_media_visible() {
    let mut app = card_feed_app(kitty());
    app.tabs[4].feed.posts[0].title = "Reserved bookmark".into();
    app.tabs[4].feed.posts[0].preview_size = Some((320, 640));
    app.state.toggle_bookmark(&app.current_post().unwrap(), 1);
    app.switch_tab(1);
    screen(&mut app, 100, 20);

    app.page_list(1, 1);
    screen(&mut app, 100, 20);

    assert_eq!(app.active_tab().feed.cards.line, 0);
    assert_eq!(app.active_tab().feed.cards.height, 11);
    assert_eq!(app.current_post().unwrap().id, "p1");
}

#[test]
fn card_media_known_portrait_size_without_a_poster_should_keep_its_geometry() {
    let mut app = video_card_with_poster(image::DynamicImage::new_rgb8(320, 640));
    if let Media::Video { poster, .. } = &mut app.tabs[4].feed.posts[0].media {
        *poster = None;
    }
    let mut pictures = ListPictures::default();
    let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();

    terminal
        .draw(|f| {
            draw_feed(f, &mut app, f.area(), &mut pictures);
        })
        .unwrap();

    assert_eq!(pictures.cards[0].pixels, (320, 640));
    assert_eq!(pictures.cards[0].size.height, pictures.cards[0].size.width);
}

#[test]
fn when_returning_to_card_media_after_eviction_its_preview_should_stay_visible() {
    for video in [false, true] {
        let mut app = video_card_with_poster(image::DynamicImage::ImageRgb8(
            image::RgbImage::from_fn(320, 640, |x, _| image::Rgb([x as u8, 128, 255])),
        ));
        app.tabs[4].feed.posts[0].title = "Full width image".into();
        if video {
            app.tabs[4].feed.posts[0].media = Media::Video {
                gif: false,
                url: Some("https://v.redd.it/example/HLSPlaylist.m3u8".into()),
                poster: Some("https://i.redd.it/p1.jpeg".into()),
            };
        } else {
            app.tabs[4].feed.posts[0].media = Media::Image {
                url: "https://i.redd.it/p1.jpeg".into(),
                preview: None,
            };
        }
        screen(&mut app, 100, 20);
        for number in 0..65 {
            let url = format!("https://i.redd.it/pressure-{number}.png");
            app.card_pictures_drawn(&[&url]);
            app.on_msg(Msg::Image {
                url,
                result: Ok(image::DynamicImage::new_rgb8(1, 1).into()),
            });
        }
        app.card_pictures_drawn(&[]);
        assert!(!app.card_images.contains_key("https://i.redd.it/p1.jpeg"));
        app.move_selection(0);
        app.move_selection(0);

        let actual = screen(&mut app, 100, 20);

        assert!(
            actual.contains('\u{10EEEE}'),
            "retained image/poster must render immediately: {actual}"
        );
        assert!(!actual.contains("loading image"), "{actual}");
        assert_eq!(app.active_tab().feed.cards.height, 11);
        std::thread::sleep(crate::app::NAV_SETTLE);
        app.tick();
        screen(&mut app, 100, 20);
        app.on_msg(Msg::Image {
            url: "https://i.redd.it/p1.jpeg".into(),
            result: Err("offline".into()),
        });
        let failed_upgrade = screen(&mut app, 100, 20);
        assert!(failed_upgrade.contains('\u{10EEEE}'), "{failed_upgrade}");
        assert!(
            !failed_upgrade.contains("loading image")
                && !failed_upgrade.contains("image unavailable"),
            "{failed_upgrade}"
        );
        app.reload();
        screen(&mut app, 100, 20);
        app.on_msg(Msg::Image {
            url: "https://i.redd.it/p1.jpeg".into(),
            result: Ok(image::DynamicImage::new_rgb8(320, 640).into()),
        });
        screen(&mut app, 100, 20);
        assert!(matches!(
            app.card_images.get("https://i.redd.it/p1.jpeg"),
            Some(CardImageSlot::Ready { .. })
        ));
        assert_eq!(app.active_tab().feed.cards.height, 11);
    }
}

#[test]
fn when_a_card_has_listing_dimensions_loading_and_failure_should_keep_its_height() {
    let mut app = card_feed_app(kitty());
    app.tabs[4].feed.posts.truncate(1);
    app.tabs[4].feed.posts[0].title = "Reserved portrait".into();
    app.tabs[4].feed.posts[0].preview_size = Some((320, 640));
    screen(&mut app, 100, 20);
    assert_eq!(app.active_tab().feed.cards.height, 11);
    app.scroll_card(16);

    app.on_msg(Msg::Image {
        url: "https://i.redd.it/p1.jpeg".into(),
        result: Err("offline".into()),
    });
    let actual = screen(&mut app, 100, 20);

    assert_eq!(app.active_tab().feed.cards.height, 11, "{actual}");
    assert_eq!(app.active_tab().feed.cards.line, 0);
    assert!(actual.contains("offline"), "{actual}");
}

#[test]
fn when_a_reserved_image_finishes_loading_the_selected_card_should_not_move() {
    let mut app = card_feed_app(kitty());
    app.tabs[4].feed.posts.truncate(1);
    app.tabs[4].feed.posts[0].title = "Reserved landscape".into();
    app.tabs[4].feed.posts[0].preview_size = Some((640, 320));
    let before = screen(&mut app, 100, 40);
    let row = |screen: &str| screen.lines().position(|line| line.contains("comments"));
    assert_eq!(app.active_tab().feed.cards.height, 20);

    app.on_msg(Msg::Image {
        url: "https://i.redd.it/p1.jpeg".into(),
        result: Ok(image::DynamicImage::new_rgb8(320, 160).into()),
    });
    let actual = screen(&mut app, 100, 40);

    assert_eq!(app.active_tab().feed.cards.height, 20);
    assert_eq!(row(&before), row(&actual));
    assert!(row(&actual).is_some());
}
