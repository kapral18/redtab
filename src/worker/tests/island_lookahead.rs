use super::*;

#[test]
fn island_lookahead_visible_reads_should_bypass_blocked_bulk_cache_warming() {
    use crate::reddit::tests::{Fixture, Reply};
    let (started, requests) = sync_channel(1);
    let fixture = Fixture::new(move |request| {
        if request.url.starts_with("/r/visible/") {
            started.send(()).unwrap();
        }
        Reply::json(serde_json::from_str(include_str!("../../../tests/fixtures/sub.json")).unwrap())
    });
    let reddit = fixture.reddit();
    let listing = reddit
        .listing("warm", Sort::Hot, TimeRange::Day, None)
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let cache = Cache::new(directory.path().to_path_buf());
    let mut bulk = Vec::new();
    for index in 0..9 {
        let job = Job::Group {
            subreddit: format!("bulk{index}"),
            generation: 1,
        };
        let key = job.cache_key(&reddit.cache_namespace()).unwrap().0;
        cache
            .store(
                &key,
                &serde_json::to_vec(&listing).unwrap(),
                crate::app::now(),
                cache.reserve_request().unwrap(),
            )
            .unwrap();
        bulk.push(job);
    }
    let (pool, messages) = Pool::start(reddit, 3, Some(cache));
    for job in bulk {
        pool.cached(job);
    }
    pool.set_group_demand(&["visible"]);
    pool.submit(Job::Group {
        subreddit: "visible".into(),
        generation: 2,
    });

    requests
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("visible HTTP must start even while bulk cache snapshots fill the result channel");

    pool.stop();
    drop(messages);
}

#[test]
fn island_lookahead_should_reuse_stale_cache_and_fetch_only_misses() {
    use crate::reddit::tests::{Fixture, Reply};
    let calls = Arc::new(Mutex::new(Vec::new()));
    let requests = calls.clone();
    let fixture = Fixture::new(move |request| {
        requests.lock().unwrap().push(request.url.clone());
        Reply::json(serde_json::from_str(include_str!("../../../tests/fixtures/sub.json")).unwrap())
    });
    let reddit = fixture.reddit();
    let listing = reddit
        .listing("warm", Sort::Hot, TimeRange::Day, None)
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let cache = Cache::new(directory.path().to_path_buf());
    let job = Job::Group {
        subreddit: "stale".into(),
        generation: 1,
    };
    let key = job.cache_key(&reddit.cache_namespace()).unwrap().0;
    cache
        .store(
            &key,
            &serde_json::to_vec(&listing).unwrap(),
            crate::app::now() - 600,
            cache.reserve_request().unwrap(),
        )
        .unwrap();
    calls.lock().unwrap().clear();
    let (pool, messages) = Pool::start(reddit, 3, Some(cache));
    pool.set_group_demand(&["stale", "cold"]);

    pool.prefetch_group("stale".into(), 1);
    assert!(
        matches!(next_message(&messages), Msg::Snapshot { refreshing: false, message, .. } if matches!(*message, Msg::Group { generation: 1, result: Ok(_), .. }))
    );
    pool.prefetch_group("cold".into(), 2);
    assert!(
        matches!(next_message(&messages), Msg::Snapshot { message, .. } if matches!(*message, Msg::Group { generation: 2, result: Ok(_), .. }))
    );

    let actual = calls.lock().unwrap();
    assert_eq!(actual.len(), 1, "{actual:?}");
    assert!(actual[0].starts_with("/r/cold/"), "{actual:?}");
}

#[test]
fn island_browsing_should_use_quota_headroom_and_keep_selected_posts_responsive() {
    use crate::reddit::tests::{Fixture, Reply};
    let fixture = Fixture::new(|request| Reply {
        status: 200,
        body: if request.url.starts_with("/comments/") {
            include_str!("../../../tests/fixtures/post.json")
        } else {
            include_str!("../../../tests/fixtures/sub.json")
        }
        .into(),
        headers: vec![
            ("X-Ratelimit-Remaining".into(), "99".into()),
            ("X-Ratelimit-Reset".into(), "600".into()),
        ],
    });
    let reddit = fixture.reddit();
    reddit
        .listing("seed", Sort::Hot, TimeRange::Day, None)
        .unwrap();
    let (pool, messages) = Pool::start(reddit, 3, None);
    let names: Vec<_> = (0..17).map(|i| format!("island{i}")).collect();
    pool.set_group_demand(&names.iter().map(String::as_str).collect::<Vec<_>>());
    for (i, name) in names.iter().take(16).enumerate() {
        pool.submit(Job::Group {
            subreddit: name.clone(),
            generation: i as u64,
        });
        let message = match next_message(&messages) {
            Msg::Snapshot { message, .. } => *message,
            message => message,
        };
        assert!(matches!(message, Msg::Group { result: Ok(_), .. }));
    }

    pool.submit(Job::Group {
        subreddit: names[16].clone(),
        generation: 16,
    });
    let message = match next_message(&messages) {
        Msg::Snapshot { message, .. } => *message,
        message => message,
    };
    assert!(matches!(
        message,
        Msg::Group {
            generation: 16,
            result: Ok(_),
            ..
        }
    ));
    pool.want_post(Some(7));
    pool.submit(Job::Post {
        generation: 7,
        permalink: "/comments/p/".into(),
    });

    let message = match next_message(&messages) {
        Msg::Snapshot { message, .. } => *message,
        message => message,
    };
    assert!(matches!(
        message,
        Msg::Post {
            generation: 7,
            result: Ok(_),
            ..
        }
    ));
    pool.set_group_demand(&[]);
}
