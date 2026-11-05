use super::*;

#[test]
fn rgb_video_sampling_should_match_the_generic_rgba_sampler() {
    let rgb = image::RgbImage::from_fn(7, 13, |x, y| {
        image::Rgb([
            (x * 57 + y * 13) as u8,
            (x * 11 + y * 41) as u8,
            (x * 31 + y * 7) as u8,
        ])
    });
    let source = DynamicImage::ImageRgb8(rgb);
    let reference = DynamicImage::ImageRgba8(source.to_rgba8());
    #[allow(deprecated)]
    let picker = Picker::from_fontsize(ratatui_image::FontSize::new(3, 5));
    for scale in [1.0, 0.63] {
        for top in [0, 2, 5] {
            let mut key = Key::new("video", &picker, Size::new(9, 7), (7, 13), top, 2, 0);
            key.video = true;
            key.left = 2;
            key.cols = 7;
            assert_eq!(
                tile_pixels(&source, None, &key, scale),
                tile_pixels(&reference, None, &key, scale)
            );
        }
    }
}

#[test]
fn video_width_regression_native_tiles_should_retain_display_resolution() {
    #[allow(deprecated)]
    let mut picker = Picker::from_fontsize(ratatui_image::FontSize::new(8, 16));
    picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
    let image = Arc::new(DynamicImage::new_rgb8(960, 540));
    let mut key = Key::new("video", &picker, Size::new(120, 34), (960, 540), 0, 16, 0);
    key.video = true;

    let actual = Encoder::default()
        .encode(&Request {
            key,
            image,
            picker,
            buffer: false,
        })
        .unwrap();

    let Tile::Kitty { frame, .. } = actual else {
        panic!("native video tile");
    };
    assert_eq!((frame.width, frame.height), (960, 256));
}
