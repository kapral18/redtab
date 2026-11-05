use super::*;

#[test]
fn frame_wait_deadlines_should_restart_after_pause_or_hiding() {
    for hidden in [false, true] {
        let t0 = Instant::now();
        let (spawn, runs) = fed_runs(None, false);
        let mut player = fed_silent_player(spawn, 0.0, t0);
        player.tick(t0);
        if hidden {
            player.set_hidden(true, t0 + Duration::from_secs(20));
        } else {
            player.toggle_pause(t0 + Duration::from_secs(20));
        }
        let resumed = t0 + Duration::from_secs(100);
        player.tick(resumed);
        assert!(player.error().is_none());
        if hidden {
            player.set_hidden(false, resumed);
        } else {
            player.toggle_pause(resumed);
        }
        player.tick(resumed);
        player.tick(resumed + Duration::from_secs(20));
        assert!(player.error().is_none());
        runs.borrow()[0].1.send(blank()).unwrap();
        player.tick(resumed + Duration::from_secs(25));
        player.tick(resumed + Duration::from_secs(45));
        assert!(player.error().is_none());

        player.tick(resumed + Duration::from_secs(56));

        assert!(player.error().is_some());
    }
}

#[test]
fn audio_master_should_seek_from_heard_time() {
    let now = Instant::now();
    let (mut player, runs) = fed_player(None, now);
    runs.borrow()[0].1.send(blank()).unwrap();
    player.tick(now);
    player.audio = Some(Audio::fixture(0.0, now));
    player.audio.as_ref().unwrap().observe(0.5, after(now, 1.0));

    player.seek(5.0, after(now, 1.0));

    assert_eq!(runs.borrow()[1].0, 5.5);
    assert_eq!(player.resume(after(now, 2.0)).at, 5.5);
}

#[test]
fn frames_show_when_due_and_wait_while_paused() {
    let t0 = Instant::now();
    let (mut player, runs) = fed_player(None, t0);
    for _ in 0..4 {
        runs.borrow()[0].1.send(blank()).unwrap();
    }

    assert!(player.tick(t0));
    assert_eq!(player.frame_number, 1, "the first frame is due at once");
    assert!(!player.tick(after(t0, 0.02)), "the second is due at 1/24 s");
    assert!(player.tick(after(t0, 0.05)));
    assert_eq!(player.frame_number, 2);

    player.toggle_pause(after(t0, 0.05));
    assert!(!player.tick(after(t0, 5.0)));
    assert_eq!(player.frame_number, 2, "paused: no frame comes due");
    assert_eq!(player.position(after(t0, 5.0)), 0.05);

    player.toggle_pause(after(t0, 5.0));
    assert!(player.tick(after(t0, 5.04)));
    assert_eq!(
        player.frame_number, 3,
        "playing again goes on from the pause"
    );
}

#[test]
fn a_seek_while_paused_shows_one_frame_and_playing_ends_that() {
    let t0 = Instant::now();
    let (mut player, runs) = fed_player(Some(60.0), t0);
    player.toggle_pause(t0);
    player.seek(SEEK_STEP, t0);
    assert_eq!(
        runs.borrow()[1].0,
        SEEK_STEP,
        "a new run starts at the target"
    );
    for _ in 0..2 {
        runs.borrow()[1].1.send(blank()).unwrap();
    }
    assert!(player.tick(after(t0, 1.0)));
    assert!(!player.tick(after(t0, 2.0)));
    assert_eq!(player.frame_number, 1, "one frame shows the new place");
    assert_eq!(player.position(after(t0, 2.0)), SEEK_STEP);

    // Seek again and play before that run's first frame, then pause: the
    // pending one-frame show is over, so nothing moves while paused.
    player.seek(SEEK_STEP, after(t0, 2.0));
    player.toggle_pause(after(t0, 2.0));
    player.toggle_pause(after(t0, 2.0));
    runs.borrow()[2].1.send(blank()).unwrap();
    player.tick(after(t0, 3.0));
    assert_eq!(player.frame_number, 1);
}

#[test]
fn when_resuming_at_a_cut_keeps_failing_the_cut_should_count_as_the_end() {
    // A server that always ends the file at the same byte: every run is cut at 2
    // frames and resuming there gives nothing.
    let t0 = Instant::now();
    let (spawn, runs) = fed_runs_logging(None, false, DROPPED);
    let mut player = fed_silent_player(spawn, 0.0, t0);
    let play_to_the_cut = |runs: &Runs| {
        let run = runs.borrow_mut().remove(0).1;
        run.send(repeated()).unwrap();
        run.send(blank()).unwrap();
    };
    play_to_the_cut(&runs);
    player.tick(t0);
    player.tick(after(t0, 0.05));
    player.tick(after(t0, 0.15));
    for at in [0.2, 0.25] {
        // The resume and its retry: no frames.
        drop(runs.borrow_mut().remove(0));
        player.tick(after(t0, at));
    }
    play_to_the_cut(&runs);
    player.tick(after(t0, 0.3));
    player.tick(after(t0, 0.35));
    player.tick(after(t0, 0.45));

    assert_eq!(
        runs.borrow()[0].0,
        0.0,
        "after a failed resume there, the cut loops"
    );
}

#[test]
fn when_a_seek_follows_a_used_retry_the_new_start_should_get_its_own() {
    let t0 = Instant::now();
    let (spawn, runs) = fed_runs_logging(None, false, DROPPED);
    let mut player = fed_silent_player(spawn, 5.0, t0);
    drop(runs.borrow_mut().remove(0));
    player.tick(t0); // cut on the way to 0:05: the retry is used
    runs.borrow_mut().clear();

    player.seek(SEEK_STEP, t0);
    drop(runs.borrow_mut().remove(0));
    player.tick(after(t0, 0.1));

    assert_eq!(runs.borrow()[0].0, 10.0, "the seek target is tried again");
}

#[test]
fn when_a_cut_point_failed_seeks_should_stop_before_it_until_playback_passes_it() {
    let t0 = Instant::now();
    let (spawn, runs) = fed_runs(None, false);
    let mut player = fed_silent_player(spawn, 0.0, t0);
    player.unresumable = Some(3.0);
    runs.borrow_mut().clear();

    player.seek(SEEK_STEP, t0);
    let clamped = runs.borrow()[0].0;
    let run = runs.borrow_mut().remove(0).1;
    for i in 0..40 {
        run.send(blank()).unwrap();
        player.tick(after(t0, f64::from(i) / MIN_FPS));
    }

    assert_eq!(clamped, 2.0, "a second before the failed point");
    assert_eq!(player.unresumable, None, "played past it: it was an outage");
}

/// Sound starts with a run's first frame; a takeover before that frame starts where
/// the run starts.
#[test]
fn the_sound_and_a_takeover_wait_for_the_first_frame() {
    let t0 = Instant::now();
    let (spawn, runs) = fed_runs(None, false);
    let with_sound = Resume {
        at: 3.0,
        paused: false,
        sound: true,
    };
    // No sound file: trying to start the sound says so, which shows when it tried.
    let mut player = Player::new(
        "u".into(),
        Size::new(10, 5),
        None,
        Some(no_sound()),
        spawn,
        with_sound,
        t0,
    );
    assert!(player.take_notice().is_none(), "no sound before a frame");
    assert_eq!(player.resume(after(t0, 1.0)).at, 3.0);

    runs.borrow()[0].1.send(blank()).unwrap();
    player.tick(after(t0, 1.0));
    assert!(
        player.take_notice().is_some(),
        "the sound starts with the frame"
    );
}
