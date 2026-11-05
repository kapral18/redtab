use super::*;

#[test]
fn popup_description_should_keep_scrollable_text_visible_beside_crowded_short_lists() {
    use crate::reddit::experience::Query;
    let body = (0..20)
        .map(|row| format!("Line {row:02}\n"))
        .collect::<String>();
    for (width, height, summary, bodies) in [
        (20, 8, body.as_str(), vec![""; 3]),
        (24, 10, "", vec![body.as_str(); 8]),
        (20, 7, "", vec![body.as_str(); 3]),
    ] {
        let mut app = description_popup(Query::Profile("reader".into()), summary, &bodies);
        let (area, before) = popup_snapshot(&mut app, width, height);
        assert!(before.contains("Entry 0"), "{before}");
        assert!(before.contains("Line 00"), "{width}x{height}: {before}");
        key(&mut app, KeyCode::Char('e'), KeyModifiers::CONTROL);
        let (after_area, after) = popup_snapshot(&mut app, width, height);
        assert_eq!(area, after_area);
        assert!(after.contains("Line 01"), "{after}");
        key(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        let (selected_area, selected) = popup_snapshot(&mut app, width, height);
        assert_eq!(area, selected_area);
        assert!(selected.contains("Entry 1"), "{selected}");
        assert!(selected.contains("Line 00"), "{selected}");
    }
}

#[test]
fn when_home_sort_opens_in_a_short_terminal_it_should_show_every_option() {
    let mut app = app(&[]);
    app.switch_tab(0);
    app.overlay = Some(Overlay::Sort { selected: 5 });

    let shown = screen(&mut app, 80, 10);

    assert!(shown.contains("rising"), "{shown}");
    assert!(shown.contains("controversial"), "{shown}");
    assert!(shown.contains("▌ c"), "{shown}");
}

#[test]
fn review_regression_wrapped_selected_menu_labels_should_stay_wholly_visible() {
    let mut app = app(&[]);
    app.reddit_menu();
    let Some(crate::app::experience::Screen::Menu {
        choices, selected, ..
    }) = app.experience.screens.last_mut()
    else {
        panic!("menu missing")
    };
    *selected = choices
        .iter()
        .position(|choice| choice.label.starts_with("Privacy, blocked users"))
        .unwrap();
    let actual = screen(&mut app, 40, 12);
    assert!(actual.contains("profile settings"), "{actual}");
    assert!(actual.contains("Privacy, blocked"), "{actual}");
    assert!(actual.contains("Esc back"), "{actual}");
}

#[test]
fn when_a_community_is_open_it_should_show_membership_and_discoverable_controls() {
    let mut app = app(&["rust"]);
    app.open_subreddit("rust");
    let shown = screen(&mut app, 120, 24);
    assert!(
        shown.contains("Joined") && shown.contains("SPC m leave"),
        "{shown}"
    );
    app.open_subreddit("armenia");
    let shown = screen(&mut app, 120, 24);
    assert!(shown.contains("Not joined"), "{shown}");
    app.set_membership(
        crate::model::MembershipTarget::Community("armenia".into()),
        true,
    );
    let shown = screen(&mut app, 120, 24);
    assert!(shown.contains("Joining r/armenia"), "{shown}");
}

#[test]
fn when_home_sort_opens_it_should_explain_where_recommendations_appear() {
    let mut app = app(&[]);
    app.switch_tab(0);
    app.overlay = Some(Overlay::Sort { selected: 0 });

    let shown = screen(&mut app, 80, 24);

    assert!(shown.contains("recommendations"), "{shown}");
}

#[test]
fn when_home_sorts_change_best_should_be_available_only_on_home() {
    let mut app = app(&["rust"]);
    app.switch_tab(0);
    press_keys(&mut app, " fs");
    assert!(screen(&mut app, 80, 24).contains("best"));
    press(&mut app, 'n');
    assert_eq!(app.active_tab().feed.sort, Sort::New);
    app.open_subreddit("rust");
    press_keys(&mut app, " fs");
    assert!(!screen(&mut app, 80, 24).contains("best"));
}

#[test]
fn popup_media_jump_should_preserve_card_pixels_around_its_bounds() {
    let mut app = card_with_image(640, 320);
    let before = placeholder_cells(&mut app, 100, 40);
    press_keys(&mut app, " bb");

    let actual = placeholder_cells(&mut app, 100, 40);

    assert!(!actual.is_empty());
    assert!(
        actual
            .iter()
            .all(|&(x, y)| !Rect::new(22, 15, 56, 10).contains((x, y).into()))
    );
    assert!(
        actual
            .iter()
            .any(|&(x, y)| x >= 78 && (15..25).contains(&y)),
        "right-side image strip remains visible"
    );
    crate::keys::handle(
        &mut app,
        crossterm::event::KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
    );
    assert_eq!(placeholder_cells(&mut app, 100, 40), before);
}

#[test]
fn popup_media_jump_should_preserve_iterm2_card_fragments() {
    // 528×256 fills the 66-column card in exactly 16 rows, so no tile holds padding.
    let mut app = card_with_image(528, 256);
    app.image_picker.set_protocol_type(ProtocolType::Iterm2);
    let CardImageSlot::Ready { image, .. } = app
        .card_images
        .get_mut("https://i.redd.it/p1.jpeg")
        .unwrap()
    else {
        panic!("loaded card")
    };
    *image = std::sync::Arc::new(image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(
        528,
        256,
        |x, _| image::Rgb([if x < 264 { 0 } else { 255 }, 0, 0]),
    )));
    screen(&mut app, 100, 40);
    press_keys(&mut app, " bb");

    let actual = screen(&mut app, 100, 40);

    assert!(actual.contains("doNotMoveCursor=1:"));
    assert!(actual.contains("Jump"));
    let CardImageSlot::Ready {
        protocol: Some(protocol),
        ..
    } = app.card_images.get("https://i.redd.it/p1.jpeg").unwrap()
    else {
        panic!("encoded fragments")
    };
    let right: Vec<_> = protocol
        .tiles
        .iter()
        .filter(|(key, _)| key.1 == 50)
        .collect();
    assert!(!right.is_empty());
    assert!(
        right.iter().all(|(_, tile)| match tile.as_ref() {
            crate::card_image::Tile::Image(protocol) =>
                card_png(protocol).pixels().all(|p| p.0 == [255, 0, 0]),
            _ => false,
        }),
        "right strips must sample the original image's right half"
    );
}

#[test]
fn popup_media_jump_should_preserve_sixel_card_fragments() {
    let mut app = card_with_image(320, 640);
    app.image_picker.set_protocol_type(ProtocolType::Sixel);
    press_keys(&mut app, " bb");

    let actual = screen(&mut app, 100, 40);

    assert!(actual.contains("\u{1b}P"));
    assert!(actual.contains("Jump"));
}

#[test]
fn popup_media_jump_should_preserve_halfblock_pixels_outside_the_popup() {
    let mut app = card_with_image(320, 640);
    app.image_picker.set_protocol_type(ProtocolType::Halfblocks);
    press_keys(&mut app, " bb");
    let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
    terminal.draw(|f| draw(f, &mut app)).unwrap();
    while app.card_renderer.pending() {
        app.card_renderer.wait_for_completion();
        app.card_renderer.poll();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
    }

    let actual = terminal.backend().buffer();

    assert!(
        actual
            .content
            .iter()
            .any(|cell| cell.fg == Color::Rgb(0, 0, 0) || cell.bg == Color::Rgb(0, 0, 0))
    );
    assert!((15..25).all(|y| {
        (22..78).all(|x| {
            actual[(x, y)].fg != Color::Rgb(0, 0, 0) && actual[(x, y)].bg != Color::Rgb(0, 0, 0)
        })
    }));
}

#[test]
fn popup_media_jump_should_keep_the_paused_detail_video_frame() {
    let mut app = open_video_post();
    let url = app.reddit.media_url(VIDEO).unwrap();
    let frame = video::Frame {
        width: 160,
        height: 90,
        pixels: video::Pixels::Raw(std::sync::Arc::new(image::DynamicImage::new_rgb8(160, 90))),
        same_as_first: false,
    };
    let mut player = video::Player::showing(&url, frame);
    player.area = app.video_area.unwrap().as_size();
    app.video = Some(player);
    press_keys(&mut app, "p");
    press_keys(&mut app, " bb");
    app.tick();

    let actual = placeholder_cells(&mut app, 100, 40);

    assert!(!actual.is_empty());
    assert!(app.video.as_ref().unwrap().hidden());
    assert!(app.video.as_ref().unwrap().paused());
    crate::keys::handle(
        &mut app,
        crossterm::event::KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
    );
    app.tick();
    assert!(app.video.as_ref().unwrap().paused());
    assert!(!app.video.as_ref().unwrap().hidden());
}

#[test]
fn popup_media_inline_errors_should_not_paint_over_the_dialog() {
    let mut app = image_post("");
    let url = "https://i.redd.it/failed.png";
    app.images
        .insert(url.into(), ImageSlot::Failed("unavailable".into()));
    let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
    let mut expected = None;

    terminal
        .draw(|f| {
            f.render_widget(
                Paragraph::new("Opaque popup\nOpaque popup\nOpaque popup"),
                f.area(),
            );
            expected = Some(f.buffer_mut().clone());
            draw_inline_image(
                f,
                &mut app,
                url,
                Rect::new(0, 0, 40, 3),
                Some(Rect::new(0, 0, 40, 3)),
            );
        })
        .unwrap();

    assert_eq!(terminal.backend().buffer(), &expected.unwrap());
}

#[test]
fn thumbnails_wait_for_an_overlay_to_close() {
    let mut app = feed_app(kitty());
    screen(&mut app, 100, 20);
    let slot = ready(&app, 32, 32);
    app.thumbs.insert("https://i.redd.it/p1.jpeg".into(), slot);

    app.help_scroll = Some(0);
    let under_help = placeholder_cells(&mut app, 100, 20);
    app.help_scroll = None;
    let after = placeholder_cells(&mut app, 100, 20);

    // kitty sends an image once, in its first cell: drawing it under the
    // popup would lose it, so it first draws after the popup closes.
    assert!(under_help.is_empty(), "{under_help:?}");
    assert_eq!(after.len(), 8, "the whole 4×2 slot");
}

#[test]
fn ui_audit_caret_should_remain_visible_at_a_short_form_wrap_boundary() {
    let mut app = form_app("Create text post");
    let Some(crate::app::experience::Screen::Input(form)) = app.experience.screens.last_mut()
    else {
        panic!("form missing")
    };
    form.fields.truncate(1);
    form.fields[0].value = "12345678901234567890123456789012".into();
    form.fields[0].cursor = 32;
    let actual = screen(&mut app, 40, 6);
    assert!(actual.contains('▏'), "{actual}");
}

#[test]
fn ui_audit_short_membership_dialogs_should_keep_target_and_cancel_visible() {
    for name in ["rust", "a_very_long_community"] {
        for join in [true, false] {
            for (width, height) in [(20, 6), (20, 8), (24, 8), (40, 12)] {
                let mut app = app(&[]);
                app.overlay = Some(Overlay::Membership {
                    target: crate::model::MembershipTarget::Community(name.into()),
                    join,
                });
                let body = auth_content(&mut app, width, height);
                let actual = screen(&mut app, width, height);
                assert!(body.contains(&format!("r/{name}")), "{actual}");
                assert!(actual.contains("Esc") || actual.contains('⎋'), "{actual}");
                assert!(actual.contains("Enter") || actual.contains('⏎'), "{actual}");
            }
        }
    }
}

#[test]
fn popup_media_help_should_preserve_thumbnails_outside_its_bounds() {
    let mut app = feed_app(kitty());
    screen(&mut app, 200, 40);
    let slot = ready(&app, 32, 32);
    app.thumbs.insert("https://i.redd.it/p1.jpeg".into(), slot);
    app.help_scroll = Some(0);

    let actual = placeholder_cells(&mut app, 200, 40);

    assert_eq!(actual.len(), 8, "uncovered 4x2 thumbnail");
}
