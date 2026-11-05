use super::*;

#[test]
fn a_thumbnail_read_from_disk_counts_as_used() {
    let dir = tempfile::tempdir().unwrap();
    let reddit = Reddit::test_at("http://127.0.0.1:9", false);
    let url = "https://i.redd.it/used.png";
    let job = Job::Thumb { url: url.into() };
    let cache = Cache::new(dir.path().to_path_buf());
    let (key, _) = job.cache_key(&reddit.cache_namespace()).unwrap();
    let image = DynamicImage::ImageRgb8(image::RgbImage::new(2, 2));
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    let day_old = crate::app::now() - 86_400;
    let order = cache.reserve_request().unwrap();
    // Stored with its modification time at `day_old`.
    cache.store(&key, bytes.get_ref(), day_old, order).unwrap();
    let path = dir.path().join(format!("{key}.cache"));
    let (pool, messages) = Pool::start(reddit, 3, Some(cache));
    pool.set_on_screen(&[url]);

    pool.submit(job);

    assert!(matches!(
        next_message(&messages),
        Msg::Snapshot {
            refreshing: false,
            ..
        }
    ));
    // The reader touches the entry after it answers.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let used = || {
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        modified.elapsed().unwrap() < std::time::Duration::from_secs(60)
    };
    while !used() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(used());
}

#[test]
fn cached_thumbnail_arrives_before_a_failed_background_refresh() {
    let dir = tempfile::tempdir().unwrap();
    let reddit = Reddit::test_at("http://127.0.0.1:9", false);
    let job = Job::Thumb {
        url: "https://i.redd.it/cached.png".into(),
    };
    let cache = Cache::new(dir.path().to_path_buf());
    let (key, _) = job.cache_key(&reddit.cache_namespace()).unwrap();
    let image =
        DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 2, image::Rgb([10, 20, 30])));
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    cache
        .store(
            &key,
            bytes.get_ref(),
            crate::app::now() - IMAGE_TTL,
            cache.reserve_request().unwrap(),
        )
        .unwrap();
    let (pool, messages) = Pool::start(reddit, 3, Some(cache));
    pool.set_on_screen(&["https://i.redd.it/cached.png"]);

    pool.submit(job);

    let Msg::Snapshot {
        message,
        source: Source { cached: true, .. },
        ..
    } = next_message(&messages)
    else {
        panic!("cached thumbnail")
    };
    let Msg::Thumb {
        result: Ok(actual), ..
    } = *message
    else {
        panic!("decoded thumbnail")
    };
    assert_eq!(actual.to_rgb8().get_pixel(0, 0).0, [10, 20, 30]);
    assert!(matches!(
        next_message(&messages),
        Msg::Thumb { result: Err(_), .. }
    ));
}

#[test]
fn when_an_image_finishes_just_after_leaving_view_it_should_remain_reusable_from_cache() {
    let server = crate::reddit::tests::local_http_server();
    let base = format!("http://{}", server.server_addr());
    let dir = tempfile::tempdir().unwrap();
    let (pool, messages) = Pool::start(
        Reddit::test_at(&base, false),
        3,
        Some(Cache::new(dir.path().to_path_buf())),
    );
    let url = format!("{base}/revisit.png");
    pool.want_image(&url);
    pool.submit(Job::Image {
        url: url.clone(),
        background: true,
    });
    let request = data_request(&server).unwrap();
    pool.unwant_image(&url);
    poster_reply(request);

    assert!(
        matches!(next_message(&messages), Msg::Snapshot { message, .. }
            if matches!(*message, Msg::Image { result: Ok(_), .. })),
        "a completed image should survive a brief departure"
    );
    pool.want_image(&url);
    pool.submit(Job::Image {
        url,
        background: true,
    });
    assert!(
        matches!(
            messages.recv_timeout(std::time::Duration::from_secs(1)),
            Ok(Msg::Snapshot {
                source: Source { cached: true, .. },
                refreshing: false,
                ..
            })
        ),
        "returning must reuse the completed image without HTTP"
    );
}
