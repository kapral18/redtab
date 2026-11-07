use super::*;

#[test]
fn when_reply_expansions_are_replaced_they_should_release_workers_during_cooldown() {
    let server = crate::reddit::tests::local_http_server();
    let reddit = Reddit::test_at(&format!("http://{}", server.server_addr()), false);
    let (pool, messages) = Pool::start(reddit, 2, None);
    pool.want_post(Some(1));
    for id in ["a", "b"] {
        pool.submit(Job::MoreReplies {
            generation: 1,
            comment_id: id.into(),
            cursor: serde_json::json!({"post":"old","parent":"t3_old","children":[]}).to_string(),
            minimum: None,
        });
    }
    let first = data_request(&server).unwrap();
    let second = data_request(&server).unwrap();
    for request in [first, second] {
        request
            .respond(
                crate::reddit::tests::served::Response::empty(429).with_header(
                    crate::reddit::tests::served::Header::new("Retry-After", "1"),
                ),
            )
            .unwrap();
    }
    pool.want_post(Some(2));
    pool.submit(Job::Post {
        generation: 2,
        permalink: "/comments/new".into(),
    });
    for _ in 0..2 {
        let message = messages
            .recv_timeout(std::time::Duration::from_millis(500))
            .expect("obsolete reads should release workers before cooldown ends");
        let message = if let Msg::Snapshot { message, .. } = message {
            *message
        } else {
            message
        };
        assert!(matches!(
            message,
            Msg::MoreReplies {
                generation: 1,
                result: Err(_),
                ..
            }
        ));
    }
    let next = data_request(&server).unwrap();
    assert!(
        next.url().starts_with("/comments/new.json"),
        "{}",
        next.url()
    );
    next.respond(crate::reddit::tests::served::Response::from_string(
        include_str!("../../../tests/fixtures/post.json"),
    ))
    .unwrap();
    let message = next_message(&messages);
    assert!(
        matches!(message, Msg::Snapshot { message, .. } if matches!(*message, Msg::Post { generation: 2, result: Ok(_), .. }))
    );
}

#[test]
fn when_active_detail_reads_are_superseded_the_next_post_should_not_wait_for_them() {
    let server = crate::reddit::tests::local_http_server();
    let base = format!("http://{}", server.server_addr());
    let (pool, messages) = Pool::start(Reddit::test_at(&base, false), 3, None);
    pool.want_post(Some(1));
    for _ in 0..2 {
        pool.submit(Job::Post {
            generation: 1,
            permalink: "/comments/old/".into(),
        });
    }
    let held = [
        data_request(&server).unwrap(),
        data_request(&server).unwrap(),
    ];
    pool.want_post(Some(2));
    pool.submit(Job::Post {
        generation: 2,
        permalink: "/comments/new/".into(),
    });
    let selected = data_request(&server);
    for request in held {
        let _ = request.respond(crate::reddit::tests::served::Response::empty(200));
    }
    let selected = selected.expect("superseded active reads must release selected workers");
    assert!(selected.url().starts_with("/comments/new.json"));
    selected
        .respond(crate::reddit::tests::served::Response::from_string(
            include_str!("../../../tests/fixtures/post.json"),
        ))
        .unwrap();
    for _ in 0..3 {
        let message = match next_message(&messages) {
            Msg::Snapshot { message, .. } => *message,
            message => message,
        };
        if let Msg::Post {
            generation: 2,
            result,
        } = message
        {
            assert!(result.is_ok());
            return;
        }
    }
    panic!("selected post result missing");
}
