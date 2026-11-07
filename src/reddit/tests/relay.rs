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
fn media_failure_should_preserve_the_upstream_status_without_its_body() {
    let fixture = Fixture::new(|r| Reply {
        status: r.url.trim_start_matches('/').parse().unwrap(),
        body: "PRIVATE upstream body".into(),
        headers: vec![],
    });
    let reddit = Reddit::test_at(&fixture.base, false);
    for status in [403, 404, 410, 429, 500, 503] {
        let local = reddit
            .media_url(&format!("{}{status}", fixture.base))
            .unwrap();
        let response = relayed(&local, None);
        assert_eq!(response.status, status);
        assert_eq!(response.body, b"Media unavailable");
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
    let text = String::from_utf8(relayed(&local, None).body).unwrap();
    let segment = text.lines().nth(1).unwrap();
    assert!(segment.starts_with(local.split("/http/").next().unwrap()));
    let response = relayed(segment, Some("bytes=0-3"));
    assert_eq!(response.status, 206);
    assert!(
        response
            .headers
            .contains(&("content-range", "bytes 0-3/100".into()))
    );
    assert_eq!(response.body, b"part");
    assert_eq!(*ranges.lock(), vec![Some("bytes=0-3".into())]);
}
