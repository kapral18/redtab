use super::*;

#[test]
fn searches_and_community_listings_should_use_scoped_hosts_and_refuse_other_origins() {
    let reddit = Reddit::with_transport(
        Transport::new().unwrap(),
        false,
        API,
        session::Session::parse(
            ".reddit.com\tTRUE\t/\tTRUE\t0\treddit_session\tfixture-session\n",
            0,
        )
        .unwrap(),
    )
    .unwrap();
    let url = reddit
        .search_url(
            "rust",
            "async",
            experience::SearchSort::Top,
            crate::model::TimeRange::Week,
            None,
        )
        .unwrap();
    let params: HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(
        (params["sort"].as_str(), params["t"].as_str()),
        ("top", "week")
    );
    assert_eq!(
        (url.host_str(), url.path()),
        (Some("api.reddit.com"), "/r/rust/search")
    );
    let communities = reddit
        .read_url(&experience::Query::Communities, None)
        .unwrap();
    assert_eq!(communities.host_str(), Some("www.reddit.com"));
    let all = reddit.listing_url("all", crate::model::Sort::Hot).unwrap();
    assert_eq!(
        (all.host_str(), all.path()),
        (Some("old.reddit.com"), "/r/all.json")
    );
    for refused in [crate::model::Sort::New, crate::model::Sort::Rising] {
        assert!(reddit.listing_url("all", refused).is_err(), "{refused:?}");
    }
    let rust = reddit.listing_url("rust", crate::model::Sort::Hot).unwrap();
    assert_eq!(
        (rust.host_str(), rust.path()),
        (Some("api.reddit.com"), "/r/rust/hot")
    );

    let error = reddit
        .request(
            Url::parse("https://new.reddit.com/search").unwrap(),
            "application/json",
        )
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "refusing to send Reddit cookies to another origin"
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
                serde_json::from_str(include_str!("../../../tests/fixtures/post.json")).unwrap(),
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
    assert_eq!(urls[0].path(), "/r/popular/top");
    let pairs: HashMap<_, _> = urls[0].query_pairs().collect();
    assert_eq!(pairs["t"], "year");
    assert_eq!(pairs["after"], "t3_next");
    assert_eq!(pairs["include_over_18"], "on");
    let pairs: HashMap<_, _> = urls[1].query_pairs().collect();
    assert_eq!(pairs["q"], "λ & space");
    assert_eq!(pairs["after"], "t3_last");
    assert_eq!(urls[2].path(), "/comments/1x4v4sn.json");
}
