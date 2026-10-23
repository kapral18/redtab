use super::*;
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tiny_http::{Header, Response, Server};

pub(crate) fn saved_export(id: &str, name: &str) -> String {
    format!(
        "# redtab-connection {}\n.reddit.com\tTRUE\t/\tTRUE\t0\treddit_session\tfixture-session\n",
        json!({"account":{"id":id,"name":name},"source":{"Browser":{"browser":"firefox","profile":"work","label":"Firefox · Work"}}})
    )
}

#[test]
fn when_loading_a_saved_account_it_should_restore_its_identity_source_and_cache_namespace() {
    let fixture = Fixture::new(|_| panic!("Restoring metadata must not use HTTP"));
    let actual = fixture.saved_reddit(&saved_export("first", "reader"));
    assert_eq!(actual.account().unwrap().id, "first");
    assert_eq!(actual.saved_id().as_deref(), Some("first"));
    assert_eq!(actual.source().unwrap().label(), "Firefox · Work");
    assert!(actual.cache_namespace().contains("account:first"));
}

#[test]
fn when_a_saved_cookie_belongs_to_another_account_verification_should_reject_switching() {
    let fixture =
        Fixture::new(|_| Reply::json(json!({"data":{"id":"other", "name":"other_reader"}})));
    let saved = fixture.saved_reddit(&saved_export("first", "reader"));
    let actual = saved.verify_saved_account().unwrap_err().to_string();
    assert!(actual.contains("different account"));
    assert!(!actual.contains("fixture-session"));
}

#[test]
fn when_a_saved_account_is_verified_it_should_accept_the_same_id_with_an_updated_username() {
    let fixture =
        Fixture::new(|_| Reply::json(json!({"data":{"id":"first", "name":"new_reader"}})));
    let saved = fixture.saved_reddit(&saved_export("first", "reader"));
    saved.verify_saved_account().unwrap();
    assert_eq!(saved.account().unwrap().name, "new_reader");
}

#[test]
fn when_saving_fails_the_runtime_saved_key_should_not_claim_the_new_entry_was_saved() {
    let fixture = Fixture::new(|_| Reply::json(json!({"data":{"id":"first", "name":"reader"}})));
    let candidate = fixture.reddit();
    candidate.verify_account().unwrap();
    let before = candidate.saved_id();
    let actual = candidate.save_connection_with(None, |_| bail!("Keyring locked"));
    assert!(actual.is_err());
    assert_eq!(candidate.saved_id(), before);
}

#[test]
fn when_a_candidate_is_unverified_saving_should_not_touch_the_credential_store() {
    let reddit = Reddit::test_at("http://127.0.0.1:9", false);
    let actual =
        reddit.save_connection_with(None, |_| panic!("Unverified credentials must not be saved"));
    assert!(actual.is_err());
}

#[test]
fn when_reddit_confirms_an_account_it_should_be_saved_only_after_verification() {
    let fixture = Fixture::new(|request| {
        assert_eq!(request.url, "/api/me.json");
        Reply::json(json!({"data":{"id":"fixture", "name":"reader"}}))
    });
    let reddit = fixture.reddit();
    let actual = reddit.verify_account().unwrap();
    let mut stored = String::new();
    reddit
        .save_connection_with(None, |text| {
            stored = text.into();
            Ok(())
        })
        .unwrap();
    assert_eq!(
        actual,
        Account {
            id: "fixture".into(),
            name: "reader".into()
        }
    );
    assert!(stored.contains("# redtab-connection"));
}

#[test]
fn when_account_identity_contains_a_path_it_should_not_be_accepted_or_saved() {
    let fixture = Fixture::new(|_| Reply::json(json!({"data":{"id":"../other", "name":"reader"}})));
    let reddit = fixture.reddit();
    assert!(reddit.verify_account().is_err());
    assert!(
        reddit
            .save_connection_with(None, |_| panic!("Invalid identity must not be saved"))
            .is_err()
    );
}

#[test]
fn when_a_connection_is_retired_queued_account_writes_should_not_reach_reddit() {
    let fixture = Fixture::new(|_| panic!("Retired connection must not issue HTTP"));
    let reddit = fixture.reddit();
    reddit.retire();
    assert!(
        reddit
            .set_subscription("neovim", true)
            .unwrap_err()
            .to_string()
            .contains("replaced")
    );
}

#[test]
fn when_an_account_is_verified_a_cookie_renewal_should_keep_its_account_cache_namespace() {
    let fixture = Fixture::new(|_| Reply::json(json!({"data":{"id":"fixture", "name":"reader"}})));
    let first = fixture.reddit();
    let second = fixture.reddit();
    first.verify_account().unwrap();
    second.verify_account().unwrap();
    assert_eq!(first.cache_namespace(), second.cache_namespace());
    assert!(first.cache_namespace().contains("account:fixture"));
}

pub(crate) struct Request {
    pub(crate) url: String,
    pub(crate) method: String,
    pub(crate) body: String,
    pub(crate) headers: HashMap<String, String>,
}

pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) body: String,
    pub(crate) headers: Vec<(String, String)>,
}

impl Reply {
    pub(crate) fn json(body: Value) -> Self {
        Self {
            status: 200,
            body: body.to_string(),
            headers: vec![],
        }
    }
    pub(crate) fn status(status: u16) -> Self {
        Self {
            status,
            body: "body must never appear in errors".into(),
            headers: vec![],
        }
    }
}

pub(crate) struct Fixture {
    pub(crate) base: String,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Fixture {
    pub(crate) fn new(handler: impl Fn(Request) -> Reply + Send + 'static) -> Self {
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
                        method: request.method().as_str().to_owned(),
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
    pub(crate) fn reddit(&self) -> Reddit {
        Reddit::test_at(&self.base, false)
    }
    pub(crate) fn saved_reddit(&self, text: &str) -> Reddit {
        Reddit::with_transport(
            Transport::new().unwrap(),
            false,
            &self.base,
            session::Session::stored(text, session::now()).unwrap(),
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
fn when_community_membership_changes_it_should_mutate_the_server_for_join_leave_and_rejoin() {
    let joined = Arc::new(AtomicBool::new(false));
    let actual = joined.clone();
    let fixture = Fixture::new(move |request| match request.url.as_str() {
        "/api/me.json" => {
            assert_eq!(request.method, "GET");
            assert!(!request.headers.contains_key("x-modhash"));
            Reply::json(json!({"kind":"t2","data":{"name":"reader","modhash":"fixture-csrf"}}))
        }
        "/api/subscribe" => {
            assert_eq!(request.method, "POST");
            assert_eq!(request.headers["x-modhash"], "fixture-csrf");
            let form: HashMap<_, _> = url::form_urlencoded::parse(request.body.as_bytes())
                .into_owned()
                .collect();
            assert_eq!(form["sr_name"], "armenia");
            assert_eq!(form["api_type"], "json");
            let join = form["action"] == "sub";
            assert_eq!(
                form.get("skip_initial_defaults").map(String::as_str),
                join.then_some("true")
            );
            let was_joined = actual.swap(join, Ordering::SeqCst);
            if !join && !was_joined {
                return Reply::status(404);
            }
            Reply::json(json!({}))
        }
        _ => panic!("unexpected membership request"),
    });
    let reddit = fixture.reddit();

    reddit.set_subscription("armenia", true).unwrap();
    assert!(joined.load(Ordering::SeqCst));
    reddit.set_subscription("armenia", true).unwrap();
    assert!(joined.load(Ordering::SeqCst));
    reddit.set_subscription("armenia", false).unwrap();
    assert!(!joined.load(Ordering::SeqCst));
    assert!(reddit.set_subscription("armenia", false).is_err());
    reddit.set_subscription("armenia", true).unwrap();
    assert!(joined.load(Ordering::SeqCst));
}

#[test]
fn when_membership_identity_is_incomplete_it_should_never_post() {
    for data in [
        json!({}),
        json!({"name":"reader"}),
        json!({"modhash":"fixture-secret"}),
        json!({"name":"reader","modhash":""}),
        json!({"name":"reader","modhash":"fixture-secret\ninvalid"}),
    ] {
        let fixture = Fixture::new(move |request| {
            assert_eq!(request.method, "GET");
            assert_eq!(request.url, "/api/me.json");
            Reply::json(json!({"data":data}))
        });
        let error = fixture
            .reddit()
            .set_subscription("armenia", true)
            .unwrap_err();
        assert!(!format!("{error:#}").contains("fixture-secret"));
    }
}

#[test]
fn when_membership_target_is_not_a_community_it_should_not_request_anything() {
    let fixture = Fixture::new(|_| panic!("invalid membership target reached HTTP"));
    for name in [
        "",
        "../armenia",
        "r/armenia",
        "a+b",
        "all",
        "POPULAR",
        "Հայաստան",
    ] {
        assert!(fixture.reddit().set_subscription(name, true).is_err());
    }
}

#[test]
fn when_membership_has_no_success_acknowledgement_it_should_fail_without_echoing_the_body() {
    for (body, success) in [
        ("{}", true),
        (r#"{"json":{"errors":[]}}"#, true),
        (
            r#"{"json":{"errors":[["fixture-secret","denied"]]}}"#,
            false,
        ),
        (r#"{"error":"fixture-secret"}"#, false),
        (r#"{"json":{"errors":"fixture-secret"}}"#, false),
        ("null", false),
        ("[]", false),
        ("fixture-secret", false),
    ] {
        let posts = Arc::new(AtomicUsize::new(0));
        let count = posts.clone();
        let fixture = Fixture::new(move |request| {
            if request.url == "/api/me.json" {
                Reply::json(json!({"data":{"name":"reader","modhash":"fixture-csrf"}}))
            } else {
                assert_eq!(request.url, "/api/subscribe");
                count.fetch_add(1, Ordering::SeqCst);
                Reply {
                    status: 200,
                    body: body.into(),
                    headers: vec![],
                }
            }
        });
        let result = fixture.reddit().set_subscription("armenia", true);
        assert_eq!(result.is_ok(), success, "response {body}");
        if let Err(error) = result {
            assert!(!format!("{error:#}").contains("fixture-secret"));
        }
        assert_eq!(posts.load(Ordering::SeqCst), 1, "writes must never retry");
    }
}

#[test]
fn when_membership_http_fails_it_should_not_redirect_retry_or_echo_the_body() {
    for status in [401, 403, 404, 429, 302, 500] {
        let posts = Arc::new(AtomicUsize::new(0));
        let count = posts.clone();
        let fixture = Fixture::new(move |request| {
            if request.url == "/api/me.json" {
                Reply::json(json!({"data":{"name":"reader","modhash":"fixture-csrf"}}))
            } else {
                assert_eq!(request.url, "/api/subscribe");
                count.fetch_add(1, Ordering::SeqCst);
                Reply {
                    status,
                    body: "fixture-secret".into(),
                    headers: vec![("Location".into(), "https://other.invalid/".into())],
                }
            }
        });
        let error = fixture
            .reddit()
            .set_subscription("armenia", true)
            .unwrap_err();
        assert!(!format!("{error:#}").contains("fixture-secret"));
        assert_eq!(posts.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn when_subscriptions_span_pages_they_should_all_load_without_duplicates() {
    let fixture = Fixture::new(|request| {
        assert!(request.headers.contains_key("cookie"));
        let url = Url::parse(&format!("http://fixture{}", request.url)).unwrap();
        assert_eq!(url.path(), "/subreddits/mine/subscriber.json");
        let query: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(query["limit"], "100");
        if query.contains_key("after") {
            assert_eq!(query["after"], "t5_cursor&next");
            Reply::json(json!({"kind":"Listing","data":{"children":[
                {"kind":"t5","data":{"display_name":"RUST"}},
                {"kind":"t5","data":{"display_name":"neovim"}}
            ],"after":null}}))
        } else {
            Reply::json(json!({"kind":"Listing","data":{"children":[
                {"kind":"t5","data":{"display_name":"rust"}}
            ],"after":"t5_cursor&next"}}))
        }
    });

    assert_eq!(
        fixture.reddit().subscriptions().unwrap(),
        ["neovim", "rust"]
    );
}

#[test]
fn when_subscription_pagination_repeats_a_cursor_it_should_fail() {
    let fixture = Fixture::new(|_| {
        Reply::json(json!({
            "kind":"Listing","data":{"children":[],"after":"t5_loop"}
        }))
    });

    assert!(
        fixture
            .reddit()
            .subscriptions()
            .unwrap_err()
            .to_string()
            .contains("cursor")
    );
}

fn home_post(id: &str, subreddit: &str) -> Value {
    json!({"kind":"t3","data":{"id":id,"title":id,"subreddit":subreddit,"permalink":format!("/r/{subreddit}/comments/{id}/title/"),"author":"reader"}})
}

#[test]
fn when_best_home_loads_it_should_preserve_browser_recommendations_and_order() {
    let fixture = Fixture::new(|request| {
        let url = Url::parse(&format!("http://fixture{}", request.url)).unwrap();
        assert!(request.headers.contains_key("cookie"));
        match url.path() {
            "/svc/shreddit/feeds/home-feed" => {
                assert_eq!(request.headers["accept"], "text/vnd.reddit.partial+html");
                assert!(request.headers.contains_key("referer"));
                assert!(
                    url.query_pairs()
                        .any(|(key, value)| key == "sort" && value == "BEST")
                );
                Reply { status: 200, body: r#"<shreddit-post id="t3_recommended" recommendation-source="discovery"></shreddit-post><shreddit-ad-post id="t3_ad"></shreddit-ad-post><shreddit-post id="t3_joined"></shreddit-post><faceplate-partial id="feed-next-page-partial" src="/svc/shreddit/feeds/home-feed?sort=BEST&amp;after=t3_joined&amp;cursor=opaque%26next"></faceplate-partial>"#.into(), headers: vec![] }
            }
            "/by_id/t3_recommended,t3_joined.json" => Reply::json(
                json!({"kind":"Listing","data":{"children":[home_post("joined","rust"),home_post("recommended","unjoined")],"after":null}}),
            ),
            _ => panic!("unexpected Home request: {}", url.path()),
        }
    });

    let listing = fixture
        .reddit()
        .home(Sort::Best, TimeRange::Day, None)
        .unwrap();
    assert_eq!(
        listing
            .posts
            .iter()
            .map(|p| (p.id.as_str(), p.subreddit.as_str()))
            .collect::<Vec<_>>(),
        [("recommended", "unjoined"), ("joined", "rust")]
    );
    let after = Url::parse(listing.after.as_ref().unwrap()).unwrap();
    assert_eq!(after.path(), "/svc/shreddit/feeds/home-feed");
    assert!(
        after
            .query_pairs()
            .any(|(key, value)| key == "cursor" && value == "opaque&next")
    );
}

#[test]
fn when_the_account_has_no_subscriptions_it_should_return_an_empty_list() {
    let fixture = Fixture::new(|_| Reply::json(empty_listing()));
    assert!(fixture.reddit().subscriptions().unwrap().is_empty());
}

#[test]
fn when_a_later_subscription_page_fails_it_should_not_return_a_partial_list() {
    let fixture = Fixture::new(|request| {
        if request.url.contains("after=") {
            Reply::status(403)
        } else {
            Reply::json(json!({"kind":"Listing","data":{"children":[
                {"kind":"t5","data":{"display_name":"rust"}}
            ],"after":"t5_next"}}))
        }
    });
    assert!(fixture.reddit().subscriptions().is_err());
}

#[test]
fn when_subscription_json_is_malformed_it_should_fail_instead_of_clearing_membership() {
    let invalid = [
        json!({}),
        json!({"kind":"Listing","data":{"children":[{"kind":"t3","data":{"display_name":"rust"}}]}}),
        json!({"kind":"Listing","data":{"children":[{"kind":"t5","data":{}}]}}),
        json!({"kind":"Listing","data":{"children":[{"kind":"t5","data":{"display_name":"bad/name"}}]}}),
        json!({"kind":"Listing","data":{"children":[],"after":7}}),
    ];
    for value in invalid {
        let fixture = Fixture::new(move |_| Reply::json(value.clone()));
        assert!(fixture.reddit().subscriptions().is_err());
    }
}

#[test]
fn when_home_is_sorted_by_top_it_should_send_the_time_range_at_the_root() {
    let fixture = Fixture::new(|request| {
        let url = Url::parse(&format!("http://fixture{}", request.url)).unwrap();
        assert_eq!(url.path(), "/top.json");
        assert!(
            url.query_pairs()
                .any(|(key, value)| key == "t" && value == "week")
        );
        Reply::json(empty_listing())
    });
    assert!(
        fixture
            .reddit()
            .home(Sort::Top, TimeRange::Week, None)
            .unwrap()
            .posts
            .is_empty()
    );
}

#[test]
fn when_signed_in_a_feed_should_use_the_exported_session_without_oauth() {
    let fixture = Fixture::new(|r| {
        if r.headers.get("cookie").map(String::as_str) == Some("reddit_session=fixture-session")
            && !r.headers.contains_key("authorization")
            && r.url.starts_with("/r/rust/hot.json?")
        {
            Reply::json(empty_listing())
        } else {
            Reply::status(403)
        }
    });

    let actual = fixture
        .reddit()
        .listing("rust", Sort::Hot, TimeRange::Day, None);

    assert!(actual.unwrap().posts.is_empty());
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
    assert_eq!(requests.load(Ordering::Relaxed), 1);
    assert_eq!(reddit.health(), Health::Paused);
    assert!(reddit.0.status.lock().cooldown.unwrap() > Instant::now() + Duration::from_secs(500));
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

#[test]
fn when_reddit_redirects_cookies_should_not_follow() {
    let requests = Arc::new(AtomicUsize::new(0));
    let calls = requests.clone();
    let fixture = Fixture::new(move |_| {
        calls.fetch_add(1, Ordering::Relaxed);
        Reply {
            status: 302,
            body: String::new(),
            headers: vec![("Location".into(), "/login".into())],
        }
    });
    let error = fixture
        .reddit()
        .search("rust", "x", None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("redirected"));
    assert!(error.contains("redtab --signin"));
    assert_eq!(requests.load(Ordering::Relaxed), 1);
}

#[test]
fn when_an_api_url_changes_origin_cookies_should_not_be_sent() {
    let reddit = Reddit::test_at("http://127.0.0.1:9/", false);
    let error = reddit
        .get(Url::parse("https://example.invalid/r/rust/hot.json").unwrap())
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "refusing to send Reddit cookies to another origin"
    );
}

#[test]
fn when_accounts_change_their_cache_namespaces_should_differ_without_containing_cookies() {
    let other = session::Session::parse(
        ".reddit.com\tTRUE\t/\tTRUE\t0\treddit_session\tother-session\n",
        0,
    )
    .unwrap();
    let reddit = Reddit::test_at("http://127.0.0.1:9/", false);
    let other = Reddit::with_transport(
        Transport::new().unwrap(),
        false,
        "http://127.0.0.1:9/",
        other,
    )
    .unwrap();
    let namespace = reddit.cache_namespace();
    assert!(!namespace.contains("fixture-session"));
    assert!(!other.cache_namespace().contains("other-session"));
    assert_ne!(namespace, other.cache_namespace());
    assert_eq!(
        namespace,
        Reddit::test_at("http://127.0.0.1:9/", false).cache_namespace()
    );
}

#[test]
fn listing_search_and_thread_requests_preserve_encoded_parameters() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let fixture = Fixture::new(move |r| {
        assert_eq!(
            r.headers.get("cookie").map(String::as_str),
            Some("reddit_session=fixture-session")
        );
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
fn when_expanding_replies_the_client_should_use_the_same_signed_in_session() {
    let fixture = Fixture::new(|r| {
        assert_eq!(
            r.headers.get("cookie").map(String::as_str),
            Some("reddit_session=fixture-session")
        );
        assert!(r.url.starts_with("/api/morechildren.json?"));
        Reply::json(
            json!({"json":{"errors":[],"data":{"things":[{"kind":"t1","data":{"id":"def","parent_id":"t3_abc","body":"expanded reply"}}]}}}),
        )
    });
    let actual = fixture
        .reddit()
        .more_replies(&json!({"post":"abc","parent":"t3_abc","children":["def"]}).to_string())
        .unwrap();
    assert_eq!(actual.comments[0].body, "expanded reply");
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
fn when_browser_home_pages_it_should_keep_the_entire_opaque_cursor() {
    let fixture = Fixture::new(|request| {
        let url = Url::parse(&format!("http://fixture{}", request.url)).unwrap();
        if url.path() == "/svc/shreddit/feeds/home-feed" {
            let query: HashMap<_, _> = url.query_pairs().into_owned().collect();
            assert_eq!(query["cursor"], "opaque&next");
            assert_eq!(query["distance"], "25");
            Reply {
                status: 200,
                body: "<shreddit-post id='t3_next'></shreddit-post>".into(),
                headers: vec![],
            }
        } else {
            assert_eq!(url.path(), "/by_id/t3_next.json");
            Reply::json(
                json!({"kind":"Listing","data":{"children":[home_post("next","unjoined")],"after":null}}),
            )
        }
    });
    let cursor = format!(
        "{}svc/shreddit/feeds/home-feed?sort=BEST&cursor=opaque%26next&distance=25",
        fixture.base
    );

    let listing = fixture
        .reddit()
        .home(Sort::Best, TimeRange::Day, Some(&cursor))
        .unwrap();

    assert_eq!(listing.posts[0].id, "next");
    assert!(listing.after.is_none());
}

#[test]
fn when_home_post_details_hide_nsfw_or_disappear_it_should_keep_the_browser_cursor() {
    let fixture = Fixture::new(|request| {
        if request.url.starts_with("/svc/shreddit/feeds/home-feed") {
            Reply { status: 200, body: "<shreddit-post id='t3_hidden'></shreddit-post><shreddit-post id='t3_gone'></shreddit-post><faceplate-partial id='feed-next-page-partial' src='/svc/shreddit/feeds/home-feed?cursor=more'></faceplate-partial>".into(), headers: vec![] }
        } else {
            let mut hidden = home_post("hidden", "unjoined");
            hidden["data"]["over_18"] = json!(true);
            Reply::json(json!({"kind":"Listing","data":{"children":[hidden],"after":null}}))
        }
    });

    let listing = fixture
        .reddit()
        .home(Sort::Best, TimeRange::Day, None)
        .unwrap();

    assert!(listing.posts.is_empty());
    assert!(listing.after.unwrap().ends_with("?cursor=more"));
}

#[test]
fn when_reddit_returns_nonlisting_json_check_should_not_report_up() {
    let fixture = Fixture::new(|_| Reply::json(json!({})));
    assert_eq!(fixture.reddit().check(), Health::Down);
}

#[test]
fn a_first_page_spends_one_request_and_the_icon_is_its_own() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let fixture = Fixture::new(move |r| {
        log.lock().push(r.url.clone());
        if r.url.starts_with("/r/rust/about.json") {
            Reply::json(
                json!({"data": {"community_icon": "https://styles.redditmedia.com/i.png", "icon_img": ""}}),
            )
        } else {
            Reply::json(empty_listing())
        }
    });
    let reddit = Reddit::test_at(&fixture.base, false);
    reddit
        .listing("rust", Sort::Hot, TimeRange::Day, None)
        .unwrap();
    let urls = seen.lock().clone();
    assert_eq!(urls.len(), 1, "{urls:?}");
    assert_eq!(
        reddit.icon("rust").unwrap().as_deref(),
        Some("https://styles.redditmedia.com/i.png")
    );
}

#[test]
fn a_missing_icon_is_an_answer_and_a_refused_or_failed_one_is_not() {
    let fixture = Fixture::new(|r| match r.url.split('/').nth(2) {
        Some("gone") => Reply::status(404),
        Some("blocked") => Reply::status(403),
        _ => Reply::status(500),
    });
    let reddit = Reddit::test_at(&fixture.base, false);
    assert_eq!(reddit.icon("gone").unwrap(), None);
    assert!(reddit.icon("blocked").is_err());
    assert!(reddit.icon("broken").is_err());
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
fn when_reddit_is_unreachable_requests_should_fail_without_a_rate_limit() {
    let reddit = Reddit::test_at("http://127.0.0.1:9/", false);
    assert_eq!(
        reddit.search("rust", "x", None).unwrap_err().to_string(),
        "Reddit connection failed"
    );
    assert_eq!(reddit.0.status.lock().cooldown, None);
    assert_eq!(reddit.health(), Health::Down);
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
