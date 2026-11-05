use super::*;

#[test]
fn when_offscreen_encodings_exceed_the_count_limit_the_oldest_should_be_released() {
    let picker = picker();
    let image = Arc::new(DynamicImage::new_rgb8(1, 1));
    let mut renderer = Renderer::default();
    renderer.begin();
    for number in 0..513 {
        renderer.tile(
            Key::new(
                &number.to_string(),
                &picker,
                Size::new(1, 1),
                (1, 1),
                0,
                1,
                0,
            ),
            &image,
            &picker,
            Preparation::Refresh,
        );
    }
    renderer.finish();
    complete(&mut renderer);
    assert_eq!(renderer.cache.len(), 513, "visible encodings are retained");

    renderer.begin();
    renderer.finish();

    assert_eq!(renderer.cache.len(), 512);
    assert!(!renderer.cache.contains_key(&Key::new(
        "0",
        &picker,
        Size::new(1, 1),
        (1, 1),
        0,
        1,
        0
    )));
}

#[test]
fn a_smaller_authoritative_refresh_should_replace_a_warm_encoding() {
    let picker = picker();
    let key = Key::new("same-url", &picker, Size::new(4, 4), (4, 4), 0, 4, 0);
    let old = Arc::new(DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        4,
        4,
        image::Rgba([255, 0, 0, 255]),
    )));
    let fresh = Arc::new(DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        2,
        2,
        image::Rgba([0, 0, 255, 255]),
    )));
    let mut renderer = Renderer::default();
    renderer.begin();
    renderer.tile(key.clone(), &old, &picker, Preparation::Refresh);
    renderer.finish();
    complete(&mut renderer);
    renderer.begin();
    renderer.tile(key.clone(), &fresh, &picker, Preparation::Refresh);
    renderer.finish();
    complete(&mut renderer);
    let shown = renderer
        .tile(key, &fresh, &picker, Preparation::Hold)
        .unwrap()
        .unwrap();
    assert_eq!(pixel(&shown), image::Rgba([0, 0, 255, 255]));
}

#[test]
fn when_new_pixels_are_preparing_the_previous_encoding_should_remain_visible() {
    let picker = picker();
    let key = Key::new("image", &picker, Size::new(2, 2), (2, 2), 0, 2, 0);
    let red = Arc::new(DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        2,
        2,
        image::Rgba([255, 0, 0, 255]),
    )));
    let blue = Arc::new(DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        2,
        2,
        image::Rgba([0, 0, 255, 255]),
    )));
    let mut renderer = Renderer::default();
    renderer.begin();
    assert!(
        renderer
            .tile(key.clone(), &red, &picker, Preparation::Refresh)
            .is_none()
    );
    renderer.finish();
    complete(&mut renderer);

    renderer.begin();
    let previous = renderer
        .tile(key.clone(), &blue, &picker, Preparation::Refresh)
        .unwrap()
        .unwrap();
    renderer.finish();

    assert_eq!(pixel(&previous), image::Rgba([255, 0, 0, 255]));
    complete(&mut renderer);
    renderer.begin();
    let current = renderer
        .tile(key, &blue, &picker, Preparation::Refresh)
        .unwrap()
        .unwrap();
    renderer.finish();
    assert_eq!(pixel(&current), image::Rgba([0, 0, 255, 255]));
}

#[test]
fn a_retained_thumbnail_should_not_replace_a_warm_full_resolution_encoding() {
    let picker = picker();
    let key = Key::new("same-url", &picker, Size::new(4, 4), (4, 4), 0, 4, 0);
    let full = Arc::new(DynamicImage::new_rgb8(4, 4));
    let preview = Arc::new(DynamicImage::new_rgb8(2, 2));
    let mut renderer = Renderer::default();
    renderer.begin();
    renderer.tile(key.clone(), &full, &picker, Preparation::Refresh);
    renderer.finish();
    complete(&mut renderer);
    renderer.begin();
    let old = renderer
        .tile(key.clone(), &full, &picker, Preparation::Hold)
        .unwrap()
        .unwrap();
    renderer.finish();
    drop(full);
    renderer.begin();
    let shown = renderer
        .tile(key, &preview, &picker, Preparation::Missing)
        .unwrap()
        .unwrap();
    renderer.finish();
    assert!(Arc::ptr_eq(&old, &shown));
    assert!(
        !renderer.pending(),
        "a cached full image must not be re-encoded as its thumbnail"
    );
}

#[test]
fn when_images_are_cleared_queued_work_and_encodings_should_be_released() {
    let picker = picker();
    let image = Arc::new(DynamicImage::new_rgb8(8, 8));
    let mut renderer = Renderer::default();
    renderer.begin();
    renderer.tile(
        Key::new("image", &picker, Size::new(8, 8), (8, 8), 0, 8, 0),
        &image,
        &picker,
        Preparation::Refresh,
    );
    renderer.finish();

    renderer.clear();
    renderer.poll();

    assert!(!renderer.pending());
    assert!(renderer.cache.is_empty());
    assert!(renderer.shared.work.lock().wanted.is_empty());
}

#[test]
fn when_images_are_cleared_the_idle_workers_downscaled_pixels_should_be_released() {
    let picker = picker();
    let image = Arc::new(DynamicImage::new_rgb8(16, 16));
    let mut encoder = Encoder::default();
    encoder
        .encode(&Request {
            key: Key::new("image", &picker, Size::new(2, 2), (16, 16), 0, 2, 0),
            image,
            picker,
            buffer: false,
        })
        .unwrap();
    let pixels = Arc::downgrade(&encoder.downscaled.as_ref().unwrap().2);
    let mut renderer = Renderer::default();
    let shared = Arc::clone(&renderer.shared);
    let worker = std::thread::spawn(move || prepare(shared, encoder));

    renderer.clear();
    {
        let mut work = renderer.shared.work.lock();
        while work.clear_encoder {
            assert!(
                !renderer
                    .shared
                    .changed
                    .wait_for(&mut work, std::time::Duration::from_secs(5))
                    .timed_out(),
                "idle renderer did not process clear"
            );
        }
    }

    assert!(
        pixels.upgrade().is_none(),
        "images-off must release derived pixels while idle"
    );
    drop(renderer);
    worker.join().unwrap();
}
