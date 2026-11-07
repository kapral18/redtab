use super::*;
use serde_json::json;
use std::os::unix::fs::PermissionsExt;

const PAGE: &str = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";

fn metadata() -> Value {
    json!({"id":"dQw4w9WgXcQ","_type":"video","availability":"public","is_live":false,"live_status":"not_live",
    "http_headers":{"User-Agent":"fixture-agent","Referer":"https://www.youtube.com/","Cookie":"PRIVATE-COOKIE"},
    "requested_formats":[
        {"url":"https://rr1.googlevideo.com/video?signature=SECRET","protocol":"https","vcodec":"avc1","acodec":"none"},
        {"url":"https://rr2.googlevideo.com/audio?signature=SECRET","protocol":"https","vcodec":"none","acodec":"mp4a"}
    ]})
}

struct Fake {
    dir: tempfile::TempDir,
    program: std::path::PathBuf,
}

impl Fake {
    fn new(body: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("probe youtube ü");
        std::fs::write(dir.path().join("info.json"), metadata().to_string()).unwrap();
        std::fs::write(&program, format!("#!/bin/sh\ncd \"$(dirname \"$0\")\"\nprintf '%s\\n' \"$@\" > args\necho call >> calls\n{body}\n")).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self { dir, program }
    }

    fn resolve(&self, resolver: &Resolver) -> Result<Sources, String> {
        resolver.resolve_with(
            PAGE,
            &Cancellation::default(),
            self.program.as_os_str(),
            TIMEOUT,
        )
    }
}

#[test]
fn when_the_extractor_returns_separate_streams_it_should_keep_audio_and_safe_headers() {
    let fake = Fake::new("/bin/cat info.json");

    let actual = fake.resolve(&Resolver::default()).unwrap();

    assert_eq!(
        actual.video,
        "https://rr1.googlevideo.com/video?signature=SECRET"
    );
    assert_eq!(
        actual.audio.as_deref(),
        Some("https://rr2.googlevideo.com/audio?signature=SECRET")
    );
    assert!(!actual.audio_in_file);
    assert_eq!(
        actual
            .headers
            .iter()
            .cloned()
            .collect::<std::collections::BTreeMap<_, _>>(),
        std::collections::BTreeMap::from([
            ("Referer".into(), "https://www.youtube.com/".into()),
            ("User-Agent".into(), "fixture-agent".into())
        ])
    );
    assert_eq!(actual.audio_headers, actual.headers);
    let args = std::fs::read_to_string(fake.dir.path().join("args")).unwrap();
    for flag in [
        "--ignore-config",
        "--no-plugin-dirs",
        "--no-cookies",
        "--no-cookies-from-browser",
        "--no-cache-dir",
        "--no-playlist",
    ] {
        assert!(
            args.lines().any(|arg| arg == flag),
            "missing {flag}: {args}"
        );
    }
}

#[test]
fn when_unneeded_metadata_is_large_it_should_still_resolve_selected_streams() {
    let fake = Fake::new(
        "case \" $* \" in *\" --dump-single-json \"*) /bin/cat full.json ;; *\" --print \"*) /bin/cat info.json ;; *) exit 2 ;; esac",
    );
    let mut full = metadata();
    full["description"] = json!("x".repeat(LIMIT));
    std::fs::write(fake.dir.path().join("full.json"), full.to_string()).unwrap();

    let actual = fake.resolve(&Resolver::default()).unwrap();

    assert_eq!(
        actual.video,
        "https://rr1.googlevideo.com/video?signature=SECRET"
    );
    assert_eq!(
        actual.audio.as_deref(),
        Some("https://rr2.googlevideo.com/audio?signature=SECRET")
    );
    let args = std::fs::read_to_string(fake.dir.path().join("args")).unwrap();
    let args: Vec<_> = args.lines().collect();
    let options = &args[..args.iter().position(|arg| *arg == "--").unwrap()];
    assert!(options.windows(2).any(|pair| {
        pair == [
            "--extractor-args",
            "youtube:skip=hls;player_client=visionos",
        ]
    }));
    assert!(options.windows(2).any(|pair| {
            pair == [
                "--print",
                "%(.{id,_type,availability,is_live,live_status,requested_formats,url,protocol,vcodec,acodec,http_headers})j",
            ]
        }));
}

#[test]
fn when_the_extractor_returns_muxed_video_it_should_play_its_audio_from_the_same_file() {
    let info = json!({"id":"dQw4w9WgXcQ","url":"https://rr1.googlevideo.com/file","protocol":"https","vcodec":"avc1","acodec":"mp4a"});

    let actual = parse(&serde_json::to_vec(&info).unwrap(), "dQw4w9WgXcQ").unwrap();

    assert!(actual.audio_in_file);
    assert_eq!(
        actual.audio.as_deref(),
        Some("https://rr1.googlevideo.com/file")
    );
}

#[test]
fn when_metadata_is_live_restricted_or_unsafe_it_should_offer_the_browser_without_details() {
    for (pointer, bad) in [
        ("/id", json!("otherother1")),
        ("/_type", json!("playlist")),
        ("/is_live", json!(true)),
        ("/live_status", json!("is_upcoming")),
        ("/availability", json!("needs_auth")),
        ("/requested_formats/0/url", json!("file:///PRIVATE-COOKIE")),
        (
            "/requested_formats/0/url",
            json!("https://googlevideo.com.attacker.test/PRIVATE-COOKIE"),
        ),
        (
            "/requested_formats/1/url",
            json!("http://127.0.0.1/PRIVATE-COOKIE"),
        ),
        ("/requested_formats/0/protocol", json!("m3u8_native")),
        (
            "/http_headers/User-Agent",
            json!("agent\r\nCookie: PRIVATE-COOKIE"),
        ),
        (
            "/http_headers/Referer",
            json!("https://attacker.test/PRIVATE-COOKIE"),
        ),
    ] {
        let mut info = metadata();
        *info.pointer_mut(pointer).unwrap() = bad;
        let error = parse(&serde_json::to_vec(&info).unwrap(), "dQw4w9WgXcQ").unwrap_err();
        assert!(!error.contains("PRIVATE-COOKIE"), "{pointer}: {error}");
    }
    assert!(parse(b"not-json PRIVATE-COOKIE", "dQw4w9WgXcQ").is_err());
}

#[test]
fn when_a_completed_video_is_revisited_it_should_reuse_memory_across_url_aliases() {
    let fake = Fake::new("/bin/cat info.json");
    let resolver = Resolver::default();
    let first = fake.resolve(&resolver).unwrap();

    let actual = resolver
        .resolve_with(
            "https://youtu.be/dQw4w9WgXcQ?feature=share",
            &Cancellation::default(),
            fake.program.as_os_str(),
            TIMEOUT,
        )
        .unwrap();

    assert_eq!(actual, first);
    assert_eq!(
        std::fs::read_to_string(fake.dir.path().join("calls")).unwrap(),
        "call\n"
    );
}

#[test]
fn when_cache_entries_expire_they_should_stop_being_reused() {
    let resolver = Resolver::default();
    let now = Instant::now();
    resolver.cache.lock().push_back(Entry {
        id: "id".into(),
        sources: Sources::file("fixture"),
        until: now + Duration::from_secs(1),
    });
    assert!(resolver.cached("id", now).is_some());

    let actual = resolver.cached("id", now + Duration::from_secs(1));

    assert!(actual.is_none());
}

#[test]
fn when_two_callers_request_the_same_video_they_should_share_one_extraction() {
    let fake = Fake::new("sleep 0.1; /bin/cat info.json");
    let resolver = std::sync::Arc::new(Resolver::default());
    let other = resolver.clone();
    let program = fake.program.clone();
    let worker = std::thread::spawn(move || {
        other.resolve_with(PAGE, &Cancellation::default(), program.as_os_str(), TIMEOUT)
    });

    let actual = fake.resolve(&resolver).unwrap();

    assert_eq!(worker.join().unwrap().unwrap(), actual);
    assert_eq!(
        std::fs::read_to_string(fake.dir.path().join("calls")).unwrap(),
        "call\n"
    );
}

#[test]
fn when_the_cache_reaches_capacity_it_should_evict_the_least_recent_video() {
    let fake = Fake::new("/bin/cat info.json");
    let resolver = Resolver::default();
    for id in 0..32 {
        let mut info = metadata();
        info["id"] = json!(format!("{id:011}"));
        std::fs::write(fake.dir.path().join("info.json"), info.to_string()).unwrap();
        resolver
            .resolve_with(
                &format!("https://youtu.be/{id:011}"),
                &Cancellation::default(),
                fake.program.as_os_str(),
                TIMEOUT,
            )
            .unwrap();
    }
    assert!(resolver.cached("00000000000", Instant::now()).is_some());
    std::fs::write(fake.dir.path().join("info.json"), metadata().to_string()).unwrap();

    fake.resolve(&resolver).unwrap();

    assert!(resolver.cached("00000000001", Instant::now()).is_none());
    assert!(resolver.cached("00000000000", Instant::now()).is_some());
    assert_eq!(resolver.cache.lock().len(), 32);
}

#[test]
fn when_a_waiting_selection_is_cancelled_it_should_not_launch_another_process() {
    let fake = Fake::new("echo $$ > pid; sleep 10");
    let resolver = std::sync::Arc::new(Resolver::default());
    let active = resolver.clone();
    let program = fake.program.clone();
    let cancelled = std::sync::Arc::new(Cancellation::default());
    let signal = cancelled.clone();
    let worker = std::thread::spawn(move || {
        active.resolve_with(PAGE, &signal, program.as_os_str(), TIMEOUT)
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    while !fake.dir.path().join("pid").exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }

    let waiting = Cancellation::default();
    waiting.cancel();
    let actual = resolver.resolve_with(
        "https://youtu.be/abcdefghijk",
        &waiting,
        fake.program.as_os_str(),
        TIMEOUT,
    );
    cancelled.cancel();
    assert!(worker.join().unwrap().is_err());

    assert_eq!(actual.unwrap_err(), CANCELLED);
    assert_eq!(
        std::fs::read_to_string(fake.dir.path().join("calls")).unwrap(),
        "call\n"
    );
}

#[test]
fn when_the_extractor_is_a_symlink_it_should_still_run_without_a_shell_command() {
    let fake = Fake::new("/bin/cat info.json");
    let alias = fake.dir.path().join("extractor-link");
    std::os::unix::fs::symlink(&fake.program, &alias).unwrap();

    let actual = Resolver::default().resolve_with(
        PAGE,
        &Cancellation::default(),
        alias.as_os_str(),
        TIMEOUT,
    );

    assert!(actual.is_ok());
}

#[test]
fn when_a_signed_stream_is_expired_it_should_not_enter_the_cache() {
    let mut info = metadata();
    info["requested_formats"][0]["url"] = json!("https://rr1.googlevideo.com/video?expire=1");
    let fake = Fake::new("/bin/cat info.json");
    std::fs::write(fake.dir.path().join("info.json"), info.to_string()).unwrap();
    let resolver = Resolver::default();

    let actual = fake.resolve(&resolver);

    assert!(actual.is_err());
    assert!(resolver.cache.lock().is_empty());
}

#[test]
fn when_the_tool_is_missing_or_fails_it_should_report_an_action_without_stderr() {
    let fake = Fake::new("echo PRIVATE-COOKIE >&2; exit 1");
    let error = fake.resolve(&Resolver::default()).unwrap_err();
    assert_eq!(error, UNAVAILABLE);
    for body in [
        "echo PRIVATE-COOKIE >&2; exit 2",
        r#"case " $* " in *" --version "*) echo 2026.07.04 ;; *) echo PRIVATE-COOKIE >&2; exit 1 ;; esac"#,
    ] {
        let old = Fake::new(body);
        let error = old.resolve(&Resolver::default()).unwrap_err();
        assert!(error.contains("Update yt-dlp"), "{body}: {error}");
        assert!(!error.contains("PRIVATE-COOKIE"));
    }
    let missing = fake.dir.path().join("deleted-probe");
    let error = Resolver::default()
        .resolve_with(PAGE, &Cancellation::default(), missing.as_os_str(), TIMEOUT)
        .unwrap_err();
    assert!(error.contains("Install yt-dlp"));
}

#[test]
fn when_the_tool_times_out_it_should_kill_and_reap_its_process() {
    let fake = Fake::new("echo $$ > pid; sleep 30");
    let program = fake.program.clone();
    let cancelled = std::sync::Arc::new(Cancellation::default());
    let registration = cancelled.process.lock();
    let signal = cancelled.clone();
    let worker = std::thread::spawn(move || {
        Resolver::default().resolve_with(PAGE, &signal, program.as_os_str(), Duration::from_secs(1))
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let pid: i32 = loop {
        if let Ok(value) = std::fs::read_to_string(fake.dir.path().join("pid"))
            && let Ok(pid) = value.trim().parse()
        {
            break pid;
        }
        assert!(
            !worker.is_finished(),
            "resolver exited before extractor started"
        );
        assert!(
            Instant::now() < deadline,
            "extractor did not publish its PID"
        );
        std::thread::sleep(Duration::from_millis(5));
    };
    drop(registration);

    let deadline = Instant::now() + Duration::from_secs(5);
    while !worker.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    let finished = worker.is_finished();
    let actual = worker.join().unwrap();

    assert!(finished, "timed-out extractor was not terminated promptly");
    assert!(actual.unwrap_err().contains("timed out"));
    assert_eq!(
        unsafe { libc::kill(pid, 0) },
        -1,
        "extractor child was not reaped"
    );
}

#[test]
fn when_the_selection_is_cancelled_it_should_stop_an_active_extractor() {
    let fake = Fake::new("echo $$ > pid; sleep 10");
    let program = fake.program.clone();
    let cancelled = std::sync::Arc::new(Cancellation::default());
    let signal = cancelled.clone();
    let worker = std::thread::spawn(move || {
        Resolver::default().resolve_with(PAGE, &signal, program.as_os_str(), TIMEOUT)
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    while !fake.dir.path().join("pid").exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let started = Instant::now();

    cancelled.cancel();
    let actual = worker.join().unwrap();

    assert_eq!(actual.unwrap_err(), CANCELLED);
    assert!(started.elapsed() < Duration::from_millis(300));
}

#[test]
fn when_input_is_not_a_supported_video_it_should_not_launch_the_extractor() {
    let fake = Fake::new("exit 99");

    let actual = Resolver::default().resolve_with(
        "https://youtube.com/playlist?list=example",
        &Cancellation::default(),
        fake.program.as_os_str(),
        TIMEOUT,
    );

    assert!(actual.is_err());
    assert!(!fake.dir.path().join("calls").exists());
}
