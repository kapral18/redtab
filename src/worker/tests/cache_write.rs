use super::*;

#[test]
fn when_a_write_finishes_a_cached_target_post_should_be_invalidated_even_after_an_error() {
    let directory = tempfile::tempdir().unwrap();
    let cache = Cache::new(directory.path().to_path_buf());
    let post = Job::Post {
        generation: 1,
        permalink: "/r/rust/comments/post/title/".into(),
    };
    let key = post.cache_key("account").unwrap().0;
    let old_order = cache.reserve_request().unwrap();
    cache
        .store(&key, b"old post", crate::app::now(), old_order)
        .unwrap();
    let job = Job::Experience {
        generation: 2,
        request: crate::reddit::experience::Request::Write(
            crate::reddit::experience::Action::Delete {
                target: crate::reddit::experience::Target {
                    id: "t3_post".into(),
                    author: "reader".into(),
                    subreddit: "rust".into(),
                    permalink: "/r/rust/comments/post/title/".into(),
                },
            },
        ),
        invalidations: vec![Invalidation::Post("/comments/post/_/comment/".into())],
    };
    let (sender, _) = std::sync::mpsc::sync_channel(1);
    cache_result(
        Msg::Experience {
            generation: 2,
            result: Err("ambiguous write".into()),
        },
        &job,
        Some(&cache),
        "account",
        &sender,
        Source::default(),
    );
    assert!(
        cache
            .load(&key, crate::app::now())
            .unwrap()
            .bytes
            .is_empty()
    );
    cache
        .store(&key, b"late old response", crate::app::now(), old_order)
        .unwrap();
    assert!(
        cache
            .load(&key, crate::app::now())
            .unwrap()
            .bytes
            .is_empty()
    );
}

#[test]
fn when_a_full_preview_leaves_memory_it_should_return_from_disk_without_http() {
    let dir = tempfile::tempdir().unwrap();
    let reddit = Reddit::test_at("http://127.0.0.1:9", false);
    let url = "https://i.redd.it/preview.png";
    let job = Job::Image {
        url: url.into(),
        background: true,
    };
    let cache = Cache::new(dir.path().to_path_buf());
    let order = cache.reserve_request().unwrap();
    let (tx, _messages) = std::sync::mpsc::sync_channel(8);
    cache_result(
        Msg::Image {
            url: url.into(),
            result: Ok(DynamicImage::new_rgb8(8, 4).into()),
        },
        &job,
        Some(&cache),
        &reddit.cache_namespace(),
        &tx,
        Source {
            order: Some(order),
            ..Source::default()
        },
    );
    let (pool, messages) = Pool::start(reddit, 3, Some(cache));
    pool.want_image(url);

    pool.submit(job);

    let Msg::Snapshot {
        message,
        refreshing: false,
        source,
        ..
    } = next_message(&messages)
    else {
        panic!("fresh cached image");
    };
    assert!(source.cached);
    assert!(
        matches!(*message, Msg::Image { result: Ok(ref image), .. } if (image.image.width(), image.image.height()) == (8, 4))
    );
}

#[test]
fn navigation_cache_focused_threads_do_not_replace_full_posts_or_reuse_missing_targets() {
    use crate::reddit::experience::{Query, Request, Response};
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path().to_path_buf());
    let page =
        crate::reddit::parse::parse_post_page(include_str!("../../../tests/fixtures/post.json"))
            .unwrap();
    let full = serde_json::to_vec(&page).unwrap();
    let namespace = "focused-cache-fixture";
    let key = Job::Post {
        generation: 1,
        permalink: page.post.permalink.clone(),
    }
    .cache_key(namespace)
    .unwrap()
    .0;
    cache
        .store(
            &key,
            &full,
            crate::app::now(),
            cache.reserve_request().unwrap(),
        )
        .unwrap();
    let job = Job::Experience {
        generation: 2,
        request: Request::Read {
            query: Query::Thread(format!("/comments/{}/_/absent", page.post.id)),
            after: None,
        },
        invalidations: Vec::new(),
    };
    assert!(job.cached_message(&full).is_none());
    let mut partial = page;
    partial.comments.clear();
    partial.post.title = "Focused context".into();
    let (tx, _messages) = sync_channel(8);

    cache_result(
        Msg::Experience {
            generation: 2,
            result: Ok(Response::Thread(Box::new(partial))),
        },
        &job,
        Some(&cache),
        namespace,
        &tx,
        Source {
            order: Some(cache.reserve_request().unwrap()),
            ..Source::default()
        },
    );

    assert_eq!(cache.load(&key, crate::app::now()).unwrap().bytes, full);
}
