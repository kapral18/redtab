use super::*;

#[test]
fn card_media_gallery_should_keep_the_middle_clear() {
    let actual = indicator_pixels(Some(Indicator::Gallery(4)), 0, 100);
    assert_eq!(actual.get_pixel(100, 50).0, [75, 130, 190, 255]);
    assert_eq!(actual.get_pixel(7, 5).0, [235, 237, 250, 255]);
}

#[test]
fn card_media_tiny_native_gif_should_keep_enough_pixels_for_its_type_badge() {
    #[allow(deprecated)]
    let mut picker = Picker::from_fontsize(ratatui_image::FontSize::new(8, 16));
    picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
    let image = Arc::new(DynamicImage::new_rgb8(16, 16));
    let mut key = Key::new("tiny-gif", &picker, Size::new(68, 34), (16, 16), 0, 16, 0);
    key.video = true;
    key.overlay = Some(Overlay {
        indicator: Indicator::Video {
            gif: true,
            play: false,
        },
        top: 0,
        rows: 16,
    });

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
    assert_eq!(frame.width, 80);
}

#[test]
fn card_media_video_should_have_a_centered_play_symbol_with_dark_backing() {
    let actual = indicator_pixels(
        Some(Indicator::Video {
            gif: false,
            play: true,
        }),
        0,
        100,
    );
    assert_eq!(actual.get_pixel(100, 50).0, [235, 237, 250, 255]);
    assert_eq!(actual.get_pixel(83, 50).0, [17, 17, 27, 255]);
    assert_eq!(actual.get_pixel(0, 50).0, [75, 130, 190, 255]);
}

#[test]
fn card_media_playing_video_should_keep_the_badge_and_remove_the_play_symbol() {
    let actual = indicator_pixels(
        Some(Indicator::Video {
            gif: false,
            play: false,
        }),
        0,
        100,
    );
    assert_eq!(actual.get_pixel(100, 50).0, [75, 130, 190, 255]);
    assert_eq!(actual.get_pixel(5, 5).0, [235, 237, 250, 255]);
}

#[test]
fn card_media_link_should_keep_its_center_clear_and_put_the_arrow_in_a_corner() {
    let actual = indicator_pixels(Some(Indicator::Link), 0, 100);
    assert_eq!(actual.get_pixel(100, 50).0, [75, 130, 190, 255]);
    assert!((180..200).any(|x| (0..20).any(|y| actual.get_pixel(x, y).0 != [75, 130, 190, 255])));
}

#[test]
fn card_media_scrolled_preview_should_keep_the_play_symbol_centered_in_the_visible_slice() {
    let actual = indicator_pixels(
        Some(Indicator::Video {
            gif: true,
            play: true,
        }),
        50,
        10,
    );
    assert_eq!(actual.dimensions(), (200, 10));
    assert_eq!(actual.get_pixel(100, 5).0, [235, 237, 250, 255]);
}

#[test]
fn card_media_plain_image_should_keep_its_original_pixels() {
    let actual = indicator_pixels(None, 0, 100);
    assert!(actual.pixels().all(|pixel| pixel.0 == [75, 130, 190, 255]));
}
