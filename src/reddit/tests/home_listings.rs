use super::*;

#[test]
fn when_home_lookups_are_denied_it_should_use_complete_native_cards() {
    let fixture = Fixture::new(|request| {
        if request.url.starts_with("/svc/shreddit/feeds/home-feed") {
            Reply {
                status: 200,
                body: r#"<shreddit-post id="t3_native" post-title="Native &amp; complete" subreddit-name="unjoined" author="reader" score="42" comment-count="7" created-timestamp="2026-10-27T00:00:00Z" permalink="/r/unjoined/comments/native/title/" post-type="image" content-href="https://i.redd.it/original.jpg" spoiler><div slot="post-media-container"><img data-post-media-primary src="https://preview.redd.it/native.jpg?width=640" width="640" height="480" srcset="https://preview.redd.it/native.jpg?width=1080 1080w"></div></shreddit-post><faceplate-partial id="feed-next-page-partial"></faceplate-partial>"#.into(),
                headers: vec![],
            }
        } else {
            Reply::status(403)
        }
    });

    let listing = fixture
        .reddit()
        .home(Sort::Best, TimeRange::Day, None)
        .unwrap();

    let post = &listing.posts[0];
    assert_eq!(post.id, "native");
    assert_eq!(post.title, "Native & complete");
    assert_eq!(post.subreddit, "unjoined");
    assert_eq!((post.score, post.comments), (Some(42), 7));
    assert!(post.spoiler && !post.nsfw);
    assert_eq!(post.preview_size, Some((1080, 810)));
    assert_eq!(
        post.media,
        crate::model::Media::Image {
            url: "https://i.redd.it/original.jpg".into(),
            preview: Some("https://preview.redd.it/native.jpg?width=1080".into()),
        }
    );
    assert!(listing.after.is_none());
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
                Reply {
                    status: 200,
                    body: format!(
                        "{}<shreddit-ad-post id='t3_ad'></shreddit-ad-post>{}<faceplate-partial id='feed-next-page-partial' src='/svc/shreddit/feeds/home-feed?sort=BEST&amp;after=t3_joined&amp;cursor=opaque%26next'></faceplate-partial>",
                        native_post("recommended", "unjoined"),
                        native_post("joined", "rust")
                    ),
                    headers: vec![],
                }
            }
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
fn when_browser_home_pages_it_should_keep_the_entire_opaque_cursor() {
    let fixture = Fixture::new(|request| {
        let url = Url::parse(&format!("http://fixture{}", request.url)).unwrap();
        assert_eq!(url.path(), "/svc/shreddit/feeds/home-feed");
        let query: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(query["cursor"], "opaque&next");
        assert_eq!(query["distance"], "25");
        Reply {
            status: 200,
            body: native_post("next", "unjoined"),
            headers: vec![],
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
fn when_home_filters_nsfw_it_should_keep_the_browser_cursor() {
    let fixture = Fixture::new(|request| {
        assert!(request.url.starts_with("/svc/shreddit/feeds/home-feed"));
        Reply {
            status: 200,
            body:
                format!(
                    "{}<faceplate-partial id='feed-next-page-partial' src='/svc/shreddit/feeds/home-feed?cursor=more'></faceplate-partial>",
                    ["hidden", "hiddena", "hiddenb", "hiddenc"]
                        .map(|id| native_post(id, "unjoined")
                            .replace(" post-type=", " nsfw post-type="))
                        .join("")
                ),
            headers: vec![],
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
fn when_a_short_best_home_continues_its_posts_should_arrive_before_the_continuation_ends() {
    use std::io::Write;
    let served = local_http_server();
    let base = format!("http://{}/", served.server_addr());
    let (shown, shown_rx) = std::sync::mpsc::channel::<()>();
    let server = std::thread::spawn(move || {
        for continuation in [false, true] {
            let request = served
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap();
            let mut stream = request.stream(None);
            if continuation {
                stream
                    .write_all(native_post("second", "rust").as_bytes())
                    .unwrap();
                let streamed = shown_rx.recv_timeout(Duration::from_secs(5)).is_ok();
                stream
                    .write_all(native_post("third", "rust").as_bytes())
                    .unwrap();
                return streamed;
            }
            stream
                .write_all(format!(
                    "{}<faceplate-partial id='feed-next-page-partial' src='/svc/shreddit/feeds/home-feed?sort=BEST&amp;after=t3_first'></faceplate-partial>",
                    native_post("first", "rust")
                ).as_bytes())
                .unwrap();
        }
        unreachable!()
    });
    let mut ids = Vec::new();
    let listing = Reddit::test_at(&base, false)
        .home_progress(Sort::Best, TimeRange::Day, None, &mut |event| {
            if let ListingEvent::Post(post) = event {
                if post.id == "second" {
                    let _ = shown.send(());
                }
                ids.push(post.id.clone());
            }
        })
        .unwrap();
    assert!(
        server.join().unwrap(),
        "continuation posts waited for the whole response"
    );
    assert_eq!(ids, ["first", "second", "third"]);
    assert_eq!(listing.posts.len(), 3);
}
