use super::*;

#[test]
fn when_reddit_has_quota_headroom_rapid_community_navigation_should_not_pause_early() {
    let (rate, url) = quota("56", "300");

    let actual = rapid_listings(&rate, &url, 56);

    assert_eq!(actual, 56, "visible navigation paused with quota remaining");
    let deadline = Instant::now() + Duration::from_millis(50);
    assert!(
        rate.admit(&url, Priority::Listing, true, || Instant::now() >= deadline)
            .is_err(),
        "navigation exceeded the reported quota"
    );
}

#[test]
fn background_burst_should_stay_bounded_without_pacing_browsing() {
    use crate::reddit::{
        tests::{Fixture, Reply},
        transport::{self, Transport},
    };
    let fixture = Fixture::new(|_| Reply {
        status: 200,
        body: String::new(),
        headers: vec![
            ("X-Ratelimit-Remaining".into(), "99".into()),
            ("X-Ratelimit-Reset".into(), "600".into()),
        ],
    });
    let transport = Transport::new().unwrap();
    let url = Url::parse(&fixture.base).unwrap();
    let rate = Rate::new(Duration::ZERO);
    let response = transport::wait(
        transport::send(transport.client.get(&fixture.base), None),
        None,
    )
    .unwrap()
    .unwrap();
    rate.observe(&response, &url, Instant::now(), 0, 0);
    for _ in 0..16 {
        let ticket = rate
            .admit(&url, Priority::Browsing, true, || false)
            .unwrap();
        let sent = Instant::now();
        let response = transport::wait(
            transport::send(transport.client.get(&fixture.base), None),
            None,
        )
        .unwrap()
        .unwrap();
        rate.observe(&response, &url, sent, ticket.sequence, ticket.older);
    }

    rate.state.lock().budgets[0].next = Instant::now();
    let deadline = Instant::now() + Duration::from_millis(50);
    assert!(
        rate.admit(&url, Priority::Background, true, || Instant::now()
            >= deadline)
            .is_err(),
        "background work exceeded its spent burst credit"
    );
    let deadline = Instant::now() + Duration::from_millis(50);
    assert!(
        rate.admit(&url, Priority::Browsing, true, || Instant::now()
            >= deadline)
            .is_ok(),
        "listing work was paced despite available quota"
    );
    rate.state.lock().budgets[0].credited -= Duration::from_secs(7);
    rate.state.lock().budgets[0].next = Instant::now();
    let deadline = Instant::now() + Duration::from_millis(50);
    assert!(
        rate.admit(&url, Priority::Background, true, || Instant::now()
            >= deadline)
            .is_ok()
    );
    rate.state.lock().budgets[0].next = Instant::now();
    let deadline = Instant::now() + Duration::from_millis(50);
    assert!(
        rate.admit(&url, Priority::Background, true, || Instant::now()
            >= deadline)
            .is_err()
    );
}

#[test]
fn when_congestion_native_settings_reply_arrives_json_quota_should_remain_independent() {
    use crate::reddit::{
        tests::{Fixture, Reply},
        transport::{self, Transport},
    };
    let fixture = Fixture::new(|_| Reply {
        status: 200,
        body: String::new(),
        headers: vec![
            ("X-Ratelimit-Remaining".into(), "600".into()),
            ("X-Ratelimit-Reset".into(), "600".into()),
        ],
    });
    let (rate, json) = quota("10", "30");
    let native = Url::parse(&fixture.base)
        .unwrap()
        .join("settings/preferences")
        .unwrap();
    let response = transport::wait(
        transport::send(Transport::new().unwrap().client.get(native.as_str()), None),
        None,
    )
    .unwrap()
    .unwrap();

    rate.observe(&response, &native, Instant::now(), 0, 0);

    assert_eq!(
        rate.state.lock().budgets[budget_index(&json)].interval,
        Duration::from_secs(3)
    );
    assert_eq!(
        budget_index(&native),
        budget_index(&Url::parse("https://www.reddit.com/svc/shreddit/graphql").unwrap())
    );
}

#[test]
fn when_congestion_background_uses_idle_capacity_three_navigation_starts_should_remain() {
    let (rate, url) = quota("100", "30");
    {
        let mut state = rate.state.lock();
        state.budgets[0].next = Instant::now();
        state.budgets[0].credited = Instant::now() - Duration::from_secs(2);
    }
    let deadline = Instant::now() + Duration::from_millis(50);
    rate.admit(&url, Priority::Background, true, || {
        Instant::now() >= deadline
    })
    .unwrap();

    let actual: Vec<_> = [Priority::Listing, Priority::Selected, Priority::Foreground]
        .into_iter()
        .map(|rank| {
            let deadline = Instant::now() + Duration::from_millis(50);
            rate.admit(&url, rank, true, || Instant::now() >= deadline)
                .is_ok()
        })
        .collect();

    assert_eq!(actual, [true, true, true]);
}

#[test]
fn when_background_uses_one_credit_eighteen_navigation_starts_should_remain() {
    let (rate, url) = quota("100", "600");
    rate.state.lock().budgets[0].next = Instant::now();
    rate.admit(&url, Priority::Background, true, || false)
        .unwrap();

    let actual = rapid_listings(&rate, &url, 18);

    assert_eq!(actual, 18);
}

#[test]
fn when_background_arrives_after_a_burst_it_should_not_wait_for_all_credits() {
    let (rate, url) = quota("100", "10");
    for _ in 0..6 {
        rate.admit(&url, Priority::Listing, true, || false).unwrap();
    }
    let deadline = Instant::now() + Duration::from_millis(450);

    let actual = rate.admit(&url, Priority::Background, true, || {
        Instant::now() >= deadline
    });

    assert!(actual.is_ok(), "background waited for a full burst refill");
}

#[test]
fn when_congestion_native_preferences_are_paced_labels_should_identify_account_settings() {
    assert_eq!(
        operation(&Url::parse("https://www.reddit.com/settings/preferences").unwrap()),
        "account settings"
    );
    assert_eq!(
        operation(&Url::parse("https://www.reddit.com/svc/shreddit/graphql").unwrap()),
        "account settings"
    );
}

#[test]
fn when_background_is_paced_it_should_leave_interactive_capacity_available() {
    let (rate, url) = quota("10", "30");
    let deadline = Instant::now() + Duration::from_millis(50);
    assert!(
        rate.admit(&url, Priority::Background, true, || Instant::now()
            >= deadline)
            .is_err()
    );

    let actual = rate.admit(&url, Priority::Listing, true, || false);

    assert!(actual.is_ok());
}

#[test]
fn when_only_background_work_is_paced_it_should_not_mark_reddit_paused() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::channel,
    };
    let (rate, url) = quota("10", "30");
    let rate = Arc::new(rate);
    let stop = Arc::new(AtomicBool::new(false));
    let worker_rate = rate.clone();
    let worker_stop = stop.clone();
    let (entered, waiting) = channel();
    let worker = std::thread::spawn(move || {
        worker_rate
            .admit(&url, Priority::Background, true, || {
                let _ = entered.send(());
                worker_stop.load(Ordering::Acquire)
            })
            .map(|_| ())
    });
    waiting.recv_timeout(Duration::from_secs(1)).unwrap();

    let actual = rate.pause();
    stop.store(true, Ordering::Release);
    rate.wake();
    assert!(worker.join().unwrap().is_err());

    assert!(
        actual.is_none(),
        "background pacing must not announce a blocked connection"
    );
}

#[test]
fn when_interactive_credit_is_available_starts_should_still_obey_base_spacing() {
    let (mut rate, url) = quota("10", "30");
    rate.base = Duration::from_millis(80);
    rate.admit(&url, Priority::Listing, true, || false).unwrap();
    let started = Instant::now();

    rate.admit(&url, Priority::Selected, true, || false)
        .unwrap();

    assert!(started.elapsed() >= Duration::from_millis(65));
}

#[test]
fn when_navigation_has_quota_credit_it_should_start_quickly_and_keep_background_spacing() {
    let (mut rate, url) = quota("100", "300");
    rate.base = Duration::from_secs(1);
    rate.state.lock().budgets[0].next = Instant::now();
    let background_started = Instant::now();
    rate.admit(&url, Priority::Background, true, || false)
        .unwrap();
    for priority in [Priority::Listing, Priority::Selected] {
        let started = Instant::now();
        rate.admit(&url, priority, true, || false).unwrap();
        let elapsed = started.elapsed();
        assert!(
            (Duration::from_millis(75)..Duration::from_millis(350)).contains(&elapsed),
            "available navigation credit waited {elapsed:?}"
        );
    }
    rate.state.lock().budgets[0].next = Instant::now();
    rate.admit(&url, Priority::Background, true, || false)
        .unwrap();
    assert!(
        (Duration::from_millis(450)..Duration::from_millis(675))
            .contains(&background_started.elapsed()),
        "background spacing followed intervening navigation: {:?}",
        background_started.elapsed()
    );
}

#[test]
fn when_quota_is_smaller_than_the_burst_it_should_limit_total_admissions() {
    let (rate, url) = quota("1", "30");
    rate.admit(&url, Priority::Listing, true, || false).unwrap();
    let deadline = Instant::now() + Duration::from_millis(50);

    let actual = rate.admit(&url, Priority::Selected, true, || {
        Instant::now() >= deadline
    });

    assert!(
        actual.is_err(),
        "an unacknowledged start already spent the last known request"
    );
}

#[test]
fn when_callers_share_admission_they_should_start_apart_and_cancel_without_consuming_a_slot() {
    let rate = Rate::new(Duration::from_millis(80));
    let home = Url::parse("https://www.reddit.com/svc/shreddit/feeds/home-feed").unwrap();
    let json = Url::parse("https://www.reddit.com/hot.json").unwrap();
    rate.admit(&home, Priority::Foreground, true, || false)
        .unwrap();
    assert!(
        rate.admit(&json, Priority::Foreground, true, || true)
            .is_err()
    );
    let start = Instant::now();
    rate.admit(&json, Priority::Background, true, || false)
        .unwrap();
    assert!(start.elapsed() >= Duration::from_millis(65));
}

#[test]
fn when_a_quota_blocks_one_pool_ready_work_should_still_use_shared_admission() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::channel,
    };
    for (blocked, ready, burst) in [
        (Priority::Selected, Priority::Listing, 0),
        (Priority::Foreground, Priority::Selected, 2),
    ] {
        let rate = Arc::new(Rate::new(Duration::ZERO));
        let home = Url::parse("https://www.reddit.com/svc/shreddit/feeds/home-feed").unwrap();
        let json = Url::parse("https://www.reddit.com/hot.json").unwrap();
        {
            let mut state = rate.state.lock();
            state.budgets[0].next = Instant::now() + Duration::from_secs(5);
            state.bursts[ready as usize] = burst;
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let stop = cancelled.clone();
        let worker_rate = rate.clone();
        let (entered, waiting) = channel();
        let worker = std::thread::spawn(move || {
            worker_rate
                .admit(&json, blocked, true, || {
                    let _ = entered.send(());
                    stop.load(Ordering::Acquire)
                })
                .map(|_| ())
        });
        waiting.recv_timeout(Duration::from_secs(1)).unwrap();
        let deadline = Instant::now() + Duration::from_millis(500);

        let actual = rate.admit(&home, ready, true, || Instant::now() >= deadline);

        cancelled.store(true, Ordering::Release);
        rate.wake();
        assert!(worker.join().unwrap().is_err());
        assert!(
            actual.is_ok(),
            "ready Home work was blocked by a JSON waiter"
        );
    }
}

#[test]
fn when_routes_contain_private_data_diagnostics_should_only_name_the_operation() {
    for (url, expected) in [
        (
            "https://www.reddit.com/search.json?q=SECRET",
            "Reddit content",
        ),
        (
            "https://www.reddit.com/comments/abc/SECRET?token=SECRET",
            "post/comments",
        ),
        ("https://www.reddit.com/user/SECRET.json", "Reddit content"),
        (
            "https://www.reddit.com/r/rust/about.json?token=SECRET",
            "r/rust info",
        ),
    ] {
        assert_eq!(operation(&Url::parse(url).unwrap()), expected);
    }
}
