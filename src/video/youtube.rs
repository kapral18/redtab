use std::collections::VecDeque;
use std::ffi::OsStr;
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde_json::Value;

use super::Sources;

const LIMIT: usize = 4 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(20);
const UNAVAILABLE: &str = "YouTube unavailable; SPC m p retries or SPC o m opens browser";
const OUTPUT_TOO_LARGE: &str = "YouTube metadata exceeds the inline limit; SPC o m opens browser";
const CANCELLED: &str = "YouTube resolution cancelled";
const FORMAT: &str = "bestvideo[protocol=https][width<=1920][height<=1920][fps<=30]+bestaudio[protocol=https]/best[protocol=https][width<=1920][height<=1920][fps<=30]";
const PLAYBACK_METADATA: &str = "%(.{id,_type,availability,is_live,live_status,requested_formats,url,protocol,vcodec,acodec,http_headers})j";
static RESOLVER: LazyLock<Resolver> = LazyLock::new(Resolver::default);

#[derive(Default)]
struct Resolver {
    cache: Mutex<VecDeque<Entry>>,
    extracting: Mutex<()>,
}

struct Entry {
    id: String,
    sources: Sources,
    until: Instant,
}

#[derive(Default)]
pub(super) struct Cancellation {
    cancelled: AtomicBool,
    process: Mutex<Option<u32>>,
}

impl Cancellation {
    pub(super) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub(super) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        if let Some(pid) = *self.process.lock() {
            // Registration and reaping share this lock, so the process group cannot be reused.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
    }
}

pub(super) fn resolve(url: &str, cancelled: &Cancellation) -> Result<Sources, String> {
    RESOLVER.resolve_with(url, cancelled, OsStr::new("yt-dlp"), TIMEOUT)
}

pub(super) fn invalidate(url: &str) {
    if let Some(id) = crate::model::youtube_id(url) {
        RESOLVER.cache.lock().retain(|entry| entry.id != id);
    }
}

impl Resolver {
    fn cached(&self, id: &str, now: Instant) -> Option<Sources> {
        let mut cache = self.cache.lock();
        cache.retain(|entry| entry.until > now);
        let index = cache.iter().position(|entry| entry.id == id)?;
        let entry = cache.remove(index)?;
        let sources = entry.sources.clone();
        cache.push_back(entry);
        Some(sources)
    }

    fn resolve_with(
        &self,
        url: &str,
        cancelled: &Cancellation,
        program: &OsStr,
        timeout: Duration,
    ) -> Result<Sources, String> {
        let id = crate::model::youtube_id(url).ok_or(UNAVAILABLE)?;
        let started = Instant::now();
        let check = || {
            if cancelled.is_cancelled() {
                Err(CANCELLED.to_owned())
            } else if started.elapsed() >= timeout {
                Err("YouTube resolution timed out; SPC o m opens browser".to_owned())
            } else {
                Ok(())
            }
        };
        check()?;
        if let Some(sources) = self.cached(&id, Instant::now()) {
            return Ok(sources);
        }
        let _permit = loop {
            check()?;
            if let Some(permit) = self.extracting.try_lock() {
                break permit;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        if let Some(sources) = self.cached(&id, Instant::now()) {
            return Ok(sources);
        }
        let page = format!("https://www.youtube.com/watch?v={id}");
        let bytes = extract(command(program, &page), cancelled, check)?;
        let sources = parse(&bytes, &id)?;
        let ttl = stream_ttl(&sources)?;
        check()?;
        let mut cache = self.cache.lock();
        cache.retain(|entry| entry.id != id && entry.until > Instant::now());
        if cache.len() >= 32 {
            cache.pop_front();
        }
        cache.push_back(Entry {
            id,
            sources: sources.clone(),
            until: Instant::now() + ttl,
        });
        Ok(sources)
    }
}

fn command(program: &OsStr, page: &str) -> Command {
    let mut command = crate::media::local_command(program.to_str().unwrap_or("yt-dlp"));
    command
        .args([
            "--ignore-config",
            "--no-plugin-dirs",
            "--no-cookies",
            "--no-cookies-from-browser",
            "--no-cache-dir",
            "--no-remote-components",
            "--js-runtimes",
            "node",
            "--no-playlist",
            "--skip-download",
            "--print",
            PLAYBACK_METADATA,
            "--no-warnings",
            "--no-progress",
            "--socket-timeout",
            "8",
            "--retries",
            "0",
            "--extractor-retries",
            "0",
            // FORMAT excludes HLS, so its manifests cannot contribute a playable stream.
            "--extractor-args",
            "youtube:skip=hls;player_client=visionos",
            "--format",
            FORMAT,
            "--",
            page,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0);
    command
}

struct Running<'a> {
    child: Child,
    running: bool,
    cancellation: &'a Cancellation,
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        if self.running {
            {
                let mut process = self.cancellation.process.lock();
                // The unreaped child owns the process group created by process_group(0).
                unsafe {
                    libc::kill(-(self.child.id() as i32), libc::SIGKILL);
                }
                *process = None;
            }
            let _ = self.child.wait();
        }
    }
}

fn extract(
    mut command: Command,
    cancellation: &Cancellation,
    check: impl Fn() -> Result<(), String>,
) -> Result<Vec<u8>, String> {
    let child = command.spawn().map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            "Install yt-dlp for YouTube playback; SPC o m opens browser".into()
        } else {
            UNAVAILABLE.to_owned()
        }
    })?;
    {
        let mut process = cancellation.process.lock();
        *process = Some(child.id());
        if cancellation.is_cancelled() {
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
        }
    }
    let mut child = Running {
        child,
        running: true,
        cancellation,
    };
    let mut stdout = child.child.stdout.take().ok_or(UNAVAILABLE)?;
    let fd = stdout.as_raw_fd();
    // Only this worker owns the pipe; polling it keeps cancellation and output bounded.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(UNAVAILABLE.into());
    }
    let mut bytes = Vec::new();
    loop {
        check()?;
        drain(&mut stdout, &mut bytes)?;
        let status = {
            let mut process = cancellation.process.lock();
            let status = child.child.try_wait().map_err(|_| UNAVAILABLE)?;
            if status.is_some() {
                child.running = false;
                *process = None;
            }
            status
        };
        if let Some(status) = status {
            drain(&mut stdout, &mut bytes)?;
            return if status.success() {
                Ok(bytes)
            } else {
                Err(UNAVAILABLE.into())
            };
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn drain(reader: &mut impl Read, bytes: &mut Vec<u8>) -> Result<(), String> {
    let mut buffer = [0; 8192];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(count) => {
                if bytes.len() + count > LIMIT {
                    return Err(OUTPUT_TOO_LARGE.into());
                }
                bytes.extend_from_slice(&buffer[..count]);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(UNAVAILABLE.into()),
        }
    }
}

fn parse(bytes: &[u8], id: &str) -> Result<Sources, String> {
    let info: Value = serde_json::from_slice(bytes).map_err(|_| UNAVAILABLE)?;
    if info["id"].as_str() != Some(id)
        || info["_type"].as_str().is_some_and(|kind| kind != "video")
        || info["is_live"].as_bool() == Some(true)
        || info["live_status"]
            .as_str()
            .is_some_and(|status| !matches!(status, "not_live" | "was_live"))
        || info["availability"]
            .as_str()
            .is_some_and(|access| !matches!(access, "public" | "unlisted"))
    {
        return Err("This YouTube video needs the browser; press SPC o m".into());
    }
    let formats = info["requested_formats"].as_array();
    let video = formats
        .and_then(|items| items.iter().find(|item| has_codec(item, "vcodec")))
        .unwrap_or(&info);
    if !has_codec(video, "vcodec") {
        return Err(UNAVAILABLE.into());
    }
    let video_url = stream_url(video)?;
    let video_headers = headers(video, &info)?;
    let audio = formats.and_then(|items| {
        items
            .iter()
            .find(|item| !has_codec(item, "vcodec") && has_codec(item, "acodec"))
    });
    let (audio, audio_headers, audio_in_file) = if let Some(audio) = audio {
        (Some(stream_url(audio)?), headers(audio, &info)?, false)
    } else if has_codec(video, "acodec") {
        (Some(video_url.clone()), video_headers.clone(), true)
    } else {
        (None, Vec::new(), false)
    };
    Ok(Sources {
        video: video_url,
        audio,
        audio_in_file,
        headers: video_headers,
        audio_headers,
    })
}

fn has_codec(value: &Value, key: &str) -> bool {
    value[key]
        .as_str()
        .is_some_and(|codec| !codec.is_empty() && codec != "none")
}

fn stream_url(format: &Value) -> Result<String, String> {
    let raw = format["url"].as_str().ok_or(UNAVAILABLE)?;
    if raw.len() > 16384 || raw.bytes().any(|byte| byte <= 0x20 || byte == 0x7f) {
        return Err(UNAVAILABLE.into());
    }
    let url = url::Url::parse(raw).map_err(|_| UNAVAILABLE)?;
    if format["protocol"].as_str() != Some("https")
        || url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || !url
            .host_str()
            .is_some_and(|host| host == "googlevideo.com" || host.ends_with(".googlevideo.com"))
    {
        return Err(UNAVAILABLE.into());
    }
    Ok(raw.into())
}

fn headers(format: &Value, info: &Value) -> Result<Vec<(String, String)>, String> {
    let Some(values) = format
        .get("http_headers")
        .or_else(|| info.get("http_headers"))
        .and_then(Value::as_object)
    else {
        return Ok(Vec::new());
    };
    let mut result = Vec::new();
    for (key, value) in values {
        if !matches!(
            key.to_ascii_lowercase().as_str(),
            "user-agent" | "referer" | "origin"
        ) {
            continue;
        }
        let value = value.as_str().ok_or(UNAVAILABLE)?;
        if value.len() > 2048 || value.bytes().any(|byte| byte < 0x20 || byte == 0x7f) {
            return Err(UNAVAILABLE.into());
        }
        if !key.eq_ignore_ascii_case("user-agent") {
            let at = url::Url::parse(value).map_err(|_| UNAVAILABLE)?;
            if at.scheme() != "https"
                || !at.username().is_empty()
                || at.password().is_some()
                || !at
                    .host_str()
                    .is_some_and(|host| matches!(host, "www.youtube.com" | "youtube.com"))
            {
                return Err(UNAVAILABLE.into());
            }
        }
        result.push((key.clone(), value.into()));
    }
    Ok(result)
}

fn stream_ttl(sources: &Sources) -> Result<Duration, String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut seconds = 300;
    for raw in std::iter::once(&sources.video).chain(sources.audio.iter()) {
        let url = url::Url::parse(raw).map_err(|_| UNAVAILABLE)?;
        if let Some((_, expiry)) = url.query_pairs().find(|(key, _)| key == "expire") {
            let expiry: u64 = expiry.parse().map_err(|_| UNAVAILABLE)?;
            seconds = seconds.min(expiry.saturating_sub(now.saturating_add(30)));
        }
    }
    if seconds == 0 {
        return Err("YouTube stream expired; SPC m p retries or SPC o m opens browser".into());
    }
    Ok(Duration::from_secs(seconds))
}

#[cfg(test)]
mod tests {
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
                Duration::from_secs(2),
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
        assert!(error.contains("browser"));
        assert!(!error.contains("PRIVATE-COOKIE"));
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
            Resolver::default().resolve_with(
                PAGE,
                &signal,
                program.as_os_str(),
                Duration::from_secs(2),
            )
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
    fn when_extractor_output_exceeds_the_limit_it_should_stop_without_parsing_it() {
        let fake = Fake::new("/usr/bin/head -c 4194305 /dev/zero");

        let actual = fake.resolve(&Resolver::default());

        assert_eq!(
            actual.unwrap_err(),
            "YouTube metadata exceeds the inline limit; SPC o m opens browser"
        );
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
}
