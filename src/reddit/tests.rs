use super::*;
use std::io::Read;
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tiny_http::{Header, Response, Server};

struct Request {
    url: String,
    headers: HashMap<String, String>,
    body: String,
}

struct Reply {
    status: u16,
    body: String,
    headers: Vec<(String, String)>,
}

impl Reply {
    fn json(body: Value) -> Self {
        Self {
            status: 200,
            body: body.to_string(),
            headers: vec![],
        }
    }
    fn status(status: u16) -> Self {
        Self {
            status,
            body: "body must never appear in errors".into(),
            headers: vec![],
        }
    }
}

struct Fixture {
    base: String,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Fixture {
    fn new(handler: impl Fn(Request) -> Reply + Send + 'static) -> Self {
        let server = Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", server.server_addr());
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let thread = std::thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                if let Some(mut request) = server.recv_timeout(Duration::from_millis(20)).unwrap() {
                    // The host's portkeeper probes new listeners; redtab never sends HEAD /.
                    if request.method() == &tiny_http::Method::Head && request.url() == "/" {
                        let _ = request.respond(Response::empty(200));
                        continue;
                    }
                    let mut body = String::new();
                    request
                        .as_reader()
                        .take(1024 * 1024)
                        .read_to_string(&mut body)
                        .unwrap();
                    let input = Request {
                        url: request.url().to_owned(),
                        body,
                        headers: request
                            .headers()
                            .iter()
                            .map(|h| {
                                (
                                    h.field.to_string().to_ascii_lowercase(),
                                    h.value.to_string(),
                                )
                            })
                            .collect(),
                    };
                    let reply = handler(input);
                    let mut response =
                        Response::from_string(reply.body).with_status_code(reply.status);
                    for (name, value) in reply.headers {
                        response.add_header(Header::from_bytes(name, value).unwrap());
                    }
                    let _ = request.respond(response);
                }
            }
        });
        Self {
            base,
            stop,
            thread: Some(thread),
        }
    }
    fn reddit(&self) -> Reddit {
        Reddit::with_transport(
            Transport::new().unwrap(),
            false,
            &self.base,
            &format!("{}auth", self.base),
        )
        .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let result = self.thread.take().unwrap().join();
        if !std::thread::panicking() {
            result.expect("fixture server");
        }
    }
}

fn empty_listing() -> Value {
    json!({"kind":"Listing","data":{"children":[],"after":null}})
}

#[test]
fn concurrent_requests_share_a_guest_and_an_expired_guest_refreshes_once() {
    let auths = Arc::new(AtomicUsize::new(0));
    let count = auths.clone();
    let fixture = Fixture::new(move |request| {
        if request.url == "/auth" {
            count.fetch_add(1, Ordering::Relaxed);
            let body: Value = serde_json::from_str(&request.body).unwrap();
            assert_eq!(body, json!({"scopes":["*","email","pii"]}));
            assert_eq!(
                request.headers["client-vendor-id"],
                request.headers["x-reddit-device-id"]
            );
            let mut reply =
                Reply::json(json!({"access_token":"guest-test-token","expires_in":3600}));
            reply
                .headers
                .push(("x-reddit-loid".into(), "guest-loid".into()));
            reply
        } else {
            assert_eq!(
                request.headers.get("authorization").map(String::as_str),
                Some("Bearer guest-test-token"),
                "path={} headers={:?}",
                request.url,
                request.headers.keys()
            );
            assert_eq!(request.headers["x-reddit-loid"], "guest-loid");
            assert!(!request.headers.contains_key("cookie"));
            Reply::json(empty_listing())
        }
    });
    let reddit = fixture.reddit();
    let run = || {
        std::thread::scope(|scope| {
            for _ in 0..6 {
                let reddit = reddit.clone();
                scope.spawn(move || reddit.search("rust", "λ & space", None).unwrap());
            }
        })
    };
    run();
    assert_eq!(auths.load(Ordering::Relaxed), 1);
    reddit.0.session.lock().as_mut().unwrap().expires = Instant::now() - Duration::from_secs(1);
    run();
    assert_eq!(auths.load(Ordering::Relaxed), 2);
    assert!(!reddit.cache_namespace().contains("guest-test-token"));
}

#[test]
fn rejection_refreshes_once_and_rate_limits_stop_without_new_identities() {
    let auths = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(AtomicUsize::new(0));
    let (count, calls) = (auths.clone(), requests.clone());
    let fixture = Fixture::new(move |request| {
        if request.url == "/auth" {
            let n = count.fetch_add(1, Ordering::Relaxed) + 1;
            return Reply::json(json!({"access_token":format!("guest-{n}"),"expires_in":3600}));
        }
        calls.fetch_add(1, Ordering::Relaxed);
        if request.headers["authorization"] == "Bearer guest-1" {
            return Reply::status(401);
        }
        let mut reply = Reply::status(429);
        reply.headers.push(("Retry-After".into(), "600".into()));
        reply
    });
    let reddit = fixture.reddit();
    assert!(
        reddit
            .search("rust", "x", None)
            .unwrap_err()
            .to_string()
            .contains("rate limit")
    );
    assert!(
        reddit
            .search("rust", "x", None)
            .unwrap_err()
            .to_string()
            .contains("paused")
    );
    assert_eq!(auths.load(Ordering::Relaxed), 2);
    assert_eq!(requests.load(Ordering::Relaxed), 2);
}

#[test]
fn a_second_rejection_reports_the_guest_session_without_the_body() {
    let auths = Arc::new(AtomicUsize::new(0));
    let count = auths.clone();
    let fixture = Fixture::new(move |request| {
        if request.url == "/auth" {
            count.fetch_add(1, Ordering::Relaxed);
            return Reply::json(json!({"access_token":"guest","expires_in":3600}));
        }
        Reply::status(401)
    });

    let error = fixture
        .reddit()
        .search("rust", "x", None)
        .unwrap_err()
        .to_string();

    assert_eq!(error, "Reddit rejected the guest session");
    assert_eq!(auths.load(Ordering::Relaxed), 2);
}

#[test]
fn listing_search_and_thread_requests_preserve_encoded_parameters() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let fixture = Fixture::new(move |r| {
        log.lock().push(r.url.clone());
        if r.url.starts_with("/comments/") {
            Reply::json(
                serde_json::from_str(include_str!("../../tests/fixtures/post.json")).unwrap(),
            )
        } else {
            Reply::json(empty_listing())
        }
    });
    let reddit = Reddit::test_at(&fixture.base, true);
    reddit
        .listing("popular", Sort::Top, TimeRange::Year, Some("t3_next"))
        .unwrap();
    reddit.search("rust", "λ & space", Some("t3_last")).unwrap();
    reddit.post("/r/neovim/comments/1x4v4sn/pokémon/").unwrap();
    let urls: Vec<Url> = seen
        .lock()
        .iter()
        .map(|s| Url::parse(&fixture.base).unwrap().join(s).unwrap())
        .collect();
    assert_eq!(urls[0].path(), "/r/popular/top.json");
    let pairs: HashMap<_, _> = urls[0].query_pairs().collect();
    assert_eq!(pairs["t"], "year");
    assert_eq!(pairs["after"], "t3_next");
    assert_eq!(pairs["include_over_18"], "on");
    let pairs: HashMap<_, _> = urls[1].query_pairs().collect();
    assert_eq!(pairs["q"], "λ & space");
    assert_eq!(pairs["after"], "t3_last");
    assert_eq!(urls[2].path(), "/comments/1x4v4sn.json");
}

#[test]
fn health_follows_the_latest_response_and_down_outranks_a_cooldown() {
    for (status, health) in [
        (200, Health::Up),
        (401, Health::Down),
        (403, Health::Up),
        (404, Health::Up),
        (429, Health::Paused),
        (500, Health::Down),
    ] {
        // `test_at` signs in at the base: a 401 gets a new guest session, then 401 again.
        let fixture = Fixture::new(move |r| match status {
            _ if r.url == "/" => Reply::json(json!({"access_token": "guest", "expires_in": 3600})),
            200 => Reply::json(empty_listing()),
            _ => Reply::status(status),
        });
        let reddit = Reddit::test_at(&fixture.base, false);
        let _ = reddit.search("rust", "x", None);
        assert_eq!(reddit.health(), health, "HTTP {status}");
    }
    // Guest authentication refused, unusable or unanswered: Reddit is down, even while a
    // refusal pauses requests.
    let refused = Fixture::new(|_| Reply::status(403));
    let unusable = Fixture::new(|_| Reply::json(json!({})));
    for base in [&refused.base, &unusable.base, "http://127.0.0.1:9/"] {
        let reddit = Reddit::with_transport(Transport::new().unwrap(), false, base, base).unwrap();
        assert!(reddit.search("rust", "x", None).is_err());
        assert_eq!(reddit.health(), Health::Down, "{base}");
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
            200 => Reply::json(empty_listing()),
            _ => Reply::status(status),
        });
        let reddit = Reddit::test_at(&fixture.base, false);
        assert_eq!(reddit.check(), health, "HTTP {status}");
    }
}

#[test]
fn continuing_a_thread_does_not_wait_for_a_morechildren_request() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let fixture = Fixture::new(move |r| {
        log.lock().push(r.url);
        Reply::status(404)
    });
    let reddit = Reddit::test_at(&fixture.base, false);
    let in_flight = reddit.0.expansion.lock();
    let (done, finished) = std::sync::mpsc::channel();
    let worker = {
        let reddit = reddit.clone();
        std::thread::spawn(move || {
            let cursor = json!({"post": "abc", "parent": "t1_def", "children": []});
            let _ = done.send(reddit.more_replies(&cursor.to_string()).is_err());
        })
    };
    let answered = finished.recv_timeout(Duration::from_secs(5));
    drop(in_flight);
    worker.join().unwrap();
    assert_eq!(answered, Ok(true));
    assert!(seen.lock()[0].starts_with("/comments/abc.json?"));
}

#[test]
fn media_requests_are_not_page_loads() {
    // i.redd.it and v.redd.it send a browser page load to a reddit.com viewer page.
    let fixture = Fixture::new(|r| {
        let page_load = r
            .headers
            .get("accept")
            .is_some_and(|a| a.contains("text/html"))
            || r.headers.contains_key("sec-fetch-mode");
        if page_load {
            Reply {
                status: 302,
                body: String::new(),
                headers: vec![("Location".into(), "https://www.reddit.com/media".into())],
            }
        } else {
            Reply {
                status: 200,
                body: "pixels".into(),
                headers: vec![],
            }
        }
    });
    let reddit = Reddit::test_at(&fixture.base, false);
    assert_eq!(
        reddit.image(&format!("{}a.jpeg", fixture.base)).unwrap(),
        b"pixels"
    );
}

#[test]
fn a_sign_in_that_never_reached_reddit_does_not_pause_requests() {
    let nothing = "http://127.0.0.1:9/";
    let reddit =
        Reddit::with_transport(Transport::new().unwrap(), false, nothing, nothing).unwrap();
    assert!(reddit.search("rust", "x", None).is_err());
    assert_eq!(reddit.0.status.lock().cooldown, None);
    assert_eq!(reddit.health(), Health::Down);
}

#[test]
fn queued_requests_do_not_each_wait_out_an_unreachable_sign_in() {
    // Accepts and drops every connection: a sign-in that gets no answer.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let tries = Arc::new(AtomicUsize::new(0));
    let counted = tries.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            counted.fetch_add(1, Ordering::SeqCst);
            drop(stream);
        }
    });
    let reddit = Reddit::with_transport(Transport::new().unwrap(), false, &base, &base).unwrap();
    for _ in 0..3 {
        let error = reddit.search("rust", "x", None).unwrap_err().to_string();
        assert_eq!(error, "Reddit connection failed");
    }
    assert_eq!(tries.load(Ordering::SeqCst), 1);
    assert_eq!(reddit.health(), Health::Down);
}

#[test]
fn a_rate_limited_sign_in_waits_as_long_as_reddit_asks() {
    let fixture = Fixture::new(|_| Reply {
        status: 429,
        body: String::new(),
        headers: vec![("Retry-After".into(), "600".into())],
    });
    let base = &fixture.base;
    let reddit = Reddit::with_transport(Transport::new().unwrap(), false, base, base).unwrap();
    assert!(reddit.search("rust", "x", None).is_err());
    let until = reddit.0.status.lock().cooldown.unwrap();
    assert!(until > Instant::now() + Duration::from_secs(500));
    assert_eq!(reddit.health(), Health::Paused);
}

#[test]
fn relay_answers_502_when_the_upstream_fails() {
    let fixture = Fixture::new(|_| Reply::status(500));
    let reddit = Reddit::test_at(&fixture.base, false);
    let local = reddit
        .media_url(&format!("{}video.mp4", fixture.base))
        .unwrap();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .proxy(None)
        .timeout_global(Some(Duration::from_secs(5)))
        .http_status_as_error(false)
        .build()
        .into();
    assert_eq!(agent.get(&local).call().unwrap().status().as_u16(), 502);
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

#[test]
fn disallowed_media_hosts_and_protocols_are_refused() {
    for url in [
        "http://i.redd.it/a.gif",
        "https://evil.test/a.gif",
        "https://i.redd.it.evil.test/a.gif",
        "file:///tmp/a",
        "https://user:password@i.redd.it/a",
        "https://i.redd.it:99/a",
    ] {
        assert!(!transport::media_allowed(&Url::parse(url).unwrap()));
    }
}
