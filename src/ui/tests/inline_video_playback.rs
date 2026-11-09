use super::*;

#[test]
fn an_unselected_video_without_autoplay_loads_and_displays_a_still_frame() {
    let mut app = card_feed_app(kitty());
    app.config.media.video_autoplay = false;
    let mut first = post_with(
        "video",
        Media::Video {
            gif: false,
            url: Some(VIDEO.into()),
            poster: None,
        },
    );
    first.title = "Video preview".into();
    first.preview_size = Some((160, 90));
    let mut selected = post_with("selected", Media::None);
    selected.title = "Selected text post".into();
    app.tabs[4].feed.posts = vec![first, selected];
    app.tabs[4].feed.selected = 1;
    app.tabs[4].feed.offset = 0;
    app.tabs[4].feed.cards = Default::default();
    first_screen(&mut app, 100, 40);
    assert!(
        app.card_images.contains_key(VIDEO),
        "visible unselected video must request a frame"
    );

    app.on_msg(Msg::Image {
        url: VIDEO.into(),
        result: Ok(image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            160,
            90,
            image::Rgb([180, 80, 50]),
        ))
        .into()),
    });
    let actual = screen(&mut app, 100, 40);

    assert!(actual.contains('\u{10EEEE}'), "{actual}");
    assert!(!actual.contains("select to preview"), "{actual}");
    assert!(
        app.video().is_none(),
        "a still preview must not start playback"
    );
}

#[test]
fn reported_failed_video_card_should_compact_and_retry_without_resizing() {
    let mut app = card_feed_app(kitty());
    app.tabs[4].feed.posts[0].media = Media::Video {
        gif: false,
        url: Some(VIDEO.into()),
        poster: None,
    };
    screen(&mut app, 100, 30);
    let url = app.reddit.media_url(VIDEO).unwrap();
    app.set_video(Some(video::Player::unavailable(
        &url,
        app.video_area.unwrap().as_size(),
    )));
    let actual = screen(&mut app, 100, 30);
    assert!(
        app.active_tab().feed.cards.height <= 7,
        "failed media reserved empty rows: {actual}"
    );
    assert!(
        app.video_area.is_none(),
        "compaction must not schedule a decoder resize"
    );
    assert!(actual.contains("p retry"), "{actual}");
    app.toggle_video_pause();
    screen(&mut app, 100, 30);
    assert!(app.video_area.is_some(), "retry restores media geometry");
}

#[test]
fn the_video_plays_only_while_its_post_shows_it() {
    let mut app = open_video_post();
    let step = |app: &mut App| {
        screen(app, 100, 30);
        if app.video().is_none()
            && app.inline_images_on()
            && let Some(area) = app.video_area
        {
            let url = app.reddit.media_url(VIDEO).unwrap();
            app.set_video(Some(video::Player::silent(&url, area.as_size())));
        }
        app.tick();
        app.video().map(|p| p.hidden())
    };

    assert_eq!(step(&mut app), Some(false), "starts with the post");
    press(&mut app, '?');
    assert_eq!(step(&mut app), Some(true), "waits under the help");
    app.help_scroll = None;
    assert_eq!(step(&mut app), Some(false));
    key(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
    assert_eq!(step(&mut app), Some(true), "waits while scrolled away");
    press_keys(&mut app, "gg");
    assert_eq!(step(&mut app), Some(false));
    press_keys(&mut app, "zi");
    assert_eq!(step(&mut app), None, "images off stops it");
    press_keys(&mut app, "zi");
    assert_eq!(step(&mut app), Some(false));
    press(&mut app, 'q');
    assert_eq!(step(&mut app), None, "leaving the post stops it");
}

#[test]
fn when_a_gif_post_opens_it_should_play_inline_without_an_image_download() {
    let (mut app, _) = open_long_post(|_| {});
    app.image_picker = kitty();
    app.show_images = true;
    let view = app.post_view.as_mut().unwrap();
    let generation = view.generation;
    let mut page = view.page.take().unwrap();
    page.post.media = Media::Image {
        url: "https://i.redd.it/f5ndv6yotovh1.gif".into(),
        preview: None,
    };

    app.on_msg(Msg::Post {
        generation,
        result: Ok(Box::new(page)),
    });
    screen(&mut app, 100, 30);
    app.tick();

    let player = app.video().expect("GIF should start the video player");
    assert_eq!(
        player.url,
        app.reddit
            .media_url("https://i.redd.it/f5ndv6yotovh1.gif")
            .unwrap()
    );
    assert!(!player.hidden(), "the GIF should have a visible video area");
    assert!(
        !app.images
            .contains_key("https://i.redd.it/f5ndv6yotovh1.gif"),
        "GIFs must not enter the full-image download queue"
    );
}

#[test]
fn a_sets_the_sound_for_later_videos_and_a_missing_ffplay_clears_it() {
    let mut app = open_video_post();
    screen(&mut app, 100, 30);
    app.tick();
    let (url, area) = {
        let player = app.video().unwrap();
        (player.url.clone(), player.area)
    };
    app.set_video(Some(video::Player::silent(&url, area)));

    // A video without sound: `a` still turns the setting over; this one stays silent.
    press_keys(&mut app, "m");
    assert!(app.video_sound_wanted());
    assert!(!app.video().unwrap().sound());
    press_keys(&mut app, "m");
    assert!(!app.video_sound_wanted(), "and back off");

    // ffplay failing, even at a later frame, leaves later videos silent too.
    press_keys(&mut app, "m");
    app.video_mut().unwrap().fail_sound();
    app.tick();
    assert!(!app.video_sound_wanted());
}

#[test]
fn a_resized_video_starts_again_as_it_was_once_the_size_holds() {
    let mut app = open_video_post();
    let step = |app: &mut App, height: u16| {
        screen(app, 100, height);
        app.tick();
    };
    step(&mut app, 30);
    let first = app.video().unwrap().area;
    let url = app.reddit.media_url(VIDEO).unwrap();
    app.set_video(Some(video::Player::silent(&url, first)));
    press_keys(&mut app, "p");

    // Two draws at once at the new size: it has not held still yet.
    step(&mut app, 40);
    step(&mut app, 40);
    assert_eq!(
        app.video().unwrap().area,
        first,
        "a size passing by changes nothing"
    );
    screen(&mut app, 100, 40);
    app.tick_at(std::time::Instant::now() + std::time::Duration::from_millis(300));
    let player = app.video().unwrap();
    assert_ne!(player.area, first, "sized for the new area");
    assert!(player.paused(), "still paused");

    press(&mut app, 'q');
    app.tick();
    assert_eq!(
        app.video_area, None,
        "the next video waits for its own draw"
    );
}
