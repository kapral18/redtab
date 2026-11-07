use super::*;

#[test]
fn when_a_scrolled_loading_card_receives_pixels_it_should_keep_the_visible_row() {
    let mut app = card_feed_app(kitty());
    app.tabs[4].feed.posts.truncate(1);
    app.tabs[4].feed.posts.push(post_with("tail", Media::None));
    app.tabs[4].feed.posts.push(post_with("tail2", Media::None));
    app.tabs[4].feed.posts[0].title = "Loading image".into();
    screen(&mut app, 100, 20);
    app.scroll_card(3);
    screen(&mut app, 100, 20);
    assert_eq!(app.active_tab().feed.cards.line, 3);
    app.scroll_card(-1);
    screen(&mut app, 100, 20);

    app.on_msg(Msg::Image {
        url: "https://i.redd.it/p1.jpeg".into(),
        result: Ok(image::DynamicImage::new_rgb8(640, 1280).into()),
    });
    screen(&mut app, 100, 20);

    assert_eq!(app.active_tab().feed.cards.line, 2);
    assert_eq!(app.active_tab().feed.cards.pending, 0);
}

#[test]
fn a_top_card_scrolled_into_keeps_its_place_when_an_answer_reranks_or_drops_it() {
    // The scroll may not be drawn yet when the answer arrives.
    for (drawn, drop) in [(true, false), (false, false), (true, true), (false, true)] {
        let mut app = card_feed_app(kitty());
        screen(&mut app, 100, 20);
        let read = app.current_post().unwrap().id;
        app.scroll_card(3);
        if drawn {
            screen(&mut app, 100, 20);
            assert!(app.active_tab().feed.cards.line > 0);
        }
        let mut posts = app.tabs[4].feed.posts.clone();
        if drop {
            posts.remove(0);
        } else {
            posts.swap(0, 1);
        }
        let (tab, generation) = (app.tabs[4].id, app.tabs[4].feed.generation);

        app.on_msg(Msg::Feed {
            tab,
            generation,
            append: false,
            result: Ok(crate::model::Listing { posts, after: None }),
        });
        screen(&mut app, 100, 20);

        assert_eq!(
            app.current_post().unwrap().id,
            read,
            "drawn {drawn} drop {drop}"
        );
        assert_eq!(app.active_tab().feed.cards.line, 3);
    }

    // Scrolling back to the top, or up at it, before the answer is drawn leaves the
    // top untouched: it shows the answer's first post from its first row.
    for back in [true, false] {
        let mut app = card_feed_app(kitty());
        screen(&mut app, 100, 20);
        if back {
            app.scroll_card(3);
            screen(&mut app, 100, 20);
            app.scroll_card(-3);
        } else {
            app.scroll_card(-5);
        }
        let mut posts = app.tabs[4].feed.posts.clone();
        posts.swap(0, 1);
        let first = posts[0].id.clone();
        let (tab, generation) = (app.tabs[4].id, app.tabs[4].feed.generation);

        app.on_msg(Msg::Feed {
            tab,
            generation,
            append: false,
            result: Ok(crate::model::Listing { posts, after: None }),
        });
        screen(&mut app, 100, 20);

        assert_eq!(app.current_post().unwrap().id, first, "back {back}");
        assert_eq!(app.active_tab().feed.selected, 0);
        assert_eq!(app.active_tab().feed.cards.line, 0);
    }
}

#[test]
fn when_scrolling_from_a_short_card_into_tall_media_it_should_keep_the_overshoot() {
    let mut app = two_tall_media_cards();
    app.tabs[4].feed.posts[0].media = Media::None;
    let mut following = app.tabs[4].feed.posts[1].clone();
    following.id = "tail".into();
    app.tabs[4].feed.posts.push(following);
    screen(&mut app, 100, 20);

    app.page_list(1, 2);
    screen(&mut app, 100, 20);

    assert_eq!(app.active_tab().feed.offset, 1);
    assert_eq!(app.active_tab().feed.cards.line, 3);
    assert_eq!(app.current_post().unwrap().id, "p2");
}

#[test]
fn when_resize_shortens_the_anchor_card_it_should_clamp_to_that_cards_content() {
    let mut app = two_tall_media_cards();
    app.scroll_card(14);
    screen(&mut app, 100, 20);

    screen(&mut app, 80, 12);

    assert_eq!(app.active_tab().feed.cards.post.as_deref(), Some("p1"));
    assert_eq!(app.active_tab().feed.cards.line, 5);
    assert_eq!(app.current_post().unwrap().id, "p1");
}

#[test]
fn when_scrolling_up_a_card_by_control_y_it_should_move_one_line_without_copying() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = card_with_image(320, 640);
    app.tabs[4].feed.posts.push(post_with("tail", Media::None));
    app.tabs[4].feed.posts.push(post_with("tail2", Media::None));
    screen(&mut app, 100, 20);
    app.scroll_card(5);

    crate::keys::handle(
        &mut app,
        KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL),
    );
    screen(&mut app, 100, 20);

    assert_eq!(app.active_tab().feed.cards.line, 4);
    assert!(app.status.is_none(), "Ctrl+y must not copy a link");
}

#[test]
fn when_returning_from_fullscreen_detail_a_reloading_card_should_keep_its_scroll() {
    let mut app = card_with_image(320, 640);
    app.tabs[4].feed.posts.push(post_with("tail", Media::None));
    app.tabs[4].feed.posts.push(post_with("tail2", Media::None));
    screen(&mut app, 100, 20);
    app.scroll_card(3);
    screen(&mut app, 100, 20);
    app.open_current_post();
    screen(&mut app, 100, 20);
    app.close_post();
    screen(&mut app, 100, 20);
    assert_eq!(app.active_tab().feed.cards.line, 3);

    app.on_msg(Msg::Image {
        url: "https://i.redd.it/p1.jpeg".into(),
        result: Ok(image::DynamicImage::new_rgb8(320, 640).into()),
    });
    placeholder_cells(&mut app, 100, 20);

    assert_eq!(app.active_tab().feed.cards.line, 3);
    assert_eq!(app.current_post().unwrap().id, "p1");
}

#[test]
fn when_scrolling_an_iterm2_or_sixel_card_it_should_encode_only_the_visible_image_rows() {
    for protocol_type in [ProtocolType::Iterm2, ProtocolType::Sixel] {
        let mut app = card_with_image(320, 640);
        app.tabs[4].feed.posts.push(post_with("tail", Media::None));
        app.tabs[4].feed.posts.push(post_with("tail2", Media::None));
        app.image_picker.set_protocol_type(protocol_type);
        screen(&mut app, 100, 20);
        app.scroll_card(3);
        app.tick_at(std::time::Instant::now() + crate::app::NAV_SETTLE);

        screen(&mut app, 100, 20);

        let CardImageSlot::Ready {
            protocol: Some(protocol),
            ..
        } = app.card_images.get("https://i.redd.it/p1.jpeg").unwrap()
        else {
            panic!("card protocol");
        };
        assert_eq!(protocol.tiles.len(), 1, "{protocol_type:?}");
        assert_eq!(protocol.tiles[&(1, 0, 7, 8)].size(), Size::new(8, 7));
        let encoded = match &*protocol.tiles[&(1, 0, 7, 8)] {
            crate::card_image::Tile::Image(ratatui_image::protocol::Protocol::ITerm2(_)) => {
                ProtocolType::Iterm2
            }
            crate::card_image::Tile::Image(ratatui_image::protocol::Protocol::Sixel(_)) => {
                ProtocolType::Sixel
            }
            _ => panic!("unexpected tile for {protocol_type:?}"),
        };
        assert_eq!(encoded, protocol_type);
    }
}

#[test]
fn when_cards_scroll_in_a_short_pane_the_selected_post_should_stay_visible() {
    let mut app = card_feed_app(Picker::halfblocks());
    app.move_selection(7);

    let actual = screen(&mut app, 60, 9);

    assert!(actual.contains("p8"), "{actual}");
    assert_eq!(app.current_post().unwrap().id, "p8");
}

#[test]
fn when_scrolling_an_upscaled_card_its_pixels_should_keep_the_same_scale() {
    let mut picker = kitty();
    picker.set_protocol_type(ProtocolType::Iterm2);
    let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(2, 2, |x, y| {
        image::Rgb([(x * 255) as u8, (y * 255) as u8, 0])
    }));
    let tile = |start| {
        let protocol = encode_tile(
            &picker,
            &image,
            None,
            (Size::new(68, 34), (2, 2), None),
            start,
            16,
            0,
        )
        .unwrap();
        card_png(&protocol)
    };

    let first = tile(14);
    let next = tile(15);

    // A square 2×2 source fills 544×544 pixels, even when only a slice is visible.
    assert_eq!(first.dimensions(), (544, 256));
    assert!((82..=84).contains(&first.get_pixel(272, 0)[1]));
    assert_ne!(first.get_pixel(272, 0), next.get_pixel(272, 0));
    assert_eq!(
        image::imageops::crop_imm(&first, 0, 16, 544, 240).to_image(),
        image::imageops::crop_imm(&next, 0, 0, 544, 240).to_image()
    );
}
