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
fn membership_changes_should_not_write_through_a_different_signed_in_account() {
    let writes = std::sync::Arc::new(AtomicUsize::new(0));
    let actual = writes.clone();
    let fixture = Fixture::new(move |request| {
        if request.url == "/api/me.json" {
            return Reply::json(json!({"data":{"name":"someoneelse","modhash":"fixture-csrf"}}));
        }
        assert_eq!(request.url, "/api/subscribe");
        actual.fetch_add(1, Ordering::SeqCst);
        Reply::json(json!({}))
    });
    let reddit = fixture.saved_reddit(&saved_export("reader", "reader"));

    for join in [true, false] {
        let result = reddit.set_subscription("rust", join);
        assert!(result.unwrap_err().to_string().contains("account changed"));
    }
    assert_eq!(writes.load(Ordering::SeqCst), 0);
}

#[test]
fn when_community_membership_changes_it_should_mutate_the_server_for_join_leave_and_rejoin() {
    let joined = Arc::new(AtomicBool::new(false));
    let actual = joined.clone();
    let fixture = Fixture::new(move |request| match request.url.as_str() {
        "/api/me.json" => {
            assert_eq!(request.method, "GET");
            assert!(!request.headers.contains_key("x-modhash"));
            Reply::json(json!({"kind":"t2","data":{"name":"READER","modhash":"fixture-csrf"}}))
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
    let reddit = fixture.saved_reddit(&saved_export("reader", "reader"));

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
fn when_nsfw_is_disabled_streamed_callbacks_should_only_publish_visible_posts() {
    let fixture = Fixture::new(|_| {
        let mut hidden = home_post("hidden", "s");
        hidden["data"]["over_18"] = json!(true);
        Reply::json(
            json!({"kind":"Listing","data":{"children":[hidden,home_post("safe","s")],"after":null}}),
        )
    });
    let mut published = Vec::new();

    let actual = fixture
        .reddit()
        .listing_progress("s", Sort::Hot, TimeRange::Day, None, &mut |event| {
            if let ListingEvent::Post(post) = event {
                published.push(post.id.clone());
            }
        })
        .unwrap();

    assert_eq!(published, ["safe"]);
    assert_eq!(
        actual
            .posts
            .iter()
            .map(|post| post.id.as_str())
            .collect::<Vec<_>>(),
        ["safe"]
    );
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
                Reply { status: 200, body: r#"<shreddit-post id="t3_recommended" recommendation-source="discovery"></shreddit-post><shreddit-ad-post id="t3_ad"></shreddit-ad-post><shreddit-post id="t3_joined"></shreddit-post><shreddit-post id="t3_gonea"></shreddit-post><shreddit-post id="t3_goneb"></shreddit-post><faceplate-partial id="feed-next-page-partial" src="/svc/shreddit/feeds/home-feed?sort=BEST&amp;after=t3_joined&amp;cursor=opaque%26next"></faceplate-partial>"#.into(), headers: vec![] }
            }
            "/by_id/t3_recommended,t3_joined,t3_gonea,t3_goneb.json" => Reply::json(
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
            .request(reddit.0.api.join(home::PATH).unwrap(), home::ACCEPT)
            .unwrap_err()
            .to_string()
            .contains("paused")
    );
    assert_eq!(requests.load(Ordering::Relaxed), 1);
    assert_eq!(reddit.health(), Health::Paused);
    assert!(reddit.pause().unwrap().seconds > 500);
}

#[test]
fn when_json_and_home_have_separate_budgets_home_should_not_wait_for_json() {
    let fixture = Fixture::new(|_| Reply {
        status: 200,
        body: empty_listing().to_string(),
        headers: vec![
            ("X-Ratelimit-Remaining".into(), "2".into()),
            ("X-Ratelimit-Reset".into(), "1.2".into()),
        ],
    });
    let reddit = fixture.reddit();
    reddit.search("rust", "first", None).unwrap();
    let url = reddit.0.api.join(home::PATH).unwrap();

    let started = Instant::now();
    reddit.request(url, home::ACCEPT).unwrap();

    assert!(
        started.elapsed() < Duration::from_millis(300),
        "Home inherited JSON's600ms quota interval: {:?}",
        started.elapsed()
    );
}

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
            ("X-Ratelimit-Reset".into(), "1.2".into()),
        ],
    });
    let reddit = fixture
        .reddit()
        .for_worker(Priority::Background, true, || false);
    reddit.search("rust", "first", None).unwrap();
    reddit
        .request(reddit.0.api.join(home::PATH).unwrap(), home::ACCEPT)
        .unwrap();

    let started = Instant::now();
    reddit.search("rust", "next", None).unwrap();

    assert!(
        started.elapsed() >= Duration::from_millis(450),
        "Home's budget erased JSON pacing: {:?}",
        started.elapsed()
    );
}

#[test]
fn when_a_response_takes_longer_than_base_pacing_the_next_request_should_not_wait_again() {
    let fixture = Fixture::new(|_| {
        std::thread::sleep(Duration::from_millis(600));
        Reply {
            status: 200,
            body: empty_listing().to_string(),
            headers: vec![
                ("X-Ratelimit-Remaining".into(), "100".into()),
                ("X-Ratelimit-Reset".into(), "1".into()),
            ],
        }
    });
    let mut reddit = fixture.reddit();
    Arc::get_mut(&mut reddit.0).unwrap().rate = rate::Rate::new(Duration::from_millis(500));
    reddit.search("rust", "first", None).unwrap();

    let (_, waited) = reddit
        .send_with_timeout(
            reddit.0.api.join("next.json").unwrap(),
            "application/json",
            Duration::from_secs(5),
            |client, url| client.get(url),
        )
        .unwrap();

    assert!(
        waited < Duration::from_millis(300),
        "pacing added after response: {waited:?}"
    );
}

#[test]
fn when_a_new_response_reports_recovered_quota_old_slow_pacing_should_not_stick() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let fixture = Fixture::new(move |_| Reply {
        status: 200,
        body: empty_listing().to_string(),
        headers: if count.fetch_add(1, Ordering::SeqCst) == 0 {
            vec![
                ("X-Ratelimit-Remaining".into(), "2".into()),
                ("X-Ratelimit-Reset".into(), "1.2".into()),
            ]
        } else {
            vec![
                ("X-Ratelimit-Remaining".into(), "100".into()),
                ("X-Ratelimit-Reset".into(), "1".into()),
            ]
        },
    });
    let reddit = fixture
        .reddit()
        .for_worker(Priority::Background, true, || false);
    reddit.search("rust", "first", None).unwrap();
    reddit.search("rust", "second", None).unwrap();

    let started = Instant::now();
    reddit.search("rust", "third", None).unwrap();

    assert!(
        started.elapsed() < Duration::from_millis(300),
        "recovered budget kept an old600ms interval: {:?}",
        started.elapsed()
    );
}

#[test]
fn when_startup_reports_quota_initial_navigation_should_use_reserved_capacity() {
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
    let visible = reddit.for_worker(Priority::Listing, true, || false);
    let started = Instant::now();

    visible
        .listing("rust", Sort::Hot, TimeRange::Day, None)
        .unwrap();

    assert!(
        started.elapsed() < Duration::from_millis(500),
        "initial navigation inherited the full three-second quota interval: {:?}",
        started.elapsed()
    );
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

    let actual = visible.search("rust", "fourth", None);

    assert_eq!(actual.unwrap_err().to_string(), REQUEST_CANCELLED);
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
    assert!(pause.cause.contains("quota"));
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

#[test]
fn when_a_safe_read_becomes_obsolete_it_should_release_header_and_body_waits() {
    use std::io::{BufRead, Write};
    use std::sync::mpsc::channel;
    for media in [false, true] {
        for body in [false, true] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}/", listener.local_addr().unwrap());
            let reddit = Reddit::test_at(&base, false);
            let obsolete = Arc::new(AtomicBool::new(false));
            let cancelled = obsolete.clone();
            let scoped = reddit.for_worker(Priority::Foreground, true, move || {
                cancelled.load(Ordering::Acquire)
            });
            let (started_tx, started_rx) = channel();
            let (release_tx, release_rx) = channel();
            let server = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut reader = std::io::BufReader::new(socket.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    assert!(reader.read_line(&mut line).unwrap() > 0);
                    if line == "\r\n" {
                        break;
                    }
                }
                if body {
                    socket
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n")
                        .unwrap();
                    socket.flush().unwrap();
                }
                started_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
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
    assert!(pause.cause.contains("r/rust listing") && pause.cause.contains("HTTP 429"));
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

    assert!(scoped.set_subscription("rust", true).is_ok());
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
            .set_subscription("rust", true)
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
        assert!(r.url.starts_with("/comments/abc.json?"));
        Reply::json(json!([
            {"kind":"Listing","data":{"children":[{"kind":"t3","data":{"id":"abc","title":"post","subreddit":"rust","permalink":"/comments/abc/"}}]}},
            {"kind":"Listing","data":{"children":[{"kind":"t1","data":{"id":"def","parent_id":"t3_abc","body":"expanded reply"}}]}}
        ]))
    });
    let actual = fixture
        .reddit()
        .more_replies(&json!({"post":"abc","parent":"t3_abc","children":["def"]}).to_string())
        .unwrap();
    assert_eq!(actual.comments[0].body, "expanded reply");
}

#[test]
fn when_the_morechildren_api_is_denied_expansion_should_read_focused_comments() {
    let fixture = Fixture::new(|request| {
        if request.url.starts_with("/api/morechildren") {
            return Reply::status(403);
        }
        assert_eq!(request.headers["cookie"], "reddit_session=fixture-session");
        let url = Url::parse("http://localhost")
            .unwrap()
            .join(&request.url)
            .unwrap();
        assert_eq!(url.path(), "/comments/1x4v4sn.json");
        let query: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(query["comment"], "c3");
        assert_eq!(query["sort"], "new");
        let mut value: Value =
            serde_json::from_str(include_str!("../../tests/fixtures/post.json")).unwrap();
        value[1]["data"]["children"] = json!([{"kind":"t1","data":{
            "id":"c3","name":"t1_c3","parent_id":"t3_1x4v4sn","link_id":"t3_1x4v4sn",
            "body":"expanded without the denied API","replies":""
        }}]);
        Reply::json(value)
    });
    let cursor = json!({"post":"1x4v4sn","parent":"t3_1x4v4sn","children":["c3"],"sort":"new"});

    let actual = fixture.reddit().more_replies(&cursor.to_string()).unwrap();

    assert_eq!(actual.comments[0].id, "c3");
    assert_eq!(actual.comments[0].body, "expanded without the denied API");
    assert_eq!(actual.refreshed, Some(vec!["c3".into()]));
    assert!(actual.more.is_none());
}

#[test]
fn private_community_explicit_reason_should_survive_the_denied_listing() {
    let fixture = Fixture::new(|_| Reply {
        status: 403,
        body: json!({"error":403,"message":"Forbidden","reason":"private"}).to_string(),
        headers: vec![],
    });
    let reddit = fixture.reddit();

    let actual = reddit
        .listing("doit", Sort::Hot, TimeRange::Day, None)
        .unwrap_err();

    assert!(actual.to_string().contains("private"), "{actual}");
    assert_eq!(reddit.health(), Health::Up);
    assert!(!reddit.needs_sign_in());
}

#[test]
fn private_community_unknown_reason_should_remain_a_generic_denial() {
    let fixture = Fixture::new(|_| Reply {
        status: 403,
        body: json!({"reason":"quarantined","message":"private SECRET"}).to_string(),
        headers: vec![],
    });
    let actual = fixture
        .reddit()
        .listing("doit", Sort::Hot, TimeRange::Day, None)
        .unwrap_err();
    assert_eq!(
        actual.downcast_ref::<AccessDenied>(),
        Some(&AccessDenied::Forbidden)
    );
    assert!(!actual.to_string().contains("SECRET"));
    assert!(!actual.to_string().contains("private"));
}

#[test]
fn private_community_malformed_body_should_remain_a_generic_denial() {
    let fixture = Fixture::new(|_| Reply {
        status: 403,
        body: "<html>private SECRET</html>".into(),
        headers: vec![],
    });
    let actual = fixture.reddit().search("doit", "query", None).unwrap_err();
    assert_eq!(
        actual.downcast_ref::<AccessDenied>(),
        Some(&AccessDenied::Forbidden)
    );
    assert!(!actual.to_string().contains("SECRET"));
}

#[test]
fn private_community_oversized_body_should_stay_bounded_and_redacted() {
    let fixture = Fixture::new(|_| Reply {
        status: 403,
        body: json!({"reason":"private","message":"SECRET".repeat(12_000)}).to_string(),
        headers: vec![],
    });
    let reddit = fixture.reddit();
    let actual = reddit
        .listing("doit", Sort::Hot, TimeRange::Day, None)
        .unwrap_err();
    assert_eq!(
        actual.downcast_ref::<AccessDenied>(),
        Some(&AccessDenied::Forbidden)
    );
    assert!(!actual.to_string().contains("SECRET"));
    assert_eq!(reddit.health(), Health::Up);
}

#[test]
fn private_community_reason_in_a_server_error_should_preserve_service_failure() {
    let fixture = Fixture::new(|_| Reply {
        status: 500,
        body: json!({"reason":"private"}).to_string(),
        headers: vec![],
    });
    let reddit = fixture.reddit();
    let actual = reddit
        .listing("doit", Sort::Hot, TimeRange::Day, None)
        .unwrap_err();
    assert!(actual.downcast_ref::<AccessDenied>().is_none());
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
            Reply { status: 200, body: "<shreddit-post id='t3_hidden'></shreddit-post><shreddit-post id='t3_gone'></shreddit-post><shreddit-post id='t3_gonea'></shreddit-post><shreddit-post id='t3_goneb'></shreddit-post><faceplate-partial id='feed-next-page-partial' src='/svc/shreddit/feeds/home-feed?cursor=more'></faceplate-partial>".into(), headers: vec![] }
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
fn continuing_a_thread_should_request_its_focused_parent() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let fixture = Fixture::new(move |r| {
        log.lock().push(r.url);
        Reply::status(404)
    });
    let reddit = Reddit::test_at(&fixture.base, false);
    let cursor = json!({"post": "abc", "parent": "t1_def", "children": []});
    assert!(reddit.more_replies(&cursor.to_string()).is_err());
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
    assert_eq!(reddit.pause(), None);
    assert_eq!(reddit.health(), Health::Down);
}

#[test]
fn clip_relay_should_serve_manifests_while_four_streams_are_stalled() {
    let server = Arc::new(Server::http("127.0.0.1:0").unwrap());
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
    let server = Arc::new(Server::http("127.0.0.1:0").unwrap());
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

#[test]
fn card_media_converted_reddit_gif_should_keep_its_gif_label() {
    let actual = parse::parse_listing_value(json!({"kind":"Listing","data":{"children":[{"kind":"t3","data":{
        "id":"animated","title":"animation","subreddit":"s","permalink":"/comments/animated/",
        "secure_media":{"reddit_video":{"hls_url":"https://v.redd.it/clip/HLSPlaylist.m3u8","is_gif":true}}
    }}]}})).unwrap();
    assert_eq!(actual.posts[0].media.tag().as_deref(), Some("gif"));
}

#[test]
fn card_media_converted_preview_animation_should_keep_its_gif_label() {
    let actual = parse::parse_listing_value(json!({"kind":"Listing","data":{"children":[{"kind":"t3","data":{
        "id":"animated","title":"animation","subreddit":"s","permalink":"/comments/animated/",
        "preview":{"images":[{"variants":{"mp4":{"source":{"url":"https://i.redd.it/animation.mp4"}}}}]}
    }}]}})).unwrap();
    assert_eq!(actual.posts[0].media.tag().as_deref(), Some("gif"));
}

#[test]
fn settings_policy_clone_should_share_session_health_and_identity_but_isolate_nsfw_namespace() {
    let fixture = Fixture::new(|_| panic!("Policy rebinding must not use HTTP"));
    let original = fixture.saved_reddit(&saved_export("reader", "reader"));
    let adult = original.with_nsfw(true);
    assert_eq!(adult.session_fingerprint(), original.session_fingerprint());
    assert_eq!(adult.account(), original.account());
    assert_eq!(adult.source(), original.source());
    assert_ne!(adult.cache_namespace(), original.cache_namespace());
    assert!(original.cache_namespace().contains("nsfw=false"));
    assert!(adult.cache_namespace().contains("nsfw=true"));
    assert!(Arc::ptr_eq(&adult.0, &original.0));
}
