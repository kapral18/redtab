use super::*;

#[test]
fn pictures_in_text_should_wait_on_the_background_queue_not_the_one_the_reader_waits_on() {
    let (priority, priority_rx) = channel();
    let (background, _background_rx) = channel();
    let (thumbs, _thumbs_rx) = channel();
    let (icons, _icons_rx) = channel();
    let (foreground_frames, foreground_frames_rx) = channel();
    let (frames, frames_rx) = channel();
    let (images, images_rx) = channel();
    let queues = Queues {
        foreground_images: priority.clone(),
        feeds: priority.clone(),
        selected: priority.clone(),
        images,
        priority,
        group_cache: background.clone(),
        background,
        thumbs,
        foreground_frames,
        frames,
        icons,
    };
    let image = |background| Work {
        job: Job::Image {
            url: "https://preview.redd.it/x.png".into(),
            background,
        },
        force: false,
        cache_policy: CachePolicy::Revalidate,
        image_ticket: None,
    };

    queues.submit(image(true));
    queues.submit(image(false));

    assert!(matches!(
        images_rx.try_recv(),
        Ok(Work {
            job: Job::Image {
                background: true,
                ..
            },
            ..
        })
    ));
    assert!(matches!(
        priority_rx.try_recv(),
        Ok(Work {
            job: Job::Image {
                background: false,
                ..
            },
            ..
        })
    ));
    assert!(
        priority_rx.try_recv().is_err(),
        "the text picture is not on it"
    );
    for background in [true, false] {
        queues.submit(Work {
            job: Job::VideoFrame {
                url: "https://v.redd.it/x/DASH_360.mp4".into(),
                background,
            },
            ..image(background)
        });
    }
    assert!(
        images_rx.try_recv().is_err() && priority_rx.try_recv().is_err(),
        "ffmpeg stills never wait in a picture queue"
    );
    let frame = |work: Result<Work, _>| match work {
        Ok(Work {
            job: Job::VideoFrame { background, .. },
            ..
        }) => Some(background),
        _ => None,
    };
    assert_eq!(frame(frames_rx.try_recv()), Some(true));
    assert_eq!(frame(foreground_frames_rx.try_recv()), Some(false));
}

#[test]
fn an_icon_lookup_waits_on_neither_home_group_loads_nor_thumbnails() {
    let (priority, _priority_rx) = channel();
    let (background, background_rx) = channel();
    let (thumbs, thumbs_rx) = channel();
    let (icons, icons_rx) = channel();
    let (priority_frames, _frames_rx) = channel();
    let (images, _images_rx) = channel();
    let queues = Queues {
        foreground_images: priority.clone(),
        feeds: priority.clone(),
        selected: priority.clone(),
        images,
        priority,
        group_cache: background.clone(),
        background,
        thumbs,
        foreground_frames: priority_frames.clone(),
        frames: priority_frames,
        icons,
    };

    queues.submit(Work {
        job: Job::Icon {
            subreddit: "s".into(),
        },
        force: false,
        cache_policy: CachePolicy::Revalidate,
        image_ticket: None,
    });

    assert!(matches!(
        icons_rx.try_recv(),
        Ok(Work {
            job: Job::Icon { .. },
            ..
        })
    ));
    assert!(background_rx.try_recv().is_err());
    assert!(thumbs_rx.try_recv().is_err());
}

#[test]
fn the_search_tabs_preview_and_discovery_should_stay_wanted_beside_other_requests() {
    use crate::reddit::experience::{ListSearch, Query};
    let (pool, _messages) = Pool::start(Reddit::test_at("http://127.0.0.1:9", false), 1, None);
    let preview = |generation| Job::Feed {
        tab: PREVIEW_FEED,
        generation,
        target: FeedTarget::Subreddit("rust".into()),
        query: FeedQuery::DEFAULT_LISTING,
        after: None,
    };
    pool.want_feeds(Some(3), HashMap::from([(PREVIEW_FEED, HashSet::from([7]))]));
    assert!(!background_unwanted(&preview(7), &pool.wanted));
    assert!(background_unwanted(&preview(6), &pool.wanted));

    let discovery = || Job::Search {
        generation: 1,
        query: Query::Communities,
        after: None,
    };
    let search = |generation| Job::Search {
        generation,
        query: Query::Search {
            text: "rust".into(),
            kind: ListSearch::Communities,
        },
        after: None,
    };
    pool.submit(discovery());
    pool.submit(search(2));
    pool.submit(search(3));
    assert!(!background_unwanted(&discovery(), &pool.wanted));
    assert!(background_unwanted(&search(2), &pool.wanted));
    assert!(!background_unwanted(&search(3), &pool.wanted));

    pool.want_search(&Query::Communities, None);
    assert!(background_unwanted(&discovery(), &pool.wanted));
    assert!(!background_unwanted(&search(3), &pool.wanted));
    if let Job::Search { query, .. } = search(3) {
        pool.want_search(&query, None);
    }
    assert!(background_unwanted(&search(3), &pool.wanted));
    pool.stop();
}

#[test]
fn navigation_cache_islands_publish_posts_before_the_final_snapshot() {
    use crate::reddit::tests::{Fixture, Reply};
    let fixture = Fixture::new(|_| {
        Reply::json(serde_json::from_str(include_str!("../../../tests/fixtures/sub.json")).unwrap())
    });
    let dir = tempfile::tempdir().unwrap();
    let reddit = fixture.reddit();
    reddit
        .listing("popular", Sort::Hot, TimeRange::Day, None)
        .unwrap();
    let (pool, messages) = Pool::start(reddit, 3, Some(Cache::new(dir.path().to_path_buf())));
    pool.submit(Job::Group {
        subreddit: "popular".into(),
        generation: 1,
    });

    assert!(matches!(
        messages.recv_timeout(std::time::Duration::from_secs(5)),
        Ok(Msg::GroupProgress { generation: 1, .. })
    ));
    assert!(
        matches!(next_message(&messages), Msg::Snapshot { message, .. } if matches!(*message, Msg::Group { generation: 1, result: Ok(_), .. }))
    );
}

#[test]
fn denied_group_responses_should_keep_their_order_after_a_newer_feed_succeeds() {
    for disk_cache in [false, true] {
        let server = crate::reddit::tests::local_http_server();
        let base = format!("http://{}", server.server_addr());
        let dir = tempfile::tempdir().unwrap();
        let (pool, messages) = Pool::start(
            Reddit::test_at(&base, false),
            4,
            disk_cache.then(|| Cache::new(dir.path().to_path_buf())),
        );
        pool.submit(Job::Group {
            subreddit: "private".into(),
            generation: 1,
        });
        let group = data_request(&server).unwrap();
        pool.submit(Job::Feed {
            tab: 1,
            generation: 2,
            target: FeedTarget::Subreddit("private".into()),
            query: FeedQuery::DEFAULT_LISTING,
            after: None,
        });
        let feed = data_request(&server).unwrap();
        feed.respond(crate::reddit::tests::served::Response::from_string(
            include_str!("../../../tests/fixtures/sub.json"),
        ))
        .unwrap();
        let successful = loop {
            let message = next_message(&messages);
            if !matches!(message, Msg::FeedProgress { .. }) {
                break message;
            }
        };
        let Msg::Snapshot {
            message,
            source: newer,
            ..
        } = successful
        else {
            panic!("successful feed snapshot")
        };
        assert!(matches!(*message, Msg::Feed { result: Ok(_), .. }));
        group
            .respond(
                crate::reddit::tests::served::Response::from_string(r#"{"reason":"private"}"#)
                    .with_status_code(403),
            )
            .unwrap();

        let Msg::Snapshot {
            message,
            source: older,
            ..
        } = next_message(&messages)
        else {
            panic!("denied group must retain its request source")
        };
        assert!(matches!(
            *message,
            Msg::Group {
                result: Err(FeedError::Access(AccessDenied::PrivateCommunity)),
                ..
            }
        ));
        assert!(!older.cached);
        assert_eq!(
            older.compare_requests(newer),
            Some(std::cmp::Ordering::Less),
            "disk_cache={disk_cache}"
        );
    }
}
