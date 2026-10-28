//! Players receive only loopback URLs. Every playlist URI is rewritten before it
//! reaches a player, and upstream redirects are handled by the shared transport.

use std::io::{BufRead, BufReader, Cursor, Read};
use std::net::TcpListener;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

use anyhow::{Result, bail};
use tiny_http::{Header, Method, Response, Server, StatusCode};
use url::Url;

use super::transport::{MediaStatus, Reader, Transport};

const MAX_REQUESTS: usize = 32;

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
        let base = format!(
            "http://{}/{}/",
            listener.local_addr()?,
            uuid::Uuid::new_v4().simple()
        );
        let server = Server::from_listener(listener, None)
            .map_err(|_| anyhow::anyhow!("cannot start media relay"))?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let local_base = base.clone();
        std::thread::spawn(move || {
            let active = Arc::new(AtomicUsize::new(0));
            while !stopped.load(Ordering::Relaxed) {
                // Reserve before receiving: even rejecting a pipelined request can
                // block behind its earlier response, so only workers own requests.
                let Some(slot) = RequestSlot::acquire(&active) else {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                };
                let Ok(Some(request)) = server.recv_timeout(Duration::from_millis(100)) else {
                    continue;
                };
                let transport = transport.clone();
                let base = local_base.clone();
                // A paused player can stop reading indefinitely. Its response must not
                // occupy the workers needed to open another rendition or audio stream.
                std::thread::spawn(move || {
                    let _slot = slot;
                    let response = serve(&transport, &base, &request).unwrap_or_else(|error| {
                        Response::from_string("Media unavailable")
                            .with_status_code(
                                error.downcast_ref::<MediaStatus>().map_or(502, |s| s.0),
                            )
                            .boxed()
                    });
                    let _ = request.respond(response);
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

fn local_url(base: &str, url: &str) -> String {
    // Keep the path and extension for ffmpeg's format detection and relative joins.
    format!("{base}{}", url.replacen("://", "/", 1))
}

fn serve(
    transport: &Transport,
    base: &str,
    request: &tiny_http::Request,
) -> Result<Response<Box<dyn Read + Send>>> {
    if !matches!(request.method(), Method::Get | Method::Head) {
        bail!("method");
    }
    let base_url = Url::parse(base)?;
    let prefix = base_url.path();
    let path = request
        .url()
        .strip_prefix(prefix)
        .ok_or_else(|| anyhow::anyhow!("relay key"))?;
    let (scheme, rest) = path
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("relay path"))?;
    let upstream = format!("{scheme}://{rest}");
    let range = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Range"))
        .map(|h| h.value.as_str());
    let response = transport.media(&upstream, range, None)?;
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
            && let Ok(header) = Header::from_bytes(name, value.as_bytes())
        {
            headers.push(header);
        }
    }
    let len = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok());
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
        return Ok(Response::from_string(text)
            .with_header(
                Header::from_bytes("Content-Type", "application/vnd.apple.mpegurl").unwrap(),
            )
            .boxed());
    }
    // Not the upstream body: without a length, tiny_http reads all of it even for HEAD.
    let reader: Box<dyn Read + Send> = if *request.method() == Method::Head {
        Box::new(Cursor::new(Vec::<u8>::new()))
    } else {
        Box::new(body)
    };
    Ok(Response::new(
        StatusCode(status),
        headers,
        reader,
        len,
        None,
    ))
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
mod tests {
    use super::*;

    #[test]
    fn request_slots_should_bound_active_streams_and_release_capacity() {
        let active = Arc::new(AtomicUsize::new(0));
        let mut slots: Vec<_> = (0..MAX_REQUESTS)
            .map(|_| RequestSlot::acquire(&active).unwrap())
            .collect();
        assert!(RequestSlot::acquire(&active).is_none());
        slots.pop();
        let replacement = RequestSlot::acquire(&active).unwrap();
        assert!(RequestSlot::acquire(&active).is_none());
        drop(replacement);
        drop(slots);
        assert_eq!(active.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn playlists_route_segments_keys_maps_and_alternate_audio() {
        let text = "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,URI=\"/audio.m3u8\"\n#EXT-X-KEY:METHOD=AES-128,URI=\"https://v.redd.it/key\"\n#EXT-X-MAP:URI=\"init.mp4\"\n../segment.ts\n";
        let base = Url::parse("https://v.redd.it/post/master.m3u8").unwrap();
        let rewritten = rewrite_playlist(text, &base, |u| {
            Ok(local_url("http://127.0.0.1:1/key/", u.as_str()))
        })
        .unwrap();
        assert_eq!(
            rewritten,
            "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,URI=\"http://127.0.0.1:1/key/https/v.redd.it/audio.m3u8\"\n#EXT-X-KEY:METHOD=AES-128,URI=\"http://127.0.0.1:1/key/https/v.redd.it/key\"\n#EXT-X-MAP:URI=\"http://127.0.0.1:1/key/https/v.redd.it/post/init.mp4\"\nhttp://127.0.0.1:1/key/https/v.redd.it/segment.ts\n"
        );
        for invalid in [
            "",
            "#EXTM3U\n#EXT-X-KEY:URI=https://v.redd.it/key",
            "#EXTM3U\n{$host}/segment.ts",
        ] {
            assert!(rewrite_playlist(invalid, &base, |u| Ok(u.to_string())).is_err());
        }
    }
}
