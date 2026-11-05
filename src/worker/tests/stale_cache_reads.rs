use super::*;

#[test]
fn when_offscreen_cache_is_requested_it_should_deliver_stale_data_without_any_http() {
    use crate::reddit::tests::{Fixture, Reply};
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = calls.clone();
    let fixture = Fixture::new(move |_| {
        count.fetch_add(1, Ordering::SeqCst);
        Reply::json(serde_json::from_str(include_str!("../../../tests/fixtures/sub.json")).unwrap())
    });
    let reddit = fixture.reddit();
    let directory = tempfile::tempdir().unwrap();
    let cache = Cache::new(directory.path().to_path_buf());
    let job = Job::Group {
        subreddit: "rust".into(),
        generation: 1,
    };
    let key = job.cache_key(&reddit.cache_namespace()).unwrap().0;
    let listing = reddit
        .listing(
            "rust",
            crate::model::Sort::Hot,
            crate::model::TimeRange::Day,
            None,
        )
        .unwrap();
    cache
        .store(
            &key,
            &serde_json::to_vec(&listing).unwrap(),
            crate::app::now() - 600,
            cache.reserve_request().unwrap(),
        )
        .unwrap();
    calls.store(0, Ordering::SeqCst);
    let (pool, messages) = Pool::start(reddit, 2, Some(cache));
    pool.cached(job);
    assert!(matches!(next_message(&messages), Msg::Snapshot {
            refreshing: false, message, ..
        } if matches!(*message, Msg::Group { result: Ok(ref listing), .. } if !listing.posts.is_empty())));
    pool.cached(Job::Group {
        subreddit: "missing".into(),
        generation: 2,
    });
    assert!(
        messages
            .recv_timeout(std::time::Duration::from_millis(150))
            .is_err()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn a_warm_feed_is_readable_after_the_server_shuts_down() {
    use std::io::{BufRead, BufReader, Write};
    let dir = tempfile::tempdir().unwrap();
    let listener = crate::reddit::tests::local_listener();
    let reddit = Reddit::test_at(&format!("http://{}", listener.local_addr().unwrap()), false);
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut reader = BufReader::new(&mut stream);
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap() != 0 {
            if line == "\r\n" {
                break;
            }
            line.clear();
        }
        let body = include_str!("../../../tests/fixtures/sub.json");
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    });
    let job = || Job::Group {
        subreddit: "popular".into(),
        generation: 0,
    };
    let (cold, messages) = Pool::start(
        reddit.clone(),
        3,
        Some(Cache::new(dir.path().to_path_buf())),
    );
    cold.submit(job());
    let Msg::Snapshot {
        message,
        source: Source { cached: false, .. },
        ..
    } = next_message(&messages)
    else {
        panic!("network snapshot")
    };
    let Msg::Group {
        result: Ok(expected),
        ..
    } = *message
    else {
        panic!("parsed listing")
    };
    server.join().unwrap();
    drop(cold);
    let (warm, messages) = Pool::start(reddit, 3, Some(Cache::new(dir.path().to_path_buf())));

    warm.submit(job());

    let Msg::Snapshot {
        message,
        source: Source { cached: true, .. },
        refreshing: false,
        ..
    } = next_message(&messages)
    else {
        panic!("fresh disk snapshot")
    };
    let Msg::Group {
        result: Ok(actual), ..
    } = *message
    else {
        panic!("cached listing")
    };
    assert_eq!(actual, expected);
}

#[test]
fn when_home_has_an_old_cached_primer_it_should_not_replay_that_incomplete_start() {
    let dir = tempfile::tempdir().unwrap();
    let reddit = Reddit::test_at("http://127.0.0.1:9", false);
    let cache = Cache::new(dir.path().to_path_buf());
    let legacy = r#"["browser-home-v1",{"Listing":{"sort":"Best","time":"Day"}},null]"#;
    let key = Cache::key(&reddit.cache_namespace(), legacy);
    let mut listing =
        crate::reddit::parse::parse_listing(include_str!("../../../tests/fixtures/sub.json"))
            .unwrap();
    let template = listing.posts[0].clone();
    listing.posts = ["a", "b", "c"]
        .into_iter()
        .map(|id| {
            let mut p = template.clone();
            p.id = id.into();
            p.permalink = format!("/comments/{id}/");
            p
        })
        .collect();
    cache
        .store(
            &key,
            &serde_json::to_vec(&listing).unwrap(),
            crate::app::now(),
            cache.reserve_request().unwrap(),
        )
        .unwrap();
    let (pool, messages) = Pool::start(reddit, 3, Some(cache));

    pool.submit(Job::Feed {
        tab: 1,
        generation: 1,
        target: FeedTarget::Home,
        query: FeedQuery::Listing {
            sort: Sort::Best,
            time: TimeRange::Day,
        },
        after: None,
    });

    assert!(
        matches!(next_message(&messages),Msg::Snapshot{message,source,..} if !source.cached && matches!(*message,Msg::Feed{result:Err(_),..}))
    );
}

#[test]
fn force_refresh_does_not_accept_a_fresh_disk_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let reddit = Reddit::test_at("http://127.0.0.1:9", false);
    let job = Job::Post {
        generation: 1,
        permalink: "/r/rust/comments/p/title/".into(),
    };
    let cache = Cache::new(dir.path().to_path_buf());
    let (key, _) = job.cache_key(&reddit.cache_namespace()).unwrap();
    let page =
        crate::reddit::parse::parse_post_page(include_str!("../../../tests/fixtures/post.json"))
            .unwrap();
    cache
        .store(
            &key,
            &serde_json::to_vec(&page).unwrap(),
            crate::app::now(),
            cache.reserve_request().unwrap(),
        )
        .unwrap();
    let (pool, messages) = Pool::start(reddit, 3, Some(cache));
    pool.want_post(Some(1));

    pool.refresh(job);

    assert!(matches!(
        next_message(&messages),
        Msg::Post { result: Err(_), .. }
    ));
}

#[test]
fn fresh_cached_replies_revalidate_when_the_parent_is_newer() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path().to_path_buf());
    let older = cache.reserve_request().unwrap();
    let newer = cache.reserve_request().unwrap();
    let reddit = Reddit::test_at("http://127.0.0.1:9", false);
    let job = Job::MoreReplies {
        generation: 1,
        comment_id: "parent".into(),
        cursor: "parent".into(),
        minimum: Some(Source {
            order: Some(newer),
            ..Source::default()
        }),
    };
    let (key, _) = job.cache_key(&reddit.cache_namespace()).unwrap();
    cache
        .store(
            &key,
            &serde_json::to_vec(&ReplyPage::complete(Vec::new())).unwrap(),
            crate::app::now(),
            older,
        )
        .unwrap();
    let (pool, messages) = Pool::start(reddit, 3, Some(cache));

    pool.want_post(Some(1));
    pool.submit(job);

    assert!(matches!(
        next_message(&messages),
        Msg::Snapshot {
            refreshing: true,
            ..
        }
    ));
    assert!(matches!(
        next_message(&messages),
        Msg::MoreReplies { result: Err(_), .. }
    ));
}
