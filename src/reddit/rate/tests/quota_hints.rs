use super::*;

#[test]
fn when_quota_is_unknown_the_same_pool_should_keep_conservative_pacing() {
    let rate = Rate::new(Duration::from_millis(600));
    let url = Url::parse("https://www.reddit.com/r/rust/hot.json").unwrap();
    let started = Instant::now();
    rate.admit(&url, Priority::Listing, true, || false).unwrap();

    rate.admit(&url, Priority::Listing, true, || false).unwrap();

    assert!(started.elapsed() >= Duration::from_millis(600));
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
    let delayed = transport::wait(
        transport::send(transport.client.get(&fixture.base), None),
        None,
    )
    .unwrap()
    .unwrap();
    let no_quota = transport::wait(
        transport::send(
            transport.client.get(url.join("no-quota").unwrap().as_str()),
            None,
        ),
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
    let old_response = transport::wait(
        transport::send(transport.client.get(&fixture.base), None),
        None,
    )
    .unwrap()
    .unwrap();
    let later = rate
        .admit(&url, Priority::Selected, true, || false)
        .unwrap();
    let no_quota = transport::wait(
        transport::send(
            transport.client.get(url.join("no-quota").unwrap().as_str()),
            None,
        ),
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
    let response = transport::wait(
        transport::send(transport.client.get(&fixture.base), None),
        None,
    )
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
    let response = transport::wait(
        transport::send(transport.client.get(&fixture.base), None),
        None,
    )
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
            transport::send(
                transport.client.get(format!("{}{path}", fixture.base)),
                None,
            ),
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
