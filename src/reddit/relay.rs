//! Players receive only loopback URLs. Every playlist URI is rewritten before it
//! reaches a player, and upstream redirects are handled by the shared transport.

use std::io::{self, BufRead, BufReader, Cursor, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::fd::AsRawFd;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use url::Url;

use super::transport::{MediaStatus, Reader, Transport};

const MAX_REQUESTS: usize = 32;
const HEADER_LIMIT: usize = 16 * 1024;
const HEADER_TIMEOUT: Duration = Duration::from_secs(3);
/// Longest wait for a player connection between stop checks.
const ACCEPT_WAIT: Duration = Duration::from_millis(250);
/// Upstream attempts to finish one body after it ends early, each resuming where the last
/// stopped. A short segment makes ffmpeg conceal the missing pictures and play on.
const RESUMES: u8 = 3;

struct Request {
    head: bool,
    url: String,
    range: Option<String>,
}

struct Response {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: Box<dyn Read + Send>,
    len: Option<u64>,
}

fn read_request(stream: &mut TcpStream) -> Result<Request> {
    let deadline = Instant::now() + HEADER_TIMEOUT;
    let mut bytes = [0; HEADER_LIMIT];
    let mut length = 0;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() || length == bytes.len() {
            bail!("request headers exceed relay limits");
        }
        stream.set_read_timeout(Some(remaining))?;
        let count = stream.read(&mut bytes[length..])?;
        if count == 0 {
            bail!("incomplete relay request");
        }
        length += count;
        let mut headers = [httparse::EMPTY_HEADER; 32];
        let mut parsed = httparse::Request::new(&mut headers);
        if parsed.parse(&bytes[..length])?.is_partial() {
            continue;
        }
        let head = match parsed.method {
            Some("GET") => false,
            Some("HEAD") => true,
            _ => bail!("unsupported relay method"),
        };
        let url = parsed
            .path
            .filter(|path| path.starts_with('/'))
            .context("relay path")?;
        let mut range = None;
        for header in parsed.headers {
            if header.name.eq_ignore_ascii_case("transfer-encoding")
                || header.name.eq_ignore_ascii_case("expect")
                || (header.name.eq_ignore_ascii_case("content-length") && header.value != b"0")
            {
                bail!("relay requests cannot contain a body");
            }
            if header.name.eq_ignore_ascii_case("range") {
                if range.is_some() {
                    bail!("duplicate range");
                }
                range = Some(std::str::from_utf8(header.value)?.to_string());
            }
        }
        return Ok(Request {
            head,
            url: url.into(),
            range,
        });
    }
}

fn respond(stream: &mut TcpStream, response: Response, head: bool) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {} Media\r\nConnection: close\r\n",
        response.status
    )?;
    for (name, value) in response.headers {
        write!(stream, "{name}: {value}\r\n")?;
    }
    if let Some(length) = response.len {
        write!(stream, "Content-Length: {length}\r\n")?;
    }
    stream.write_all(b"\r\n")?;
    if !head {
        let mut body = response.body;
        if let Some(length) = response.len {
            io::copy(&mut body.take(length), stream)?;
        } else {
            io::copy(&mut body, stream)?;
        }
    }
    Ok(())
}

struct RequestSlot(Arc<AtomicUsize>);

impl RequestSlot {
    fn acquire(active: &Arc<AtomicUsize>) -> Option<Self> {
        active
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                (count < MAX_REQUESTS).then_some(count + 1)
            })
            .ok()
            .map(|_| Self(active.clone()))
    }
}

impl Drop for RequestSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

pub(super) struct Relay {
    base: String,
    stop: Arc<AtomicBool>,
}

impl Relay {
    pub fn start(transport: Transport) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let base = format!(
            "http://{}/{}/",
            listener.local_addr()?,
            uuid::Uuid::new_v4().simple()
        );
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let local_base = base.clone();
        std::thread::spawn(move || {
            let active = Arc::new(AtomicUsize::new(0));
            while !stopped.load(Ordering::Relaxed) {
                let Some(slot) = RequestSlot::acquire(&active) else {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                };
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) => {
                        drop(slot);
                        if error.kind() == io::ErrorKind::WouldBlock {
                            wait_for_connection(&listener, ACCEPT_WAIT);
                        } else {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        continue;
                    }
                };
                let transport = transport.clone();
                let base = local_base.clone();
                std::thread::spawn(move || {
                    let _slot = slot;
                    if stream.set_nonblocking(false).is_err() {
                        return;
                    }
                    let request = read_request(&mut stream);
                    let head = request.as_ref().is_ok_and(|request| request.head);
                    let response = request
                        .and_then(|request| serve(&transport, &base, &request))
                        .unwrap_or_else(|error| Response {
                            status: error.downcast_ref::<MediaStatus>().map_or(502, |s| s.0),
                            headers: Vec::new(),
                            body: Box::new(Cursor::new(b"Media unavailable")),
                            len: Some(17),
                        });
                    let _ = respond(&mut stream, response, head);
                });
            }
        });
        Ok(Self { base, stop })
    }

    pub fn url(&self, url: &str) -> String {
        local_url(&self.base, url)
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        // The threads are detached: active streams end when their local player closes,
        // and must not block terminal restoration while an external player is still open.
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Sleeps until a connection waits on `listener` or `timeout` passes.
fn wait_for_connection(listener: &TcpListener, timeout: Duration) {
    let mut descriptor = libc::pollfd {
        fd: listener.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one initialized pollfd for a descriptor the listener keeps open; the
    // timeout bounds the call, and its result only decides when to try accept again.
    unsafe {
        libc::poll(&mut descriptor, 1, timeout.as_millis() as i32);
    }
}

fn local_url(base: &str, url: &str) -> String {
    // Keep the path and extension for ffmpeg's format detection and relative joins.
    format!("{base}{}", url.replacen("://", "/", 1))
}

fn serve(transport: &Transport, base: &str, request: &Request) -> Result<Response> {
    let base_url = Url::parse(base)?;
    let prefix = base_url.path();
    let path = request
        .url
        .strip_prefix(prefix)
        .ok_or_else(|| anyhow::anyhow!("relay key"))?;
    let (scheme, rest) = path
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("relay path"))?;
    let upstream = format!("{scheme}://{rest}");
    let response = transport.media(&upstream, request.range.as_deref(), None)?;
    let status = response.status().as_u16();
    let final_url = Url::parse(&response.uri().to_string())?;
    let playlist_hint = final_url.path().ends_with(".m3u8")
        || response
            .headers()
            .get("content-type")
            .and_then(|h| h.to_str().ok())
            .is_some_and(|s| s.to_ascii_lowercase().contains("mpegurl"));
    let mut headers = Vec::new();
    for name in ["content-type", "content-range", "accept-ranges"] {
        if let Some(value) = response.headers().get(name)
            && let Ok(value) = value.to_str()
        {
            headers.push((name, value.to_string()));
        }
    }
    let len = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let etag = response
        .headers()
        .get("etag")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let mut body = BufReader::new(Reader::new(response, u64::MAX, None));
    let peek = body.fill_buf()?;
    let looks_like_playlist = peek.starts_with(b"#EXTM3U");
    // DASH and other XML manifests can reference remote resources too; only HLS
    // has a rewrite contract here. Media images never use this relay.
    if peek.starts_with(b"<?xml") || peek.starts_with(b"<MPD") {
        bail!("unsupported media manifest");
    }
    if playlist_hint || looks_like_playlist {
        let mut text = String::new();
        body.take(1024 * 1024 + 1).read_to_string(&mut text)?;
        if text.len() > 1024 * 1024 {
            bail!("playlist too large");
        }
        let text = rewrite_playlist(&text, &final_url, |url| {
            if !transport.media_allowed(url) {
                bail!("unsupported playlist target");
            }
            Ok(local_url(base, url.as_str()))
        })?;
        return Ok(Response {
            status: 200,
            headers: vec![("Content-Type", "application/vnd.apple.mpegurl".into())],
            len: Some(text.len() as u64),
            body: Box::new(Cursor::new(text.into_bytes())),
        });
    }
    let reader: Box<dyn Read + Send> = if request.head {
        Box::new(Cursor::new(Vec::<u8>::new()))
    } else if let Some((total, (start, end))) = len.zip(match status {
        206 => headers
            .iter()
            .find(|(name, _)| *name == "content-range")
            .and_then(|(_, value)| content_range(value))
            .map(|(start, end)| (start, Some(end))),
        200 => Some((0, None)),
        _ => None,
    }) {
        Box::new(Resuming {
            transport: transport.clone(),
            url: upstream,
            etag,
            body: Box::new(body),
            start,
            end,
            sent: 0,
            total,
            resumes: 0,
        })
    } else {
        Box::new(body)
    };
    Ok(Response {
        status,
        headers,
        body: reader,
        len,
    })
}

/// `bytes 100-199/1000` as its first and last byte.
fn content_range(value: &str) -> Option<(u64, u64)> {
    let (first, last) = value
        .strip_prefix("bytes ")?
        .split_once('/')?
        .0
        .split_once('-')?;
    Some((first.parse().ok()?, last.parse().ok()?))
}

/// An upstream body that resumes with a ranged request when it ends before its length.
struct Resuming {
    transport: Transport,
    url: String,
    etag: Option<String>,
    body: Box<dyn Read + Send>,
    start: u64,
    end: Option<u64>,
    sent: u64,
    total: u64,
    resumes: u8,
}

impl Resuming {
    fn resume(&mut self) -> Result<()> {
        let from = self.start + self.sent;
        let range = format!(
            "bytes={from}-{}",
            self.end.map(|end| end.to_string()).unwrap_or_default()
        );
        let response = self.transport.media(&self.url, Some(&range), None)?;
        let resumed = response
            .headers()
            .get("content-range")
            .and_then(|value| value.to_str().ok())
            .and_then(content_range)
            .is_some_and(|(first, _)| first == from);
        let same = self.etag.as_deref().is_none_or(|etag| {
            response
                .headers()
                .get("etag")
                .and_then(|value| value.to_str().ok())
                == Some(etag)
        });
        if response.status().as_u16() != 206 || !resumed || !same {
            bail!("upstream cannot resume");
        }
        self.body = Box::new(Reader::new(response, u64::MAX, None));
        Ok(())
    }
}

impl Read for Resuming {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() || self.sent >= self.total {
            return Ok(0);
        }
        let room = buf.len().min((self.total - self.sent) as usize);
        loop {
            let failure = match self.body.read(&mut buf[..room]) {
                Ok(0) => io::Error::new(io::ErrorKind::UnexpectedEof, "upstream ended early"),
                Ok(n) => {
                    self.sent += n as u64;
                    self.resumes = 0;
                    return Ok(n);
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => error,
            };
            if self.resumes >= RESUMES {
                return Err(failure);
            }
            self.resumes += 1;
            self.resume().map_err(|_| failure)?;
        }
    }
}

fn rewrite_playlist(
    text: &str,
    upstream: &Url,
    mut local: impl FnMut(&Url) -> Result<String>,
) -> Result<String> {
    if !text.trim_start().starts_with("#EXTM3U") {
        bail!("invalid HLS playlist");
    }
    let mut lines = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        // Variable substitution can conceal URLs from the relay. Reject this extension.
        if line.contains("{$") || line.starts_with("#EXT-X-DEFINE:") {
            bail!("unsupported HLS variables");
        }
        if !line.is_empty() && !line.starts_with('#') {
            lines.push(local(&upstream.join(line)?)?);
        } else {
            let mut rest = line;
            let mut rewritten = String::new();
            while let Some((before, after)) = rest.split_once("URI=") {
                rewritten.push_str(before);
                let quoted = after
                    .strip_prefix('"')
                    .ok_or_else(|| anyhow::anyhow!("unquoted HLS URI"))?;
                let (uri, tail) = quoted
                    .split_once('"')
                    .ok_or_else(|| anyhow::anyhow!("unterminated HLS URI"))?;
                rewritten.push_str("URI=\"");
                rewritten.push_str(&local(&upstream.join(uri)?)?);
                rewritten.push('"');
                rest = tail;
            }
            rewritten.push_str(rest);
            lines.push(rewritten);
        }
    }
    Ok(lines.join("\n") + "\n")
}

#[cfg(test)]
mod tests;
