use super::*;

#[test]
fn when_reddit_is_unreachable_requests_should_fail_without_a_rate_limit() {
    let reddit = Reddit::test_at("http://127.0.0.1:9/", false);
    assert_eq!(
        reddit.search("rust", "x", None).unwrap_err().to_string(),
        "Reddit connection failed"
    );
    assert_eq!(reddit.pause(), None);
    assert_eq!(reddit.health(), Health::Down);
}

#[test]
fn health_follows_the_latest_response_and_down_outranks_a_cooldown() {
    for (status, health) in [
        (200, Health::Up),
        (401, Health::SignedOut),
        (403, Health::Up),
        (404, Health::Up),
        (429, Health::Paused),
        (500, Health::Down),
    ] {
        let fixture = Fixture::new(move |_| match status {
            200 => Reply::json(empty_listing()),
            _ => Reply::status(status),
        });
        let reddit = Reddit::test_at(&fixture.base, false);
        let _ = reddit.search("rust", "x", None);
        assert_eq!(reddit.health(), health, "HTTP {status}");
    }
}

#[test]
fn check_is_up_only_when_the_listing_loads() {
    for (status, health) in [
        (200, Health::Up),
        (403, Health::Down),
        (404, Health::Down),
        (429, Health::Paused),
    ] {
        let fixture = Fixture::new(move |_| match status {
            200 => Reply {
                status: 200,
                body: "<faceplate-partial id='feed-next-page-partial'></faceplate-partial>".into(),
                headers: vec![],
            },
            _ => Reply::status(status),
        });
        let reddit = Reddit::test_at(&fixture.base, false);
        assert_eq!(reddit.check(), health, "HTTP {status}");
    }
}

#[test]
fn when_reddit_returns_nonlisting_json_check_should_not_report_up() {
    let fixture = Fixture::new(|_| Reply::json(json!({})));
    assert_eq!(fixture.reddit().check(), Health::Down);
}
