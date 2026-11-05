use super::*;

#[test]
fn a_video_frame_over_its_share_of_output_shows_late_rather_than_never() {
    let mut app = card_playback_app();
    #[allow(deprecated)]
    let mut picker =
        ratatui_image::picker::Picker::from_fontsize(ratatui_image::FontSize::new(8, 16));
    picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
    app.image_picker = picker;
    let now = app.card_video.as_ref().unwrap().since + NAV_SETTLE;
    app.sync_video_with(now, |url, area, _| {
        let mut player = Player::showing(&url, playback_frame(1)).at_rate(30.0);
        player.area = area;
        player
    });
    // Noise compresses to a frame between a 30 fps and a 24 fps share of the budget.
    let mut seed = 7u32;
    let mut noise = || {
        std::sync::Arc::new(image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(
            344,
            352,
            |_, _| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                image::Rgb([(seed >> 16) as u8, (seed >> 8) as u8, seed as u8])
            },
        )))
    };
    let mut key = crate::card_image::Key::new(
        "video",
        &app.image_picker,
        Size::new(43, 22),
        (344, 352),
        0,
        22,
        0,
    );
    key.video = true;
    let draw = |app: &mut App, image: &std::sync::Arc<image::DynamicImage>| {
        app.card_renderer.begin();
        app.card_renderer.tile(
            key.clone(),
            image,
            &app.image_picker,
            crate::card_image::Preparation::Refresh,
        );
        app.card_renderer.finish();
    };
    draw(&mut app, &noise());
    app.card_renderer.wait_for_completion();
    app.card_renderer.poll();

    let held = noise();
    draw(&mut app, &held);
    assert!(
        !app.card_renderer.pending(),
        "at 30 frames a second it waits for the output budget"
    );

    let wait = app.card_renderer.video_wake_in(Instant::now()).unwrap();
    // The wait alone wakes the loop, even with no frame of the player's due sooner.
    app.video = None;
    app.card_video = None;
    assert!(app.wake_in() <= wait);
    std::thread::sleep(wait);
    assert!(app.card_renderer.poll(), "the wait ends in a redraw");
    draw(&mut app, &held);
    assert!(app.card_renderer.pending(), "the frame goes out then");
}

#[test]
fn when_graphics_are_pending_selected_video_should_still_consume_its_frames() {
    let mut app = card_playback_app();
    let now = app.card_video.as_ref().unwrap().since + NAV_SETTLE;
    app.sync_video_with(now, |url, area, _| Player::pending(&url, area));
    app.video = Some(Player::buffered(
        &app.video.as_ref().unwrap().url,
        Size::new(68, 68),
        playback_frame(2),
    ));
    let image = std::sync::Arc::new(image::DynamicImage::new_rgb8(4, 4));
    let mut key = crate::card_image::Key::new(
        "pending-video",
        &app.image_picker,
        Size::new(4, 4),
        (4, 4),
        0,
        4,
        0,
    );
    key.video = true;
    app.card_renderer.begin();
    app.card_renderer.tile(
        key,
        &image,
        &app.image_picker,
        crate::card_image::Preparation::Refresh,
    );
    app.card_renderer.finish();
    assert!(app.card_renderer.pending());

    app.sync_video_with(now, |_, _, _| panic!("keep the selected decoder"));

    assert!(app.video.as_ref().unwrap().frame().is_some());
}
