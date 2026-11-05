use super::*;

#[test]
fn card_media_kitty_video_should_transmit_decoded_resolution_and_scale_in_the_terminal() {
    let actual = kitty_video_tile();
    let Tile::Kitty { frame, size, .. } = actual else {
        panic!("native video tile");
    };
    assert_eq!((frame.width, frame.height), (160, 76));
    assert_eq!(size, Size::new(68, 16));
}

#[test]
fn card_media_kitty_video_should_transmit_a_frame_only_once_with_a_stable_id() {
    let actual = kitty_video_tile();
    let next = kitty_video_tile();
    let area = Rect::new(0, 0, 68, 16);
    let mut first = ratatui::buffer::Buffer::empty(area);
    let mut second = ratatui::buffer::Buffer::empty(area);
    (&actual).render(area, &mut first);
    (&actual).render(area, &mut second);
    assert!(first[(0, 0)].symbol().contains("_G"));
    assert!(!second[(0, 0)].symbol().contains("_G"));
    let (Tile::Kitty { id: first, .. }, Tile::Kitty { id: second, .. }) = (actual, next) else {
        panic!("native tiles");
    };
    assert_eq!(first, second);
}

#[test]
fn kitty_video_should_upload_to_a_hidden_buffer_before_reusing_the_previous_one() {
    #[allow(deprecated)]
    let mut picker = Picker::from_fontsize(ratatui_image::FontSize::new(1, 1));
    picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
    let sources: Vec<_> = (0..3)
        .map(|_| Arc::new(DynamicImage::new_rgb8(4, 4)))
        .collect();
    let mut renderer = Renderer::default();
    let mut ids = Vec::new();
    for image in sources {
        renderer.begin();
        let keys: Vec<_> = [0, 2]
            .into_iter()
            .map(|top| {
                let mut key = Key::new("video", &picker, Size::new(4, 4), (4, 4), top, 2, 0);
                key.video = true;
                renderer.tile(key.clone(), &image, &picker, Preparation::Refresh);
                key
            })
            .collect();
        renderer.finish();
        complete(&mut renderer);
        let frame_ids: Vec<_> = keys
            .into_iter()
            .map(|key| {
                let tile = renderer
                    .tile(key, &image, &picker, Preparation::Hold)
                    .unwrap()
                    .unwrap();
                let Tile::Kitty { id, .. } = tile.as_ref() else {
                    panic!("native video");
                };
                *id
            })
            .collect();
        ids.push(frame_ids);
    }
    assert!(
        ids[0].iter().all(|id| !ids[1].contains(id)),
        "uploads must not replace visible strips"
    );
    assert_eq!(ids[0], ids[2], "two buffers bound native image storage");
}

#[test]
fn scrolling_video_should_keep_each_visible_strips_upload_buffer_hidden() {
    #[allow(deprecated)]
    let mut picker = Picker::from_fontsize(ratatui_image::FontSize::new(1, 1));
    picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
    let old = Arc::new(DynamicImage::new_rgb8(4, 6));
    let next = Arc::new(DynamicImage::new_rgb8(4, 6));
    let mut renderer = Renderer::default();
    let mut previous = HashMap::new();
    for (image, tops) in [(&old, [0, 2]), (&old, [2, 4]), (&next, [2, 4])] {
        renderer.begin();
        let keys: Vec<_> = tops
            .into_iter()
            .map(|top| {
                let mut key = Key::new("video", &picker, Size::new(4, 6), (4, 6), top, 2, 0);
                key.video = true;
                renderer.tile(key.clone(), image, &picker, Preparation::Refresh);
                key
            })
            .collect();
        renderer.finish();
        complete(&mut renderer);
        for key in keys {
            let tile = renderer
                .tile(key.clone(), image, &picker, Preparation::Hold)
                .unwrap()
                .unwrap();
            let Tile::Kitty { id, .. } = tile.as_ref() else {
                panic!("native video");
            };
            if Arc::ptr_eq(image, &next) {
                assert_ne!(
                    previous[&key.start], *id,
                    "a retained strip must not be overwritten during upload"
                );
            }
            previous.insert(key.start, *id);
        }
    }
}
