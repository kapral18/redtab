use super::*;

#[test]
fn bookmark_and_search_preview_videos_load_their_first_frame_with_autoplay_off() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("frame.mp4");
    let output = match std::process::Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=red:s=16x16:d=0.1",
            "-frames:v",
            "1",
            "-c:v",
            "mpeg4",
            "-movflags",
            "+faststart",
            "-y",
        ])
        .arg(&path)
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => panic!("{error}"),
    };
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bytes = std::fs::read(path).unwrap();
    let server = crate::reddit::tests::local_http_server();
    let base = format!("http://{}/", server.server_addr());
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stopped = stop.clone();
    let serving = std::thread::spawn(move || {
        while !stopped.load(std::sync::atomic::Ordering::Relaxed) {
            if let Some(request) = server.recv_timeout(Duration::from_millis(20)).unwrap() {
                let _ = request.respond(crate::reddit::tests::served::Response::from_data(
                    bytes.clone(),
                ));
            }
        }
    });
    let mut results = Vec::new();
    for (search_preview, converted_gif) in [(false, false), (false, true), (true, false)] {
        let (mut app, messages) = app_at(&base, &[]);
        app.config.media.video_autoplay = false;
        show_thumbnails(&mut app);
        app.active = if search_preview {
            SEARCH_TAB
        } else {
            BOOKMARKS_TAB
        };
        let url = if converted_gif {
            format!("{base}frame.gif?format=mp4")
        } else {
            format!("{base}frame.mp4")
        };
        let mut saved = post("saved-video", "s");
        saved.media = if converted_gif {
            Media::Image {
                url: url.clone(),
                preview: None,
            }
        } else {
            Media::Video {
                gif: false,
                url: Some(url.clone()),
                poster: None,
            }
        };
        if search_preview {
            app.search.preview.posts = vec![saved];
        } else {
            app.state.toggle_bookmark(&saved, 1);
        }
        app.card_pictures_drawn(&[&url]);
        app.thumbs_drawn(&[&url]);
        let replies = (0..2)
            .map(|_| messages.recv_timeout(Duration::from_secs(5)))
            .collect::<Vec<_>>();
        results.push((search_preview, converted_gif, app, url, replies));
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    serving.join().unwrap();
    for (search_preview, converted_gif, mut app, url, replies) in results {
        for reply in replies {
            let reply = reply.unwrap_or_else(|error| {
                    let card_error = match app.card_images.get(&url) {
                        Some(CardImageSlot::Failed(error)) => Some(error),
                        _ => None,
                    };
                    let thumb_error = match app.thumbs.get(&url) {
                        Some(ImageSlot::Failed(error)) => Some(error),
                        _ => None,
                    };
                    panic!(
                        "search_preview={search_preview}, converted_gif={converted_gif}: {error}; card={card_error:?}; thumb={thumb_error:?}"
                    )
                });
            app.on_msg(reply);
        }
        assert!(
            matches!(app.thumbs.get(&url), Some(ImageSlot::Ready { .. })),
            "video thumbnail must use first-frame decoding"
        );

        let Some(CardImageSlot::Ready { image, .. }) = app.card_images.get(&url) else {
            if let Some(CardImageSlot::Failed(error)) = app.card_images.get(&url) {
                panic!("bookmark first frame failed: {error}");
            }
            panic!("bookmark first frame was not decoded");
        };
        let pixel = image.to_rgb8().get_pixel(0, 0).0;
        assert!(
            pixel[0] > 200 && pixel[1] < 40 && pixel[2] < 40,
            "{pixel:?}"
        );
        assert!(app.video().is_none());
    }
}

#[test]
fn when_youtube_media_opens_it_should_use_the_original_browser_url_and_pause_inline() {
    let mut app = card_playback_app();
    let url = "https://youtu.be/dQw4w9WgXcQ?feature=share";
    app.tabs[0].feed.posts[0].media = Media::Link {
        url: url.into(),
        thumb: None,
    };
    app.card_video_drawn(Some(Size::new(68, 38)));
    let at = app.card_video.as_ref().unwrap().since + NAV_SETTLE;
    app.sync_video_with(at, |url, _, _| {
        Player::showing(
            &url,
            video::Frame {
                width: 1,
                height: 1,
                pixels: video::Pixels::Png(vec![]),
                same_as_first: false,
            },
        )
    });
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("browser.sh");
    let output = dir.path().join("url");
    std::fs::write(&script, "printf '%s' \"$2\" > \"$1\"\n").unwrap();
    app.config.commands.browser = vec![
        "/bin/sh".into(),
        script.to_string_lossy().into(),
        output.to_string_lossy().into(),
    ];

    app.open_media();

    assert!(app.selected_video().unwrap().paused());
    let deadline = Instant::now() + Duration::from_secs(2);
    while std::fs::read_to_string(&output).ok().as_deref() != Some(url) {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(std::fs::read_to_string(output).unwrap(), url);
}

#[test]
fn card_media_same_item_scroll_should_keep_pause_and_decoder_geometry() {
    let mut app = card_playback_app();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 20)).unwrap();
    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    let later = app.card_video.as_ref().unwrap().since + NAV_SETTLE;
    app.sync_video_with(later, |url, area, _| Player::silent(&url, area));
    app.toggle_video_pause();

    let decoder_area = app.video().unwrap().area;
    let before = app.video().unwrap().position(later);
    app.scroll_card(3);
    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    app.sync_video_with(later, |_, _, _| panic!("same card must keep its player"));

    let player = app.card_player().unwrap();
    assert!(player.paused());
    assert_eq!(player.area, decoder_area);
    assert_eq!(player.position(later), before);
}

#[test]
fn when_a_detail_video_returns_it_should_use_the_current_sound_preference() {
    let mut app = card_playback_app();
    app.video_sound_on = true;
    let open = |app: &mut App, index: usize| {
        let post = app.tabs[0].feed.posts[index].clone();
        app.show_post(post.clone(), &post.permalink);
        app.post_view.as_mut().unwrap().page = Some(PostPage {
            post,
            body: String::new(),
            comments: vec![],
        })
        .into();
        app.video_area = Some(Rect::new(0, 0, 68, 38));
    };
    let showing = |url: String, area, resume: video::Resume| {
        let mut player = Player::showing(&url, playback_frame(1));
        player.area = area;
        if resume.sound {
            player.toggle_sound(Instant::now());
        }
        player
    };
    open(&mut app, 0);
    app.sync_video_with(Instant::now(), showing);
    open(&mut app, 1);
    app.sync_video_with(Instant::now(), showing);
    app.toggle_video_sound();
    assert!(!app.video_sound_wanted());
    open(&mut app, 0);
    app.sync_video_with(Instant::now(), |url, area, resume| {
        assert!(
            !resume.sound,
            "restoring a snapshot must respect the latest mute"
        );
        showing(url, area, resume)
    });
    app.toggle_video_sound();
    assert!(app.video_sound_wanted());
    assert!(
        app.selected_video().unwrap().sound(),
        "one press changes actual sound too"
    );
}

#[test]
fn when_manually_started_video_is_paused_resize_should_keep_it_paused() {
    let mut app = card_playback_app();
    app.config.media.video_autoplay = false;
    app.toggle_video_pause();
    let start = app.card_video.as_ref().unwrap().since + NAV_SETTLE;
    app.sync_video_with(start, |url, area, _| Player::silent(&url, area));
    app.toggle_video_pause();
    assert!(app.card_player().unwrap().paused());
    app.card_video_drawn(Some(Size::new(40, 20)));
    app.sync_video_with(start, |_, _, _| panic!("resize waits"));
    app.sync_video_with(start + VIDEO_RESIZE_SETTLE, |url, area, resume| {
        assert!(
            resume.paused,
            "an old play request must not unpause a resized player"
        );
        Player::silent(&url, area)
    });
}

#[test]
fn card_media_sidebar_focus_should_not_pause_a_visible_video() {
    let mut app = card_playback_app();
    let later = app.card_video.as_ref().unwrap().since + NAV_SETTLE;
    app.sync_video_with(later, |url, area, _| Player::silent(&url, area));
    app.focus = Focus::Sidebar;
    app.card_video_drawn(Some(Size::new(68, 68)));

    app.sync_video_with(later, |_, _, _| panic!("keep the visible video"));

    assert!(!app.card_player().unwrap().hidden());
}

#[test]
fn card_media_new_player_should_start_muted() {
    let mut app = card_playback_app();
    app.video_sound_on = true;
    let later = app.card_video.as_ref().unwrap().since + NAV_SETTLE;
    let mut actual = None;

    app.sync_video_with(later, |url, area, resume| {
        actual = Some(resume.sound);
        Player::silent(&url, area)
    });

    assert_eq!(actual, Some(false));
    assert!(app.card_player().is_some());
}

#[test]
fn card_media_offscreen_player_should_hide_without_forgetting_pause() {
    let mut app = card_playback_app();
    let later = app.card_video.as_ref().unwrap().since + NAV_SETTLE;
    app.sync_video_with(later, |url, area, _| Player::silent(&url, area));
    app.toggle_video_pause();

    app.card_video_drawn(None);
    app.sync_video_with(later, |_, _, _| panic!("offscreen must keep its player"));

    assert!(app.video().unwrap().hidden());
    assert!(app.video().unwrap().paused());
}

#[test]
fn reported_failed_video_should_wait_for_explicit_retry_after_revisiting() {
    let mut app = failed_card_playback_app();
    app.tabs[0].feed.selected = 1;
    app.card_video_drawn(Some(Size::new(68, 68)));
    let next = app.card_video.as_ref().unwrap().since + NAV_SETTLE;
    app.sync_video_with(next, |url, area, _| {
        let mut player = Player::showing(&url, playback_frame(2));
        player.area = area;
        player
    });
    app.tabs[0].feed.selected = 0;
    app.card_video_drawn(Some(Size::new(68, 68)));
    assert!(app.selected_video().unwrap().error().is_some());
    let returned = app.card_video.as_ref().unwrap().since + NAV_SETTLE;
    app.sync_video_with(returned, |_, _, _| {
        panic!("navigation must not retry failed media")
    });
    app.toggle_video_pause();
    app.card_video_drawn(Some(Size::new(68, 68)));
    app.sync_video_with(returned + NAV_SETTLE, |url, area, _| {
        Player::silent(&url, area)
    });
    assert!(app.selected_video().unwrap().error().is_none());
}
