use super::*;

#[test]
fn when_a_loaded_video_is_deselected_its_frame_should_remain_visible() {
    let mut app = card_playback_app();
    app.show_sidebar = false;
    app.image_picker = ratatui_image::picker::Picker::halfblocks();
    for post in &mut app.tabs[0].feed.posts {
        post.preview_size = Some((160, 90));
    }
    app.card_images.insert(
        "https://i.redd.it/poster.jpg".into(),
        CardImageSlot::Failed("The image format could not be determined".into()),
    );
    let start = app.card_video.as_ref().unwrap().since + NAV_SETTLE;
    app.sync_video_with(start, |url, area, _| {
        let mut player = Player::showing(
            &url,
            video::Frame {
                width: 160,
                height: 90,
                pixels: video::Pixels::Raw(std::sync::Arc::new(image::DynamicImage::ImageRgb8(
                    image::RgbImage::from_pixel(160, 90, image::Rgb([180, 80, 50])),
                ))),
                same_as_first: false,
            },
        );
        player.area = area;
        player
    });
    let render = |app: &mut App| {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 60)).unwrap();
        loop {
            terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
            if !app.card_renderer.pending() {
                break;
            }
            app.card_renderer.wait_for_completion();
            app.card_renderer.poll();
        }
        terminal.backend().buffer().clone()
    };
    let has_frame = |buffer: &ratatui::buffer::Buffer| {
        buffer.content().iter().any(|cell| {
            cell.fg == ratatui::style::Color::Rgb(180, 80, 50)
                || cell.bg == ratatui::style::Color::Rgb(180, 80, 50)
        })
    };
    assert!(has_frame(&render(&mut app)), "selected frame must render");

    app.config.media.video_autoplay = false;
    app.move_selection(1);
    assert!(
        has_frame(&render(&mut app)),
        "deselection must retain the frame before the next playback tick"
    );
    app.sync_video_with(start, |_, _, _| {
        panic!("inactive previews must not start playback")
    });
    assert!(
        has_frame(&render(&mut app)),
        "the retired frame must replace the failed still preview"
    );
    assert!(app.video.is_none());

    if let Media::Video { url, .. } = &mut app.tabs[0].feed.posts[0].media {
        *url = Some("https://v.redd.it/changed.mp4".into());
    }
    assert!(
        !has_frame(&render(&mut app)),
        "a new source and another post must not borrow the old frame"
    );
}

#[test]
fn when_a_loaded_video_is_revisited_after_an_image_it_should_keep_its_player() {
    let mut app = card_playback_app();
    app.tabs[0].feed.posts[1].media = Media::None;
    let start = app.card_video.as_ref().unwrap().since + NAV_SETTLE;
    app.sync_video_with(start, |url, area, _| {
        let mut player = Player::showing(&url, playback_frame(1));
        player.area = area;
        player
    });
    app.tabs[0].feed.selected = 1;
    app.card_video_drawn(Some(Size::new(68, 68)));
    app.sync_video_with(start, |_, _, _| panic!("an image must not start a video"));
    assert!(
        app.selected_video().is_none(),
        "the previous video is hidden"
    );

    app.tabs[0].feed.selected = 0;
    app.card_video_drawn(Some(Size::new(68, 68)));
    let returned = app.card_video.as_ref().unwrap().since;
    app.sync_video_with(returned, |_, _, _| panic!("a warm return must not restart"));
    assert!(
        app.card_player().and_then(Player::frame).is_some(),
        "show the loaded frame immediately, even during selection settling"
    );
    app.sync_video_with(returned + NAV_SETTLE, |_, _, _| {
        panic!("the same decoded stream must resume")
    });
    assert!(!app.card_player().unwrap().hidden());
}

#[test]
fn reported_returning_between_loaded_videos_should_not_restart() {
    let mut app = card_playback_app();
    let start = app.card_video.as_ref().unwrap().since + NAV_SETTLE;
    app.sync_video_with(start, |url, area, _| {
        let mut player = Player::showing(&url, playback_frame(1));
        player.area = area;
        player
    });
    app.tabs[0].feed.selected = 1;
    app.card_video_drawn(Some(Size::new(68, 68)));
    let next = app.card_video.as_ref().unwrap().since + NAV_SETTLE;
    app.sync_video_with(next, |url, area, _| {
        let mut player = Player::showing(&url, playback_frame(2));
        player.area = area;
        player
    });
    app.tabs[0].feed.selected = 0;
    app.card_video_drawn(Some(Size::new(68, 68)));
    let returned = app.card_video.as_ref().unwrap().since + NAV_SETTLE;
    app.sync_video_with(returned, |_, _, _| {
        panic!("loaded video must resume without startup")
    });
    let frame = app
        .card_player()
        .and_then(Player::frame)
        .expect("retain the first video's frame while its decoder waits");
    let video::Pixels::Raw(pixels) = &frame.pixels else {
        panic!("decoded pixels")
    };
    assert_eq!(pixels.to_rgb8().get_pixel(0, 0).0, [1, 0, 0]);
}

#[test]
fn when_recent_video_frames_are_revisited_they_should_stay_bounded_and_match_the_source() {
    for manual in [false, true] {
        let mut app = card_playback_app();
        let template = app.tabs[0].feed.posts[0].clone();
        app.tabs[0].feed.posts = (0..10)
            .map(|index| {
                let mut post = template.clone();
                post.id = format!("video{index}");
                post
            })
            .collect();
        for index in 0..10 {
            app.tabs[0].feed.selected = index;
            app.card_video_drawn(Some(Size::new(68, 68)));
            let at = app.card_video.as_ref().unwrap().since + NAV_SETTLE;
            app.sync_video_with(at, |url, area, _| {
                let mut player = Player::showing(&url, playback_frame(index as u8 + 1));
                player.area = area;
                player
            });
        }
        assert!(
            app.video_previews
                .iter()
                .filter(|preview| preview.player.is_some())
                .count()
                <= VIDEO_PLAYER_MAX
        );
        assert!(
            app.video_previews
                .iter()
                .filter_map(|preview| preview.player.as_ref())
                .all(Player::hidden)
        );
        app.tabs[0].feed.selected = 1;
        app.card_video_drawn(Some(Size::new(68, 68)));
        let (frame, _) = app
            .video_preview(true)
            .expect("a recent frame appears before debounce");
        let video::Pixels::Raw(pixels) = &frame.pixels else {
            panic!("decoded pixels")
        };
        assert_eq!(pixels.to_rgb8().get_pixel(0, 0).0, [2, 0, 0]);
        if manual {
            app.toggle_video_pause();
        }
        let returned = app.card_video.as_ref().unwrap().since + NAV_SETTLE;
        app.sync_video_with(returned, |url, area, _| Player::silent(&url, area));
        assert!(
            app.card_player().and_then(Player::frame).is_some(),
            "retiring the outgoing player must not evict the destination"
        );
        app.tabs[0].feed.selected = 0;
        app.card_video_drawn(Some(Size::new(68, 68)));
        assert!(
            app.video_preview(true).is_none(),
            "the oldest of ten frames was evicted"
        );
        app.tabs[0].feed.selected = 1;
        app.tabs[0].feed.posts[1].media = Media::Video {
            gif: false,
            url: Some("https://v.redd.it/changed.mp4".into()),
            poster: None,
        };
        app.card_video_drawn(Some(Size::new(68, 68)));
        assert!(
            app.video_preview(true).is_none(),
            "a new source cannot display old pixels"
        );
        app.show_images = false;
        app.sync_video_with(Instant::now(), |_, _, _| panic!("images off"));
        assert!(app.video.is_none());
        assert!(app.video_previews.is_empty());
    }
}

#[test]
fn when_large_video_previews_exceed_the_byte_budget_the_oldest_should_expire() {
    let mut app = card_playback_app();
    let template = app.tabs[0].feed.posts[0].clone();
    app.tabs[0].feed.posts = (0..4)
        .map(|index| {
            let mut post = template.clone();
            post.id = format!("video{index}");
            post
        })
        .collect();
    for index in 0..4 {
        app.tabs[0].feed.selected = index;
        app.card_video_drawn(Some(Size::new(68, 68)));
        let at = app.card_video.as_ref().unwrap().since + NAV_SETTLE;
        app.sync_video_with(at, |url, area, _| {
            let frame = video::Frame {
                width: 2048,
                height: 2048,
                pixels: video::Pixels::Raw(std::sync::Arc::new(image::DynamicImage::new_rgba8(
                    2048, 2048,
                ))),
                same_as_first: false,
            };
            let mut player = Player::showing(&url, frame);
            player.area = area;
            player
        });
    }
    app.tabs[0].feed.selected = 0;
    app.card_video_drawn(Some(Size::new(68, 68)));
    assert!(
        app.video_preview(true).is_none(),
        "three16MiB previews cannot all remain cached"
    );
    app.tabs[0].feed.selected = 1;
    app.card_video_drawn(Some(Size::new(68, 68)));
    assert!(
        app.video_preview(true).is_some(),
        "keep newer previews within32MiB"
    );
    app.toggle_video_pause();
    let returned = app.card_video.as_ref().unwrap().since + NAV_SETTLE;
    app.sync_video_with(returned, |url, area, _| Player::silent(&url, area));
    assert!(
        app.card_player().and_then(Player::frame).is_some(),
        "manual play must keep the selected preview at the byte limit"
    );
}

#[test]
fn when_a_youtube_link_changes_alias_the_loaded_player_should_stay() {
    let mut app = card_playback_app();
    app.tabs[0].feed.posts[0].media = Media::Link {
        url: "https://youtu.be/abcdefghijk".into(),
        thumb: None,
    };
    app.card_video_drawn(Some(Size::new(68, 68)));
    let start = app.card_video.as_ref().unwrap().since + NAV_SETTLE;
    app.sync_video_with(start, |url, area, _| {
        let mut player = Player::showing(&url, playback_frame(1));
        player.area = area;
        player
    });
    app.tabs[0].feed.posts[0].media = Media::Link {
        url: "https://www.youtube.com/watch?v=abcdefghijk&feature=share".into(),
        thumb: None,
    };
    app.card_video_drawn(Some(Size::new(68, 68)));
    app.sync_video_with(Instant::now(), |_, _, _| {
        panic!("the same video must not restart")
    });
    assert!(app.card_player().and_then(Player::frame).is_some());
}

#[test]
fn when_no_card_remains_a_loaded_video_should_retire() {
    let mut app = card_playback_app();
    let start = app.card_video.as_ref().unwrap().since + NAV_SETTLE;
    app.sync_video_with(start, |url, area, _| {
        let mut player = Player::showing(&url, playback_frame(1));
        player.area = area;
        player
    });
    app.tabs[0].feed.posts.clear();
    app.card_video_drawn(None);
    app.sync_video_with(start, |_, _, _| {
        panic!("an empty view must not start video")
    });
    assert!(app.video.is_none(), "no card remains to resume the decoder");
    assert!(app.video_preview(true).is_none());
}

#[test]
fn card_media_open_detail_should_own_the_single_player() {
    let mut app = card_playback_app();
    let later = app.card_video.as_ref().unwrap().since + NAV_SETTLE;
    app.sync_video_with(later, |url, area, _| Player::silent(&url, area));
    app.toggle_video_pause();
    let post = app.tabs[0].feed.posts[0].clone();
    app.show_post(post.clone(), &post.permalink);
    app.post_view.as_mut().unwrap().page = Some(PostPage {
        post,
        body: String::new(),
        comments: vec![],
    })
    .into();
    app.video_area = Some(Rect::new(0, 0, 40, 12));
    app.card_video_drawn(Some(Size::new(68, 68)));
    let mut actual = None;

    app.sync_video_with(later, |url, area, resume| {
        actual = Some(resume.paused);
        Player::silent(&url, area)
    });

    assert_eq!(actual, Some(true));
    assert!(app.card_player().is_none());
}

#[test]
fn warm_return_controls_should_act_on_the_selected_video_before_settling() {
    for action in [0, 1, 2] {
        let mut app = card_playback_app();
        let first = app.card_video.as_ref().unwrap().since + NAV_SETTLE;
        app.sync_video_with(first, |url, area, _| {
            let mut player = Player::silent(&url, area);
            player.show(playback_frame(1));
            player
        });
        app.tabs[0].feed.selected = 1;
        app.card_video_drawn(Some(Size::new(68, 68)));
        let second = app.card_video.as_ref().unwrap().since + NAV_SETTLE;
        app.sync_video_with(second, |url, area, _| {
            let mut player = Player::silent(&url, area);
            player.show(playback_frame(2));
            player
        });
        app.tabs[0].feed.selected = 0;
        app.card_video_drawn(Some(Size::new(68, 68)));
        match action {
            0 => {
                app.toggle_video_pause();
                assert!(app.selected_video().unwrap().paused());
            }
            1 => {
                app.seek_video(true);
                assert!(app.selected_video().unwrap().position(Instant::now()) >= video::SEEK_STEP);
            }
            _ => {
                app.toggle_video_sound();
                assert!(app.video_sound_wanted());
            }
        }
        let returned = app.card_video.as_ref().unwrap().since + NAV_SETTLE;
        app.sync_video_with(returned, |_, _, _| {
            panic!("a control must not restart playback")
        });
        if action == 0 {
            assert!(app.selected_video().unwrap().paused());
        }
    }
}

#[test]
fn when_a_cached_frame_covers_pending_resolution_leaving_should_still_retire_it() {
    let mut app = card_playback_app();
    app.tabs[0].feed.posts[1].media = Media::None;
    let start = app.card_video.as_ref().unwrap().since + NAV_SETTLE;
    app.sync_video_with(start, |url, area, _| {
        let mut player = Player::pending(&url, area);
        player.retain_frame(Some(playback_frame(1)));
        player
    });
    assert!(app.card_player().unwrap().resolving());
    app.tabs[0].feed.selected = 1;
    app.card_video_drawn(Some(Size::new(68, 68)));
    app.sync_video_with(start, |_, _, _| panic!("an image cannot start video"));
    assert!(
        app.video.is_none(),
        "a retained preview must not keep obsolete extraction alive"
    );
}

#[test]
fn when_detail_returns_to_the_same_card_its_frame_should_cover_ownership_change() {
    let mut app = card_playback_app();
    let post = app.tabs[0].feed.posts[0].clone();
    app.show_post(post.clone(), &post.permalink);
    app.post_view.as_mut().unwrap().page = Some(PostPage {
        post,
        body: String::new(),
        comments: vec![],
    })
    .into();
    app.video_area = Some(Rect::new(0, 0, 68, 68));
    app.sync_video_with(Instant::now(), |url, area, _| {
        let mut player = Player::showing(&url, playback_frame(1));
        player.area = area;
        player
    });
    app.close_post();
    app.card_video_drawn(Some(Size::new(68, 68)));
    assert!(
        app.video_preview(true).is_some(),
        "the next draw can reuse detail pixels before card playback settles"
    );
}
