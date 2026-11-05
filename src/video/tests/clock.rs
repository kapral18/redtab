use super::*;

#[test]
fn frames_come_due_at_their_runs_rate() {
    let t0 = Instant::now();
    let (mut player, runs) = fed_player(Some(10.0), t0);
    let run = runs.borrow_mut().remove(0).1;
    run.rate.set(30.0);
    for i in 0..16 {
        run.send(blank()).unwrap();
        player.tick(after(t0, f64::from(i) / 30.0));
    }

    assert_eq!(player.shown, 16, "frame 15 is due at 0.5 s");
    run.send(blank()).unwrap();
    player.tick(after(t0, 0.52));
    assert_eq!(player.shown, 16, "frame 16 waits for 0.533 s");
    player.tick(after(t0, 0.54));
    assert_eq!(player.shown, 17);

    // A run cut short goes on from where its frames reached at that rate.
    let (mut player, runs) = fed_player(Some(60.0), t0);
    let first = runs.borrow_mut().remove(0).1;
    first.rate.set(30.0);
    first.send(blank()).unwrap();
    first.send(blank()).unwrap();
    drop(first);
    player.tick(after(t0, 0.1));
    player.tick(after(t0, 0.2));
    assert_eq!(runs.borrow()[0].0, 2.0 / 30.0);

    // Frames the sound has passed are skipped by their index at that rate.
    let (mut player, runs) = fed_player(Some(60.0), t0);
    let run = runs.borrow_mut().remove(0).1;
    run.rate.set(30.0);
    run.send(blank()).unwrap();
    player.tick(t0);
    player.audio = Some(Audio::fixture(0.0, t0));
    player.audio.as_ref().unwrap().observe(10.0, t0);
    player.tick(t0);
    let target = &player.decoder.as_ref().unwrap().progress.target;
    assert_eq!(target.load(Ordering::Relaxed), 300);
}

#[test]
fn the_clock_stands_still_while_stopped_and_resumes_from_there() {
    let t0 = Instant::now();
    let mut clock = Clock::new(3.0, t0);
    let t1 = t0 + Duration::from_secs(2);
    assert_eq!(clock.position(t1), 5.0);
    clock.stop(t1);
    let t2 = t1 + Duration::from_secs(10);
    assert_eq!(clock.position(t2), 5.0);
    assert_eq!(clock.when(6.0, t2), None);
    clock.start(t2);
    assert_eq!(clock.position(t2 + Duration::from_secs(1)), 6.0);
    assert_eq!(clock.when(6.0, t2), Some(t2 + Duration::from_secs(1)));
}
