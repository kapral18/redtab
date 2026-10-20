//! Direct, read-only Reddit access using an anonymous mobile guest session.

pub mod parse;
mod relay;
#[cfg(test)]
mod tests;
mod text;
mod transport;

use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use image::DynamicImage;
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};
use url::Url;
use wreq::header::{HeaderMap, HeaderValue};

use crate::model::{Listing, PostPage, ReplyPage, Sort, TimeRange};
use transport::{RUNTIME, Reader, Transport};

const API: &str = "https://oauth.reddit.com/";
const AUTH: &str = "https://www.reddit.com/auth/v2/oauth/access-token/loid";
// Public Android application identifier, not a user credential or application secret.
const CLIENT_ID: &str = "ohXpoqrZYub1kg";
const USER_AGENT: &str = "Reddit/Version 2024.47.0/Build 2029755/Android 14";
const IMAGE_LIMIT: u64 = 64 * 1024 * 1024;
const DECODE_LIMIT: u64 = 512 * 1024 * 1024;

#[derive(Clone)]
pub struct Reddit(Arc<Inner>);

struct Inner {
    transport: Transport,
    api: Url,
    auth: String,
    show_nsfw: bool,
    session: Mutex<Option<Token>>,
    /// Never held across a request, so the UI can read it while one is in flight.
    status: Mutex<Status>,
    /// Reddit's morechildren contract allows one in-flight expansion per client.
    expansion: Mutex<()>,
    icons: Mutex<HashMap<String, (Instant, Option<String>)>>,
    relay: OnceLock<relay::Relay>,
}

#[derive(Default)]
struct Status {
    /// Whether the latest request found Reddit up; `None` before the first.
    up: Option<bool>,
    cooldown: Option<Instant>,
    /// A sign-in got no answer: until then, requests fail at once instead of each
    /// waiting out its own attempt.
    unreachable: Option<Instant>,
}

/// The connection status, from the requests redtab already makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
    Unknown,
    Up,
    /// Requests wait out a cooldown, usually Reddit's rate limit.
    Paused,
    Down,
}

#[derive(Clone)]
struct Token {
    headers: HeaderMap,
    expires: Instant,
    id: uuid::Uuid,
}

impl Reddit {
    pub fn new(show_nsfw: bool) -> Result<Self> {
        Self::with_transport(Transport::new()?, show_nsfw, API, AUTH)
    }

    fn with_transport(
        transport: Transport,
        show_nsfw: bool,
        api: &str,
        auth: &str,
    ) -> Result<Self> {
        Ok(Self(Arc::new(Inner {
            transport,
            api: Url::parse(api)?,
            auth: auth.to_owned(),
            show_nsfw,
            session: Mutex::new(None),
            status: Mutex::new(Status::default()),
            expansion: Mutex::new(()),
            icons: Mutex::new(HashMap::new()),
            relay: OnceLock::new(),
        })))
    }

    /// Local HTTP fixtures exercise the same client/parser without contacting Reddit.
    #[cfg(test)]
    pub fn test_at(base: &str, show_nsfw: bool) -> Self {
        let mut transport = Transport::new().unwrap();
        transport.media_origin = Some(Url::parse(base).unwrap());
        let reddit = Self::with_transport(
            transport,
            show_nsfw,
            &format!("{}/", base.trim_end_matches('/')),
            base,
        )
        .unwrap();
        *reddit.0.session.lock() = Some(Token {
            headers: HeaderMap::new(),
            expires: Instant::now() + Duration::from_secs(3600),
            id: uuid::Uuid::new_v4(),
        });
        reddit
    }

    pub fn cache_namespace(&self) -> String {
        // Deliberately excludes tokens and the temporary relay URL.
        format!("reddit-json-v1\n{}\nnsfw={}", self.0.api, self.0.show_nsfw)
    }

    pub fn listing(
        &self,
        subreddit: &str,
        sort: Sort,
        time: TimeRange,
        after: Option<&str>,
    ) -> Result<Listing> {
        let mut url = self.subreddit_url(subreddit, &format!("{}.json", sort.as_str()))?;
        self.listing_params(&mut url, after);
        if sort.takes_time() {
            url.query_pairs_mut().append_pair("t", time.as_str());
        }
        let mut listing = parse::parse_listing_value(self.get(url)?)?;
        if !self.0.show_nsfw {
            listing.posts.retain(|p| !p.nsfw);
        }
        if after.is_none() && !matches!(subreddit, "all" | "popular") {
            listing.icon = self.icon(subreddit);
        }
        Ok(listing)
    }

    pub fn search(&self, subreddit: &str, query: &str, after: Option<&str>) -> Result<Listing> {
        let mut url = self.subreddit_url(subreddit, "search.json")?;
        self.listing_params(&mut url, after);
        url.query_pairs_mut()
            .append_pair("q", query)
            .append_pair("restrict_sr", "on")
            .append_pair("sort", "relevance")
            .append_pair("t", "all");
        let mut listing = parse::parse_listing_value(self.get(url)?)?;
        if !self.0.show_nsfw {
            listing.posts.retain(|p| !p.nsfw);
        }
        Ok(listing)
    }

    fn subreddit_url(&self, subreddit: &str, suffix: &str) -> Result<Url> {
        if subreddit.is_empty()
            || !subreddit
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'+'))
        {
            bail!("invalid subreddit name");
        }
        Ok(self.0.api.join(&format!("r/{subreddit}/{suffix}"))?)
    }

    fn listing_params(&self, url: &mut Url, after: Option<&str>) {
        url.query_pairs_mut()
            .append_pair("limit", "25")
            .append_pair("raw_json", "1")
            .append_pair(
                "include_over_18",
                if self.0.show_nsfw { "on" } else { "off" },
            );
        if let Some(after) = after {
            url.query_pairs_mut().append_pair("after", after);
        }
    }

    fn icon(&self, subreddit: &str) -> Option<String> {
        let key = subreddit.to_ascii_lowercase();
        {
            let icons = self.0.icons.lock();
            if let Some((until, icon)) = icons.get(&key)
                && Instant::now() < *until
            {
                return icon.clone();
            }
        }
        let result = self
            .subreddit_url(subreddit, "about.json")
            .and_then(|mut url| {
                url.query_pairs_mut().append_pair("raw_json", "1");
                self.get(url)
            });
        let seconds = if result.is_ok() { 900 } else { 60 };
        let icon = result.ok().and_then(|v| {
            ["community_icon", "icon_img"].into_iter().find_map(|key| {
                v["data"][key]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            })
        });
        let mut icons = self.0.icons.lock();
        if icons.len() >= 256 {
            icons.retain(|_, (until, _)| *until > Instant::now());
        }
        if icons.len() >= 256 {
            icons.clear();
        }
        icons.insert(
            key,
            (Instant::now() + Duration::from_secs(seconds), icon.clone()),
        );
        icon
    }

    pub fn post(&self, permalink: &str) -> Result<PostPage> {
        // Use only the post id; no slug or external URL can change the authenticated origin.
        let id = parse::post_id(permalink).context("invalid post permalink")?;
        let mut url = self.0.api.join(&format!("comments/{id}.json"))?;
        url.query_pairs_mut()
            .append_pair("raw_json", "1")
            .append_pair("limit", "100")
            .append_pair("depth", "10");
        let page = parse::parse_post_page_value(self.get(url)?)?;
        if page.post.nsfw && !self.0.show_nsfw {
            bail!("this is NSFW; set show_nsfw = true in config.toml to see it");
        }
        Ok(page)
    }

    pub fn more_replies(&self, cursor: &str) -> Result<ReplyPage> {
        let more: parse::MoreRequest =
            serde_json::from_str(cursor).context("invalid reply cursor")?;
        more.validate()?;
        if more.children.is_empty() {
            let mut url = self.0.api.join(&format!("comments/{}.json", more.post))?;
            url.query_pairs_mut()
                .append_pair("comment", more.parent.trim_start_matches("t1_"))
                .append_pair("context", "0")
                .append_pair("depth", "10")
                .append_pair("limit", "100")
                .append_pair("raw_json", "1");
            return parse::focused_replies(self.get(url)?, &more);
        }
        let mut url = self.0.api.join("api/morechildren.json")?;
        url.query_pairs_mut()
            .append_pair("api_type", "json")
            .append_pair("raw_json", "1")
            .append_pair("link_id", &format!("t3_{}", more.post))
            .append_pair(
                "children",
                &more.children[..more.children.len().min(100)].join(","),
            );
        let response = {
            let _expansion = self.0.expansion.lock();
            self.get(url)?
        };
        parse::expanded_replies(response, &more)
    }

    pub fn image(&self, url: &str) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        Reader::new(self.0.transport.media(url, None)?, IMAGE_LIMIT).read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    pub fn still_image(&self, url: &str) -> Result<DynamicImage> {
        decode_still(Reader::new(self.0.transport.media(url, None)?, IMAGE_LIMIT))
    }

    pub fn media_url(&self, url: &str) -> Result<String> {
        let parsed = Url::parse(url).context("invalid media URL")?;
        if !self.0.transport.media_allowed(&parsed) {
            bail!("unsupported media host or protocol");
        }
        if self.0.relay.get().is_none() {
            let relay = relay::Relay::start(self.0.transport.clone())?;
            let _ = self.0.relay.set(relay);
        }
        Ok(self
            .0
            .relay
            .get()
            .expect("relay initialized")
            .url(parsed.as_str()))
    }

    pub fn health(&self) -> Health {
        let status = self.0.status.lock();
        match status.up {
            Some(false) => Health::Down,
            _ if status.cooldown.is_some_and(|until| Instant::now() < until) => Health::Paused,
            Some(true) => Health::Up,
            None => Health::Unknown,
        }
    }

    fn record(&self, up: bool) {
        self.0.status.lock().up = Some(up);
    }

    /// Asks Reddit for one listing, for `--check`, and returns the health it showed.
    pub fn check(&self) -> Health {
        let mut url = self.0.api.join("r/popular/hot.json").expect("static path");
        url.query_pairs_mut()
            .append_pair("limit", "1")
            .append_pair("raw_json", "1");
        // A refused `r/popular` means no feed loads, though Reddit answered.
        match (self.get(url), self.health()) {
            (Err(_), Health::Up) => Health::Down,
            (_, health) => health,
        }
    }

    fn get(&self, url: Url) -> Result<Value> {
        let mut retried = false;
        loop {
            let token = self.token()?;
            let response = RUNTIME
                .block_on(
                    self.0
                        .transport
                        .client
                        .get(url.as_str())
                        .headers(token.headers.clone())
                        .timeout(Duration::from_secs(20))
                        .send(),
                )
                .map_err(|_| {
                    self.record(false);
                    transport::network_error()
                })?;
            let status = response.status().as_u16();
            self.observe_limits(&response);
            if status == 401 && !retried {
                retried = true;
                let mut session = self.0.session.lock();
                if session.as_ref().is_some_and(|t| t.id == token.id) {
                    *session = None;
                }
                continue;
            }
            // Refusing one resource, such as a private subreddit, is Reddit answering.
            self.record(!matches!(status, 401 | 500..));
            match status {
                200..=299 => return transport::json(response),
                401 => bail!("Reddit rejected the guest session"),
                403 => bail!("Reddit denied guest access (HTTP 403)"),
                404 => bail!("Reddit content not found (HTTP 404)"),
                429 => bail!("Reddit rate limit reached; wait before retrying"),
                _ => bail!("Reddit: HTTP {status}"),
            }
        }
    }

    fn observe_limits(&self, response: &wreq::Response) {
        let number = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<f64>().ok())
                .filter(|n| n.is_finite())
        };
        if response.status().as_u16() == 429
            || number("x-ratelimit-remaining").is_some_and(|n| n < 1.0)
        {
            let seconds = number("retry-after")
                .or_else(|| number("x-ratelimit-reset"))
                .unwrap_or(60.0)
                .clamp(1.0, 86400.0);
            self.0.status.lock().cooldown = Some(Instant::now() + Duration::from_secs_f64(seconds));
        }
    }

    fn token(&self) -> Result<Token> {
        // Single flight across worker threads, including failures. Only expired/rejected
        // tokens refresh; rate limits never generate new guest identities.
        let mut session = self.0.session.lock();
        let now = Instant::now();
        {
            let status = self.0.status.lock();
            if status.unreachable.is_some_and(|until| now < until) {
                return Err(transport::network_error());
            }
            if status.cooldown.is_some_and(|until| now < until) {
                bail!("Reddit requests paused; wait before retrying");
            }
        }
        if let Some(token) = &*session
            && now < token.expires
        {
            return Ok(token.clone());
        }
        let token = self.authenticate()?;
        *session = Some(token.clone());
        Ok(token)
    }

    fn authenticate(&self) -> Result<Token> {
        let device = uuid::Uuid::new_v4().to_string();
        let response = RUNTIME
            .block_on(
                self.0
                    .transport
                    .client
                    .post(&self.0.auth)
                    .basic_auth(CLIENT_ID, Some(""))
                    .header("User-Agent", USER_AGENT)
                    .header("client-vendor-id", &device)
                    .header("X-Reddit-Device-Id", &device)
                    .header("x-reddit-retry", "algo=no-retries")
                    .header("x-reddit-compression", "1")
                    .header("x-reddit-qos", "10.000")
                    .header(
                        "x-reddit-media-codecs",
                        "available-codecs=video/avc, video/hevc, video/x-vnd.on2.vp9",
                    )
                    .header("Content-Type", "application/json; charset=UTF-8")
                    .json(&json!({"scopes": ["*", "email", "pii"]}))
                    .timeout(Duration::from_secs(10))
                    .send(),
            )
            .map_err(|_| {
                let mut status = self.0.status.lock();
                status.up = Some(false);
                status.unreachable = Some(Instant::now() + Duration::from_secs(5));
                transport::network_error()
            })?;
        self.observe_limits(&response);
        if !response.status().is_success() {
            // A rate limit is Reddit answering; any other refusal leaves no guest session.
            self.record(response.status().as_u16() == 429);
            self.hold_sign_in();
            bail!("guest authentication: HTTP {}", response.status().as_u16());
        }
        guest_token(response).inspect_err(|_| {
            self.record(false);
            self.hold_sign_in();
        })
    }

    /// Reddit refused a guest session: requests wait at least a minute, longer when a rate
    /// limit asked for it, so retries do not keep asking for new guest identities.
    fn hold_sign_in(&self) {
        let mut status = self.0.status.lock();
        let floor = Instant::now() + Duration::from_secs(60);
        status.cooldown = Some(status.cooldown.map_or(floor, |until| until.max(floor)));
    }
}

/// The guest session a successful authentication response carries.
fn guest_token(response: wreq::Response) -> Result<Token> {
    #[derive(Deserialize)]
    struct Reply {
        access_token: String,
        expires_in: u64,
    }
    let mut headers = HeaderMap::new();
    headers.insert("User-Agent", HeaderValue::from_static(USER_AGENT));
    for name in ["x-reddit-loid", "x-reddit-session"] {
        if let Some(value) = response.headers().get(name) {
            let mut value = value.clone();
            value.set_sensitive(true);
            headers.insert(name, value);
        }
    }
    let reply: Reply = serde_json::from_value(transport::json(response)?)
        .map_err(|_| anyhow::anyhow!("invalid guest authentication response"))?;
    if reply.access_token.is_empty() || reply.expires_in < 2 {
        bail!("invalid guest token or expiry");
    }
    let mut bearer = HeaderValue::from_str(&format!("Bearer {}", reply.access_token))
        .map_err(|_| anyhow::anyhow!("invalid guest token"))?;
    bearer.set_sensitive(true);
    headers.insert("Authorization", bearer);
    Ok(Token {
        headers,
        expires: Instant::now()
            + Duration::from_secs(reply.expires_in.saturating_sub(30).clamp(1, 86400)),
        id: uuid::Uuid::new_v4(),
    })
}

/// Decodes an image; a GIF only up to its first frame, reading nothing past it.
fn decode_still(mut r: impl Read) -> Result<DynamicImage> {
    let mut bytes = Vec::with_capacity(4);
    (&mut r).take(4).read_to_end(&mut bytes)?;
    if bytes.starts_with(b"GIF8") {
        return first_gif_frame(bytes.as_slice().chain(r));
    }
    r.read_to_end(&mut bytes)?;
    Ok(image::load_from_memory(&bytes)?)
}

/// A GIF's first frame on its canvas, as `image` decodes GIFs.
fn first_gif_frame(r: impl Read) -> Result<DynamicImage> {
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);
    options.set_memory_limit(gif::MemoryLimit::Bytes(
        DECODE_LIMIT.try_into().expect("non-zero limit"),
    ));
    let mut decoder = options.read_info(r).context("not a GIF")?;
    let (width, height) = (u32::from(decoder.width()), u32::from(decoder.height()));
    // The frame is bounded by the decoder; the canvas, a header field, is not.
    if u64::from(width) * u64::from(height) * 4 > DECODE_LIMIT {
        bail!("the GIF is too large: {width}×{height}");
    }
    let frame = decoder.read_next_frame()?.context("the GIF has no frame")?;
    let part = image::ImageBuffer::<image::Rgba<u8>, &[u8]>::from_raw(
        frame.width.into(),
        frame.height.into(),
        &frame.buffer[..],
    )
    .context("a cut GIF frame")?;
    let mut canvas = image::RgbaImage::new(width, height);
    image::imageops::replace(&mut canvas, &part, frame.left.into(), frame.top.into());
    Ok(DynamicImage::ImageRgba8(canvas))
}

#[cfg(test)]
mod image_tests {
    use super::*;

    /// A GIF of `frames` frames, each `(left, top, width, height, rgba)`, on a canvas of
    /// `screen`, without the trailer that ends the file.
    fn gif_without_trailer(
        screen: (u16, u16),
        frames: &[(u16, u16, u16, u16, Vec<u8>)],
    ) -> Vec<u8> {
        let mut encoder = gif::Encoder::new(Vec::new(), screen.0, screen.1, &[]).unwrap();
        for (left, top, width, height, rgba) in frames {
            let mut frame = gif::Frame::from_rgba(*width, *height, &mut rgba.clone());
            frame.left = *left;
            frame.top = *top;
            encoder.write_frame(&frame).unwrap();
        }
        let mut bytes = encoder.into_inner().unwrap();
        assert_eq!(bytes.pop(), Some(0x3B), "the trailer");
        bytes
    }

    /// Reading past what it was given fails.
    struct Nothing;

    impl Read for Nothing {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("read past the first frame"))
        }
    }

    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];

    #[test]
    fn when_a_gif_streams_its_first_frame_should_decode_without_reading_further() {
        let first = gif_without_trailer((2, 2), &[(0, 0, 2, 2, [RED, BLUE, BLUE, RED].concat())]);

        let actual = decode_still(first.as_slice().chain(Nothing))
            .unwrap()
            .to_rgba8();

        let pixels: Vec<[u8; 4]> = actual.pixels().map(|p| p.0).collect();
        assert_eq!(pixels, [RED, BLUE, BLUE, RED]);
    }

    #[test]
    fn when_the_first_frame_is_smaller_than_the_canvas_it_should_sit_at_its_offset() {
        let first = gif_without_trailer((2, 2), &[(1, 0, 1, 2, [RED, BLUE].concat())]);

        let actual = decode_still(first.as_slice().chain(Nothing))
            .unwrap()
            .to_rgba8();

        let clear = [0, 0, 0, 0];
        let pixels: Vec<[u8; 4]> = actual.pixels().map(|p| p.0).collect();
        assert_eq!(pixels, [clear, RED, clear, BLUE]);
    }

    #[test]
    fn when_a_gif_url_serves_a_png_it_should_still_decode() {
        // Reddit's `.gif?format=png8` video posters are PNG files.
        let mut png = Vec::new();
        image::RgbaImage::from_raw(1, 1, RED.to_vec())
            .unwrap()
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();

        let actual = decode_still(png.as_slice()).unwrap().to_rgba8();

        assert_eq!(actual.get_pixel(0, 0).0, RED);
    }

    #[test]
    fn when_a_gif_canvas_is_too_large_it_should_fail_without_allocating_it() {
        // 65535×65535 RGBA is 17 GB; its first frame is one pixel.
        let huge = gif_without_trailer((65535, 65535), &[(0, 0, 1, 1, RED.to_vec())]);

        let actual = decode_still(huge.as_slice().chain(Nothing));

        let error = actual.unwrap_err().to_string();
        assert!(error.contains("too large"), "{error}");
    }
}
