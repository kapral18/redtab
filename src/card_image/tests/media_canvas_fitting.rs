use super::*;

#[test]
fn a_height_fitted_media_canvas_should_show_both_ends_of_a_tall_source() {
    for video in [false, true] {
        let picker = picker();
        let image = Arc::new(DynamicImage::ImageRgba8(image::RgbaImage::from_fn(
            2,
            1600,
            |_, y| {
                if y < 800 {
                    image::Rgba([255, 0, 0, 255])
                } else {
                    image::Rgba([0, 0, 255, 255])
                }
            },
        )));
        let mut key = Key::new("portrait", &picker, Size::new(1, 10), (2, 1600), 0, 10, 0);
        key.video = video;
        let tile = Encoder::default()
            .encode(&Request {
                key,
                image,
                picker,
                buffer: false,
            })
            .unwrap();
        let actual = decoded(&tile);
        assert!(
            actual.pixels().any(|p| p.0 == [255, 0, 0, 255]),
            "top must remain visible"
        );
        assert!(
            actual.pixels().any(|p| p.0 == [0, 0, 255, 255]),
            "bottom must remain visible, video={video}"
        );
    }
}

#[test]
fn card_media_portrait_frames_should_fit_without_stretching_into_a_wide_reservation() {
    let picker = picker();
    let image = Arc::new(DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        20,
        40,
        image::Rgba([180, 50, 90, 255]),
    )));
    let mut key = Key::new(
        "portrait",
        &picker,
        Size::new(200, 100),
        (200, 100),
        0,
        100,
        0,
    );
    key.video = true;

    let encoded = Encoder::default()
        .encode(&Request {
            key,
            image,
            picker,
            buffer: false,
        })
        .unwrap();
    let actual = decoded(&encoded);

    assert_eq!(actual.get_pixel(25, 50).0, [17, 17, 27, 255]);
    assert_eq!(actual.get_pixel(100, 50).0, [180, 50, 90, 255]);
    assert_eq!(actual.get_pixel(175, 50).0, [17, 17, 27, 255]);
}
