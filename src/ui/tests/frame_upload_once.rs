use super::*;

#[test]
fn when_a_frame_shrinks_the_drawn_video_the_decoder_should_not_restart() {
    let mut app = open_video_post();
    app.post_view
        .as_mut()
        .unwrap()
        .page
        .as_mut()
        .unwrap()
        .post
        .preview_size = Some((1600, 900));
    let step = |app: &mut App| {
        screen(app, 100, 30);
        app.tick();
    };
    step(&mut app);
    let (url, decoding) = {
        let player = app.video.as_ref().unwrap();
        (player.url.clone(), player.area)
    };
    // A very wide frame takes far fewer rows than the box it was decoded for.
    let frame = video::Frame {
        width: 160,
        height: 9,
        pixels: video::Pixels::Raw(std::sync::Arc::new(image::DynamicImage::new_rgb8(160, 9))),
        same_as_first: true,
    };
    let mut player = video::Player::showing(&url, frame);
    player.area = decoding;
    app.video = Some(player);

    step(&mut app);
    std::thread::sleep(std::time::Duration::from_millis(300));
    step(&mut app);

    let player = app.video.as_ref().unwrap();
    assert!(
        player.frame().is_some(),
        "the same player, not a restarted one"
    );
    assert_eq!(player.area, decoding);
}

#[test]
fn a_video_frame_goes_to_the_terminal_once() {
    let mut app = open_video_post();
    let frame = || video::Frame {
        width: 16,
        height: 9,
        pixels: video::Pixels::Raw(std::sync::Arc::new(image::DynamicImage::new_rgb8(16, 9))),
        same_as_first: false,
    };
    let url = app.reddit.media_url(VIDEO).unwrap();
    app.video = Some(video::Player::showing(&url, frame()));
    let sends = |app: &mut App| screen(app, 100, 40).matches("_Ga=T").count();

    assert_eq!(sends(&mut app), 2);
    assert_eq!(sends(&mut app), 0, "a redraw repeats the cells only");
    app.video.as_mut().unwrap().show(frame());
    assert_eq!(
        sends(&mut app),
        2,
        "the next frame sends both visible tiles"
    );
}

#[test]
fn a_picture_shown_twice_should_go_to_the_terminal_once_not_every_frame() {
    let url = "https://preview.redd.it/c.png";
    let mut app = picture_comment(url);
    let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
    let mut answer = reply("r".into());
    answer.body = format!("{PICTURE}{url}");
    page.comments[0].replies.push(answer);
    // Wide: each place would size it by its own column.
    let slot = ready(&app, 2000, 100);
    app.images.insert(url.into(), slot);
    let sends = |app: &mut App| screen(app, 100, 40).matches("_Gq=2").count();

    sends(&mut app);

    assert_eq!(sends(&mut app), 0, "a redraw repeats only the cells");
}
