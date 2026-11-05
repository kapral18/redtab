use super::*;

#[test]
fn when_quota_is_unknown_the_same_pool_should_keep_conservative_pacing() {
    let rate = Rate::new(Duration::from_secs(1));
    let url = Url::parse("https://www.reddit.com/r/rust/hot.json").unwrap();
    rate.admit(&url, Priority::Listing, true, || false).unwrap();
    let started = Instant::now();

    rate.admit(&url, Priority::Listing, true, || false).unwrap();

    assert!(started.elapsed() >= Duration::from_millis(900));
}

#[test]
fn when_an_older_request_finishes_before_a_delayed_newer_hint_its_reservation_should_survive() {
    use crate::reddit::{
        tests::{Fixture, Reply},
        transport::{self, Transport},
    };
    let fixture = Fixture::new(|request| Reply {
        status: 200,
        body: String::new(),
        headers: if request.url == "/no-quota" {
            vec![]
        } else {
            vec![
                ("X-Ratelimit-Remaining".into(), "1".into()),
                ("X-Ratelimit-Reset".into(), "30".into()),
            ]
        },
    });
    let transport = Transport::new().unwrap();
    let rate = Rate::new(Duration::ZERO);
    let url = Url::parse(&fixture.base).unwrap();
    let older = rate.admit(&url, Priority::Listing, true, || false).unwrap();
    let old_sent = Instant::now();
    let newer = rate
        .admit(&url, Priority::Selected, true, || false)
        .unwrap();
    let new_sent = Instant::now();
    let delayed = transport::wait(transport.client.get(&fixture.base).send(), None)
        .unwrap()
        .unwrap();
    let no_quota = transport::wait(
        transport
            .client
            .get(url.join("no-quota").unwrap().as_str())
            .send(),
        None,
    )
    .unwrap()
    .unwrap();
    rate.observe(&no_quota, &url, old_sent, older.sequence, older.older);
    drop(older);
    rate.observe(&delayed, &url, new_sent, newer.sequence, newer.older);
    let deadline = Instant::now() + Duration::from_millis(50);

    let actual = rate.admit(&url, Priority::Listing, true, || Instant::now() >= deadline);

    assert!(
        actual.is_err(),
        "an older completion cannot release a reservation belonging to a delayed hint"
    );
}

#[test]
fn when_a_later_request_finishes_without_quota_its_debit_should_survive_an_older_hint() {
    use crate::reddit::{
        tests::{Fixture, Reply},
        transport::{self, Transport},
    };
    let fixture = Fixture::new(|request| Reply {
        status: 200,
        body: String::new(),
        headers: if request.url == "/no-quota" {
            vec![]
        } else {
            vec![
                ("X-Ratelimit-Remaining".into(), "1".into()),
                ("X-Ratelimit-Reset".into(), "30".into()),
            ]
        },
    });
    let transport = Transport::new().unwrap();
    let rate = Rate::new(Duration::ZERO);
    let url = Url::parse(&fixture.base).unwrap();
    let older = rate.admit(&url, Priority::Listing, true, || false).unwrap();
    let sent = Instant::now();
    let old_response = transport::wait(transport.client.get(&fixture.base).send(), None)
        .unwrap()
        .unwrap();
    let later = rate
        .admit(&url, Priority::Selected, true, || false)
        .unwrap();
    let no_quota = transport::wait(
        transport
            .client
            .get(url.join("no-quota").unwrap().as_str())
            .send(),
        None,
    )
    .unwrap()
    .unwrap();
    rate.observe(&no_quota, &url, Instant::now(), later.sequence, later.older);
    drop(later);
    rate.observe(&old_response, &url, sent, older.sequence, older.older);
    let deadline = Instant::now() + Duration::from_millis(50);

    let actual = rate.admit(&url, Priority::Listing, true, || Instant::now() >= deadline);

    assert!(
        actual.is_err(),
        "the completed later request already consumed the older hint's last quota slot"
    );
}

#[test]
fn when_an_older_request_is_pending_a_newer_hint_should_reserve_its_quota() {
    use crate::reddit::{
        tests::{Fixture, Reply},
        transport::{self, Transport},
    };
    let fixture = Fixture::new(|_| Reply {
        status: 200,
        body: String::new(),
        headers: vec![
            ("X-Ratelimit-Remaining".into(), "1".into()),
            ("X-Ratelimit-Reset".into(), "30".into()),
        ],
    });
    let transport = Transport::new().unwrap();
    let rate = Rate::new(Duration::ZERO);
    let url = Url::parse(&fixture.base).unwrap();
    let _older = rate.admit(&url, Priority::Listing, true, || false).unwrap();
    let newer = rate
        .admit(&url, Priority::Selected, true, || false)
        .unwrap();
    let response = transport::wait(transport.client.get(&fixture.base).send(), None)
        .unwrap()
        .unwrap();
    rate.observe(&response, &url, Instant::now(), newer.sequence, newer.older);
    let deadline = Instant::now() + Duration::from_millis(50);

    let actual = rate.admit(&url, Priority::Listing, true, || Instant::now() >= deadline);

    assert!(
        actual.is_err(),
        "the older request can still consume the response's last quota slot"
    );
}

#[test]
fn when_later_starts_are_not_in_a_quota_hint_they_should_be_deducted() {
    use crate::reddit::{
        tests::{Fixture, Reply},
        transport::{self, Transport},
    };
    let fixture = Fixture::new(|_| Reply {
        status: 200,
        body: String::new(),
        headers: vec![
            ("X-Ratelimit-Remaining".into(), "1".into()),
            ("X-Ratelimit-Reset".into(), "30".into()),
        ],
    });
    let transport = Transport::new().unwrap();
    let rate = Rate::new(Duration::ZERO);
    let url = Url::parse(&fixture.base).unwrap();
    let first = rate.admit(&url, Priority::Listing, true, || false).unwrap();
    let sent = Instant::now();
    let _second = rate
        .admit(&url, Priority::Selected, true, || false)
        .unwrap();
    let response = transport::wait(transport.client.get(&fixture.base).send(), None)
        .unwrap()
        .unwrap();
    rate.observe(&response, &url, sent, first.sequence, first.older);
    let deadline = Instant::now() + Duration::from_millis(50);

    let actual = rate.admit(&url, Priority::Listing, true, || Instant::now() >= deadline);

    assert!(
        actual.is_err(),
        "the second start already spent the response's remaining request"
    );
}

#[test]
fn when_quota_is_replenished_continuous_navigation_should_not_postpone_background() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::channel,
    };
    let (rate, url) = quota("100", "3");
    let rate = Arc::new(rate);
    let starts = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let producer_rate = rate.clone();
    let producer_url = url.clone();
    let producer_starts = starts.clone();
    let producer_stop = stop.clone();
    let (entered, waiting) = channel();
    let producer = std::thread::spawn(move || {
        for _ in 0..20 {
            if producer_rate
                .admit(&producer_url, Priority::Selected, true, || {
                    let _ = entered.send(());
                    producer_stop.load(Ordering::Acquire)
                })
                .is_err()
            {
                break;
            }
            producer_starts.fetch_add(1, Ordering::SeqCst);
        }
    });
    waiting.recv_timeout(Duration::from_secs(1)).unwrap();
    let deadline = Instant::now() + Duration::from_millis(250);
    let registered_at = AtomicUsize::new(usize::MAX);

    let actual = rate.admit(&url, Priority::Background, true, || {
        let _ = registered_at.compare_exchange(
            usize::MAX,
            starts.load(Ordering::SeqCst),
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
        Instant::now() >= deadline
    });
    let before = starts.load(Ordering::SeqCst);
    let while_waiting = before.saturating_sub(registered_at.load(Ordering::SeqCst));
    stop.store(true, Ordering::Release);
    rate.wake();
    producer.join().unwrap();

    assert!(
        actual.is_ok(),
        "navigation repeatedly postponed the background slot"
    );
    assert!(
        while_waiting <= 3,
        "background waited for {while_waiting} selected starts after registering"
    );
}

#[test]
fn when_budget_replies_arrive_out_of_order_the_older_hint_should_not_restore_slow_pacing() {
    use crate::reddit::{
        tests::{Fixture, Reply},
        transport::{self, Transport},
    };
    let fixture = Fixture::new(|request| Reply {
        status: 200,
        body: String::new(),
        headers: vec![
            (
                "X-Ratelimit-Remaining".into(),
                if request.url == "/old" { "2" } else { "100" }.into(),
            ),
            ("X-Ratelimit-Reset".into(), "1".into()),
        ],
    });
    let transport = Transport::new().unwrap();
    let response = |path| {
        transport::wait(
            transport
                .client
                .get(format!("{}{path}", fixture.base))
                .send(),
            None,
        )
        .unwrap()
        .unwrap()
    };
    let old = response("old");
    let new = response("new");
    let rate = Rate::new(Duration::ZERO);
    let url = Url::parse(&fixture.base).unwrap();
    let home = url.join(super::super::super::home::PATH).unwrap();
    let now = Instant::now();
    rate.observe(&old, &url, now - Duration::from_secs(3), 0, 0);
    rate.observe(&old, &home, now, 0, 0);
    rate.observe(&new, &url, now - Duration::from_secs(1), 0, 0);

    rate.observe(&old, &url, now - Duration::from_secs(2), 0, 0);

    assert_eq!(
        rate.state.lock().budgets[0].interval,
        Duration::from_millis(10)
    );
}
