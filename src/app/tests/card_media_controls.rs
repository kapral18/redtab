use super::*;

#[test]
fn card_media_unselected_posterless_card_should_not_borrow_the_selected_error() {
    let mut app = failed_card_playback_app();
    for post in &mut app.tabs[0].feed.posts {
        if let Media::Video { poster, .. } = &mut post.media {
            *poster = None;
        }
        post.preview_size = Some((640, 80));
    }
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 80)).unwrap();

    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();

    let actual = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert_eq!(
        actual.matches("unavailable · O opens media").count(),
        1,
        "{actual}"
    );
    assert!(actual.contains("loading image"), "{actual}");
}

#[test]
fn card_media_failed_decoder_with_a_poster_should_keep_error_and_retry_visible() {
    let mut app = failed_card_playback_app();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 20)).unwrap();

    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();

    let actual = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(actual.contains("p retry"), "{actual}");
    assert!(actual.contains("fixture decoder unavailable"), "{actual}");
    assert!(matches!(
        app.card_images.get("https://i.redd.it/poster.jpg"),
        Some(CardImageSlot::Ready { .. })
    ));
}

#[test]
fn card_media_failed_decoder_in_a_narrow_pane_should_keep_retry_and_help_visible() {
    let mut app = failed_card_playback_app();
    app.show_sidebar = false;
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(24, 20)).unwrap();

    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();

    let actual = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(actual.contains("p retry"), "{actual}");
    assert!(actual.contains("F1"), "{actual}");
}

#[test]
fn card_media_failed_decoder_in_sidebar_should_show_sidebar_controls() {
    let mut app = failed_card_playback_app();
    app.focus_sidebar();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 20)).unwrap();

    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();

    let footer = (0..100)
        .map(|x| terminal.backend().buffer()[(x, 19)].symbol())
        .collect::<String>();
    let words = footer.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(words.contains("l list"), "{footer}");
    assert!(!footer.contains("p retry"), "{footer}");
}

#[test]
fn card_media_posterless_sidebar_should_offer_return_instead_of_play() {
    let mut app = card_playback_app();
    if let Media::Video { poster, .. } = &mut app.tabs[0].feed.posts[0].media {
        *poster = None;
    }
    app.config.media.video_autoplay = false;
    app.focus_sidebar();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 20)).unwrap();

    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();

    let actual = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(!actual.contains("p play/pause"), "{actual}");
    assert!(!app.video_controls_available());
}

#[test]
fn card_media_review_silent_card_notice_should_not_promise_later_sound() {
    let mut app = card_playback_app();
    let later = app.card_video.as_ref().unwrap().since + NAV_SETTLE;
    app.sync_video_with(later, |url, area, _| Player::silent(&url, area));

    app.toggle_video_sound();

    assert_eq!(app.status.as_ref().unwrap().text, "This video has no sound");
}
