use super::*;

#[test]
fn when_a_partial_card_becomes_fully_visible_its_remaining_strips_should_be_ready() {
    let mut app = card_with_image(640, 640);
    app.tabs[4].feed.posts.push(post_with(
        "p2",
        Media::Image {
            url: "https://i.redd.it/p2.jpeg".into(),
            preview: None,
        },
    ));
    app.card_pictures_drawn(&["https://i.redd.it/p1.jpeg", "https://i.redd.it/p2.jpeg"]);
    app.on_msg(Msg::Image {
        url: "https://i.redd.it/p2.jpeg".into(),
        result: Ok(image::DynamicImage::new_rgb8(640, 640).into()),
    });
    screen(&mut app, 201, 57);

    app.move_selection(1);
    let shown = first_screen(&mut app, 201, 57);

    assert!(
        !shown.contains("preparing image"),
        "already-decoded clipped cards should prepare their fitted canvas"
    );
    assert!(!app.card_renderer.pending());
    assert!(
        !shown.contains("_G"),
        "the offscreen strips should already be uploaded"
    );
}

#[test]
fn when_card_previews_resize_their_native_strips_should_fill_the_fitted_box() {
    for kind in ["image", "gallery", "video", "gif", "link"] {
        let mut app = card_with_image(320, 640);
        app.config.media.video_autoplay = false;
        let url = "https://i.redd.it/p1.jpeg";
        app.tabs[4].feed.posts[0].media = match kind {
            "gallery" => Media::Gallery {
                images: vec![url.into()],
                previews: Vec::new(),
                thumb: None,
            },
            "video" | "gif" => Media::Video {
                url: Some(VIDEO.into()),
                poster: Some(url.into()),
                gif: kind == "gif",
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
        let mut fitted = Vec::new();
        for (width, height) in [(160, 60), (100, 30), (160, 60)] {
            let cells = placeholder_cells(&mut app, width, height);
            let CardImageSlot::Ready {
                protocol: Some(protocol),
                ..
            } = app.card_images.get(url).unwrap()
            else {
                panic!("decoded {kind}");
            };
            assert!(!cells.is_empty(), "{kind} at {width}x{height}");
            assert!(protocol.size.height <= height * 3 / 5);
            let left = cells.iter().map(|p| p.0).min().unwrap();
            let right = cells.iter().map(|p| p.0).max().unwrap();
            let top = cells.iter().map(|p| p.1).min().unwrap();
            let bottom = cells.iter().map(|p| p.1).max().unwrap();
            assert_eq!(
                (right - left + 1, bottom - top + 1),
                (protocol.size.width, protocol.size.height)
            );
            for ((_, _, rows, cols), tile) in &protocol.tiles {
                let crate::card_image::Tile::KittyImage { upload, size, .. } = tile.as_ref() else {
                    panic!("native still");
                };
                assert_eq!((*cols, *rows), (size.width, size.height));
                assert!(
                    upload.contains(&format!(",c={cols},r={rows},")),
                    "{kind} native placement"
                );
            }
            fitted.push(protocol.size);
            assert!(
                matches!(app.card_images.get(url), Some(CardImageSlot::Ready { .. })),
                "resize must keep decoded {kind}"
            );
        }
        assert_ne!(fitted[0], fitted[1]);
        assert_eq!(fitted[0], fitted[2]);
        let warm = first_screen(&mut app, 160, 60);
        assert!(
            !warm.contains("_G"),
            "returning to a loaded size must reuse {kind} uploads"
        );
        assert!(!app.card_renderer.pending());
    }
}

#[test]
fn feed_rows_load_and_place_thumbnails_only_for_visible_image_posts() {
    let mut app = feed_app(kitty());

    let s = screen(&mut app, 100, 20);

    let mut requested: Vec<&str> = app.thumbs.keys().map(String::as_str).collect();
    requested.sort();
    assert_eq!(
        requested,
        [
            "https://i.redd.it/p1.jpeg",
            "https://i.redd.it/p3.jpeg",
            "https://i.redd.it/p4.jpeg",
            "https://i.redd.it/p5.jpeg"
        ]
    );
    // Every row gets the slot, so the post without an image stays aligned;
    // the slot costs the titles its columns compared with a text-only list.
    let text_only = screen(&mut feed_app(Picker::halfblocks()), 100, 20);
    assert_eq!(title_run(&s, "p1"), title_run(&s, "p2"));
    assert_eq!(
        title_run(&s, "p2") + THUMB_RESERVE,
        title_run(&text_only, "p2")
    );
    let title_col = |screen: &str, id: &str| {
        let needle = format!("{id} x");
        let line = screen.lines().find(|l| l.contains(&needle)).unwrap();
        line[..line.find(&needle).unwrap()].chars().count()
    };
    assert_eq!(title_col(&s, "p1"), title_col(&s, "p2"));

    let taller = screen(&mut app, 100, 23);
    assert!(taller.contains("p6 x"), "{taller}");
    assert!(app.thumbs.contains_key("https://i.redd.it/p6.jpeg"));
    assert!(!app.thumbs.contains_key("https://i.redd.it/p7.jpeg"));

    let p1_row = s.lines().position(|l| l.contains("p1 x")).unwrap() as u16;
    let image = image::DynamicImage::new_rgb8(256, 256);
    let protocol = app.image_picker.new_resize_protocol(image);
    app.thumbs.insert(
        "https://i.redd.it/p1.jpeg".into(),
        ImageSlot::Ready {
            protocol: Box::new(protocol),
            refresh: crate::app::ImageRefresh::Idle,
            pixels: (256, 256),
        },
    );
    let cells = placeholder_cells(&mut app, 100, 20);

    // Sidebar 24 + inset 2: the list starts at x 26 with the selection bar,
    // so the 4×2 thumbnail fills x 27..=30 on the row's two lines only.
    let expected: Vec<(u16, u16)> = (p1_row..p1_row + 2)
        .flat_map(|y| (27..=30).map(move |x| (x, y)))
        .collect();
    let mut cells = cells;
    cells.sort_by_key(|&(x, y)| (y, x));
    assert_eq!(cells, expected);
}

#[test]
fn a_failed_thumbnail_keeps_its_row_aligned_until_reload_retries_it() {
    let mut app = feed_app(kitty());
    screen(&mut app, 100, 20);
    app.thumbs.insert(
        "https://i.redd.it/p1.jpeg".into(),
        ImageSlot::Failed("timed out".into()),
    );

    let s = screen(&mut app, 100, 20);

    assert!(matches!(
        app.thumbs.get("https://i.redd.it/p1.jpeg"),
        Some(ImageSlot::Failed(_))
    ));
    assert_eq!(title_run(&s, "p1"), title_run(&s, "p2"));

    app.reload();
    screen(&mut app, 100, 20);

    assert!(matches!(
        app.thumbs.get("https://i.redd.it/p1.jpeg"),
        Some(ImageSlot::Loading)
    ));
}

#[test]
fn a_full_thumbnail_cache_keeps_only_what_is_on_screen() {
    let mut app = feed_app(kitty());
    for i in 0..400 {
        app.thumbs.insert(
            format!("https://i.redd.it/old{i}.jpeg"),
            ImageSlot::Failed("x".into()),
        );
    }

    screen(&mut app, 100, 20);

    let mut kept: Vec<&str> = app.thumbs.keys().map(String::as_str).collect();
    kept.sort();
    assert_eq!(
        kept,
        [
            "https://i.redd.it/p1.jpeg",
            "https://i.redd.it/p3.jpeg",
            "https://i.redd.it/p4.jpeg",
            "https://i.redd.it/p5.jpeg"
        ]
    );
}

#[test]
fn a_skipped_thumbnail_loads_again_when_drawn() {
    let mut app = feed_app(kitty());
    screen(&mut app, 100, 20);

    app.on_msg(Msg::ThumbSkipped {
        url: "https://i.redd.it/p1.jpeg".into(),
    });
    let gone = !app.thumbs.contains_key("https://i.redd.it/p1.jpeg");
    screen(&mut app, 100, 20);

    assert!(gone);
    assert!(matches!(
        app.thumbs.get("https://i.redd.it/p1.jpeg"),
        Some(ImageSlot::Loading)
    ));
}

#[test]
fn a_skipped_refresh_keeps_a_loaded_thumbnail() {
    let mut app = feed_app(kitty());
    screen(&mut app, 100, 20);
    app.on_msg(Msg::Thumb {
        url: "https://i.redd.it/p1.jpeg".into(),
        result: Ok(image::DynamicImage::new_rgb8(4, 4)),
    });

    app.on_msg(Msg::ThumbSkipped {
        url: "https://i.redd.it/p1.jpeg".into(),
    });

    assert!(matches!(
        app.thumbs.get("https://i.redd.it/p1.jpeg"),
        Some(ImageSlot::Ready { .. })
    ));
}

/// With no thumbnail to show, rows keep the full width.
#[test]
fn lists_with_no_thumbnail_to_show_keep_text_only_rows() {
    let mut spoilers = feed_app(kitty());
    for p in &mut spoilers.tabs[4].feed.posts {
        p.spoiler = true;
    }
    let mut images_off = feed_app(kitty());
    images_off.show_images = false;
    let mut half_blocks = feed_app(Picker::halfblocks());
    let reference = title_run(&screen(&mut half_blocks, 100, 20), "p2");

    for (name, mut app) in [
        ("half-blocks", half_blocks),
        ("images off", images_off),
        ("spoilers", spoilers),
    ] {
        let s = screen(&mut app, 100, 20);
        assert!(app.thumbs.is_empty(), "{name}");
        assert_eq!(title_run(&s, "p1"), reference, "{name}");
        assert_eq!(title_run(&s, "p2"), reference, "{name}");
    }
}

#[test]
fn a_scrolled_home_card_loads_thumbnails_only_for_whole_rows() {
    let mut app = app(&["s"]);
    app.image_picker = kitty();
    app.show_images = true;
    app.all_subs.groups[0].posts = (1..=3)
        .map(|i| {
            post_with(
                &format!("p{i}"),
                Media::Image {
                    url: format!("https://i.redd.it/p{i}.jpeg"),
                    preview: None,
                },
            )
        })
        .collect();

    // 12 lines leave the card 4 inner lines: p1's two lines fit, p2's do not.
    screen(&mut app, 100, 12);
    let mut top: Vec<String> = app.thumbs.keys().cloned().collect();
    top.sort();
    app.move_selection(2);
    app.tick_at(std::time::Instant::now() + crate::app::NAV_SETTLE);
    screen(&mut app, 100, 12);

    assert_eq!(top, ["https://i.redd.it/p1.jpeg"]);
    // Scrolled to p3: p2's first line is above the card's top.
    assert!(app.thumbs.contains_key("https://i.redd.it/p3.jpeg"));
    assert!(!app.thumbs.contains_key("https://i.redd.it/p2.jpeg"));
}
