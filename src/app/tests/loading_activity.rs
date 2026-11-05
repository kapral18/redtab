use super::*;

#[test]
fn card_video_frame_preparation_should_not_show_image_loading_activity() {
    let mut app = card_playback_app();
    app.tabs[0].feed.posts.truncate(1);
    if let Media::Video { poster, .. } = &mut app.tabs[0].feed.posts[0].media {
        *poster = None;
    }
    #[allow(deprecated)]
    let mut picker = Picker::from_fontsize(ratatui_image::FontSize::new(8, 16));
    picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
    app.image_picker = picker;
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 20)).unwrap();
    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    let later = app.card_video.as_ref().unwrap().since + NAV_SETTLE;
    app.sync_video_with(later, |url, _, _| {
        Player::showing(
            &url,
            video::Frame {
                width: 16,
                height: 9,
                pixels: video::Pixels::Raw(std::sync::Arc::new(image::DynamicImage::new_rgb8(
                    16, 9,
                ))),
                same_as_first: false,
            },
        )
    });

    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();

    assert!(app.card_player().unwrap().frame().is_some());
    assert!(
        app.card_renderer.pending(),
        "playback still waits for graphics"
    );
    assert!(
        !app.images_loading(),
        "ordinary playback is not image loading"
    );
    app.card_renderer.wait_for_completion();
    assert!(
        app.card_renderer.pending(),
        "unpublished tiles still need polling"
    );
    assert!(!app.images_loading());
    assert!(app.card_renderer.poll());
    assert!(!app.card_renderer.pending());
}

#[test]
fn when_initial_content_or_a_modal_is_busy_its_loader_should_animate_and_stop_when_idle() {
    use crate::app::experience::Screen;
    use crate::reddit::experience::{Query, Request};
    for kind in 0..3 {
        let mut app = app_with(&[]);
        app.active = 1; // An idle Bookmarks view behind the modal.
        match kind {
            0 => {
                app.active = 2;
                app.all_subs.loading = true;
            }
            1 => {
                app.overlay = Some(Overlay::Auth);
                app.auth.phase = crate::auth::Phase::LoadingAccounts;
            }
            _ => {
                app.overlay = Some(Overlay::Experience);
                app.experience.screens.push(Screen::Loading(Query::Multis));
                app.experience.pending = Some((
                    1,
                    Request::Read {
                        query: Query::Multis,
                        after: None,
                    },
                ));
            }
        }
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 20)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
        assert!(app.loading_since.is_some(), "kind {kind}: busy draw");
        app.loading_since = Some(Instant::now() - Duration::from_millis(100));
        app.tick();
        assert_eq!(app.loading_frame, 1, "kind {kind}: busy tick");

        app.help_scroll = Some(0);
        app.loading_since = Some(Instant::now() - Duration::from_millis(100));
        app.tick();
        assert_eq!(app.loading_frame, 1, "kind {kind}: help pauses");
        app.help_scroll = None;
        app.all_subs.loading = false;
        app.auth.phase = crate::auth::Phase::Choose;
        app.experience.pending = None;
        app.tick();
        assert_eq!(app.loading_frame, 1, "kind {kind}: completion stops");
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
        assert!(app.loading_since.is_none(), "kind {kind}: idle draw");
    }
}

#[test]
fn when_account_lists_outlast_home_the_visible_sidebar_should_keep_animating() {
    let mut app = app_with(&[]);
    app.active = 0;
    app.on_msg(Msg::Feed {
        tab: app.tabs[0].id,
        generation: app.tabs[0].feed.generation,
        append: false,
        result: Ok(listing("rust", &["existing"], None)),
    });
    app.all_subs.freshness = None;
    app.all_subs.loading = true;
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();

    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    assert!(app.loading_since.is_some());
    app.loading_since = Some(Instant::now() - Duration::from_millis(100));
    app.tick();
    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    assert_eq!(app.loading_frame, 1);
    assert_eq!(
        terminal
            .backend()
            .buffer()
            .content()
            .chunks(120)
            .flat_map(|row| row.iter().take(usize::from(app.sidebar_width())))
            .filter(|cell| cell.symbol() == "⠙")
            .count(),
        2
    );

    app.show_sidebar = false;
    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    assert!(app.loading_since.is_none());
    app.tick();
    assert_eq!(app.loading_frame, 1);

    app.show_sidebar = true;
    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    app.loading_since = Some(Instant::now() - Duration::from_millis(100));
    app.on_msg(Msg::Subscriptions {
        generation: app.all_subs.generation,
        result: Err("offline".into()),
    });
    app.tick();
    assert_eq!(app.loading_frame, 1);
}
