use super::*;

#[test]
fn a_detail_image_resize_should_keep_the_body_being_read_in_place() {
    let mut app = image_post(&paragraphs("body", 1..=20));
    let slot = ready(&app, 320, 1280);
    app.images.insert("https://i.redd.it/big.jpeg".into(), slot);
    detail_pictures(&mut app, 80, 30);
    app.scroll_post(26);
    detail_pictures(&mut app, 80, 30);
    let old = app.post_view.as_ref().unwrap().scroll;

    detail_pictures(&mut app, 100, 40);

    let view = app.post_view.as_ref().unwrap();
    assert_eq!(view.rendered_pictures[0].2, 21);
    assert_eq!(view.scroll, old + 6);
}

#[test]
fn main_and_embedded_copies_of_a_detail_image_should_have_independent_sizes() {
    let mut app = image_post(&format!("{PICTURE}https://i.redd.it/big.jpeg"));
    let slot = ready(&app, 320, 1280);
    app.images.insert("https://i.redd.it/big.jpeg".into(), slot);
    detail_pictures(&mut app, 80, 30);

    let pictures = &app.post_view.as_ref().unwrap().rendered_pictures;
    assert_eq!(pictures.len(), 2);
    assert_eq!(pictures[0].2, 15);
    assert_eq!(pictures[1].2, TEXT_PICTURE_ROWS as usize);
}

#[test]
fn a_detail_gallery_should_use_each_selected_images_own_shape_and_hide_spoilers() {
    let mut app = image_post("gallery caption");
    let media = Media::Gallery {
        thumb: None,
        previews: Vec::new(),
        images: vec![
            "https://i.redd.it/big.jpeg".into(),
            "https://i.redd.it/wide.jpeg".into(),
        ],
    };
    app.post_view
        .as_mut()
        .unwrap()
        .page
        .as_mut()
        .unwrap()
        .post
        .media = media;
    for (url, w, h) in [("big", 320, 1280), ("wide", 1280, 320)] {
        let slot = ready(&app, w, h);
        app.images
            .insert(format!("https://i.redd.it/{url}.jpeg"), slot);
    }
    assert_eq!(detail_pictures(&mut app, 80, 30)[0].size.height, 15);
    app.scroll_post(20);
    app.gallery_step(true);
    let next = detail_pictures(&mut app, 80, 30);
    assert_eq!(next[0].url.as_deref(), Some("https://i.redd.it/wide.jpeg"));
    assert_eq!(next[0].size, Size::new(76, 10));
    assert_eq!(app.post_view.as_ref().unwrap().scroll, 0);
    app.post_view
        .as_mut()
        .unwrap()
        .page
        .as_mut()
        .unwrap()
        .post
        .spoiler = true;
    assert!(detail_pictures(&mut app, 80, 30).is_empty());
    app.toggle_spoilers();
    assert_eq!(detail_pictures(&mut app, 80, 30)[0].size.height, 10);
}

#[test]
fn the_whole_detail_image_should_render_in_its_first_viewport() {
    let mut app = image_post("body below image");
    screen(&mut app, 100, 40);
    app.on_msg(Msg::Image {
        url: "https://i.redd.it/big.jpeg".into(),
        result: Ok(image::DynamicImage::new_rgb8(320, 1280).into()),
    });
    screen(&mut app, 100, 40);

    let cells = placeholder_cells(&mut app, 100, 40);

    assert_eq!(cells.len(), 10 * 19);
    assert_eq!(app.post_view.as_ref().unwrap().scroll, 0);
}

#[test]
fn toggling_a_detail_image_should_keep_body_and_comments_at_the_same_position() {
    for comments in [false, true] {
        let mut app = image_post_with_comments(37);
        if !comments {
            app.post_view.as_mut().unwrap().page.as_mut().unwrap().body =
                "repeated body paragraph\n\n".repeat(80);
        }
        let slot = ready(&app, 320, 1280);
        app.images.insert("https://i.redd.it/big.jpeg".into(), slot);
        detail_pictures(&mut app, 80, 30);
        let view = app.post_view.as_mut().unwrap();
        view.scroll = if comments {
            view.row_starts[10] + 1
        } else {
            160
        };
        detail_pictures(&mut app, 80, 30);
        let view = app.post_view.as_ref().unwrap();
        let before = view.rendered_document[view.scroll].to_string();
        let scroll = view.scroll;

        app.toggle_images();
        assert!(detail_pictures(&mut app, 80, 30).is_empty());
        let view = app.post_view.as_ref().unwrap();
        assert_eq!(view.rendered_document[view.scroll].to_string(), before);
        assert_eq!(view.scroll, scroll - 15);

        app.toggle_images();
        detail_pictures(&mut app, 80, 30);
        let view = app.post_view.as_ref().unwrap();
        assert_eq!(view.rendered_document[view.scroll].to_string(), before);
        assert_eq!(view.scroll, scroll);
    }
}

#[test]
fn a_short_detail_image_should_stay_when_page_scrolling_clamps_at_the_top() {
    let mut app = image_post("short");
    let at_top = screen(&mut app, 100, 40);
    key(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
    let paged = screen(&mut app, 100, 40);
    let redrawn = screen(&mut app, 100, 40);

    assert!(at_top.contains("loading image"));
    assert!(paged.contains("loading image"));
    assert_eq!(paged, redrawn);
    assert_eq!(app.post_view.as_ref().unwrap().scroll, 0);
}

#[test]
fn when_a_post_image_is_small_the_text_should_start_right_below_it() {
    // A 140×73 image fills 68 columns of 8×16 cells: 18 picture rows.
    let mut app = image_post("first line of the body");
    let slot = ready(&app, 140, 73);
    app.images.insert("https://i.redd.it/big.jpeg".into(), slot);

    let s = screen(&mut app, 100, 50);

    let row = |text: &str| s.lines().position(|l| l.contains(text)).unwrap();
    let gap = row("first line of the body")
        - s.lines()
            .position(|line| line.contains('╰') || line.contains('╚'))
            .unwrap();
    // 18 picture rows plus three rows of spacing.
    assert_eq!(gap, 21, "rows between header and text");
}

#[test]
fn a_post_picture_should_fill_the_width_when_wide_and_stay_within_half_when_tall() {
    // 8×16 px cells, 100 columns; a wide picture may take 40 rows, a tall one 20.
    let font = ratatui_image::FontSize::new(8, 16);
    let cells = |w, h| {
        let size = picture_cells(w, h, font, 100, 40, 20, MAX_ENLARGE);
        (size.width, size.height)
    };

    assert_eq!(cells(1600, 900), (100, 29), "wide: the whole width");
    assert_eq!(
        cells(4000, 400),
        (100, 5),
        "very wide: still the whole width"
    );
    assert_eq!(
        cells(800, 700),
        (91, 40),
        "barely wide: until the wide limit"
    );
    assert_eq!(cells(900, 1600), (23, 20), "tall: within half");
    assert_eq!(cells(1000, 1000), (40, 20), "square counts as tall");
    assert_eq!(cells(140, 73), (35, 10), "small: at most twice its size");
}

#[test]
fn a_picture_fitted_again_into_its_cells_should_keep_them() {
    // The layout sizes a picture, then the draw fits it into those cells: a wide one
    // must still fill the width. 4000×420 is 5.25 rows across 100 columns of 8×16.
    let font = ratatui_image::FontSize::new(8, 16);
    for (w, h) in [
        (4000, 420),
        (400, 3000),
        (275, 1500),
        (1500, 1000),
        (1600, 900),
        (900, 1600),
        (140, 73),
    ] {
        let laid = picture_cells(w, h, font, 100, 40, 20, MAX_ENLARGE);
        let drawn = picture_cells(
            w,
            h,
            font,
            laid.width,
            laid.height,
            laid.height,
            MAX_ENLARGE,
        );
        assert_eq!(drawn, laid, "{w}×{h}");
    }
}

#[test]
fn main_media_should_fit_the_usable_height_in_cards_and_detail() {
    for (width, height) in [(80, 20), (100, 40), (200, 20)] {
        for (pixels, video) in [
            ((320, 1280), false),
            ((1080, 1920), true),
            ((1600, 900), false),
        ] {
            let mut app = card_feed_app(kitty());
            app.tabs[4].feed.posts.truncate(1);
            let post = &mut app.tabs[4].feed.posts[0];
            post.preview_size = Some(pixels);
            if video {
                post.media = Media::Video {
                    gif: false,
                    url: Some(VIDEO.into()),
                    poster: None,
                };
            }
            let mut pictures = ListPictures::default();
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|f| {
                    draw_feed(f, &mut app, f.area(), &mut pictures);
                })
                .unwrap();
            let picture = &pictures.cards[0];
            assert_eq!(
                picture.area.as_size(),
                picture.size,
                "card {pixels:?} in {width}x{height}"
            );
            assert_eq!(picture.skipped, 0);
            assert!(
                usize::from(picture.size.height) * 5 <= app.active_tab().feed.cards.viewport * 3,
                "card media must stay within 60% of its viewport"
            );
            assert!(app.active_tab().feed.cards.height <= app.active_tab().feed.cards.viewport);

            let mut app = if video {
                open_video_post()
            } else {
                image_post("caption")
            };
            app.post_view
                .as_mut()
                .unwrap()
                .page
                .as_mut()
                .unwrap()
                .post
                .preview_size = Some(pixels);
            let pictures = detail_pictures(&mut app, width, height);
            let picture = &pictures[0];
            assert_eq!(
                picture.area.as_size(),
                picture.size,
                "detail {pixels:?} in {width}x{height}"
            );
            assert_eq!(picture.skipped, 0);
            assert!(
                usize::from(picture.size.height) * 5
                    <= app.post_view.as_ref().unwrap().page_height * 3,
                "detail media must stay within 60% of its viewport"
            );
            assert!(
                picture.area.bottom() < height,
                "leave space for status/spacing"
            );
            assert!(picture.area.right() <= width);
            if video {
                assert_eq!(app.video_area.unwrap().as_size(), picture.size);
            }
        }
    }
}

#[test]
fn a_tall_detail_image_should_fit_below_the_fixed_header() {
    let mut app = image_post("body below the full image");
    let slot = ready(&app, 320, 1280);
    app.images.insert("https://i.redd.it/big.jpeg".into(), slot);

    let shown = screen(&mut app, 100, 40);
    let view = app.post_view.as_ref().unwrap();

    assert!(shown.contains("body below the full image"));
    assert_eq!(view.rendered_pictures.first().map(|p| p.2), Some(19));
    assert_eq!(view.scroll, 0);
}

#[test]
fn a_detail_image_should_clip_visible_rows_and_scroll_continuously_into_the_body() {
    let mut app = image_post(&paragraphs("body after image", 1..=20));
    let slot = ready(&app, 320, 1280);
    app.images.insert("https://i.redd.it/big.jpeg".into(), slot);
    let first = detail_pictures(&mut app, 80, 30);
    assert_eq!(first[0].size, Size::new(8, 15));
    assert_eq!(first[0].skipped, 0);
    assert_eq!(first[0].area.width, 8);

    app.scroll_post(6);
    let middle = detail_pictures(&mut app, 80, 30);
    assert_eq!(middle[0].skipped, 5);
    assert_eq!(
        middle[0].area.height,
        first[0].size.height - middle[0].skipped
    );
    assert_eq!(app.post_view.as_ref().unwrap().scroll, 6);

    app.scroll_post(6);
    let end = detail_pictures(&mut app, 80, 30);
    assert_eq!(end[0].skipped, 11);
    assert_eq!(end[0].area.height, 4);
    assert_eq!(app.post_view.as_ref().unwrap().scroll, 12);
    app.scroll_post(-6);
    let back = detail_pictures(&mut app, 80, 30);
    assert_eq!(back[0].skipped, 5);
}

#[test]
fn when_detail_uses_sixel_it_should_keep_viewport_fitted_document_geometry() {
    let mut app = open_video_post();
    app.image_picker.set_protocol_type(ProtocolType::Sixel);
    app.post_view
        .as_mut()
        .unwrap()
        .page
        .as_mut()
        .unwrap()
        .post
        .preview_size = Some((1080, 1920));
    let actual = detail_pictures(&mut app, 80, 30);
    assert_eq!(actual[0].size, Size::new(17, 15));
}

#[test]
fn a_loading_detail_image_should_reserve_known_geometry_and_keep_it_after_failure() {
    let mut app = image_post("body after image");
    app.post_view
        .as_mut()
        .unwrap()
        .page
        .as_mut()
        .unwrap()
        .post
        .preview_size = Some((320, 1280));
    let first = detail_pictures(&mut app, 80, 30);
    assert_eq!(first[0].size, Size::new(8, 15));
    app.images.insert(
        "https://i.redd.it/big.jpeg".into(),
        ImageSlot::Failed("offline".into()),
    );
    let failed = detail_pictures(&mut app, 80, 30);
    assert_eq!(failed[0].size, first[0].size);

    app.post_view.as_mut().unwrap().page = None.into();
    assert!(detail_pictures(&mut app, 80, 30).is_empty());
}
