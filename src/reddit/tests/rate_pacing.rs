use super::*;

#[test]
fn when_json_and_home_have_separate_budgets_home_should_not_reset_json() {
    let fixture = Fixture::new(|request| Reply {
        status: 200,
        body: empty_listing().to_string(),
        headers: vec![
            (
                "X-Ratelimit-Remaining".into(),
                if request.url.starts_with(home::PATH) {
                    "100"
                } else {
                    "2"
                }
                .into(),
            ),
            (
                "X-Ratelimit-Reset".into(),
                if request.url.starts_with(home::PATH) {
                    "0.01"
                } else {
                    "12"
                }
                .into(),
            ),
        ],
    });
    let reddit = fixture
        .reddit()
        .for_worker(Priority::Background, true, || false);
    reddit.search("rust", "first", None).unwrap();
    reddit
        .request(reddit.0.api.join(home::PATH).unwrap(), home::ACCEPT)
        .unwrap();

    let deadline = Instant::now() + Duration::from_millis(100);
    let actual = reddit.0.rate.admit(
        &reddit.0.api.join("search.json").unwrap(),
        Priority::Background,
        true,
        || Instant::now() >= deadline,
    );
    assert!(
        matches!(actual,Err(error) if error.to_string()==REQUEST_CANCELLED),
        "Home erased the JSON admission delay"
    );
}

#[test]
fn when_a_success_reports_negative_remaining_quota_it_should_pause() {
    let fixture = Fixture::new(|_| Reply {
        status: 200,
        body: empty_listing().to_string(),
        headers: vec![
            ("X-Ratelimit-Remaining".into(), "-1".into()),
            ("X-Ratelimit-Reset".into(), "2".into()),
        ],
    });
    let reddit = fixture.reddit();
    reddit.search("rust", "x", None).unwrap();
    assert!(
        reddit.pause().is_some(),
        "negative remaining quota is exhausted"
    );
}

#[test]
fn when_success_reports_a_small_budget_the_next_request_should_be_paced() {
    let starts = Arc::new(Mutex::new(Vec::new()));
    let arrivals = starts.clone();
    let fixture = Fixture::new(move |_| {
        arrivals.lock().push(Instant::now());
        Reply {
            status: 200,
            body: empty_listing().to_string(),
            headers: vec![
                ("X-Ratelimit-Remaining".into(), "2".into()),
                ("X-Ratelimit-Reset".into(), "0.4".into()),
            ],
        }
    });
    let reddit = fixture
        .reddit()
        .for_worker(Priority::Background, true, || false);
    reddit.search("rust", "first", None).unwrap();

    reddit.search("rust", "second", None).unwrap();

    let starts = starts.lock();
    let spacing = starts[1].duration_since(starts[0]);
    assert!(
        spacing >= Duration::from_millis(150),
        "quota must space request starts, not response completions: {spacing:?}"
    );
}

#[test]
fn when_rate_limited_requests_should_pause_without_retrying() {
    let requests = Arc::new(AtomicUsize::new(0));
    let calls = requests.clone();
    let fixture = Fixture::new(move |_| {
        calls.fetch_add(1, Ordering::Relaxed);
        Reply {
            status: 429,
            body: String::new(),
            headers: vec![("Retry-After".into(), "600".into())],
        }
    });
    let reddit = fixture.reddit();
    assert!(reddit.search("rust", "x", None).is_err());
    assert!(
        reddit
            .request(reddit.0.api.join(home::PATH).unwrap(), home::ACCEPT)
            .is_err()
    );
    assert_eq!(requests.load(Ordering::Relaxed), 1);
    assert_eq!(reddit.health(), Health::Paused);
    assert!(reddit.pause().unwrap().seconds > 500);
}

#[test]
fn when_a_worker_waits_for_a_long_quota_interval_the_wait_should_be_visible() {
    let fixture = Fixture::new(|_| Reply {
        status: 200,
        body: empty_listing().to_string(),
        headers: vec![
            ("X-Ratelimit-Remaining".into(), "2".into()),
            ("X-Ratelimit-Reset".into(), "30".into()),
        ],
    });
    let reddit = fixture.reddit();
    reddit.search("rust", "first", None).unwrap();
    reddit.search("rust", "second", None).unwrap();
    reddit.search("rust", "third", None).unwrap();
    let (entered, waiting) = std::sync::mpsc::channel();
    let scoped = reddit.for_worker(Priority::Foreground, true, move || {
        let _ = entered.send(());
        false
    });
    let worker = std::thread::spawn(move || scoped.search("rust", "next", None));
    waiting.recv_timeout(Duration::from_secs(1)).unwrap();
    let pause = reddit.pause();
    reddit.retire();
    assert!(worker.join().unwrap().is_err());

    let pause = pause.expect("quota pacing must explain the pending15s wait");
    assert!((14..=15).contains(&pause.seconds));
}

#[test]
fn when_every_response_repeats_quota_navigation_should_not_get_unlimited_bursts() {
    let fixture = Fixture::new(|_| Reply {
        status: 200,
        body: empty_listing().to_string(),
        headers: vec![
            ("X-Ratelimit-Remaining".into(), "10".into()),
            ("X-Ratelimit-Reset".into(), "30".into()),
        ],
    });
    let reddit = fixture.reddit();
    reddit.subscriptions().unwrap();
    let deadline = Instant::now() + Duration::from_millis(500);
    let visible = reddit.for_worker(Priority::Listing, true, move || Instant::now() >= deadline);
    visible.search("rust", "first", None).unwrap();
    visible.search("rust", "second", None).unwrap();
    visible.search("rust", "third", None).unwrap();
    visible.search("rust", "fourth", None).unwrap();
    visible.search("rust", "fifth", None).unwrap();
    visible.search("rust", "sixth", None).unwrap();
    visible.search("rust", "seventh", None).unwrap();
    visible.search("rust", "eighth", None).unwrap();
    visible.search("rust", "ninth", None).unwrap();
    visible.search("rust", "tenth", None).unwrap();

    let actual = visible.search("rust", "eleventh", None);

    assert_eq!(actual.unwrap_err().to_string(), REQUEST_CANCELLED);
}

#[test]
fn when_rate_headers_are_dates_exhausted_or_malformed_the_wait_should_follow_valid_hints() {
    let future = (chrono::Utc::now() + chrono::Duration::seconds(4))
        .format("%a, %d %b %Y %H:%M:%S GMT")
        .to_string();
    for (status, headers, maximum) in [
        (429, vec![("Retry-After", future)], Some(4)),
        (
            429,
            vec![
                ("Retry-After", "-3".into()),
                ("X-Ratelimit-Reset", "2".into()),
            ],
            Some(2),
        ),
        (
            200,
            vec![
                ("X-Ratelimit-Remaining", "0".into()),
                ("X-Ratelimit-Reset", "2".into()),
            ],
            Some(2),
        ),
        (
            200,
            vec![
                ("X-Ratelimit-Remaining", "NaN".into()),
                ("X-Ratelimit-Reset", "2".into()),
            ],
            None,
        ),
    ] {
        let fixture = Fixture::new(move |_| Reply {
            status,
            body: empty_listing().to_string(),
            headers: headers
                .iter()
                .map(|(key, value)| ((*key).into(), value.clone()))
                .collect(),
        });
        let reddit = fixture.reddit();
        let result = reddit.search("rust", "x", None);
        assert_eq!(result.is_ok(), status == 200);
        match maximum {
            Some(max) => assert!((1..=max).contains(&reddit.pause().unwrap().seconds)),
            None => assert!(reddit.pause().is_none()),
        }
    }
}
