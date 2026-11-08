use super::*;

#[test]
fn when_a_safe_read_becomes_obsolete_it_should_release_header_and_body_waits() {
    use std::sync::mpsc::channel;
    for media in [false, true] {
        for body in [false, true] {
            let served = crate::reddit::tests::local_http_server();
            let base = format!("http://{}/", served.server_addr());
            let reddit = Reddit::test_at(&base, false);
            let obsolete = Arc::new(AtomicBool::new(false));
            let cancelled = obsolete.clone();
            let scoped = reddit.for_worker(Priority::Foreground, true, move || {
                cancelled.load(Ordering::Acquire)
            });
            let (started_tx, started_rx) = channel();
            let (release_tx, release_rx) = channel::<()>();
            let server = std::thread::spawn(move || {
                let request = served
                    .recv_timeout(Duration::from_secs(3))
                    .unwrap()
                    .unwrap();
                let held = if body {
                    Ok(request.stream(Some(100)))
                } else {
                    Err(request)
                };
                started_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
                drop(held);
            });
            let (done_tx, done_rx) = channel();
            let read = std::thread::spawn(move || {
                let result = if media {
                    scoped.still_image(&format!("{base}poster.png")).map(|_| ())
                } else {
                    scoped.search("rust", "x", None).map(|_| ())
                };
                done_tx.send(result).unwrap();
            });
            started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            obsolete.store(true, Ordering::Release);
            let result = done_rx.recv_timeout(Duration::from_millis(1200));
            release_tx.send(()).unwrap();
            server.join().unwrap();
            read.join().unwrap();
            assert!(
                matches!(result, Ok(Err(_))),
                "obsolete read must stop before server releases it: media={media}, body={body}, {result:?}"
            );
            assert_ne!(
                reddit.health(),
                Health::Down,
                "cancellation is not a connection failure"
            );
        }
    }
}

#[test]
fn when_a_safe_worker_read_is_limited_it_should_stay_pending_then_resume() {
    let count = Arc::new(AtomicUsize::new(0));
    let calls = count.clone();
    let fixture = Fixture::new(move |_| {
        if calls.fetch_add(1, Ordering::SeqCst) == 0 {
            Reply {
                status: 429,
                body: String::new(),
                headers: vec![("Retry-After".into(), "1".into())],
            }
        } else {
            Reply::json(empty_listing())
        }
    });
    let reddit = fixture.reddit();
    let scoped = reddit.for_worker(Priority::Foreground, true, || false);
    let (tx, rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        tx.send(scoped.search("rust", "PRIVATE_QUERY", None))
            .unwrap()
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    while reddit.pause().is_none() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    let pause = reddit.pause().unwrap();
    assert!((1..=2).contains(&pause.seconds));
    assert!(!pause.cause.contains("PRIVATE_QUERY") && !pause.cause.contains("fixture-session"));
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "read must remain pending during cooldown"
    );
    assert!(rx.recv_timeout(Duration::from_secs(3)).unwrap().is_ok());
    thread.join().unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 2);
    assert!(reddit.pause().is_none());
}

#[test]
fn when_a_waiting_read_is_retired_it_should_stop_without_another_request() {
    let count = Arc::new(AtomicUsize::new(0));
    let calls = count.clone();
    let fixture = Fixture::new(move |_| {
        calls.fetch_add(1, Ordering::SeqCst);
        Reply {
            status: 429,
            body: String::new(),
            headers: vec![("Retry-After".into(), "600".into())],
        }
    });
    let reddit = fixture.reddit();
    let scoped = reddit.for_worker(Priority::Foreground, true, || false);
    let (tx, rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || tx.send(scoped.search("rust", "x", None)).unwrap());
    let deadline = Instant::now() + Duration::from_secs(2);
    while reddit.pause().is_none() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "the read must stay pending before retirement"
    );
    reddit.retire();
    assert!(rx.recv_timeout(Duration::from_secs(1)).unwrap().is_err());
    thread.join().unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[test]
fn when_a_transmitted_write_becomes_obsolete_its_acknowledgment_should_still_finish() {
    let obsolete = Arc::new(AtomicBool::new(false));
    let after_send = obsolete.clone();
    let fixture = Fixture::new(move |request| {
        if request.method == "GET" {
            Reply::json(json!({"data":{"name":"reader","modhash":"fixture-token"}}))
        } else {
            after_send.store(true, Ordering::Release);
            Reply::json(json!({}))
        }
    });
    let scoped = fixture
        .reddit()
        .for_worker(Priority::Foreground, false, move || {
            obsolete.load(Ordering::Acquire)
        });

    assert!(
        scoped
            .set_membership(
                &crate::model::MembershipTarget::Community("rust".into()),
                true
            )
            .is_ok()
    );
}

#[test]
fn when_a_write_receives429_even_a_read_retry_context_should_not_replay_it() {
    let count = Arc::new(AtomicUsize::new(0));
    let calls = count.clone();
    let fixture = Fixture::new(move |request| {
        if request.method == "GET" {
            Reply::json(json!({"data":{"name":"reader","modhash":"fixture-token"}}))
        } else {
            calls.fetch_add(1, Ordering::SeqCst);
            Reply {
                status: 429,
                body: String::new(),
                headers: vec![("Retry-After".into(), "1".into())],
            }
        }
    });
    assert!(
        fixture
            .reddit()
            .for_worker(Priority::Foreground, true, || false)
            .set_membership(
                &crate::model::MembershipTarget::Community("rust".into()),
                true
            )
            .is_err()
    );
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[test]
fn when_a_session_is_rejected_requests_should_stop_without_disclosing_the_response() {
    let requests = Arc::new(AtomicUsize::new(0));
    let calls = requests.clone();
    let fixture = Fixture::new(move |_| {
        calls.fetch_add(1, Ordering::Relaxed);
        Reply::status(401)
    });
    let reddit = fixture.reddit();
    let error = reddit.search("rust", "x", None).unwrap_err().to_string();
    assert_eq!(error, session::SIGN_IN);
    assert_eq!(
        reddit.search("rust", "x", None).unwrap_err().to_string(),
        error
    );
    assert_eq!(requests.load(Ordering::Relaxed), 1);
    assert_eq!(reddit.health(), Health::SignedOut);
}
