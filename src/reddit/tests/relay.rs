use super::*;

#[test]
fn media_relay_should_end_short_rather_than_splice_a_refused_resume() {
    let body: Vec<u8> = (0..90_000u32).map(|i| (i % 251) as u8).collect();
    let (base, upstream) = truncating_upstream(body.clone(), true);
    let reddit = Reddit::test_at(&base, false);

    let received = relayed_body(&reddit, &format!("{base}segment.ts"), None);

    assert!(received.len() < body.len());
    assert!(
        received == body[..received.len()],
        "no foreign bytes after the cut"
    );
    assert_eq!(upstream.join().unwrap(), [None, Some(body.len() / 3)]);
}

#[test]
fn clip_relay_should_serve_manifests_while_four_streams_are_stalled() {
    let server = Arc::new(crate::reddit::tests::local_http_server());
    let base = format!("http://{}/", server.server_addr());
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    let upstream = server.clone();
    let accept = std::thread::spawn(move || {
        while !stopped.load(Ordering::Relaxed) {
            let Ok(Some(request)) = upstream.recv_timeout(Duration::from_millis(20)) else {
                continue;
            };
            std::thread::spawn(move || {
                if request.url().ends_with(".m3u8") {
                    let _ = request.respond(Response::from_string("#EXTM3U\n#EXT-X-ENDLIST\n"));
                } else {
                    let response = Response::new(
                        tiny_http::StatusCode(200),
                        vec![],
                        std::io::Read::take(std::io::repeat(0), 64 * 1024 * 1024),
                        Some(64 * 1024 * 1024),
                        None,
                    );
                    let _ = request.respond(response);
                }
            });
        }
    });
    let reddit = Reddit::test_at(&base, false);
    let video = reddit.media_url(&format!("{base}video.mp4")).unwrap();
    let manifest = reddit.media_url(&format!("{base}next.m3u8")).unwrap();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .proxy(None)
        .timeout_global(Some(Duration::from_secs(1)))
        .build()
        .into();
    let held: Vec<_> = (0..4).map(|_| agent.get(&video).call().unwrap()).collect();
    let result = agent
        .get(&manifest)
        .call()
        .and_then(|mut r| r.body_mut().read_to_string());
    drop(held);
    stop.store(true, Ordering::Relaxed);
    accept.join().unwrap();
    assert_eq!(
        result.expect("paused video responses must not starve another playlist"),
        "#EXTM3U\n#EXT-X-ENDLIST\n"
    );
}

#[test]
fn clip_relay_acceptor_should_resume_after_capacity_frees_with_a_pipelined_client() {
    let server = Arc::new(crate::reddit::tests::local_http_server());
    let base = format!("http://{}/", server.server_addr());
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    let upstream = server.clone();
    let accept = std::thread::spawn(move || {
        while !stopped.load(Ordering::Relaxed) {
            let Ok(Some(request)) = upstream.recv_timeout(Duration::from_millis(20)) else {
                continue;
            };
            std::thread::spawn(move || {
                if request.url().ends_with(".m3u8") {
                    let _ = request.respond(Response::from_string("#EXTM3U\n#EXT-X-ENDLIST\n"));
                } else {
                    let response = Response::new(
                        tiny_http::StatusCode(200),
                        vec![],
                        std::io::Read::take(std::io::repeat(0), 64 * 1024 * 1024),
                        Some(64 * 1024 * 1024),
                        None,
                    );
                    let _ = request.respond(response);
                }
            });
        }
    });
    let reddit = Reddit::test_at(&base, false);
    let video = reddit.media_url(&format!("{base}video.mp4")).unwrap();
    let manifest = reddit.media_url(&format!("{base}next.m3u8")).unwrap();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .proxy(None)
        .timeout_global(Some(Duration::from_secs(1)))
        .build()
        .into();
    use std::io::{BufRead, Write};
    let video_url = url::Url::parse(&video).unwrap();
    let host = format!("127.0.0.1:{}", video_url.port().unwrap());
    let mut pipelined = std::net::TcpStream::connect(&host).unwrap();
    pipelined
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    write!(
        pipelined,
        "GET {} HTTP/1.1\r\nHost: {host}\r\n\r\n",
        video_url.path()
    )
    .unwrap();
    let mut reader = std::io::BufReader::new(pipelined.try_clone().unwrap());
    loop {
        let mut line = String::new();
        assert!(reader.read_line(&mut line).unwrap() > 0);
        if line == "\r\n" {
            break;
        }
    }
    let mut held: Vec<_> = (0..31).map(|_| agent.get(&video).call().unwrap()).collect();
    let manifest_url = url::Url::parse(&manifest).unwrap();
    write!(
        pipelined,
        "GET {} HTTP/1.1\r\nHost: {host}\r\n\r\n",
        manifest_url.path()
    )
    .unwrap();
    // Wait at capacity before freeing slots. If the old acceptor handles this
    // probe first it returns503; if it handles the pipeline first it stays blocked
    // and the final request below times out even after capacity becomes available.
    let overloaded = agent.get(&manifest).call();
    // The pipelined response cannot finish until its paused first response finishes.
    // Free two other slots so that client and an independent request can proceed.
    held.pop();
    held.pop();
    let result = agent
        .get(&manifest)
        .call()
        .and_then(|mut r| r.body_mut().read_to_string());
    let _ = pipelined.shutdown(std::net::Shutdown::Both);
    drop(held);
    stop.store(true, Ordering::Relaxed);
    accept.join().unwrap();
    assert!(matches!(overloaded, Err(ureq::Error::Timeout(_))));
    assert_eq!(
        result.expect("paused video responses must not starve another playlist"),
        "#EXTM3U\n#EXT-X-ENDLIST\n"
    );
}

#[test]
fn media_failure_should_preserve_the_upstream_status_without_its_body() {
    let fixture = Fixture::new(|r| Reply {
        status: r.url.trim_start_matches('/').parse().unwrap(),
        body: "PRIVATE upstream body".into(),
        headers: vec![],
    });
    let reddit = Reddit::test_at(&fixture.base, false);
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .proxy(None)
        .timeout_global(Some(Duration::from_secs(5)))
        .http_status_as_error(false)
        .build()
        .into();
    for status in [403, 404, 410, 429, 500, 503] {
        let local = reddit
            .media_url(&format!("{}{status}", fixture.base))
            .unwrap();
        let mut response = agent.get(&local).call().unwrap();
        assert_eq!(response.status().as_u16(), status);
        assert_eq!(
            response.body_mut().read_to_string().unwrap(),
            "Media unavailable"
        );
    }
}

#[test]
fn media_relay_should_resume_a_body_that_ends_before_its_length() {
    let body: Vec<u8> = (0..90_000u32).map(|i| (i % 251) as u8).collect();
    for (range, from) in [(None, 0), (Some("bytes=1000-"), 1000)] {
        let (base, upstream) = truncating_upstream(body.clone(), false);
        let reddit = Reddit::test_at(&base, false);

        let received = relayed_body(&reddit, &format!("{base}segment.ts"), range);

        assert!(
            received == body[from..],
            "resumed bytes must continue the body exactly from {from}"
        );
        let resumed_at = from + (body.len() - from) / 3;
        assert_eq!(
            upstream.join().unwrap(),
            [range.map(|_| from), Some(resumed_at)]
        );
    }
}

#[test]
fn relay_follows_redirects_rewrites_playlist_and_serves_byte_ranges() {
    let ranges = Arc::new(Mutex::new(Vec::new()));
    let log = ranges.clone();
    let fixture = Fixture::new(move |r| {
        assert!(!r.headers.contains_key("authorization"));
        assert!(!r.headers.contains_key("cookie"));
        if r.url == "/start.m3u8" {
            return Reply {
                status: 302,
                body: String::new(),
                headers: vec![("Location".into(), "/folder/master.m3u8".into())],
            };
        }
        if r.url == "/folder/master.m3u8" {
            return Reply {
                status: 200,
                body: "#EXTM3U\nsegment.mp4\n".into(),
                headers: vec![],
            };
        }
        assert_eq!(r.url, "/folder/segment.mp4");
        log.lock().push(r.headers.get("range").cloned());
        Reply {
            status: 206,
            body: "part".into(),
            headers: vec![("Content-Range".into(), "bytes 0-3/100".into())],
        }
    });
    let reddit = Reddit::test_at(&fixture.base, false);
    let local = reddit
        .media_url(&format!("{}start.m3u8", fixture.base))
        .unwrap();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .proxy(None)
        .timeout_global(Some(Duration::from_secs(5)))
        .build()
        .into();
    let text = agent
        .get(&local)
        .call()
        .unwrap()
        .body_mut()
        .read_to_string()
        .unwrap();
    let segment = text.lines().nth(1).unwrap();
    assert!(segment.starts_with(local.split("/http/").next().unwrap()));
    let mut response = agent
        .get(segment)
        .header("Range", "bytes=0-3")
        .call()
        .unwrap();
    assert_eq!(response.status().as_u16(), 206);
    assert_eq!(response.headers()["content-range"], "bytes 0-3/100");
    assert_eq!(response.body_mut().read_to_string().unwrap(), "part");
    assert_eq!(*ranges.lock(), vec![Some("bytes=0-3".into())]);
}
