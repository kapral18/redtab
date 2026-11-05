use super::*;

#[test]
fn when_audio_skips_the_remaining_video_frames_eof_should_loop_instead_of_resuming_a_cut() {
    let now = Instant::now();
    let (mut player, runs) = fed_player(Some(10.0), now);
    let sender = runs.borrow_mut().remove(0).1;
    sender.send(blank()).unwrap();
    player.tick(now);
    player
        .decoder
        .as_ref()
        .unwrap()
        .progress
        .read
        .store(240, Ordering::Relaxed);
    player.audio = Some(Audio::fixture(0.0, now));
    player.audio.as_ref().unwrap().observe(10.0, now);
    drop(sender);

    player.tick(now);

    assert_eq!(runs.borrow()[0].0, 0.0);
}

#[test]
fn audio_master_should_hold_video_until_delayed_audio_is_ready() {
    let now = Instant::now();
    let (mut player, runs) = fed_player(None, now);
    runs.borrow()[0].1.send(blank()).unwrap();
    player.tick(now);
    player.audio = Some(Audio::fixture(0.0, now));
    assert!(player.waiting_for_audio());
    assert!(player.loading());
    player.audio.as_ref().unwrap().observe(0.0, after(now, 0.6));
    runs.borrow()[0].1.send(blank()).unwrap();

    player.tick(after(now, 0.6));

    assert_eq!(player.frame_number, 1);
    assert_eq!(player.position(after(now, 0.6)), 0.0);
    assert!(!player.loading());
}

#[test]
fn audio_master_should_catch_up_after_graphics_delay_without_restarting_sound() {
    let now = Instant::now();
    let (mut player, runs) = fed_player(None, now);
    runs.borrow()[0].1.send(blank()).unwrap();
    player.tick(now);
    player.audio = Some(Audio::fixture(0.0, now));
    player.audio.as_ref().unwrap().observe(0.5, after(now, 1.0));
    for _ in 0..20 {
        runs.borrow()[0].1.send(blank()).unwrap();
    }

    player.tick(after(now, 1.0));
    player.tick(after(now, 1.0));

    assert!(player.audio.is_some());
    assert_eq!(player.position(after(now, 1.0)), 0.5);
    assert_eq!(player.shown, 13);
    assert_eq!(runs.borrow().len(), 1);
}

#[test]
fn audio_master_should_keep_its_position_when_muted() {
    let now = Instant::now();
    let (mut player, runs) = fed_player(None, now);
    runs.borrow()[0].1.send(blank()).unwrap();
    player.tick(now);
    player.sound = true;
    player.audio = Some(Audio::fixture(0.0, now));
    player.audio.as_ref().unwrap().observe(0.5, after(now, 1.0));

    player.toggle_sound(after(now, 1.0));

    assert!((player.position(after(now, 1.1)) - 0.6).abs() < 0.001);
    assert!(player.audio.is_none());
}

#[test]
fn audio_master_should_pause_at_heard_time() {
    let now = Instant::now();
    let (mut player, runs) = fed_player(None, now);
    runs.borrow()[0].1.send(blank()).unwrap();
    player.tick(now);
    player.audio = Some(Audio::fixture(0.0, now));
    player.audio.as_ref().unwrap().observe(0.5, after(now, 1.0));

    player.toggle_pause(after(now, 1.0));

    assert_eq!(player.position(after(now, 2.0)), 0.5);
    assert!(player.audio.is_none());
}

#[test]
fn audio_eof_should_keep_the_sound_preference_for_the_next_loop() {
    let now = Instant::now();
    let (mut player, runs) = fed_player(None, now);
    runs.borrow()[0].1.send(blank()).unwrap();
    player.tick(now);
    player.sound = true;
    let audio = Audio::fixture(0.0, now);
    audio.observe(0.25, now);
    audio.finish();
    player.audio = Some(audio);

    player.tick(now);

    assert!(player.sound());
    assert!(player.audio.is_none());
    assert_eq!(player.position(now), 0.25);
}

#[test]
fn audio_eof_before_a_clock_sample_should_keep_playback_muted_with_a_notice() {
    let now = Instant::now();
    let (mut player, _) = fed_player(None, now);
    player.sound = true;
    let audio = Audio::fixture(0.0, now);
    audio.finish();
    player.audio = Some(audio);

    player.tick(now);

    assert!(!player.sound());
    assert!(player.take_notice().is_some());
}

#[test]
fn when_a_still_picture_has_sound_it_should_keep_looping() {
    let t0 = Instant::now();
    let (spawn, runs) = fed_runs(Some(0.5), true);
    let sources = Sources {
        video: "v".into(),
        audio: Some("a".into()),
        audio_in_file: false,
        headers: Vec::new(),
        audio_headers: Vec::new(),
    };
    let mut player = Player::new(
        "u".into(),
        Size::new(10, 5),
        None,
        Some(sources),
        spawn,
        Resume::default(),
        t0,
    );

    let started = runs_in_a_second(&mut player, &runs, 12, repeated, t0);

    assert!(
        started > 1,
        "the picture plays again with its sound: {started} runs"
    );
}

/// The log arrives on its own thread: a frame before the stream mapping decides
/// nothing about sound.
#[test]
fn sound_is_decided_only_once_the_log_lists_the_streams() {
    let t0 = Instant::now();
    let log = Arc::new(Mutex::new(Log::default()));
    let shared = Arc::clone(&log);
    let senders = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let record = std::rc::Rc::clone(&senders);
    let spawn: Spawn = Box::new(move |_, _| {
        let (tx, frames) = frame_channel(8);
        record.borrow_mut().push(tx);
        Ok(Decoder {
            child: None,
            frames,
            log: Arc::clone(&shared),
            progress: Arc::new(DecodeProgress::default()),
        })
    });
    let sources = Sources {
        video: "v".into(),
        audio: Some("v".into()),
        audio_in_file: true,
        headers: Vec::new(),
        audio_headers: Vec::new(),
    };
    let mut player = Player::new(
        "u".into(),
        Size::new(10, 5),
        None,
        Some(sources),
        spawn,
        Resume::default(),
        t0,
    );

    senders.borrow()[0].send(blank()).unwrap();
    player.tick(t0);
    assert_eq!(
        player.sources.as_ref().unwrap().audio.as_deref(),
        Some("v"),
        "not known yet"
    );

    {
        let mut log = log.lock();
        log.audio = true;
        log.streams_read = true;
    }
    senders.borrow()[0].send(blank()).unwrap();
    player.tick(after(t0, 0.05));
    let sources = player.sources.as_ref().unwrap();
    assert_eq!(sources.audio.as_deref(), Some("v"), "it has sound");
    assert!(!sources.audio_in_file, "decided");
}
