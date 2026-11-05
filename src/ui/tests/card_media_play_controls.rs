use super::*;

#[test]
fn card_media_detail_sidebar_should_offer_return_instead_of_play_keys() {
    let mut app = open_video_post();
    app.config.media.video_autoplay = false;
    app.focus_sidebar();

    let actual = screen(&mut app, 180, 40);
    let words = actual.split_whitespace().collect::<Vec<_>>().join(" ");

    assert!(words.contains("l return"), "{actual}");
    assert!(!words.contains("p play"), "{actual}");
}

#[test]
fn card_media_pause_key_should_control_the_selected_card() {
    let mut app = video_card_with_poster(image::DynamicImage::new_rgb8(320, 180));
    app.video = Some(video::Player::silent(VIDEO, Size::new(68, 17)));

    press_keys(&mut app, "p");

    assert!(app.video.as_ref().unwrap().paused());
}

#[test]
fn card_media_unavailable_stream_should_not_offer_a_play_button() {
    let mut app = video_card_with_poster(image::DynamicImage::new_rgb8(320, 180));
    if let Media::Video { url, .. } = &mut app.tabs[4].feed.posts[0].media {
        *url = None;
    }
    let mut pictures = ListPictures::default();
    let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();

    terminal
        .draw(|f| {
            draw_feed(f, &mut app, f.area(), &mut pictures);
        })
        .unwrap();

    assert!(matches!(
        pictures.cards[0].indicator,
        Some(Indicator::Video { play: false, .. })
    ));
    assert!(!app.video_controls_available());
}

#[test]
fn card_media_review_manual_detail_should_show_play_instead_of_loading() {
    let mut app = open_video_post();
    app.config.media.video_autoplay = false;

    let actual = screen(&mut app, 100, 30);
    let words = actual.split_whitespace().collect::<Vec<_>>().join(" ");

    assert!(words.contains("p play"), "{actual}");
    assert!(!actual.contains("loading video"), "{actual}");
}

#[test]
fn card_media_waiting_video_should_show_play_controls() {
    let mut app = video_card_with_poster(image::DynamicImage::new_rgb8(320, 180));

    let actual = screen(&mut app, 100, 20);

    let footer = actual
        .lines()
        .last()
        .unwrap()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(footer.contains("p play"), "{actual}");
}
