use super::*;

#[test]
fn when_cached_images_are_delivered_they_should_release_only_terminal_claims() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path().to_path_buf());
    let (pool, work) = queued_image_pool();
    for (name, fresh, wanted) in [
        ("fresh", true, true),
        ("stale", false, true),
        ("skip", true, false),
    ] {
        let url = format!("https://preview.redd.it/{name}.png");
        let job = Job::Image {
            url: url.clone(),
            background: true,
        };
        let (key, _) = job.cache_key("test").unwrap();
        let now = crate::app::now();
        cache
            .store(
                &key,
                &encode_png(&DynamicImage::new_rgb8(2, 2)).unwrap(),
                if fresh { now } else { now - IMAGE_TTL - 1 },
                cache.reserve_request().unwrap(),
            )
            .unwrap();
        if wanted {
            pool.want_image(&url);
        }
        pool.submit(Job::Image {
            url: url.clone(),
            background: true,
        });
        let ticket = work.recv().unwrap().image_ticket.unwrap();
        let mut claim = ImageClaim {
            wanted: &pool.wanted,
            url: Some(&url),
            ticket: Some(&ticket),
            claimed: false,
        };
        let (sender, receiver) = sync_channel(1);

        let complete = cached_reply(
            &job,
            Some(&cache),
            "test",
            &sender,
            &pool.wanted,
            &mut claim,
            CachePolicy::Revalidate,
        );
        let message = next_message(&receiver);

        assert_eq!(complete, fresh || !wanted, "{name}");
        assert_eq!(
            pool.wanted.image_tickets.lock().unwrap().contains_key(&url),
            !complete,
            "{name}"
        );
        assert!(
            matches!(message,
                Msg::Snapshot { refreshing, .. } if wanted && refreshing != fresh
            ) || matches!(message, Msg::ImageSkipped { .. } if !wanted),
            "{name}"
        );
    }
}

#[test]
fn when_the_account_list_is_cached_it_should_load_without_waiting_for_reddit() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path().to_path_buf());
    let reddit = Reddit::test_at("http://127.0.0.1:9", false);
    let job = Job::Subscriptions { generation: 7 };
    let (key, _) = job.cache_key(&reddit.cache_namespace()).unwrap();
    cache
            .store(
                &key,
                br#"[{"community":{"name":"neovim","icon":{"url":"https://i.redd.it/neovim.png"}}},{"community":{"name":"rust","icon":"missing"}}]"#,
                crate::app::now(),
                cache.reserve_request().unwrap(),
            )
            .unwrap();
    let (pool, messages) = Pool::start(reddit, 3, Some(cache));

    pool.submit(job);

    let Msg::Snapshot {
        message,
        source: Source { cached: true, .. },
        refreshing: false,
        ..
    } = next_message(&messages)
    else {
        panic!("fresh cached subscriptions")
    };
    let Msg::Subscriptions {
        generation: 7,
        result: Ok(names),
    } = *message
    else {
        panic!("subscription result")
    };
    assert_eq!(
        names,
        [
            Subscription::Community {
                name: "neovim".into(),
                icon: crate::model::SubscriptionIcon::Url("https://i.redd.it/neovim.png".into())
            },
            Subscription::Community {
                name: "rust".into(),
                icon: crate::model::SubscriptionIcon::Missing
            }
        ]
    );
}

#[test]
fn when_membership_has_a_cached_list_it_should_still_write_then_fetch_the_new_account_list() {
    use crate::reddit::tests::{Fixture, Reply};
    use serde_json::json;
    let joined = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let actual = joined.clone();
    let fixture = Fixture::new(
        move |request| match request.url.split('?').next().unwrap() {
            "/api/me.json" => {
                Reply::json(json!({"data":{"name":"reader","modhash":"fixture-csrf"}}))
            }
            "/api/subscribe" => {
                assert_eq!(request.method, "POST");
                actual.store(true, Ordering::SeqCst);
                Reply::json(json!({}))
            }
            "/subreddits/mine/subscriber.json" => {
                assert!(actual.load(Ordering::SeqCst));
                Reply::json(
                    json!({"kind":"Listing","data":{"children":[{"kind":"t5","data":{"display_name":"armenia"}}],"after":null}}),
                )
            }
            _ => panic!("unexpected membership job request"),
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path().to_path_buf());
    let reddit = fixture.reddit();
    let (key, _) = (Job::Subscriptions { generation: 1 })
        .cache_key(&reddit.cache_namespace())
        .unwrap();
    cache
        .store(
            &key,
            br#"[{"community":{"name":"rust","icon":"unknown"}}]"#,
            crate::app::now(),
            cache.reserve_request().unwrap(),
        )
        .unwrap();
    let (pool, messages) = Pool::start(reddit, 3, Some(cache));
    pool.submit(Job::Membership {
        generation: 2,
        target: crate::model::MembershipTarget::Community("armenia".into()),
        join: true,
    });
    assert!(matches!(
        next_message(&messages),
        Msg::Membership {
            generation: 2,
            result: Ok(())
        }
    ));
    assert!(joined.load(Ordering::SeqCst));
    pool.submit(Job::Subscriptions { generation: 3 });
    let Msg::Snapshot {
        message, source, ..
    } = next_message(&messages)
    else {
        panic!("fresh list")
    };
    assert!(!source.cached);
    assert!(
        matches!(*message, Msg::Subscriptions { generation: 3, result: Ok(names) } if names == [Subscription::Community { name: "armenia".into(), icon: crate::model::SubscriptionIcon::Unknown }])
    );
}

#[test]
fn a_days_old_icon_answers_from_disk_without_asking_reddit() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path().to_path_buf());
    let reddit = Reddit::test_at("http://127.0.0.1:9", false);
    let icons = [("withicon", Some("/i.png")), ("plain", None)];
    for (name, icon) in icons {
        let (key, _) = Job::Icon {
            subreddit: name.into(),
        }
        .cache_key(&reddit.cache_namespace())
        .unwrap();
        let order = cache.reserve_request().unwrap();
        let day_old = crate::app::now() - 86_400;
        let bytes = serde_json::to_vec(&icon).unwrap();
        cache.store(&key, &bytes, day_old, order).unwrap();
    }
    let namespace = reddit.cache_namespace();
    let (pool, messages) = Pool::start(reddit, 3, Some(cache));

    for (name, icon) in icons {
        pool.submit(Job::Icon {
            subreddit: name.to_uppercase(),
        });
        let Msg::Snapshot {
            message,
            refreshing: false,
            ..
        } = next_message(&messages)
        else {
            panic!("fresh disk snapshot")
        };
        let Msg::Icon { result, .. } = *message else {
            panic!("icon reply")
        };
        assert_eq!(result.unwrap().as_deref(), icon);
    }
    assert!(
        messages
            .recv_timeout(std::time::Duration::from_millis(300))
            .is_err()
    );
    // Read again counts as used: pruning keeps it over entries never read.
    let (key, _) = Job::Icon {
        subreddit: "plain".into(),
    }
    .cache_key(&namespace)
    .unwrap();
    let used = std::fs::metadata(dir.path().join(format!("{key}.cache")))
        .unwrap()
        .modified()
        .unwrap();
    assert!(used.elapsed().unwrap() < std::time::Duration::from_secs(60));
}

#[test]
fn navigation_cache_thread_links_reuse_the_full_post_snapshot() {
    use crate::reddit::experience::{Query, Request, Response};
    let dir = tempfile::tempdir().unwrap();
    let reddit = Reddit::test_at("http://127.0.0.1:9", false);
    let page =
        crate::reddit::parse::parse_post_page(include_str!("../../../tests/fixtures/post.json"))
            .unwrap();
    let cache = Cache::new(dir.path().to_path_buf());
    let key = Job::Post {
        generation: 1,
        permalink: page.post.permalink.clone(),
    }
    .cache_key(&reddit.cache_namespace())
    .unwrap()
    .0;
    cache
        .store(
            &key,
            &serde_json::to_vec(&page).unwrap(),
            crate::app::now(),
            cache.reserve_request().unwrap(),
        )
        .unwrap();
    let (pool, messages) = Pool::start(reddit, 2, Some(cache));

    pool.submit(Job::Experience {
        generation: 2,
        request: Request::Read {
            query: Query::Thread(page.post.permalink.clone()),
            after: None,
        },
        invalidations: Vec::new(),
    });

    assert!(matches!(next_message(&messages), Msg::Snapshot {
            message, source: Source { cached: true, .. }, refreshing: false, ..
        } if matches!(*message, Msg::Experience { generation: 2, result: Ok(Response::Thread(ref actual)) } if actual.post.id == page.post.id)));
}

#[test]
fn when_a_feed_ends_with_incomplete_json_it_should_publish_posts_without_caching_a_partial_page() {
    use std::io::Write;
    let server = crate::reddit::tests::local_http_server();
    let base = format!("http://{}", server.server_addr());
    let dir = tempfile::tempdir().unwrap();
    let reddit = Reddit::test_at(&base, false);
    let job = Job::Feed {
        tab: 1,
        generation: 1,
        target: FeedTarget::Subreddit("s".into()),
        query: FeedQuery::DEFAULT_LISTING,
        after: None,
    };
    let (key, _) = job.cache_key(&reddit.cache_namespace()).unwrap();
    let (pool, messages) = Pool::start(reddit, 3, Some(Cache::new(dir.path().to_path_buf())));
    pool.submit(job);
    let request = data_request(&server).unwrap();
    let body = r#"{"kind":"Listing","data":{"children":[{"kind":"t3","data":{"id":"arrived","title":"arrived","subreddit":"s","permalink":"/comments/arrived/"}},"#;
    request
        .stream(Some(body.len()))
        .write_all(body.as_bytes())
        .unwrap();

    let progress = next_message(&messages);
    let terminal = next_message(&messages);

    assert!(matches!(progress,Msg::FeedProgress{post,..} if post.id=="arrived"));
    assert!(
        matches!(terminal,Msg::Snapshot{message,..} if matches!(*message,Msg::Feed{result:Err(_),..}))
    );
    assert!(
        Cache::new(dir.path().to_path_buf())
            .load(&key, crate::app::now())
            .is_none()
    );
}

#[test]
fn stale_content_arrives_before_a_failed_refresh() {
    let dir = tempfile::tempdir().unwrap();
    let reddit = Reddit::test_at("http://127.0.0.1:9", false);
    let job = Job::Group {
        subreddit: "rust".into(),
        generation: 0,
    };
    let cache = Cache::new(dir.path().to_path_buf());
    let (key, _) = job.cache_key(&reddit.cache_namespace()).unwrap();
    let listing =
        crate::reddit::parse::parse_listing(include_str!("../../../tests/fixtures/sub.json"))
            .unwrap();
    cache
        .store(
            &key,
            &serde_json::to_vec(&listing).unwrap(),
            crate::app::now() - CONTENT_TTL,
            cache.reserve_request().unwrap(),
        )
        .unwrap();
    let (pool, messages) = Pool::start(reddit, 3, Some(cache));

    pool.submit(job);

    let Msg::Snapshot {
        message,
        source: Source { cached: true, .. },
        refreshing: true,
        ..
    } = next_message(&messages)
    else {
        panic!("stale disk snapshot first")
    };
    let Msg::Group {
        result: Ok(actual), ..
    } = *message
    else {
        panic!("cached listing")
    };
    assert_eq!(actual, listing);
    let Msg::Snapshot {
        message, source, ..
    } = next_message(&messages)
    else {
        panic!("failed revalidation snapshot")
    };
    assert!(!source.cached);
    assert!(source.order.is_some());
    assert!(matches!(*message, Msg::Group { result: Err(_), .. }));
}

#[test]
fn stale_media_resolution_does_not_trigger_a_cached_viewer_action() {
    let dir = tempfile::tempdir().unwrap();
    let reddit = Reddit::test_at("http://127.0.0.1:9", false);
    let job = Job::ResolveMedia {
        permalink: "/r/pics/comments/p/gallery/".into(),
    };
    let cache = Cache::new(dir.path().to_path_buf());
    let (key, _) = job.cache_key(&reddit.cache_namespace()).unwrap();
    let page =
        crate::reddit::parse::parse_post_page(include_str!("../../../tests/fixtures/gallery.json"))
            .unwrap();
    cache
        .store(
            &key,
            &serde_json::to_vec(&page).unwrap(),
            crate::app::now() - CONTENT_TTL,
            cache.reserve_request().unwrap(),
        )
        .unwrap();
    let (pool, messages) = Pool::start(reddit, 3, Some(cache));

    pool.submit(job);

    assert!(matches!(
        next_message(&messages),
        Msg::MediaResolved { result: Err(_), .. }
    ));
}
