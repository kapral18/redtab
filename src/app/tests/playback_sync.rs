use super::*;

#[test]
fn when_graphics_are_pending_selected_video_should_still_consume_its_frames() {
    let mut app = card_playback_app();
    let now = app.card_video.as_ref().unwrap().since + NAV_SETTLE;
    app.sync_video_with(now, |url, area, _| Player::pending(&url, area));
    app.set_video(Some(Player::buffered(
        &app.video().unwrap().url,
        Size::new(68, 68),
        playback_frame(2),
    )));
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

    assert!(app.video().unwrap().frame().is_some());
}
