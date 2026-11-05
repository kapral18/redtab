use super::*;

#[test]
fn when_link_thumbnails_are_small_a_large_pane_should_keep_the_card_compact() {
    let app = card_feed_app(kitty());
    let mut post = post_with(
        "article",
        Media::Link {
            url: "https://volkskrant.nl/article".into(),
            thumb: Some("https://preview.redd.it/article.png".into()),
        },
    );
    post.title = "An article headline".into();
    post.preview_size = Some((140, 70));
    let card = expanded_card(
        &app,
        &post,
        180,
        &CardOpts {
            selected: true,
            show_subreddit: true,
            reserve: 0,
        },
        80,
    );
    assert!(
        card.image_size.width <= 27 && card.image_size.height <= 8,
        "140px thumbnails must not become pane-wide: {:?}",
        card.image_size
    );
    assert!(
        card.height() <= 12,
        "compact image and text share their rows"
    );
}

#[test]
fn when_decoded_link_pixels_are_small_the_declared_large_preview_should_not_enlarge_them() {
    let mut app = card_feed_app(kitty());
    app.tabs[4].feed.posts.truncate(1);
    let url = "https://preview.redd.it/article.jpeg";
    let post = &mut app.tabs[4].feed.posts[0];
    post.media = Media::Link {
        url: "https://example.com/story".into(),
        thumb: Some(url.into()),
    };
    post.preview_size = Some((1600, 900));
    app.card_image_metadata.insert(
        url.into(),
        crate::worker::ImageMetadata {
            size: (140, 70),
            fingerprint: [0; 32],
        },
    );

    let card = expanded_card(
        &app,
        &app.tabs[4].feed.posts[0],
        180,
        &CardOpts {
            selected: true,
            show_subreddit: true,
            reserve: 0,
        },
        80,
    );
    assert!(
        card.image_size.width <= 27 && card.image_size.height <= 8,
        "{:?}",
        card.image_size
    );
}

#[test]
fn when_link_images_are_large_their_preview_should_use_thirty_percent_of_the_viewport() {
    let app = card_feed_app(kitty());
    let mut post = post_with(
        "article",
        Media::Link {
            url: "https://example.com/article".into(),
            thumb: Some("https://preview.redd.it/article.png".into()),
        },
    );
    post.preview_size = Some((1600, 900));
    let card = expanded_card(
        &app,
        &post,
        180,
        &CardOpts {
            selected: true,
            show_subreddit: true,
            reserve: 0,
        },
        80,
    );
    assert_eq!(card.image_size.height, 24);
}

#[test]
fn when_a_link_has_no_image_its_text_card_should_identify_the_domain() {
    let app = card_feed_app(kitty());
    let post = post_with(
        "article",
        Media::Link {
            url: "https://volkskrant.nl/article".into(),
            thumb: None,
        },
    );
    let card = expanded_card(
        &app,
        &post,
        100,
        &CardOpts {
            selected: true,
            show_subreddit: true,
            reserve: 0,
        },
        60,
    );
    let text = card
        .visible(0, card.height())
        .iter()
        .flat_map(|line| line.spans.iter().map(|span| span.content.as_ref()))
        .collect::<String>();
    assert!(
        text.contains("VO"),
        "use a small domain monogram when there is no image: {text}"
    );
    assert_eq!(card.image_size, Size::default());
}

#[test]
fn when_a_landscape_card_image_loads_it_should_fit_the_available_height() {
    let mut app = card_with_image(640, 320);

    let cells = placeholder_cells(&mut app, 100, 20);

    assert_eq!(cells.iter().map(|cell| cell.0).min(), Some(45));
    assert_eq!(cells.iter().map(|cell| cell.0).max(), Some(76));
    assert!(matches!(app.card_images.get("https://i.redd.it/p1.jpeg"),
            Some(CardImageSlot::Ready { protocol: Some(protocol), .. })
                if protocol.size == Size::new(32, 8)));
}

#[test]
fn when_a_portrait_card_image_loads_it_should_fit_height_and_center_in_the_viewport() {
    let mut app = card_with_image(320, 640);

    let cells = placeholder_cells(&mut app, 100, 20);

    assert_eq!(cells.iter().map(|cell| cell.0).min(), Some(57));
    assert_eq!(cells.iter().map(|cell| cell.0).max(), Some(64));
    assert!(matches!(app.card_images.get("https://i.redd.it/p1.jpeg"),
            Some(CardImageSlot::Ready { protocol: Some(protocol), .. })
                if protocol.size == Size::new(8, 8)));
}

#[test]
fn when_a_small_card_image_loads_it_should_fill_its_fitted_box_without_the_text_upscaling_limit() {
    let mut app = card_with_image(16, 8);

    let cells = placeholder_cells(&mut app, 100, 20);

    assert_eq!(cells.iter().map(|cell| cell.0).min(), Some(45));
    assert_eq!(cells.iter().map(|cell| cell.0).max(), Some(76));
}

#[test]
fn when_a_card_shrinks_fine_detail_it_should_filter_all_contributing_pixels() {
    let mut app = card_with_image(2176, 64);
    app.image_picker.set_protocol_type(ProtocolType::Iterm2);
    let CardImageSlot::Ready { image, .. } = app
        .card_images
        .get_mut("https://i.redd.it/p1.jpeg")
        .unwrap()
    else {
        panic!("card image");
    };
    *image = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(2176, 64, |x, _| {
        let value = if matches!(x % 4, 1 | 2) { 255 } else { 0 };
        image::Rgb([value; 3])
    }))
    .into();

    screen(&mut app, 100, 20);

    let CardImageSlot::Ready {
        protocol: Some(protocol),
        ..
    } = app.card_images.get("https://i.redd.it/p1.jpeg").unwrap()
    else {
        panic!("card protocol");
    };
    let crate::card_image::Tile::Image(encoded) = protocol.tiles[&(0, 0, 1, 66)].as_ref() else {
        panic!("still-image tile");
    };
    let pixels = card_png(encoded);
    assert_eq!(pixels.dimensions(), (528, 16));
    assert!((120..=136).contains(&pixels.get_pixel(264, 8)[0]));
}

#[test]
fn when_resizing_a_card_it_should_refit_the_image_to_the_new_width() {
    let mut app = card_with_image(640, 320);
    placeholder_cells(&mut app, 100, 20);

    let cells = placeholder_cells(&mut app, 80, 20);

    assert_eq!(cells.iter().map(|cell| cell.0).min(), Some(35));
    assert_eq!(cells.iter().map(|cell| cell.0).max(), Some(66));
    assert!(matches!(app.card_images.get("https://i.redd.it/p1.jpeg"),
        Some(CardImageSlot::Ready { protocol: Some(protocol), .. })
            if protocol.size == Size::new(32, 8)));
}
