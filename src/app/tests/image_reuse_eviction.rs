use super::*;

#[test]
fn when_a_loaded_card_becomes_main_media_it_should_reuse_the_decoded_pixels() {
    let mut app = app_with(&[]);
    let url = "https://i.redd.it/ready.png";
    app.card_pictures_drawn(&[url]);
    app.on_msg(Msg::Image {
        url: url.into(),
        result: Ok(image::DynamicImage::new_rgb8(640, 320).into()),
    });

    app.request_image(url);

    assert!(
        !app.queued_images.contains_key(url),
        "loaded full pixels must not queue a cache decode or HTTP read"
    );
    assert!(matches!(
        app.images.get(url),
        Some(ImageSlot::Ready {
            pixels: (640, 320),
            ..
        })
    ));
}

#[test]
fn when_a_loading_text_picture_becomes_main_media_it_should_promote_the_shared_fetch() {
    let mut app = app_with(&[]);
    let url = "https://i.redd.it/shared.png";
    app.request_text_picture(url);
    assert_eq!(app.queued_images.get(url), Some(&true));

    app.request_image(url);

    assert_eq!(app.queued_images.get(url), Some(&false));
    assert!(matches!(app.images.get(url), Some(ImageSlot::Loading)));
}

#[test]
fn when_a_main_preview_refresh_is_skipped_after_returning_it_should_resume() {
    let mut app = app_with(&[]);
    let url = "https://i.redd.it/refresh.png";
    app.request_image(url);
    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Image {
            url: url.into(),
            result: Ok(image::DynamicImage::new_rgb8(10, 5).into()),
        }),
        fetched_at: now() - 8 * 86_400,
        refreshing: true,
        source: Source {
            cached: true,
            ..Source::default()
        },
    });

    app.on_msg(Msg::ImageSkipped { url: url.into() });

    assert_eq!(app.queued_images.get(url), Some(&false));
    assert!(matches!(
        app.images.get(url),
        Some(ImageSlot::Ready {
            refresh: ImageRefresh::Loading,
            ..
        })
    ));
    app.on_msg(Msg::Image {
        url: url.into(),
        result: Ok(image::DynamicImage::new_rgb8(20, 10).into()),
    });
    assert!(matches!(
        app.images.get(url),
        Some(ImageSlot::Ready {
            pixels: (20, 10),
            refresh: ImageRefresh::Idle,
            ..
        })
    ));
}

#[test]
fn when_a_card_returns_after_a_skipped_image_it_should_resume_the_fetch() {
    let mut app = app_with(&[]);
    let url = "https://i.redd.it/revisit.png";
    app.card_pictures_drawn(&[url]);
    app.card_pictures_drawn(&[]);
    app.card_pictures_drawn(&[url]);

    app.on_msg(Msg::ImageSkipped { url: url.into() });

    assert!(matches!(
        app.card_images.get(url),
        Some(CardImageSlot::Loading)
    ));
    assert!(app.queued_images.contains_key(url));
}

#[test]
fn navigating_back_through_twelve_loaded_images_should_not_queue_another_image_job() {
    let mut app = app_with(&[]);
    for i in 0..12 {
        let url = format!("https://i.redd.it/warm{i}.png");
        app.card_pictures_drawn(&[&url]);
        app.on_msg(Msg::Image {
            url,
            result: Ok(image::DynamicImage::new_rgb8(320, 640).into()),
        });
    }
    for i in (0..12).rev() {
        let url = format!("https://i.redd.it/warm{i}.png");
        app.card_pictures_drawn(&[&url]);
        assert!(
            matches!(app.card_images.get(&url), Some(CardImageSlot::Ready { .. })),
            "loaded {url} must retain decoded pixels"
        );
        assert!(
            !app.queued_images.contains_key(&url),
            "loaded {url} must not be requested again"
        );
    }
}

#[test]
fn when_recent_card_images_exceed_the_count_limit_the_oldest_pixels_should_be_evicted() {
    let mut app = app_with(&[]);
    for number in 0..66 {
        let url = format!("https://i.redd.it/{number}.png");
        app.card_pictures_drawn(&[&url]);
        app.on_msg(Msg::Image {
            url,
            result: Ok(image::DynamicImage::new_rgb8(1, 1).into()),
        });
    }

    app.card_pictures_drawn(&[]);

    assert_eq!(app.card_images.len(), 64);
    assert!(!app.card_images.contains_key("https://i.redd.it/0.png"));
    assert!(!app.card_images.contains_key("https://i.redd.it/1.png"));
    assert!(app.card_images.contains_key("https://i.redd.it/2.png"));
    assert_eq!(
        app.card_image_metadata
            .get("https://i.redd.it/0.png")
            .map(|metadata| metadata.size),
        Some((1, 1))
    );
}

#[test]
fn when_recent_card_pixels_exceed_the_byte_limit_the_oldest_image_should_be_evicted() {
    let mut app = app_with(&[]);
    let large = "https://i.redd.it/large.png";
    let small = "https://i.redd.it/small.png";
    app.card_pictures_drawn(&[large]);
    app.on_msg(Msg::Image {
        url: large.into(),
        result: Ok(image::DynamicImage::new_rgba8(4096, 4096).into()),
    });
    app.card_pictures_drawn(&[]);
    assert!(
        app.card_images.contains_key(large),
        "exactly 64 MiB may stay cached"
    );
    app.card_pictures_drawn(&[small]);
    app.on_msg(Msg::Image {
        url: small.into(),
        result: Ok(image::DynamicImage::new_rgb8(1, 1).into()),
    });

    app.card_pictures_drawn(&[]);

    assert!(!app.card_images.contains_key(large));
    assert!(app.card_images.contains_key(small));
    assert_eq!(
        app.card_image_metadata
            .get(large)
            .map(|metadata| metadata.size),
        Some((4096, 4096))
    );
}

#[test]
fn when_a_retained_preview_upgrade_is_skipped_it_should_resume_without_losing_pixels() {
    let mut app = app_with(&[]);
    let url = "https://i.redd.it/retained.png";
    app.card_pictures_drawn(&[url]);
    app.on_msg(Msg::Image {
        url: url.into(),
        result: Ok(image::DynamicImage::new_rgb8(640, 320).into()),
    });
    app.card_images.clear();
    app.card_pictures_drawn(&[url]);
    app.card_pictures_drawn(&[]);
    app.card_pictures_drawn(&[url]);

    app.on_msg(Msg::ImageSkipped { url: url.into() });

    assert!(matches!(
        app.card_images.get(url),
        Some(CardImageSlot::Cached {
            refreshing: true,
            ..
        })
    ));
    assert!(app.queued_images.contains_key(url));
    app.on_msg(Msg::Image {
        url: url.into(),
        result: Err("offline".into()),
    });
    assert!(matches!(
        app.card_images.get(url),
        Some(CardImageSlot::Cached {
            refreshing: false,
            ..
        })
    ));
    assert!(!app.queued_images.contains_key(url));
}

#[test]
fn when_retained_previews_exceed_the_limit_the_oldest_should_be_released() {
    let mut app = app_with(&[]);
    for number in 0..257 {
        let url = format!("https://i.redd.it/{number}.png");
        app.card_pictures_drawn(&[&url]);
        app.on_msg(Msg::Image {
            url,
            result: Ok(image::DynamicImage::new_rgb8(1, 1).into()),
        });
    }

    app.card_pictures_drawn(&["https://i.redd.it/0.png", "https://i.redd.it/1.png"]);

    assert_eq!(app.card_previews.len(), 256);
    assert_eq!(app.card_previews["https://i.redd.it/1.png"].width(), 1);
    assert_eq!(app.card_previews["https://i.redd.it/1.png"].height(), 1);
    assert!(matches!(
        app.card_images.get("https://i.redd.it/0.png"),
        Some(CardImageSlot::Loading)
    ));
    assert!(matches!(
        app.card_images.get("https://i.redd.it/1.png"),
        Some(CardImageSlot::Cached { .. })
    ));
}

#[test]
fn when_images_are_disabled_retained_card_previews_should_be_released() {
    let mut app = app_with(&[]);
    let url = "https://i.redd.it/off.png";
    app.card_pictures_drawn(&[url]);
    app.on_msg(Msg::Image {
        url: url.into(),
        result: Ok(image::DynamicImage::new_rgb8(10, 5).into()),
    });
    assert_eq!(app.card_previews.len(), 1);
    app.show_images = false;

    app.card_pictures_drawn(&[]);

    assert!(app.card_previews.is_empty());
    assert!(app.card_images.is_empty());
    assert!(app.card_preview_order.is_empty());
}

#[test]
fn when_a_shared_disk_preview_refresh_fails_both_consumers_should_keep_cached_pixels() {
    for background in [false, true] {
        let mut app = app_with(&[]);
        let url = "https://i.redd.it/stale-failure.png";
        app.request_image(url);
        app.card_pictures_drawn(&[url]);
        app.on_msg(Msg::Snapshot {
            message: Box::new(Msg::Image {
                url: url.into(),
                result: Ok(image::DynamicImage::new_rgb8(10, 5).into()),
            }),
            fetched_at: now() - 8 * 86_400,
            refreshing: true,
            source: Source {
                cached: true,
                ..Source::default()
            },
        });
        assert!(matches!(
            app.card_images.get(url),
            Some(CardImageSlot::Cached {
                refreshing: true,
                ..
            })
        ));
        app.on_msg(Msg::Image {
            url: url.into(),
            result: Err("offline".into()),
        });

        assert!(matches!(
            app.images.get(url),
            Some(ImageSlot::Ready {
                pixels: (10, 5),
                refresh: ImageRefresh::Failed,
                ..
            })
        ));
        assert!(matches!(
            app.card_images.get(url),
            Some(CardImageSlot::Cached {
                refreshing: false,
                ..
            })
        ));
        assert!(!app.queued_images.contains_key(url));

        app.request_text_picture(url);
        assert!(
            !app.queued_images.contains_key(url),
            "wait for explicit retry"
        );
        app.retry_failed_pictures();
        if background {
            app.request_text_picture(url);
        } else {
            app.request_image(url);
        }
        app.card_pictures_drawn(&[url]);
        assert_eq!(app.queued_images.get(url), Some(&background));
        assert!(matches!(
            app.images.get(url),
            Some(ImageSlot::Ready {
                pixels: (10, 5),
                refresh: ImageRefresh::Loading,
                ..
            })
        ));
        app.on_msg(Msg::Image {
            url: url.into(),
            result: Ok(image::DynamicImage::new_rgb8(20, 10).into()),
        });
        assert!(matches!(
            app.images.get(url),
            Some(ImageSlot::Ready {
                pixels: (20, 10),
                refresh: ImageRefresh::Idle,
                ..
            })
        ));
        assert!(matches!(
            app.card_images.get(url),
            Some(CardImageSlot::Ready { image, .. }) if image.width() == 20
        ));
        assert!(!app.queued_images.contains_key(url));
    }
}
