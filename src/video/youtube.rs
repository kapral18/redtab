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
const UNAVAILABLE: &str = "YouTube unavailable; p retries or O opens browser";
const OUTDATED: &str = "Update yt-dlp to 2026.08.19 or newer; O opens browser";
/// The first yt-dlp release with every option and player client `command` passes.
const MINIMUM_VERSION: (u32, u32, u32) = (2026, 8, 19);
const OUTPUT_TOO_LARGE: &str = "YouTube metadata exceeds the inline limit; O opens browser";
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
                Err("YouTube resolution timed out; O opens browser".to_owned())
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
        let bytes = match extract(command(program, &page), cancelled, check) {
            // Older releases ignore the unknown player client, so their failures exit 1
            // like any other. The probe needs no permit: other videos go on meanwhile.
            Err(error) if error == UNAVAILABLE && check().is_ok() => {
                drop(_permit);
                return Err(if outdated(program, cancelled, check) {
                    OUTDATED
                } else {
                    UNAVAILABLE
                }
                .into());
            }
            result => result?,
        };
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

/// yt-dlp without user config, its output piped, in its own process group.
fn base(program: &OsStr) -> Command {
    let mut command = crate::media::local_command(program.to_str().unwrap_or("yt-dlp"));
    command
        .arg("--ignore-config")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0);
    command
}

fn command(program: &OsStr, page: &str) -> Command {
    let mut command = base(program);
    command.args([
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
    ]);
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
            "Install yt-dlp for YouTube playback; O opens browser".into()
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
            } else if status.code() == Some(2) {
                // yt-dlp exits 2 for an option it does not know, before any network use.
                Err(OUTDATED.into())
            } else {
                Err(UNAVAILABLE.into())
            };
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Whether `program --version` names a release older than `MINIMUM_VERSION`. An unknown
/// answer counts as current, so only a known old version replaces the generic message.
fn outdated(
    program: &OsStr,
    cancellation: &Cancellation,
    check: impl Fn() -> Result<(), String>,
) -> bool {
    let mut command = base(program);
    command.arg("--version");
    let deadline = Instant::now() + Duration::from_secs(2);
    let Ok(bytes) = extract(command, cancellation, || {
        check()?;
        if Instant::now() < deadline {
            Ok(())
        } else {
            Err(UNAVAILABLE.into())
        }
    }) else {
        return false;
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut parts = text.trim().split('.').map(|part| part.parse::<u32>().ok());
    match (
        parts.next().flatten(),
        parts.next().flatten(),
        parts.next().flatten(),
    ) {
        (Some(year), Some(month), Some(day)) => (year, month, day) < MINIMUM_VERSION,
        _ => false,
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
        return Err("This YouTube video needs the browser; press O".into());
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
        return Err("YouTube stream expired; p retries or O opens browser".into());
    }
    Ok(Duration::from_secs(seconds))
}

#[cfg(test)]
mod tests;
