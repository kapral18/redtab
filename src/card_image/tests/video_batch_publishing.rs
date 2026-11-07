use super::*;

#[test]
fn video_strips_should_publish_together_after_the_last_completion() {
    let picker = picker();
    let red = Arc::new(DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        2,
        4,
        image::Rgba([255, 0, 0, 255]),
    )));
    let blue = Arc::new(DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        2,
        4,
        image::Rgba([0, 0, 255, 255]),
    )));
    let keys: Vec<_> = [0, 2]
        .into_iter()
        .map(|top| {
            let mut key = Key::new("video", &picker, Size::new(2, 4), (2, 4), top, 2, 0);
            key.video = true;
            key
        })
        .collect();
    let mut renderer = Renderer::default();
    renderer.started = true;
    let mut encoder = Encoder::default();
    for key in &keys {
        let done = Completed {
            key: key.clone(),
            image: Arc::downgrade(&red),
            source_size: (red.width(), red.height()),
            result: encoder
                .encode(&Request {
                    key: key.clone(),
                    image: Arc::clone(&red),
                    picker: picker.clone(),
                    buffer: false,
                })
                .map(Arc::new),
        };
        renderer.shared.work.lock().completed.push(done);
    }
    assert!(renderer.poll());

    // Stop at the real worker's between-strips state, with actual encoded pixels.
    let mut encoder = Encoder::default();
    let completion = |encoder: &mut Encoder, key: &Key| Completed {
        key: key.clone(),
        image: Arc::downgrade(&blue),
        source_size: (blue.width(), blue.height()),
        result: encoder
            .encode(&Request {
                key: key.clone(),
                image: Arc::clone(&blue),
                picker: picker.clone(),
                buffer: true,
            })
            .map(Arc::new),
    };
    {
        let mut work = renderer.shared.work.lock();
        work.wanted = vec![(keys[1].clone(), Arc::downgrade(&blue))];
        work.completed.push(completion(&mut encoder, &keys[0]));
    }
    assert!(
        !renderer.poll(),
        "one finished strip must not expose a partial frame"
    );
    renderer.begin();
    for key in &keys {
        let tile = renderer
            .tile(key.clone(), &blue, &picker, Preparation::Refresh)
            .unwrap()
            .unwrap();
        assert_eq!(pixel(&tile), image::Rgba([255, 0, 0, 255]));
    }
    renderer.finish();
    {
        let mut work = renderer.shared.work.lock();
        assert_eq!(
            work.wanted.len(),
            1,
            "do not requeue a finished but unpublished strip"
        );
        work.pending = None;
        work.wanted.clear();
        work.completed.push(completion(&mut encoder, &keys[1]));
    }
    assert!(renderer.poll());
    for key in keys {
        let tile = renderer
            .tile(key, &blue, &picker, Preparation::Hold)
            .unwrap()
            .unwrap();
        assert_eq!(pixel(&tile), image::Rgba([0, 0, 255, 255]));
    }
}

#[test]
fn large_video_frames_keep_the_current_picture_while_the_output_budget_recovers() {
    #[allow(deprecated)]
    let mut picker = Picker::from_fontsize(ratatui_image::FontSize::new(8, 16));
    picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
    let mut seed = 1u32;
    let image = Arc::new(DynamicImage::ImageRgb8(image::RgbImage::from_fn(
        960,
        540,
        |_, _| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            image::Rgb([(seed >> 16) as u8, (seed >> 8) as u8, seed as u8])
        },
    )));
    let mut key = Key::new("video", &picker, Size::new(120, 34), (960, 540), 0, 16, 0);
    key.video = true;
    let mut renderer = Renderer::default();
    renderer.begin();
    renderer.tile(key.clone(), &image, &picker, Preparation::Refresh);
    renderer.finish();
    complete(&mut renderer);
    let current = renderer
        .tile(key.clone(), &image, &picker, Preparation::Hold)
        .unwrap()
        .unwrap();
    let next = Arc::new(DynamicImage::new_rgb8(960, 540));

    renderer.begin();
    let actual = renderer
        .tile(key, &next, &picker, Preparation::Refresh)
        .unwrap()
        .unwrap();
    renderer.finish();

    assert!(
        Arc::ptr_eq(&current, &actual),
        "retain the displayed frame during backpressure"
    );
    assert!(
        !renderer.pending(),
        "do not encode and queue another large frame immediately"
    );
}

#[test]
fn video_frames_over_their_share_at_the_playing_rate_wait_for_the_output_budget() {
    #[allow(deprecated)]
    let mut picker = Picker::from_fontsize(ratatui_image::FontSize::new(8, 16));
    picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
    let mut seed = 7u32;
    let image = Arc::new(DynamicImage::ImageRgb8(image::RgbImage::from_fn(
        344,
        352,
        |_, _| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            image::Rgb([(seed >> 16) as u8, (seed >> 8) as u8, seed as u8])
        },
    )));
    let mut key = Key::new("video", &picker, Size::new(43, 22), (344, 352), 0, 22, 0);
    key.video = true;
    // Whether the frame after this one waits, at `fps`.
    let next_waits = |fps: f64| {
        let mut renderer = Renderer::default();
        renderer.set_video_rate(fps);
        renderer.begin();
        renderer.tile(key.clone(), &image, &picker, Preparation::Refresh);
        renderer.finish();
        complete(&mut renderer);
        let current = renderer
            .tile(key.clone(), &image, &picker, Preparation::Hold)
            .unwrap()
            .unwrap();
        let Tile::Kitty { upload, .. } = current.as_ref() else {
            panic!("a kitty video frame");
        };
        let share = |fps: f64| (VIDEO_BYTES_PER_SECOND / fps) as usize;
        assert!(
            (share(30.0)..share(24.0)).contains(&upload.len()),
            "a frame within a 24 fps share but over a 30 fps one: {}",
            upload.len()
        );
        renderer.begin();
        renderer.tile(
            key.clone(),
            &Arc::new(DynamicImage::new_rgb8(344, 352)),
            &picker,
            Preparation::Refresh,
        );
        renderer.finish();
        !renderer.pending()
    };

    assert!(next_waits(30.0), "30 a second would exceed the budget");
    assert!(!next_waits(24.0), "24 a second fit it: every frame shows");
}

#[test]
fn when_video_advances_during_encoding_the_inflight_picture_should_finish() {
    let picker = picker();
    let old = Arc::new(DynamicImage::new_rgb8(4, 4));
    let next = Arc::new(DynamicImage::new_rgb8(4, 4));
    let mut key = Key::new("video", &picker, Size::new(4, 4), (4, 4), 0, 4, 0);
    key.video = true;
    let mut renderer = Renderer::default();
    renderer.started = true;
    renderer.begin();
    renderer.tile(key.clone(), &old, &picker, Preparation::Refresh);
    renderer.finish();

    renderer.begin();
    renderer.tile(key.clone(), &next, &picker, Preparation::Refresh);
    renderer.finish();

    let work = renderer.shared.work.lock();
    assert!(Weak::ptr_eq(&work.wanted[0].1, &Arc::downgrade(&old)));
    assert!(Arc::ptr_eq(&work.pending.as_ref().unwrap()[0].image, &old));
}

#[test]
fn when_video_finishes_during_a_draw_the_completed_picture_should_still_publish() {
    let picker = picker();
    let old = Arc::new(DynamicImage::new_rgb8(4, 4));
    let next = Arc::new(DynamicImage::new_rgb8(4, 4));
    let mut key = Key::new("video", &picker, Size::new(4, 4), (4, 4), 0, 4, 0);
    key.video = true;
    let mut renderer = Renderer::default();
    renderer.started = true;
    renderer.begin();
    renderer.tile(key.clone(), &old, &picker, Preparation::Refresh);
    renderer.finish();
    renderer.begin();
    renderer.tile(key, &next, &picker, Preparation::Refresh);
    // The real worker completes after begin's poll and before finish.
    let request = renderer
        .shared
        .work
        .lock()
        .pending
        .take()
        .unwrap()
        .remove(0);
    let result = Encoder::default().encode(&request).map(Arc::new);
    {
        let mut work = renderer.shared.work.lock();
        work.wanted.clear();
        work.completed.push(Completed {
            key: request.key,
            image: Arc::downgrade(&old),
            source_size: (4, 4),
            result,
        });
    }

    renderer.finish();

    assert!(renderer.poll(), "publish the picture that just finished");
    assert!(!renderer.pending());
}

#[test]
fn superseding_a_partial_video_batch_should_discard_its_pixels_and_finish_the_new_viewport() {
    let picker = picker();
    let old = Arc::new(DynamicImage::new_rgb8(4, 6));
    let next = Arc::new(DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        4,
        6,
        image::Rgba([0, 0, 255, 255]),
    )));
    let key = |top| {
        let mut key = Key::new("video", &picker, Size::new(4, 6), (4, 6), top, 2, 0);
        key.video = true;
        key
    };
    let mut renderer = Renderer::default();
    renderer.started = true;
    renderer.begin();
    for top in [0, 2] {
        renderer.tile(key(top), &old, &picker, Preparation::Refresh);
    }
    renderer.finish();
    // Complete exactly one real encoding before the source and viewport change.
    let request = renderer
        .shared
        .work
        .lock()
        .pending
        .as_mut()
        .unwrap()
        .remove(0);
    let result = Encoder::default().encode(&request).map(Arc::new);
    {
        let mut work = renderer.shared.work.lock();
        work.wanted.remove(0);
        work.completed.push(Completed {
            key: request.key,
            image: Arc::downgrade(&old),
            source_size: (old.width(), old.height()),
            result,
        });
    }
    renderer.begin();
    for top in [2, 4] {
        renderer.tile(key(top), &next, &picker, Preparation::Refresh);
    }
    renderer.finish();
    assert!(renderer.shared.work.lock().completed.is_empty());
    let shared = Arc::clone(&renderer.shared);
    let worker = std::thread::spawn(move || prepare(shared, Encoder::default()));
    complete(&mut renderer);
    for top in [2, 4] {
        let tile = renderer
            .tile(key(top), &next, &picker, Preparation::Hold)
            .unwrap()
            .unwrap();
        assert_eq!(pixel(&tile), image::Rgba([0, 0, 255, 255]));
    }
    assert!(!renderer.pending());
    drop(renderer);
    worker.join().unwrap();
}

#[test]
fn when_viewports_change_quickly_only_the_latest_batch_should_remain_queued() {
    let picker = picker();
    let image = Arc::new(DynamicImage::new_rgb8(8, 8));
    let mut renderer = Renderer::default();
    for width in 1..=100 {
        renderer.begin();
        renderer.tile(
            Key::new("image", &picker, Size::new(width, 8), (8, 8), 0, 8, 0),
            &image,
            &picker,
            Preparation::Refresh,
        );
        renderer.finish();
    }
    complete(&mut renderer);

    renderer.begin();
    let current = renderer
        .tile(
            Key::new("image", &picker, Size::new(100, 8), (8, 8), 0, 8, 0),
            &image,
            &picker,
            Preparation::Refresh,
        )
        .unwrap()
        .unwrap();
    renderer.finish();

    assert_eq!(current.size(), Size::new(100, 8));
    assert!(!renderer.pending());
    assert!(
        renderer
            .shared
            .work
            .lock()
            .pending
            .as_ref()
            .is_none_or(|batch| batch.len() <= 1)
    );
}
