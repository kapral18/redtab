use super::*;

#[test]
fn bounded_browsing_should_leave_the_last_three_credits_for_selected_posts() {
    let (rate, url) = quota("6", "600");
    for _ in 0..3 {
        rate.admit(&url, Priority::Browsing, true, || false)
            .unwrap();
    }

    for priority in [Priority::Browsing, Priority::Background] {
        rate.state.lock().budgets[0].next = Instant::now();
        let deadline = Instant::now() + Duration::from_millis(50);
        assert!(
            rate.admit(&url, priority, true, || Instant::now() >= deadline)
                .is_err()
        );
    }
    for _ in 0..3 {
        let deadline = Instant::now() + Duration::from_millis(50);
        assert!(
            rate.admit(&url, Priority::Selected, true, || Instant::now()
                >= deadline)
                .is_ok()
        );
    }
}

#[test]
fn low_quota_browsing_waiters_should_not_block_a_selected_fairness_turn() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::channel,
    };
    let (mut rate, url) = quota("19", "561");
    rate.base = Duration::from_secs(1);
    let rate = Arc::new(rate);
    for _ in 0..16 {
        rate.admit(&url, Priority::Browsing, true, || false)
            .unwrap();
    }
    for _ in 0..2 {
        rate.admit(&url, Priority::Selected, true, || false)
            .unwrap();
    }
    let stop = Arc::new(AtomicBool::new(false));
    let worker_rate = rate.clone();
    let worker_url = url.clone();
    let worker_stop = stop.clone();
    let (entered, waiting) = channel();
    let worker = std::thread::spawn(move || {
        worker_rate
            .admit(&worker_url, Priority::Browsing, true, || {
                let _ = entered.send(());
                worker_stop.load(Ordering::Acquire)
            })
            .map(|_| ())
    });
    waiting.recv_timeout(Duration::from_secs(1)).unwrap();
    let deadline = Instant::now() + Duration::from_millis(300);

    let actual = rate.admit(&url, Priority::Selected, true, || {
        Instant::now() >= deadline
    });

    stop.store(true, Ordering::Release);
    rate.wake();
    assert!(worker.join().unwrap().is_err());
    assert!(actual.is_ok(), "paced browsing held a ready post open");
}

#[test]
fn when_quota_cannot_cover_the_reserve_only_interactive_reads_should_progress() {
    let (rate, url) = quota("2", "30");
    rate.state.lock().budgets[0].next = Instant::now();
    let deadline = Instant::now() + Duration::from_millis(50);

    let actual = rate.admit(&url, Priority::Background, true, || {
        Instant::now() >= deadline
    });

    assert!(
        actual.is_err(),
        "background work spent the protected interactive quota"
    );
    for priority in [Priority::Listing, Priority::Selected] {
        let deadline = Instant::now() + Duration::from_millis(50);
        assert!(
            rate.admit(&url, priority, true, || Instant::now() >= deadline)
                .is_ok()
        );
    }
}

#[test]
fn waiting_background_spacing_should_not_block_listings_or_starve_its_fair_turn() {
    use std::sync::{Arc, mpsc::channel};

    let (mut rate, url) = quota("100", "300");
    rate.base = Duration::from_secs(1);
    rate.state.lock().budgets[0].next = Instant::now();
    let started = Instant::now();
    rate.admit(&url, Priority::Background, true, || false)
        .unwrap();
    let rate = Arc::new(rate);
    let worker_rate = Arc::clone(&rate);
    let worker_url = url.clone();
    let (entered, waiting) = channel();
    let background = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_millis(1200);
        worker_rate
            .admit(&worker_url, Priority::Background, true, || {
                let _ = entered.send(());
                Instant::now() >= deadline
            })
            .map(|_| started.elapsed())
    });
    waiting.recv_timeout(Duration::from_secs(1)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut foreground = Vec::new();
    for _ in 0..5 {
        if rate
            .admit(&url, Priority::Listing, true, || Instant::now() >= deadline)
            .is_err()
        {
            break;
        }
        foreground.push(started.elapsed());
    }
    let actual_background = background.join().unwrap();

    assert_eq!(foreground.len(), 5);
    assert!(
        foreground[3] < Duration::from_millis(550),
        "ready listings waited behind background spacing: {foreground:?}"
    );
    let actual_background = actual_background.unwrap();
    assert!(
        (Duration::from_millis(450)..Duration::from_millis(950)).contains(&actual_background),
        "background lost its paced fair turn: {actual_background:?}"
    );
}

#[test]
fn when_burst_credit_recovers_background_should_make_progress() {
    let (rate, url) = quota("10", "0.2");
    rate.admit(&url, Priority::Listing, true, || false).unwrap();
    rate.admit(&url, Priority::Listing, true, || false).unwrap();
    rate.admit(&url, Priority::Listing, true, || false).unwrap();
    let deadline = Instant::now() + Duration::from_millis(150);

    let actual = rate.admit(&url, Priority::Background, true, || {
        Instant::now() >= deadline
    });

    assert!(
        actual.is_ok(),
        "paced traffic must resume after one quota interval"
    );
}

#[test]
fn when_background_is_waiting_after_two_selected_starts_it_should_get_its_fair_turn() {
    use std::sync::{Arc, mpsc::channel};

    let (rate, url) = quota("100", "300");
    let rate = Arc::new(rate);
    rate.admit(&url, Priority::Selected, true, || false)
        .unwrap();
    rate.admit(&url, Priority::Selected, true, || false)
        .unwrap();
    let (entered, waiting) = channel();
    let background_rate = Arc::clone(&rate);
    let background_url = url.clone();
    let background = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_millis(600);
        background_rate
            .admit(&background_url, Priority::Background, true, || {
                let _ = entered.send(());
                Instant::now() >= deadline
            })
            .is_ok()
    });
    waiting.recv_timeout(Duration::from_secs(1)).unwrap();

    rate.admit(&url, Priority::Selected, true, || false)
        .unwrap();
    let actual = background.join().unwrap();

    assert!(
        actual,
        "selected starts repeatedly delayed the waiting background"
    );
}

#[test]
fn when_selected_reads_keep_arriving_other_classes_should_still_make_progress() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc::channel,
    };
    let url = Url::parse("https://www.reddit.com/hot.json").unwrap();
    let rate = Arc::new(Rate::new(Duration::from_millis(30)));
    let selected_starts = Arc::new(AtomicUsize::new(0));
    rate.state.lock().last_start = Some(Instant::now() + Duration::from_secs(5));
    let (entered, waiting) = channel();
    let (done, results) = channel();
    let mut threads = Vec::new();
    for (priority, name) in [
        (Priority::Foreground, "feed"),
        (Priority::Background, "background"),
    ] {
        let rate = rate.clone();
        let url = url.clone();
        let selected_starts = selected_starts.clone();
        let entered = entered.clone();
        let done = done.clone();
        threads.push(std::thread::spawn(move || {
            let first = std::sync::atomic::AtomicBool::new(true);
            rate.admit(&url, priority, true, || {
                if first.swap(false, std::sync::atomic::Ordering::Relaxed) {
                    entered.send(()).unwrap();
                }
                false
            })
            .unwrap();
            done.send((name, selected_starts.load(Ordering::SeqCst)))
                .unwrap();
        }));
        waiting.recv_timeout(Duration::from_secs(1)).unwrap();
    }
    let selected = rate.clone();
    let starts = selected_starts.clone();
    let producer = std::thread::spawn(move || {
        for _ in 0..24 {
            selected
                .admit(&url, Priority::Selected, true, || false)
                .unwrap();
            starts.fetch_add(1, Ordering::SeqCst);
        }
    });
    let deadline = Instant::now() + Duration::from_secs(1);
    while rate.state.lock().waiting[0][Priority::Selected as usize] == 0 {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    rate.state.lock().last_start = None;
    rate.wake();
    let first = results.recv_timeout(Duration::from_secs(1));
    let second = results.recv_timeout(Duration::from_secs(1));
    producer.join().unwrap();
    for thread in threads {
        thread.join().unwrap();
    }
    let (first, before_feed) = first.unwrap();
    let (second, before_background) = second.unwrap();
    assert_eq!(first, "feed");
    assert_eq!(second, "background");
    assert!(
        before_feed <= 2,
        "feed waited for {before_feed} selected starts"
    );
    assert!(
        before_background <= 4,
        "background waited for {before_background} selected starts"
    );
}

#[test]
fn when_selected_and_feed_reads_are_waiting_selected_should_get_the_next_paced_slot() {
    use std::sync::{Arc, mpsc::channel};
    let url = Url::parse("https://www.reddit.com/hot.json").unwrap();
    let rate = Arc::new(Rate::new(Duration::from_millis(80)));
    rate.state.lock().last_start = Some(Instant::now() + Duration::from_secs(5));
    let (entered, waiting) = channel();
    let (finished, results) = channel();
    let mut threads = Vec::new();
    for (priority, name) in [
        (Priority::Foreground, "feed"),
        (Priority::Selected, "selected"),
    ] {
        let rate = Arc::clone(&rate);
        let url = url.clone();
        let entered = entered.clone();
        let finished = finished.clone();
        threads.push(std::thread::spawn(move || {
            let first = std::sync::atomic::AtomicBool::new(true);
            rate.admit(&url, priority, true, || {
                if first.swap(false, std::sync::atomic::Ordering::Relaxed) {
                    entered.send(()).unwrap();
                }
                false
            })
            .unwrap();
            finished.send((name, Instant::now())).unwrap();
        }));
        waiting.recv_timeout(Duration::from_secs(1)).unwrap();
    }
    rate.state.lock().last_start = None;
    rate.wake();
    let first = results.recv_timeout(Duration::from_secs(1)).unwrap();
    let second = results.recv_timeout(Duration::from_secs(1)).unwrap();
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(first.0, "selected");
    assert_eq!(second.0, "feed");
    assert!(second.1.duration_since(first.1) >= Duration::from_millis(65));
}
