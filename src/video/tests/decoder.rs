use super::*;

#[test]
fn ppm_frames_are_read_whole_across_short_reads() {
    let mut stream = b"P6\n2 1\n255\n".to_vec();
    stream.extend([1, 2, 3, 4, 5, 6]);
    stream.extend(b"P6 1 2 255 ");
    stream.extend([7, 8, 9, 10, 11, 12]);
    let mut r = BufReader::with_capacity(1, Trickle(&stream));
    assert_eq!(
        read_ppm(&mut r).unwrap(),
        Some((2, 1, vec![1, 2, 3, 4, 5, 6]))
    );
    assert_eq!(
        read_ppm(&mut r).unwrap(),
        Some((1, 2, vec![7, 8, 9, 10, 11, 12]))
    );
    assert_eq!(read_ppm(&mut r).unwrap(), None);
}

#[test]
fn a_cut_ppm_frame_is_an_error_not_a_frame() {
    let mut r = BufReader::new(&b"P6\n2 1\n255\n\x01\x02\x03"[..]);
    assert!(read_ppm(&mut r).is_err());
}

#[test]
fn when_a_log_line_is_not_utf8_the_rest_of_the_log_should_still_be_read() {
    let log = Mutex::new(Log::default());
    let stderr = b"[info]     title           : caf\xe9\n[info]   Duration: 00:00:10.00, start: 0\n[info] Stream mapping:\n";

    read_log(&stderr[..], &log);

    let log = log.lock();
    assert_eq!(log.duration, Some(10.0));
    assert!(log.streams_read);
}

#[test]
fn a_log_that_never_names_the_rate_should_not_hold_frames_back() {
    let (tx, rx) = sync_channel(1);
    std::thread::spawn(move || {
        let log = Mutex::new(Log::default());
        let _ = tx.send(logged_rate(&log, Duration::from_millis(20)));
    });

    assert_eq!(rx.recv_timeout(Duration::from_secs(1)), Ok(MIN_FPS));
}

#[test]
fn ffmpeg_duration_line_gives_seconds() {
    let line = "[info]   Duration: 00:01:02.50, start: 0.000000, bitrate: N/A";
    assert_eq!(parse_duration(line), Some(62.5));
    assert_eq!(parse_duration("[info]   Duration: N/A, start: 0"), None);
}

#[test]
fn when_output_stalls_the_decoder_should_discard_frames_behind_the_audio_target() {
    let (tx, rx) = sync_channel(1);
    let progress = Arc::new(DecodeProgress::default());
    progress.read.store(1, Ordering::Relaxed);
    tx.send(Decoded {
        index: 0,
        frame: blank(),
        rate: MIN_FPS,
    })
    .unwrap();
    let reader_progress = Arc::clone(&progress);
    let source = b"P6\n1 1\n255\n\x01\x02\x03".repeat(30);
    let worker = std::thread::spawn(move || {
        read_frames(std::io::Cursor::new(source), tx, &reader_progress, || {
            MIN_FPS
        });
    });

    progress.target.store(12, Ordering::Relaxed);
    let actual = [
        rx.recv_timeout(Duration::from_secs(2)).unwrap().index,
        rx.recv_timeout(Duration::from_secs(2)).unwrap().index,
        rx.recv_timeout(Duration::from_secs(2)).unwrap().index,
    ];
    drop(rx);
    worker.join().unwrap();

    assert_eq!(actual[0], 0);
    assert!(
        actual[2] >= 12,
        "bounded pending frames must give way to current audio: {actual:?}"
    );
}

#[test]
fn when_frames_end_before_the_log_it_should_wait_for_the_log() {
    let t0 = Instant::now();
    let log = Arc::new(Mutex::new(Log {
        input_error: true,
        error: Some("Stream ends prematurely".into()),
        ..Log::default()
    }));
    let shared = Arc::clone(&log);
    let runs: Runs = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let record = std::rc::Rc::clone(&runs);
    let spawn: Spawn = Box::new(move |_, at| {
        let (tx, frames) = frame_channel(64);
        record.borrow_mut().push((at, tx));
        Ok(Decoder {
            child: None,
            frames,
            log: Arc::clone(&shared),
            progress: Arc::new(DecodeProgress::default()),
        })
    });
    let mut player = fed_silent_player(spawn, 0.0, t0);
    let first = runs.borrow_mut().remove(0).1;
    first.send(repeated()).unwrap();
    first.send(blank()).unwrap();
    drop(first);

    player.tick(t0);
    player.tick(after(t0, 0.15));
    let while_logging = runs.borrow().len();
    log.lock().ended = true;
    player.tick(after(t0, 0.2));

    assert_eq!(while_logging, 0, "nothing decided before the log ends");
    assert_eq!(
        runs.borrow()[0].0,
        2.0 / MIN_FPS,
        "then it goes on from the cut"
    );
}

#[test]
fn metadata_that_reads_like_the_streams_should_not_describe_them() {
    let log = Mutex::new(Log::default());
    let stderr = b"[info] Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'v.mp4':\n\
[info]   Metadata:\n\
[info]     title           : Duration: 00:00:01.00, start: 0\n\
[info]     comment         : x Stream #0:1: Audio: aac\n\
[info]     k               : Stream mapping:\n\
[info]   Duration: 00:00:05.00, start: 0.000000, bitrate: 41 kb/s\n\
[info]   Stream #0:0(und): Video: h264 (High), yuv420p, 64x36, 30 fps, 30 tbr\n";

    read_log(&stderr[..], &log);

    let log = log.lock();
    assert_eq!(log.duration, Some(5.0));
    assert!(!log.audio, "a silent file");
    assert!(!log.streams_read, "the mapping is not listed yet");
}

#[test]
fn when_ffmpeg_logs_an_error_from_a_component_it_should_be_noted() {
    // Lines ffmpeg 9 printed for a GIF whose connection dropped.
    let stderr =
        b"[http @ 0x7951404000] [error] Stream ends prematurely at 60000, should be 2802623\n\
[in#0/gif @ 0x7950808000] [error] Error during demuxing: Input/output error\n";
    let log = Mutex::new(Log::default());

    read_log(&stderr[..], &log);

    let log = log.lock();
    assert_eq!(
        log.error.as_deref(),
        Some("Error during demuxing: Input/output error")
    );
    assert!(log.input_error, "a dropped connection is an input failure");
    assert!(log.ended);
}
