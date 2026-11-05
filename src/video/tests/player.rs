use super::*;

#[test]
fn when_youtube_decoding_fails_it_should_hide_stream_tokens_and_keep_install_guidance() {
    let mut player = Player::unavailable(
        "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
        Size::new(20, 10),
    );
    player.error = Some("failed https://rr1.googlevideo.com/file?token=PRIVATE".into());
    assert_eq!(
        player.error(),
        Some("YouTube playback failed; open media externally")
    );

    player.error = Some("ffmpeg is not installed".into());

    assert_eq!(player.error(), Some("ffmpeg is not installed"));
}

#[test]
fn stalled_video_frames_should_release_the_decoder_and_report_failure() {
    for started in [false, true] {
        let t0 = Instant::now();
        let (spawn, runs) = fed_runs(None, false);
        let mut player = fed_silent_player(spawn, 0.0, t0);
        if started {
            runs.borrow()[0].1.send(blank()).unwrap();
        }
        player.tick(t0);

        assert!(player.tick(t0 + Duration::from_secs(31)));

        assert!(player.error().is_some());
        assert!(!player.loading());
        assert!(player.decoder.is_none());
        assert_eq!(player.wake_in(t0 + Duration::from_secs(31)), None);
    }
}

#[test]
fn when_a_retained_frame_covers_startup_the_position_should_wait_for_new_frames() {
    let start = Instant::now();
    let (mut player, runs) = fed_player(Some(10.0), start);
    player.retain_frame(Some(blank()));
    assert_eq!(player.position(start + Duration::from_secs(5)), 0.0);
    runs.borrow()[0].1.send(blank()).unwrap();
    player.tick(start + Duration::from_secs(5));
    assert_eq!(player.position(start + Duration::from_secs(6)), 1.0);
}

#[test]
fn when_a_run_of_unknown_length_ends_on_an_error_it_should_go_on_from_there() {
    // A streamed GIF has no known duration; a dropped connection ends it with an error.
    let t0 = Instant::now();
    let (spawn, runs) = fed_runs_logging(None, false, DROPPED);
    let mut player = fed_silent_player(spawn, 0.0, t0);
    let first = runs.borrow_mut().remove(0).1;
    first.send(repeated()).unwrap();
    first.send(blank()).unwrap();
    drop(first);

    player.tick(t0);
    player.tick(after(t0, 0.05));
    player.tick(after(t0, 0.15));

    assert_eq!(runs.borrow()[0].0, 2.0 / MIN_FPS, "not back to 0:00");
}

#[test]
fn when_a_still_on_a_cutting_server_settles_it_should_hold_its_frame() {
    // As above, for a still GIF: once resuming at the cut failed, the still holds.
    let t0 = Instant::now();
    let (spawn, runs) = fed_runs_logging(None, false, DROPPED);
    let mut player = fed_silent_player(spawn, 0.0, t0);
    let play_to_the_cut = |runs: &Runs| {
        let run = runs.borrow_mut().remove(0).1;
        run.send(repeated()).unwrap();
        run.send(repeated()).unwrap();
    };
    play_to_the_cut(&runs);
    player.tick(t0);
    player.tick(after(t0, 0.05));
    player.tick(after(t0, 0.15));
    for at in [0.2, 0.25] {
        drop(runs.borrow_mut().remove(0));
        player.tick(after(t0, at));
    }
    play_to_the_cut(&runs);
    player.tick(after(t0, 0.3));
    player.tick(after(t0, 0.35));
    player.tick(after(t0, 0.45));

    assert!(runs.borrow().is_empty(), "no more downloads");
    assert_eq!(player.wake_in(after(t0, 0.5)), None, "held");
}

#[test]
fn when_a_link_cannot_be_opened_it_should_be_an_error_without_a_retry() {
    let t0 = Instant::now();
    let error = Some(("Error opening input: Connection refused", true));
    let (spawn, runs) = fed_runs_logging(None, false, error);
    let mut player = fed_silent_player(spawn, 0.0, t0);

    drop(runs.borrow_mut().remove(0));
    player.tick(t0);

    assert!(runs.borrow().is_empty());
    assert_eq!(
        player.error(),
        Some("Error opening input: Connection refused")
    );
}

#[test]
fn when_a_silent_run_never_changes_it_should_keep_its_frame_without_restarting() {
    // ffmpeg's fps filter turns one GIF frame into 1, 2 or many copies, depending on
    // the frame delay; the duration may be unknown.
    for (frames, duration) in [(1, Some(0.04)), (2, None), (12, Some(0.5))] {
        let t0 = Instant::now();
        let (mut player, runs) = fed_player(duration, t0);

        let started = runs_in_a_second(&mut player, &runs, frames, repeated, t0);

        assert_eq!(started, 1, "{frames} frames: decoder runs started in 1 s");
        assert!(player.frame().is_some(), "the still frame stays on screen");
        assert_eq!(
            player.wake_in(after(t0, 1.0)),
            None,
            "nothing waits to play"
        );
        assert!(player.error().is_none());
    }
}

#[test]
fn a_file_shows_at_its_first_frame_whether_it_has_sound() {
    let t0 = Instant::now();
    let in_file = || Sources {
        video: "v".into(),
        audio: Some("v".into()),
        audio_in_file: true,
        headers: Vec::new(),
        audio_headers: Vec::new(),
    };
    let with_sound = Resume {
        sound: true,
        ..Resume::default()
    };

    let (spawn, runs) = fed_runs(None, false);
    let mut silent = Player::new(
        "u".into(),
        Size::new(10, 5),
        None,
        Some(in_file()),
        spawn,
        with_sound,
        t0,
    );
    runs.borrow()[0].1.send(blank()).unwrap();
    silent.tick(t0);
    assert!(!silent.sound(), "no sound to play");
    assert!(silent.take_notice().is_some());
    assert_eq!(silent.sources.as_ref().unwrap().audio, None);

    let (spawn, runs) = fed_runs(None, true);
    let mut loud = Player::new(
        "u".into(),
        Size::new(10, 5),
        None,
        Some(in_file()),
        spawn,
        Resume::default(),
        t0,
    );
    runs.borrow()[0].1.send(blank()).unwrap();
    loud.tick(t0);
    assert_eq!(loud.sources.as_ref().unwrap().audio.as_deref(), Some("v"));
}

#[test]
fn a_stream_that_resolves_but_cannot_decode_is_an_error_on_screen() {
    let t0 = Instant::now();
    let (tx, rx) = sync_channel(1);
    tx.send(Ok(no_sound())).unwrap();
    let spawn: Spawn = Box::new(|_, _| Err(io::Error::other("broken")));
    let mut player = Player::new(
        "u".into(),
        Size::new(10, 5),
        Some(rx),
        None,
        spawn,
        Resume::default(),
        t0,
    );

    assert!(player.tick(t0), "the error needs a draw");
    assert_eq!(player.error(), Some("ffmpeg: broken"));
}

#[test]
fn a_stream_without_a_frame_is_an_error_on_screen() {
    let t0 = Instant::now();
    let (mut player, runs) = fed_player(None, t0);
    runs.borrow_mut().clear();

    assert!(player.tick(t0), "the error needs a draw");
    assert!(player.error().is_some());
    assert!(runs.borrow().is_empty(), "no run starts again");
    assert_eq!(player.wake_in(after(t0, 1.0)), None);
}

#[test]
fn when_video_waits_for_graphics_the_input_poll_should_not_spin_on_overdue_frames() {
    use crate::app::App;
    use crate::card_image::Key;
    use crate::config::Config;
    use crate::reddit::Reddit;
    use crate::worker::Pool;
    use ratatui_image::picker::Picker;
    let directory = tempfile::tempdir().unwrap();
    let start = Instant::now() - Duration::from_secs(1);
    let (mut player, runs) = fed_player(Some(10.0), start);
    runs.borrow()[0].1.send(blank()).unwrap();
    runs.borrow()[0].1.send(blank()).unwrap();
    player.tick(start);
    assert_eq!(player.wake_in(Instant::now()), Some(Duration::ZERO));
    let reddit = Reddit::test_at("http://127.0.0.1:9", false);
    let (pool, _messages) = Pool::start(reddit.clone(), 1, None);
    let picker = Picker::halfblocks();
    let mut app = App::new(
        Config::default(),
        pool,
        reddit,
        directory.path().join("state.json"),
        picker.clone(),
    );
    app.video = Some(player);
    let image = Arc::new(image::DynamicImage::new_rgb8(16, 16));
    let key = Key::new("lag-probe", &picker, Size::new(2, 1), (16, 16), 0, 1, 0);
    app.card_renderer.begin();
    app.card_renderer.tile(
        key,
        &image,
        &picker,
        crate::card_image::Preparation::Refresh,
    );
    app.card_renderer.finish();
    assert!(app.card_renderer.pending());
    assert!(
        app.wake_in() >= Duration::from_millis(10),
        "drain late frames while graphics are pending without spinning on expired deadlines"
    );
    let (input, events) = std::sync::mpsc::channel();
    input
        .send(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('q'),
            crossterm::event::KeyModifiers::NONE,
        ))
        .unwrap();
    let key = events.recv_timeout(app.wake_in()).unwrap();
    crate::keys::handle(&mut app, key);
    assert!(app.quit, "graphics preparation must not consume quit input");
    app.card_renderer.clear();
    assert_eq!(
        app.wake_in(),
        Duration::ZERO,
        "ready graphics allow the overdue frame immediately"
    );
}
