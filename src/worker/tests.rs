mod cache_write;
mod cached_answer_delivery;
mod cached_images;
mod downloads;
mod island_lookahead;
mod jobs;
mod membership_read_invalidation;
mod priority_pool_starts;
mod queue_priority_ordering;
mod run;
mod stale_cache_reads;
mod superseded_read_release;
mod threads;
mod worker_release_cancellation;

use super::*;

fn next_message(messages: &Receiver<Msg>) -> Msg {
    loop {
        let message = messages
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("worker response");
        if !matches!(message, Msg::GroupProgress { .. }) {
            return message;
        }
    }
}

fn data_request(
    server: &crate::reddit::tests::served::Server,
) -> Option<crate::reddit::tests::served::Request> {
    server
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap()
}

fn poster_reply(request: crate::reddit::tests::served::Request) {
    let png = encode_png(&DynamicImage::new_rgb8(2, 2)).unwrap();
    request
        .respond(crate::reddit::tests::served::Response::from_data(png))
        .unwrap();
}

fn next_image(messages: &Receiver<Msg>) {
    loop {
        let message = match next_message(messages) {
            Msg::Snapshot { message, .. } => *message,
            message => message,
        };
        if let Msg::Image { result, .. } = message {
            assert!(result.is_ok(), "image result: {result:?}");
            return;
        }
    }
}

fn queued_image_pool() -> (Pool, Receiver<Work>) {
    let (sender, receiver) = channel();
    (
        Pool {
            queues: Queues {
                foreground_images: sender.clone(),
                feeds: sender.clone(),
                selected: sender.clone(),
                priority: sender.clone(),
                background: sender.clone(),
                group_cache: sender.clone(),
                images: sender.clone(),
                thumbs: sender.clone(),
                foreground_frames: sender.clone(),
                frames: sender.clone(),
                icons: sender,
            },
            wanted: Arc::default(),
            cache: None,
            threads: 1,
        },
        receiver,
    )
}

fn assert_queued_image_can_promote(cache: Option<Cache>) {
    let server = crate::reddit::tests::local_http_server();
    let base = format!("http://{}", server.server_addr());
    let (pool, messages) = Pool::start(Reddit::test_at(&base, false), 3, cache);
    for name in ["first", "second"] {
        let url = format!("{base}/{name}.png");
        pool.want_image(&url);
        pool.submit(Job::Image {
            url,
            background: true,
        });
    }
    let first = data_request(&server).unwrap();
    let second = data_request(&server).unwrap();
    let url = format!("{base}/promoted.png");
    pool.want_image(&url);
    pool.submit(Job::Image {
        url: url.clone(),
        background: true,
    });

    pool.submit(Job::Image {
        url,
        background: false,
    });
    let promoted = data_request(&server);

    poster_reply(first);
    poster_reply(second);
    let promoted = promoted.expect("foreground image must bypass queued background images");
    assert_eq!(promoted.url(), "/promoted.png");
    poster_reply(promoted);
    for _ in 0..3 {
        next_image(&messages);
    }
    let extra = data_request(&server)
        .map(|request| (request.method().to_string(), request.url().to_string()));
    assert!(
        extra.is_none(),
        "promotion fetched again from its old queue: {extra:?}"
    );
}
