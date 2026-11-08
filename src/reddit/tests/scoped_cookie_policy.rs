use super::*;

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
    assert!(fixture.reddit().search("rust", "x", None).is_err());
    assert_eq!(requests.load(Ordering::Relaxed), 1);
}

#[test]
fn a_search_host_should_get_only_reddit_wide_cookies_and_its_401_should_not_sign_out() {
    let cookies = Arc::new(Mutex::new(Vec::new()));
    let seen = cookies.clone();
    let fixture = Fixture::new(move |request| {
        seen.lock().push(request.headers.get("cookie").cloned());
        if request.url.contains("q=refused") {
            return Reply {
                status: 401,
                body: String::new(),
                headers: vec![],
            };
        }
        Reply::json(empty_listing())
    });
    let mut reddit = Reddit::with_transport(
        Transport::new().unwrap(),
        false,
        "http://localhost:9/",
        session::Session::parse(
            ".reddit.com\tTRUE\t/\tTRUE\t0\treddit_session\tfixture-session\nwww.reddit.com\tFALSE\t/\tTRUE\t0\tprefs\twww-only\n",
            0,
        )
        .unwrap(),
    )
    .unwrap();
    let inner = Arc::get_mut(&mut reddit.0).unwrap();
    inner.classic = Url::parse(&fixture.base).unwrap();
    inner.old = Url::parse(&fixture.base).unwrap();

    reddit.search("rust", "async", None).unwrap();
    reddit
        .listing(
            "all",
            crate::model::Sort::Hot,
            crate::model::TimeRange::Day,
            None,
        )
        .unwrap();
    assert_eq!(
        cookies.lock().as_slice(),
        [
            Some("reddit_session=fixture-session".to_string()),
            Some("reddit_session=fixture-session".to_string())
        ]
    );

    let error = reddit.search("rust", "refused", None).unwrap_err();
    assert_eq!(error.to_string(), CLASSIC_REFUSED);
    assert!(!reddit.needs_sign_in());
    assert_ne!(reddit.health(), Health::Down);

    let mut www_only = Reddit::with_transport(
        Transport::new().unwrap(),
        false,
        "http://localhost:9/",
        session::Session::parse(
            "www.reddit.com\tFALSE\t/\tTRUE\t0\treddit_session\twww-session\n",
            0,
        )
        .unwrap(),
    )
    .unwrap();
    Arc::get_mut(&mut www_only.0).unwrap().classic = Url::parse(&fixture.base).unwrap();
    let requests = cookies.lock().len();
    let error = www_only.search("rust", "async", None).unwrap_err();
    assert_eq!(error.to_string(), CLASSIC_SESSION);
    assert_eq!(cookies.lock().len(), requests);
    assert!(!www_only.needs_sign_in());
    assert_ne!(www_only.health(), Health::Down);

    let mut expired = Reddit::with_transport(
        Transport::new().unwrap(),
        false,
        "http://localhost:9/",
        session::Session::parse(
            ".reddit.com\tTRUE\t/\tTRUE\t1\treddit_session\tfixture-session\n",
            0,
        )
        .unwrap(),
    )
    .unwrap();
    Arc::get_mut(&mut expired.0).unwrap().classic = Url::parse(&fixture.base).unwrap();
    let error = expired.search("rust", "async", None).unwrap_err();
    assert_eq!(error.to_string(), session::SIGN_IN);
}
