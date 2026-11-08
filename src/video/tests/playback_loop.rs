use super::*;

#[test]
fn a_run_of_unknown_length_that_ends_loops_from_the_start() {
    // `[gif @ …] [error] LZW decode failed`: a bad frame, but the file was read to its end.
    for error in [None, Some(("LZW decode failed", false))] {
        let t0 = Instant::now();
        let (spawn, runs) = fed_runs_logging(None, false, error);
        let mut player = fed_silent_player(spawn, 0.0, t0);
        let first = runs.borrow_mut().remove(0).1;
        first.send(repeated()).unwrap();
        first.send(blank()).unwrap();
        drop(first);
        player.tick(t0);
        player.tick(after(t0, 0.05));
        player.tick(after(t0, 0.15));
        assert_eq!(runs.borrow()[0].0, 0.0, "{error:?}");
    }
}

#[test]
fn when_a_resumed_run_is_cut_before_its_first_frame_it_should_retry_once() {
    let t0 = Instant::now();
    let (spawn, runs) = fed_runs_logging(None, false, DROPPED);
    let mut player = fed_silent_player(spawn, 5.0, t0);

    drop(runs.borrow_mut().remove(0));
    player.tick(t0);
    let retried = runs.borrow()[0].0;
    drop(runs.borrow_mut().remove(0));
    player.tick(after(t0, 0.1));
    let then = runs.borrow()[0].0;

    assert_eq!(retried, 5.0, "cut on the way to 0:05: once more from there");
    assert_eq!(then, 0.0, "cut again: no endless retries");
}

#[test]
fn when_a_run_from_the_start_is_cut_before_its_first_frame_it_should_retry_once() {
    let t0 = Instant::now();
    let (spawn, runs) = fed_runs_logging(None, false, DROPPED);
    let mut player = fed_silent_player(spawn, 0.0, t0);

    drop(runs.borrow_mut().remove(0));
    player.tick(t0);
    let retried = runs.borrow().len();
    drop(runs.borrow_mut().remove(0));
    player.tick(after(t0, 0.1));

    assert_eq!(retried, 1, "one more try");
    assert!(runs.borrow().is_empty(), "then no more");
    assert!(player.error().is_some());
}

#[test]
fn a_run_past_the_end_or_at_it_plays_again_from_the_start() {
    let t0 = Instant::now();
    // A run started past the last frame gives none: back to the start.
    let (spawn, runs) = fed_runs(None, false);
    let mut player = fed_silent_player(spawn, 5.0, t0);
    drop(runs.borrow_mut().remove(0));
    player.tick(t0);
    assert_eq!(runs.borrow()[0].0, 0.0);
    assert!(player.error().is_none());

    // A 1 s video that ran to its end loops rather than resuming at 1 s.
    let (mut player, runs) = fed_player(Some(1.0), t0);
    let first = runs.borrow_mut().remove(0).1;
    first.send(repeated()).unwrap();
    for _ in 1..24 {
        first.send(blank()).unwrap();
    }
    drop(first);
    for i in 0..=25 {
        player.tick(after(t0, f64::from(i) / MIN_FPS));
    }
    assert_eq!(runs.borrow()[0].0, 0.0);
}
