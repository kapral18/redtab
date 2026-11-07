use super::*;

#[test]
fn when_subscription_preloads_are_blocked_a_card_image_should_still_start() {
    let server = crate::reddit::tests::local_http_server();
    let base = format!("http://{}", server.server_addr());
    let (pool, messages) = Pool::start(Reddit::test_at(&base, false), 4, None);
    pool.submit(Job::Group {
        subreddit: "slow_a".into(),
        generation: 1,
    });
    pool.submit(Job::Group {
        subreddit: "slow_b".into(),
        generation: 1,
    });
    let first = data_request(&server).unwrap();
    let second = data_request(&server).unwrap();
    let url = format!("{base}/poster.png");
    pool.want_image(&url);

    pool.submit(Job::Image {
        url: url.clone(),
        background: true,
    });
    let poster = data_request(&server);

    first
        .respond(crate::reddit::tests::served::Response::from_string(
            include_str!("../../../tests/fixtures/sub.json"),
        ))
        .unwrap();
    second
        .respond(crate::reddit::tests::served::Response::from_string(
            include_str!("../../../tests/fixtures/sub.json"),
        ))
        .unwrap();
    let poster = poster.expect("visible media must not wait for subscription group requests");
    assert_eq!(poster.url(), "/poster.png");
    poster_reply(poster);
    next_image(&messages);
}

#[test]
fn when_a_queued_card_image_is_promoted_it_should_start_once_on_the_priority_pool() {
    assert_queued_image_can_promote(None);
}

#[test]
fn when_feed_workers_are_occupied_selected_post_and_media_should_start_independently() {
    let server = crate::reddit::tests::local_http_server();
    let base = format!("http://{}", server.server_addr());
    let (pool, messages) = Pool::start(Reddit::test_at(&base, false), 3, None);
    for tab in 0..2 {
        pool.submit(Job::Feed {
            tab,
            generation: 1,
            target: FeedTarget::Subreddit(format!("held{tab}")),
            query: FeedQuery::DEFAULT_LISTING,
            after: None,
        });
    }
    let first = data_request(&server).unwrap();
    let second = data_request(&server).unwrap();
    pool.want_post(Some(7));
    pool.submit(Job::Post {
        generation: 7,
        permalink: "/r/rust/comments/p/title/".into(),
    });
    let url = format!("{base}/selected.png");
    pool.want_image(&url);
    pool.submit(Job::Image {
        url,
        background: false,
    });
    let selected: Vec<_> = (0..2).filter_map(|_| data_request(&server)).collect();
    for request in &selected {
        assert!(request.url().contains("/comments/p.json") || request.url() == "/selected.png");
    }
    let count = selected.len();
    for request in selected {
        if request.url() == "/selected.png" {
            poster_reply(request);
        } else {
            request
                .respond(crate::reddit::tests::served::Response::from_string(
                    include_str!("../../../tests/fixtures/post.json"),
                ))
                .unwrap();
        }
    }
    for request in [first, second] {
        request
            .respond(crate::reddit::tests::served::Response::from_string(
                include_str!("../../../tests/fixtures/sub.json"),
            ))
            .unwrap();
    }
    assert_eq!(
        count, 2,
        "cold selected content and media must bypass occupied feed workers"
    );
    for _ in 0..4 {
        next_message(&messages);
    }
}

#[test]
fn visible_search_pages_should_start_before_passive_feed_previews() {
    use crate::reddit::experience::Query;
    use crate::reddit::tests::{Fixture, Reply};
    let calls = Arc::new(Mutex::new(Vec::new()));
    let observed = calls.clone();
    let fixture = Fixture::new(move |request| {
        observed.lock().unwrap().push(request.url.clone());
        Reply {
            status: 200,
            body: serde_json::json!({"kind":"Listing","data":{"children":[],"after":null}})
                .to_string(),
            headers: if request.url.starts_with("/r/seed/") {
                vec![
                    ("x-ratelimit-remaining".into(), "0".into()),
                    ("x-ratelimit-reset".into(), "1".into()),
                ]
            } else {
                Vec::new()
            },
        }
    });
    let reddit = Reddit::test_at_paced(&fixture.base, std::time::Duration::from_millis(100));
    reddit
        .listing("seed", Sort::Hot, TimeRange::Day, None)
        .unwrap();
    let (pool, messages) = Pool::start(reddit, 3, None);
    pool.want_feeds(None, HashMap::from([(PREVIEW_FEED, HashSet::from([1]))]));
    pool.submit(Job::Feed {
        tab: PREVIEW_FEED,
        generation: 1,
        target: FeedTarget::Subreddit("preview".into()),
        query: FeedQuery::DEFAULT_LISTING,
        after: None,
    });
    pool.submit(Job::Search {
        generation: 2,
        query: Query::Communities,
        after: Some("next".into()),
    });
    for _ in 0..2 {
        messages
            .recv_timeout(std::time::Duration::from_secs(30))
            .unwrap();
    }
    let actual = calls.lock().unwrap();
    assert!(
        actual[1].starts_with("/subreddits/popular.json"),
        "visible community rows should precede passive preview: {actual:?}"
    );
    assert!(actual[2].starts_with("/r/preview/"), "{actual:?}");
}
