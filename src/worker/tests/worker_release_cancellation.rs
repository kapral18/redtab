use super::*;

#[test]
fn abandoned_collections_release_workers_during_cooldown() {
    let server = crate::reddit::tests::local_http_server();
    let reddit = Reddit::test_at(&format!("http://{}", server.server_addr()), false);
    let (pool, messages) = Pool::start(reddit, 2, None);
    pool.submit(Job::Experience {
        generation: 1,
        request: crate::reddit::experience::Request::Read {
            query: crate::reddit::experience::Query::Multis,
            after: None,
        },
        invalidations: Vec::new(),
    });
    let request = data_request(&server).unwrap();
    request
        .respond(
            tiny_http::Response::empty(429)
                .with_header(tiny_http::Header::from_bytes("Retry-After", "10").unwrap()),
        )
        .unwrap();
    pool.want_experience(None);
    let message = messages
        .recv_timeout(std::time::Duration::from_millis(700))
        .expect("abandoned collection should release its worker before cooldown");
    assert!(matches!(
        message,
        Msg::Experience {
            generation: 1,
            result: Err(_)
        }
    ));
}

#[test]
fn a_first_frame_job_cancels_when_its_video_leaves_the_viewport() {
    if std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_err()
    {
        return;
    }
    let server = crate::reddit::tests::local_http_server();
    let base = format!("http://{}/", server.server_addr());
    let (started, began) = std::sync::mpsc::sync_channel(1);
    let (release, wait) = std::sync::mpsc::sync_channel(1);
    let serving = std::thread::spawn(move || {
        loop {
            let request = server
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap()
                .unwrap();
            if request.url() != "/slow.mp4" {
                let _ = request.respond(tiny_http::Response::empty(200));
                continue;
            }
            started.send(()).unwrap();
            let _ = wait.recv_timeout(std::time::Duration::from_secs(2));
            let _ = request.respond(tiny_http::Response::empty(200));
            break;
        }
    });
    let reddit = Reddit::test_at(&base, false);
    let (pool, messages) = Pool::start(reddit, 1, None);
    let url = format!("{base}slow.mp4");
    pool.want_image(&url);
    pool.submit(Job::VideoFrame {
        url: url.clone(),
        background: true,
    });
    began
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();

    pool.unwant_image(&url);
    let result = messages.recv_timeout(std::time::Duration::from_secs(1));
    let _ = release.send(());
    serving.join().unwrap();

    assert!(
        matches!(result, Ok(Msg::ImageSkipped { .. })),
        "decoder should retire before the stalled request completes"
    );
}

#[test]
fn when_a_paused_community_is_refreshed_the_old_generation_should_release_the_worker() {
    let server = crate::reddit::tests::local_http_server();
    let reddit = Reddit::test_at(&format!("http://{}", server.server_addr()), false);
    let (pool, messages) = Pool::start(reddit, 3, None);
    pool.set_group_demand(&["same"]);
    pool.submit(Job::Group {
        subreddit: "same".into(),
        generation: 1,
    });
    data_request(&server)
        .unwrap()
        .respond(
            tiny_http::Response::empty(429)
                .with_header(tiny_http::Header::from_bytes("Retry-After", "1").unwrap()),
        )
        .unwrap();
    pool.submit(Job::Group {
        subreddit: "same".into(),
        generation: 2,
    });
    let message = messages
        .recv_timeout(std::time::Duration::from_millis(500))
        .expect("new generation should release the old paused read");
    let message = if let Msg::Snapshot { message, .. } = message {
        *message
    } else {
        message
    };
    assert!(
        matches!(message, Msg::Group { generation: 1, result: Err(ref error), .. } if error.as_str() == crate::reddit::REQUEST_CANCELLED)
    );
    data_request(&server)
        .unwrap()
        .respond(tiny_http::Response::from_string(include_str!(
            "../../../tests/fixtures/sub.json"
        )))
        .unwrap();
    assert!(
        matches!(next_message(&messages), Msg::Snapshot { message, .. } if matches!(*message, Msg::Group { generation: 2, result: Ok(_), .. }))
    );
}

#[test]
fn when_missing_icons_leave_the_view_queued_and_waiting_lookups_should_be_released() {
    let server = crate::reddit::tests::local_http_server();
    let reddit = Reddit::test_at(&format!("http://{}", server.server_addr()), false);
    let (pool, messages) = Pool::start(reddit, 2, None);
    pool.set_visible_icons(&["old_a", "old_b"]);
    for name in ["old_a", "old_b"] {
        pool.submit(Job::Icon {
            subreddit: name.into(),
        });
    }
    let first = data_request(&server).unwrap();
    first
        .respond(
            tiny_http::Response::empty(429)
                .with_header(tiny_http::Header::from_bytes("Retry-After", "1").unwrap()),
        )
        .unwrap();
    pool.set_visible_icons(&["new"]);
    pool.submit(Job::Icon {
        subreddit: "new".into(),
    });
    for _ in 0..2 {
        let message = messages
            .recv_timeout(std::time::Duration::from_millis(500))
            .expect("unused icon work should leave the queue during cooldown");
        let message = if let Msg::Snapshot { message, .. } = message {
            *message
        } else {
            message
        };
        assert!(
            matches!(message, Msg::Icon { result: Err(ref error), .. } if error == crate::reddit::REQUEST_CANCELLED)
        );
    }
    let next = data_request(&server).unwrap();
    assert!(
        next.url().starts_with("/r/new/about.json"),
        "{}",
        next.url()
    );
    next.respond(tiny_http::Response::from_string(
        "{\"data\":{\"icon_img\":\"https://styles.redditmedia.com/new.png\"}}",
    ))
    .unwrap();
    assert!(
        matches!(next_message(&messages), Msg::Snapshot { message, .. } if matches!(*message, Msg::Icon { result: Ok(Some(_)), .. }))
    );
}

#[test]
fn when_waiting_communities_leave_the_view_the_current_group_should_get_the_workers() {
    let server = crate::reddit::tests::local_http_server();
    let reddit = Reddit::test_at(&format!("http://{}", server.server_addr()), false);
    let (pool, messages) = Pool::start(reddit, 4, None);
    pool.set_group_demand(&["old_a", "old_b"]);
    for name in ["old_a", "old_b"] {
        pool.prefetch_group(name.into(), 1);
    }
    let first = data_request(&server).unwrap();
    let second = data_request(&server).unwrap();
    for request in [first, second] {
        request
            .respond(
                tiny_http::Response::empty(429)
                    .with_header(tiny_http::Header::from_bytes("Retry-After", "1").unwrap()),
            )
            .unwrap();
    }
    pool.set_group_demand(&["new"]);
    pool.submit(Job::Group {
        subreddit: "new".into(),
        generation: 2,
    });
    for _ in 0..2 {
        let message = messages
            .recv_timeout(std::time::Duration::from_millis(500))
            .expect("offscreen waits should release workers during cooldown");
        let message = if let Msg::Snapshot { message, .. } = message {
            *message
        } else {
            message
        };
        assert!(
            matches!(message, Msg::Group { generation: 1, result: Err(ref error), .. } if error.as_str() == crate::reddit::REQUEST_CANCELLED)
        );
    }
    let next = data_request(&server).unwrap();
    assert!(next.url().starts_with("/r/new/"), "{}", next.url());
    next.respond(tiny_http::Response::from_string(include_str!(
        "../../../tests/fixtures/sub.json"
    )))
    .unwrap();
    assert!(
        matches!(next_message(&messages), Msg::Snapshot { message, .. } if matches!(*message, Msg::Group { generation: 2, result: Ok(_), .. }))
    );
}
