use super::*;

#[test]
fn when_stream_headers_are_forwarded_they_should_precede_the_input_as_http_header_lines() {
    let actual = header_args(&[
        ("User-Agent".into(), "fixture-agent".into()),
        ("Referer".into(), "https://www.youtube.com/".into()),
    ]);

    assert_eq!(
        actual,
        [
            "-headers",
            "User-Agent: fixture-agent\r\nReferer: https://www.youtube.com/\r\n"
        ]
    );
}

#[test]
fn the_frame_rate_is_the_one_ffmpeg_outputs_not_the_sources() {
    let log = Mutex::new(Log::default());
    let stderr = b"[info]   Metadata:\n\
[info]     title           : Output #0\n\
[info]     Output #0       : x\n\
[info]   Stream #0:0(und): Video: h264 (Main), yuv420p, 1280x720, 60 fps, 60 tbr, 15360 tbn\n\
[info] Stream mapping:\n\
[info] Output #0, image2pipe, to 'pipe:':\n\
[info]   Metadata:\n\
[info]     title           : clip: Video: x, 2 fps\n\
[info]     Stream #0:0: Video: x, 24 fps, y: v\n\
[info]     k\n\
\x20 Stream #0:0: Video: x, 24 fps, y: v\n\
[info]     comment         : Output #0, Stream #0:9: Video: x, 0 fps\n\
[info]   Stream #0:0(und): Video: ppm, rgb24(pc, gbr/unknown/unknown, progressive), 64x36, q=2-31, 200 kb/s, 29.97 fps, 29.97 tbn\n";

    read_log(&stderr[..], &log);

    assert_eq!(log.lock().rate, Some(29.97));

    // Without one, frames must not wait for a rate that never comes.
    let log = Mutex::new(Log::default());
    read_log(
            &b"[info] Output #0, image2pipe, to 'pipe:':\n[info]   Stream #0:0: Video: ppm, rgb24, 64x36\n"[..],
            &log,
        );
    assert_eq!(log.lock().rate, Some(MIN_FPS));
}

#[test]
fn source_rate_expressions_should_need_an_fps_filter_taking_a_string() {
    let before_5_0 = "fps AVOptions:\n   fps               <video_rate> ..FV....... A string describing desired output framerate (default \"25\")\n";
    let since_5_0 = "fps AVOptions:\n   fps               <string>     ..FV....... A string describing desired output framerate (default \"25\")\n   start_time        <double>     ..FV....... Assume the first PTS should be this value.\n";

    assert!(!supports_fps_expressions(before_5_0));
    assert!(supports_fps_expressions(since_5_0));
}

#[test]
fn video_decodes_at_the_sources_own_rate_up_to_30_fps() {
    let Some(fast) = decoded_second(60) else {
        return;
    };
    let pal = decoded_second(25).unwrap();

    if *FPS_EXPRESSIONS {
        assert_eq!(fast, (30, 30.0), "60 fps halves evenly");
        assert_eq!(pal, (25, 25.0), "25 fps keeps every frame");
    } else {
        assert_eq!(fast, (24, MIN_FPS));
        assert_eq!(pal, (24, MIN_FPS));
    }
}

#[test]
fn when_ffmpeg_logs_a_bad_frame_it_should_not_count_as_a_cut_input() {
    // Lines ffmpeg 9 printed for a GIF with a corrupt frame, read to its end.
    let stderr = b"[gif @ 0x7950854e00] [error] LZW decode failed\n\
[vist#0:0/gif @ 0x1] [dec:gif @ 0x2] [error] Error submitting packet to decoder: Invalid data\n";
    let log = Mutex::new(Log::default());

    read_log(&stderr[..], &log);

    let log = log.lock();
    assert_eq!(
        log.error.as_deref(),
        Some("Error submitting packet to decoder: Invalid data"),
        "nested components are read through"
    );
    assert!(!log.input_error);
}

#[test]
fn video_frames_should_keep_source_detail_up_to_the_display_size() {
    let decoded = |w, h| first_frame(w, h, (856, 1170));
    if decoded(2, 2).is_none() {
        return;
    }

    assert_eq!(decoded(1280, 720), Some((856, 481)));
    assert_eq!(decoded(540, 674), Some((540, 674)));
    assert_eq!(decoded(900, 900), Some((856, 856)));
    // Smaller than that: as it is; the terminal enlarges it.
    assert_eq!(decoded(100, 80), Some((100, 80)));
}

#[test]
fn a_still_preview_returns_the_first_frame_without_starting_playback() {
    if std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_err()
    {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("preview.gif");
    let mut file = std::fs::File::create(&path).unwrap();
    let mut encoder = gif::Encoder::new(&mut file, 4, 2, &[90, 120, 150]).unwrap();
    encoder
        .write_frame(&gif::Frame::from_palette_pixels(
            4,
            2,
            vec![0; 8],
            [90, 120, 150],
            None,
        ))
        .unwrap();
    drop(encoder);
    drop(file);

    let image =
        first_frame_preview(url::Url::from_file_path(&path).unwrap().as_str(), || false).unwrap();

    assert_eq!((image.width(), image.height()), (4, 2));
    assert_eq!(image.to_rgb8().get_pixel(0, 0).0, [90, 120, 150]);
    assert!(
        first_frame_preview(url::Url::from_file_path(&path).unwrap().as_str(), || true).is_err()
    );
}

#[test]
fn video_frames_should_keep_portrait_width_for_scrolling() {
    let Some(actual) = first_frame(1080, 1920, (608, 1088)) else {
        return;
    };
    assert_eq!(actual, (608, 1080));
}

#[test]
fn video_decode_box_should_follow_full_document_height() {
    assert_eq!(
        frame_box(Size::new(100, 100), FontSize::new(16, 36)),
        (1280, 3600)
    );
}

#[test]
fn ranged_requests_should_need_ffmpeg_http_request_size() {
    let before_8_1 = "HTTP AVOptions:\n  -multiple_requests <boolean>    ED......... use persistent connections (default false)\n";
    let since_8_1 = "HTTP AVOptions:\n  -request_size      <int64>      .D......... size (in bytes) of requests to make (from 0 to I64_MAX) (default 0)\n  -initial_request_size <int64>      .D......... size (in bytes) of initial requests\n";
    let initial_only =
        "  -initial_request_size <int64>      .D......... size of initial requests\n";

    assert!(!supports_ranged(before_8_1));
    assert!(supports_ranged(since_8_1));
    assert!(!supports_ranged(initial_only));
}

#[test]
fn when_a_plain_gif_plays_it_should_stream_without_seeking_its_input() {
    let gif = "http://127.0.0.1:1/k/https/i.redd.it/a.gif";
    let mp4 = "http://127.0.0.1:1/k/https/preview.redd.it/a.gif?format=mp4&s=x";

    let args = |url: &str, at: f64| input_args(url, at).join(" ");

    assert_eq!(
        args(gif, 0.0),
        format!("-rw_timeout 10000000 -seekable 0 -i {gif}")
    );
    assert_eq!(
        args(gif, 5.0),
        format!("-rw_timeout 10000000 -seekable 0 -i {gif} -ss 5.000"),
        "a later start decodes past earlier frames"
    );
    assert_eq!(
        args(mp4, 5.0),
        format!("-rw_timeout 10000000 -reconnect 1 -ss 5.000 -i {mp4}"),
        "an MP4 behind a .gif name still seeks"
    );
    let youtube = "https://rr1---sn-x.googlevideo.com/videoplayback?itag=251";
    let ranged = if *RANGED_REQUESTS {
        " -request_size 1048576 -multiple_requests 1"
    } else {
        ""
    };
    assert_eq!(
        args(youtube, 5.0),
        format!(
            "-rw_timeout 10000000 -probesize 32768{ranged} -reconnect 1 -ss 5.000 -i {youtube}"
        ),
        "googlevideo paces one open-ended response near playback speed"
    );
    let lookalike = "https://googlevideo.com.example/videoplayback";
    assert_eq!(
        args(lookalike, 5.0),
        format!("-rw_timeout 10000000 -reconnect 1 -ss 5.000 -i {lookalike}")
    );
}

#[test]
fn a_run_moves_once_a_frame_differs_from_its_first() {
    let mut first = FirstFrame::default();
    let mut same = |rgb: &[u8]| {
        let same = first.same(rgb);
        first.keep(rgb.to_vec());
        same
    };

    let actual = [
        same(&[1, 2, 3]),
        same(&[1, 2, 3]),
        same(&[1, 2]),
        same(&[1, 2, 3]),
    ];

    assert_eq!(
        actual,
        [true, true, false, false],
        "a change, even back, moves it"
    );
}

#[test]
fn a_tall_video_drawn_at_its_decoded_size_should_fetch_only_what_its_rows_show() {
    let portrait = "#EXTM3U\n\
            #EXT-X-STREAM-INF:BANDWIDTH=400000,RESOLUTION=360x640\nCMAF_360.m3u8\n\
            #EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=480x854\nCMAF_480.m3u8\n\
            #EXT-X-STREAM-INF:BANDWIDTH=1600000,RESOLUTION=720x1280\nCMAF_720.m3u8\n";
    // A 784×720 box whose tall frames get 400 px: 360×640 already covers 225×400.
    let tall = pick_rendition(portrait, (784, 720), 400, None).unwrap();
    assert_eq!(tall.0, "CMAF_360.m3u8");
}

#[test]
fn video_width_regression_card_decode_should_retain_display_resolution() {
    let Some(actual) = first_frame(1280, 720, (1280, 720)) else {
        return;
    };
    assert_eq!(actual, (1280, 720));
}
