use super::*;

#[test]
fn video_startup_should_overlap_decoder_preparation_with_stream_resolution() {
    let (prepared, preparation) = sync_channel(1);
    let (resolved, resolution) = sync_channel(1);

    let actual = prepare_playback(
        || {
            resolved.send(()).unwrap();
            preparation.recv_timeout(Duration::from_secs(2)).unwrap();
            Ok(Sources::file("https://example.org/video.mp4"))
        },
        move || {
            prepared.send(()).unwrap();
            resolution.recv_timeout(Duration::from_secs(2)).unwrap();
        },
    )
    .unwrap();

    assert_eq!(actual.video, "https://example.org/video.mp4");
    assert_eq!(
        actual.audio.as_deref(),
        Some("https://example.org/video.mp4")
    );
}

#[test]
fn the_rendition_is_the_smallest_that_needs_no_scaling_up() {
    let pick =
        |frame_box: (u32, u32)| pick_rendition(MASTER, frame_box, frame_box.1, None).unwrap();
    // A 600×300 box fits a 16:9 picture 533 wide: 640×360 covers it, and of the
    // two the one with the better sound.
    assert_eq!(
        pick((600, 300)),
        ("CMAF_360.m3u8".into(), Some("CMAF_AUDIO_128.m3u8".into()))
    );
    // Taller than 360 but narrow: the width limits the fit, 640 covers it.
    assert_eq!(pick((600, 2000)).0, "CMAF_360.m3u8");
    // Larger than every rendition: the largest.
    assert_eq!(pick((4000, 3000)).0, "CMAF_720.m3u8");
    // The display budget retains enough detail to choose the 720p rendition.
    let bounded = |master| {
        pick_rendition(master, (856, 1170), 1170, Some(MAX_VIDEO_PIXELS))
            .unwrap()
            .0
    };
    assert_eq!(bounded(MASTER), "CMAF_720.m3u8");
    assert_eq!(pick((856, 1170)).0, "CMAF_720.m3u8");
    // If both streams are smaller than the display, choose the larger one.
    let with_270 = "#EXTM3U\n\
            #EXT-X-STREAM-INF:BANDWIDTH=300000,RESOLUTION=480x270\nCMAF_270.m3u8\n\
            #EXT-X-STREAM-INF:BANDWIDTH=500000,RESOLUTION=640x360\nCMAF_360.m3u8\n";
    assert_eq!(bounded(with_270), "CMAF_360.m3u8");
}

#[test]
fn a_byte_range_playlist_plays_its_whole_file() {
    let ranged = "#EXTM3U\n#EXT-X-MAP:URI=\"CMAF_720.mp4\",BYTERANGE=\"893@0\"\n#EXTINF:4,\n#EXT-X-BYTERANGE:677420@1033\nCMAF_720.mp4\n#EXTINF:4,\n#EXT-X-BYTERANGE:614215\nCMAF_720.mp4\n";
    assert_eq!(media_file(ranged).as_deref(), Some("CMAF_720.mp4"));
    // Segments in files of their own: the playlist itself is the stream.
    let split =
        "#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:4,\nseg1.m4s\n#EXTINF:4,\nseg2.m4s\n";
    assert_eq!(media_file(split), None);
}

#[test]
fn native_hls_denial_should_recover_video_and_audio_through_the_relay() {
    use crate::reddit::tests::{Fixture, Reply};
    let fixture = Fixture::new(|request| {
        assert!(!request.headers.contains_key("cookie"));
        assert!(!request.headers.contains_key("authorization"));
        match request.url.as_str() {
            "/clip/HLSPlaylist.m3u8?token=preserved" => Reply::status(403),
            "/clip/DASHPlaylist.mpd?token=preserved" => Reply {
                status: 302,
                body: String::new(),
                headers: vec![("Location".into(), "/renditions/DASHPlaylist.mpd".into())],
            },
            "/renditions/DASHPlaylist.mpd" => Reply {
                status: 200,
                body: include_str!("../../../tests/fixtures/reddit-video-dash.mpd").into(),
                headers: vec![],
            },
            "/renditions/CMAF_360.mp4" => Reply {
                status: 200,
                body: "video".into(),
                headers: vec![],
            },
            "/renditions/CMAF_AUDIO_128.mp4" => Reply {
                status: 200,
                body: "audio".into(),
                headers: vec![],
            },
            other => panic!("unexpected upstream request: {other}"),
        }
    });
    let reddit = fixture.reddit();
    let upstream = format!("{}clip/HLSPlaylist.m3u8?token=preserved", fixture.base);
    let sources = resolve(&upstream, (784, 720), 500, None, || {
        Ok(reddit
            .video_fallback(&upstream, || false)
            .expect("the supplied DASH files should resolve"))
    })
    .expect("a denied HLS master must not hide accessible DASH video/audio");
    assert_eq!(
        sources.video,
        reddit
            .media_url(&format!("{}renditions/CMAF_360.mp4", fixture.base))
            .unwrap()
    );
    assert_eq!(
        sources.audio.as_deref(),
        Some(
            reddit
                .media_url(&format!("{}renditions/CMAF_AUDIO_128.mp4", fixture.base))
                .unwrap()
        )
        .as_deref()
    );
    assert!(!sources.audio_in_file);
    assert_eq!(
        crate::reddit::tests::relayed(&sources.video, None).body,
        b"video"
    );
    assert_eq!(
        crate::reddit::tests::relayed(sources.audio.as_ref().unwrap(), None).body,
        b"audio"
    );
}

#[test]
fn accessible_hls_should_not_request_an_alternative() {
    let fixture = crate::reddit::tests::Fixture::new(|_| crate::reddit::tests::Reply {
        status: 200,
        body: "#EXTM3U\n#EXT-X-ENDLIST\n".into(),
        headers: vec![],
    });
    let url = format!("{}HLSPlaylist.m3u8", fixture.base);
    let sources = resolve(&url, (640, 360), 360, None, || panic!("HLS already works")).unwrap();
    assert_eq!(sources.video, url);
}

#[test]
fn media_failure_should_explain_playlist_denial_without_exposing_the_url() {
    let server = crate::reddit::tests::local_http_server();
    let url = format!(
        "http://{}/video.m3u8?signature=PRIVATE",
        server.server_addr()
    );
    let responder = std::thread::spawn(move || {
        for status in [403, 404, 410] {
            let request = server
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap();
            request
                .respond(crate::reddit::tests::served::Response::empty(status))
                .unwrap();
        }
    });
    for (status, expected) in [
        (403, "Reddit denied access to this video (HTTP 403)"),
        (404, "Video is no longer available (HTTP 404)"),
        (410, "Video is no longer available (HTTP 410)"),
    ] {
        let error =
            resolve(&url, (640, 360), 360, None, || Err("no alternative".into())).unwrap_err();
        assert_eq!(error, expected, "upstream HTTP {status}");
    }
    responder.join().unwrap();
}
