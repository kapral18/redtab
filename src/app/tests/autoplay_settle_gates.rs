use super::*;

#[test]
fn when_a_youtube_card_is_selected_it_should_reuse_debounce_and_manual_play_gates() {
    let mut app = card_playback_app();
    app.tabs[0].feed.posts[0].media = Media::Link {
        url: "https://youtu.be/dQw4w9WgXcQ?feature=share".into(),
        thumb: None,
    };
    app.card_video_drawn(Some(Size::new(68, 38)));
    let now = app.card_video.as_ref().unwrap().since;
    assert_eq!(app.wanted_video(now), None);
    let settled = now + NAV_SETTLE;
    assert_eq!(
        app.wanted_video(settled).as_deref(),
        Some("https://www.youtube.com/watch?v=dQw4w9WgXcQ")
    );
    app.config.media.video_autoplay = false;
    assert_eq!(app.wanted_video(settled), None);

    app.toggle_video_pause();

    assert_eq!(
        app.wanted_video(settled).as_deref(),
        Some("https://www.youtube.com/watch?v=dQw4w9WgXcQ")
    );
}

#[test]
fn card_media_sidebar_navigation_should_autoplay_the_visible_settled_selection() {
    let mut app = card_playback_app();
    app.focus = Focus::Sidebar;
    app.card_video_drawn(Some(Size::new(68, 68)));
    let since = app.card_video.as_ref().unwrap().since;

    assert!(app.wanted_video(since).is_none());
    assert!(app.wanted_video(since + NAV_SETTLE).is_some());
}

#[test]
fn card_media_spoilers_and_disabled_images_should_not_request_playback() {
    let mut app = card_playback_app();
    app.tabs[0].feed.posts[0].spoiler = true;
    app.card_video_drawn(Some(Size::new(68, 68)));
    assert!(!app.video_controls_available());
    assert!(app.wanted_video(Instant::now() + NAV_SETTLE).is_none());

    app.tabs[0].feed.posts[0].spoiler = false;
    app.show_images = false;
    app.card_video_drawn(Some(Size::new(68, 68)));

    assert!(!app.video_controls_available());
    assert!(app.wanted_video(Instant::now() + NAV_SETTLE).is_none());
}

#[test]
fn when_resizing_video_scrolls_offscreen_its_settle_deadline_should_be_cancelled() {
    let mut app = card_playback_app();
    let start = Instant::now() - Duration::from_secs(2);
    app.card_video.as_mut().unwrap().since = start - NAV_SETTLE;
    app.sync_video_with(start, |url, area, _| Player::silent(&url, area));
    assert!(app.video().is_some());
    app.card_video_drawn(Some(Size::new(40, 80)));
    let resized = start + Duration::from_millis(1);
    app.sync_video_with(resized, |_, _, _| {
        panic!("resize must settle before restarting")
    });
    assert!(app.video_resize.is_some());
    app.card_video_drawn(None);
    app.sync_video_with(resized + VIDEO_RESIZE_SETTLE, |_, _, _| {
        panic!("offscreen video must not restart")
    });
    assert!(app.video().unwrap().hidden());
    assert!(
        app.video_resize.is_none(),
        "an absent video area cannot finish a pending resize"
    );
    assert!(
        !app.wake_in().is_zero(),
        "the removed area's expired deadline must not spin"
    );
    app.card_video_drawn(Some(Size::new(40, 80)));
    let returned = Instant::now();
    app.sync_video_with(returned, |_, _, _| {
        panic!("return starts a fresh resize settle")
    });
    assert_eq!(app.video_resize, Some((Size::new(40, 80), returned)));
    app.sync_video_with(returned + VIDEO_RESIZE_SETTLE, |url, area, _| {
        Player::silent(&url, area)
    });
    assert!(app.video_resize.is_none());
    assert_eq!(app.video().unwrap().area, Size::new(40, 80));
}

#[test]
fn card_media_settle_tick_should_start_without_an_extra_draw() {
    let mut app = card_playback_app();
    let since = app.card_video.as_ref().unwrap().since;
    app.sync_video_with(since, |_, _, _| panic!("must wait for settling"));

    app.sync_video_with(since + NAV_SETTLE, |url, area, _| {
        Player::silent(&url, area)
    });

    assert!(app.card_player().is_some());
}
