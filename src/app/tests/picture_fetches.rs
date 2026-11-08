use super::*;

#[test]
fn a_shared_picture_should_reach_card_and_detail_and_survive_the_detail_closing() {
    let mut app = app_with(&[]);
    let url = "https://i.redd.it/shared.png";
    app.card_pictures_drawn(&[url]);
    app.request_text_picture(url);

    app.on_msg(Msg::Image {
        url: url.into(),
        result: Ok(image::DynamicImage::new_rgb8(640, 320).into()),
    });

    assert!(matches!(
        app.images.get(url),
        Some(ImageSlot::Ready {
            pixels: (640, 320),
            ..
        })
    ));
    assert!(matches!(
        app.card_images.get(url),
        Some(CardImageSlot::Ready { image, .. }) if (image.width(), image.height()) == (640, 320)
    ));
    assert!(!app.queued_images.contains_key(url));

    app.close_post();

    assert!(app.images.is_empty());
    assert!(matches!(
        app.card_images.get(url),
        Some(CardImageSlot::Ready { .. })
    ));
}

#[test]
fn a_second_fetch_of_a_loaded_picture_should_not_replace_it() {
    let mut app = app_with(&[]);
    let url = "https://preview.redd.it/twice.png";
    app.request_text_picture(url);
    let image = |side| Ok(image::DynamicImage::new_rgb8(side, side).into());

    app.on_msg(Msg::Image {
        url: url.into(),
        result: image(10),
    });
    app.on_msg(Msg::Image {
        url: url.into(),
        result: image(20),
    });

    assert!(matches!(
        app.images.get(url),
        Some(ImageSlot::Ready {
            pixels: (10, 10),
            ..
        })
    ));
}

#[test]
fn scroll_pictures_wait_from_the_first_movement_until_it_stops() {
    let mut app = app_with(&[]);
    let (thumb, picture) = (
        "https://i.redd.it/row.png",
        "https://preview.redd.it/text.png",
    );
    app.move_selection(1);
    app.thumbs_drawn(&[thumb]);
    app.request_text_picture(picture);

    assert!(!app.thumbs.contains_key(thumb), "mid-run rows wait");
    assert!(!app.images.contains_key(picture), "mid-run pictures wait");
    assert!(
        app.tick_at(std::time::Instant::now() + NAV_SETTLE),
        "the pause redraws"
    );
    app.thumbs_drawn(&[thumb]);
    app.request_text_picture(picture);
    assert!(app.thumbs.contains_key(thumb));
    assert!(app.images.contains_key(picture));
}

#[test]
fn stale_thumbnail_refresh_survives_eviction_and_revisit_until_terminal_delivery() {
    for terminal in 0..3 {
        let mut app = app_with(&[]);
        let url = "/image.png";
        app.thumbs_drawn(&[url]);
        app.on_msg(Msg::Snapshot {
            message: Box::new(Msg::Thumb {
                url: url.into(),
                result: Ok(image::DynamicImage::ImageRgba8(
                    image::ImageBuffer::from_pixel(2, 2, image::Rgba([255, 0, 0, 255])),
                )),
            }),
            fetched_at: now(),
            refreshing: true,
            source: Source {
                cached: true,
                ..Source::default()
            },
        });
        for i in 0..THUMB_CACHE_MAX {
            app.thumbs
                .insert(format!("/old/{i}"), ImageSlot::Failed("unused".into()));
        }
        app.thumbs_drawn(&[]);
        assert!(
            matches!(app.thumbs.get(url), Some(ImageSlot::Ready { .. })),
            "the offscreen cached preview must survive while HTTP is outstanding"
        );
        app.thumbs_drawn(&[url]);
        assert!(
            matches!(app.thumbs.get(url), Some(ImageSlot::Ready { .. })),
            "revisiting must reuse the preview, not replace it with another Loading request"
        );
        app.on_msg(match terminal {
            0 => Msg::Thumb {
                url: url.into(),
                result: Ok(image::DynamicImage::new_rgb8(2, 2)),
            },
            1 => Msg::Thumb {
                url: url.into(),
                result: Err("offline".into()),
            },
            _ => Msg::ThumbSkipped { url: url.into() },
        });
        assert!(matches!(app.thumbs.get(url), Some(ImageSlot::Ready { .. })));
        for i in 0..THUMB_CACHE_MAX {
            app.thumbs
                .insert(format!("/old/{i}"), ImageSlot::Failed("unused".into()));
        }
        app.thumbs_drawn(&[]);
        assert!(
            !app.thumbs.contains_key(url),
            "terminal delivery must release the eviction pin"
        );
    }
}

#[test]
fn cached_thumbnail_retries_keep_pixels_after_failure_or_skipped_work() {
    for retry in ["reload", "revisit"] {
        let mut app = app_with(&[]);
        let url = "/image.png";
        app.thumbs_drawn(&[url]);
        app.on_msg(Msg::Snapshot {
            message: Box::new(Msg::Thumb {
                url: url.into(),
                result: Ok(image::DynamicImage::ImageRgba8(
                    image::ImageBuffer::from_pixel(2, 2, image::Rgba([255, 0, 0, 255])),
                )),
            }),
            fetched_at: now(),
            refreshing: true,
            source: Source {
                cached: true,
                ..Source::default()
            },
        });
        let render = |app: &mut App| {
            let Some(ImageSlot::Ready { protocol, .. }) = app.thumbs.get_mut(url) else {
                panic!("cached pixels must remain usable");
            };
            let area = ratatui::layout::Rect::new(0, 0, 2, 1);
            let mut buffer = ratatui::buffer::Buffer::empty(area);
            ratatui::widgets::StatefulWidget::render(
                ratatui_image::StatefulImage::default(),
                area,
                &mut buffer,
                protocol.as_mut(),
            );
            buffer
        };
        let cached_pixels = render(&mut app);
        app.on_msg(if retry == "revisit" {
            Msg::ThumbSkipped { url: url.into() }
        } else {
            Msg::Thumb {
                url: url.into(),
                result: Err("offline".into()),
            }
        });
        app.thumbs_drawn(&[]);
        if retry == "reload" {
            app.reload();
        }
        app.thumbs_drawn(&[url]);
        assert!(
            matches!(
                app.thumbs.get(url),
                Some(ImageSlot::Ready {
                    refresh: ImageRefresh::Loading,
                    ..
                })
            ),
            "{retry} must restart the refresh without losing usable pixels"
        );
        assert_eq!(
            render(&mut app),
            cached_pixels,
            "{retry} must keep rendering the same cached image"
        );
    }
}

#[test]
fn when_a_list_picture_arrives_it_should_be_cut_to_the_slots_shape_around_its_middle() {
    // 8×16 px cells: a 4×2-cell slot is 32×32 px, square.
    let font = ratatui_image::FontSize::new(8, 16);
    // A wide picture whose middle third is red: the crop keeps exactly that.
    let wide = image::RgbImage::from_fn(300, 100, |x, _| {
        if (100..200).contains(&x) {
            image::Rgb([255, 0, 0])
        } else {
            image::Rgb([0, 0, 255])
        }
    });

    let cut = fill_slot(image::DynamicImage::ImageRgb8(wide), font).to_rgb8();
    let tall = fill_slot(image::DynamicImage::new_rgb8(100, 300), font);

    assert_eq!(cut.dimensions(), (100, 100));
    assert!(
        cut.pixels().all(|p| p.0 == [255, 0, 0]),
        "the middle, not an edge"
    );
    assert_eq!((tall.width(), tall.height()), (100, 100));
}

#[test]
fn a_picture_asked_for_again_while_still_queued_should_be_fetched_once() {
    let (mut app, messages) = app_with_messages(&[]);
    let url = "https://preview.redd.it/once.png";
    app.request_text_picture(url);
    app.drop_text_picture(url);
    app.request_text_picture(url);

    // Wait for the first answer, then briefly for a second one.
    let mut fetched = 0;
    let mut wait = std::time::Duration::from_secs(5);
    while let Ok(msg) = messages.recv_timeout(wait) {
        if matches!(&msg, Msg::Image { url: u, .. } if u == url) {
            fetched += 1;
            wait = std::time::Duration::from_millis(200);
        }
        app.on_msg(msg);
    }

    assert_eq!(fetched, 1);
}

#[test]
fn revisiting_a_failed_card_should_wait_for_explicit_retry() {
    let mut app = app_with(&[]);
    let url = "https://i.redd.it/failed.png";
    app.card_pictures_drawn(&[url]);
    app.on_msg(Msg::Image {
        url: url.into(),
        result: Err("offline".into()),
    });
    app.card_pictures_drawn(&[]);
    app.card_pictures_drawn(&[url]);
    assert!(matches!(
        app.card_images.get(url),
        Some(CardImageSlot::Failed(_))
    ));
    assert!(!app.queued_images.contains_key(url));
    app.retry_failed_pictures();
    app.card_pictures_drawn(&[url]);
    assert!(matches!(
        app.card_images.get(url),
        Some(CardImageSlot::Loading)
    ));
    assert!(app.queued_images.contains_key(url));
}
