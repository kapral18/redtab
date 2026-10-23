//! Direct Reddit access using an exported browser session.

mod accounts;
mod credentials;
pub mod experience;
mod home;
mod membership;
pub mod parse;
mod relay;
mod session;
#[cfg(test)]
pub(crate) mod tests;
mod text;
mod transport;

pub use accounts::SavedAccount;
pub(crate) use accounts::saved_revision;
pub(crate) use membership::is_community;

use std::collections::HashSet;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use image::DynamicImage;
use parking_lot::Mutex;
use serde_json::Value;
use url::Url;

use crate::model::{Listing, PostPage, ReplyPage, Sort, TimeRange};
use transport::{RUNTIME, Reader, Transport};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Account {
    pub id: String,
    pub name: String,
}

impl Account {
    fn valid(&self) -> bool {
        !self.id.is_empty()
            && self.id.len() <= 64
            && self.id.bytes().all(|c| c.is_ascii_alphanumeric())
            && !self.name.is_empty()
            && self.name.len() <= 64
            && self
                .name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
    }
}

pub fn sign_in(input: impl Read) -> Result<Account> {
    let text = session::read_import(input)?;
    let candidate = Reddit::from_export(&text, false)?;
    let account = candidate.verify_account()?;
    candidate.save_connection(None)?;
    Ok(account)
}

pub(crate) fn read_store() -> Result<String> {
    accounts::load(None)
}
pub(crate) fn saved_accounts() -> Result<Vec<SavedAccount>> {
    accounts::list()
}
pub(crate) fn read_account(id: &str) -> Result<String> {
    accounts::load(Some(id))
}
pub(crate) fn remove_store(id: &str) -> Result<()> {
    accounts::remove(id)
}
pub(crate) fn write_store(text: &str) -> Result<()> {
    accounts::save(text)
}
pub(crate) fn activate_account(id: &str, expected: &str, text: &str) -> Result<()> {
    accounts::activate(id, expected, text)
}

const API: &str = "https://www.reddit.com/";
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36";
const IMAGE_LIMIT: u64 = 64 * 1024 * 1024;
const DECODE_LIMIT: u64 = 512 * 1024 * 1024;

#[derive(Clone)]
pub struct Reddit(Arc<Inner>);

struct Inner {
    transport: Transport,
    api: Url,
    show_nsfw: bool,
    session: session::Session,
    account: Mutex<Option<Account>>,
    source: Mutex<Option<crate::auth::Source>>,
    saved_key: Mutex<Option<String>>,
    retired: AtomicBool,
    /// Never held across a request, so the UI can read it while one is in flight.
    status: Mutex<Status>,
    /// Reddit's morechildren contract allows one in-flight expansion per client.
    expansion: Mutex<()>,
    relay: OnceLock<relay::Relay>,
}

#[derive(Default)]
struct Status {
    /// Whether the latest request found Reddit up; `None` before the first.
    up: Option<bool>,
    cooldown: Option<Instant>,
    rejected: bool,
}

/// Reddit has no such resource (HTTP 404): an answer, unlike a refusal (403 can block the
/// account) or a failed request.
#[derive(Debug)]
struct NotFound;

impl std::fmt::Display for NotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Reddit content not found (HTTP 404)")
    }
}

impl std::error::Error for NotFound {}

/// The connection status, from the requests redtab already makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
    SignedOut,
    Unknown,
    Up,
    /// Requests wait out a cooldown, usually Reddit's rate limit.
    Paused,
    Down,
}

impl Reddit {
    pub fn disconnected(show_nsfw: bool) -> Result<Self> {
        Self::with_transport(
            Transport::new()?,
            show_nsfw,
            API,
            session::Session::disconnected(),
        )
    }

    pub fn from_export(text: &str, show_nsfw: bool) -> Result<Self> {
        Self::with_transport(
            Transport::new()?,
            show_nsfw,
            API,
            session::Session::parse(text, session::now())?,
        )
    }

    pub fn account(&self) -> Option<Account> {
        self.0.account.lock().clone()
    }

    pub(crate) fn saved_id(&self) -> Option<String> {
        self.0.saved_key.lock().clone()
    }

    pub(crate) fn from_saved(text: &str, show_nsfw: bool) -> Result<Self> {
        Self::with_transport(
            Transport::new()?,
            show_nsfw,
            API,
            session::Session::stored(text, session::now())?,
        )
    }

    pub fn source(&self) -> Option<crate::auth::Source> {
        self.0.source.lock().clone()
    }

    pub fn session_fingerprint(&self) -> &str {
        &self.0.session.fingerprint
    }

    pub fn needs_sign_in(&self) -> bool {
        !self.0.session.valid() || self.0.status.lock().rejected
    }

    pub fn retire(&self) {
        self.0.retired.store(true, Ordering::Release);
    }

    pub fn verify_account(&self) -> Result<Account> {
        let me = self.get(self.0.api.join("api/me.json")?)?;
        let account = Account {
            id: me["data"]["id"].as_str().unwrap_or_default().into(),
            name: me["data"]["name"].as_str().unwrap_or_default().into(),
        };
        if !account.valid() {
            bail!(
                "Reddit did not confirm a signed-in account; sign in in your browser and try again"
            );
        }
        *self.0.account.lock() = Some(account.clone());
        Ok(account)
    }

    pub(crate) fn verify_saved_account(&self) -> Result<()> {
        let expected = self.account();
        let actual = self.verify_account()?;
        if expected.is_some_and(|account| account.id != actual.id) {
            bail!("Saved session belongs to a different account; reconnect it from your browser");
        }
        Ok(())
    }

    pub fn save_connection(&self, source: Option<crate::auth::Source>) -> Result<()> {
        self.save_connection_with(source, crate::auth::save_export)
    }

    pub(crate) fn save_connection_with(
        &self,
        source: Option<crate::auth::Source>,
        save: impl FnOnce(&str) -> Result<()>,
    ) -> Result<()> {
        let account = self
            .account()
            .context("Verify the Reddit account before saving")?;
        save(&self.0.session.export(session::Connection {
            account: account.clone(),
            source: source.clone(),
        })?)?;
        *self.0.source.lock() = source;
        *self.0.saved_key.lock() = Some(account.id);
        Ok(())
    }
    pub fn new(show_nsfw: bool) -> Result<Self> {
        let session = session::Session::load()?;
        Self::with_transport(Transport::new()?, show_nsfw, API, session)
    }

    fn with_transport(
        transport: Transport,
        show_nsfw: bool,
        api: &str,
        session: session::Session,
    ) -> Result<Self> {
        let account = Mutex::new(session.connection.as_ref().map(|c| c.account.clone()));
        let source = Mutex::new(session.connection.as_ref().and_then(|c| c.source.clone()));
        let saved_key =
            Mutex::new((session.fingerprint != "disconnected").then(|| session.saved_id()));
        Ok(Self(Arc::new(Inner {
            transport,
            api: Url::parse(api)?,
            show_nsfw,
            session,
            account,
            source,
            saved_key,
            retired: AtomicBool::new(false),
            status: Mutex::new(Status::default()),
            expansion: Mutex::new(()),
            relay: OnceLock::new(),
        })))
    }

    /// Local HTTP fixtures exercise the same client/parser without contacting Reddit.
    #[cfg(test)]
    pub fn test_at(base: &str, show_nsfw: bool) -> Self {
        let mut transport = Transport::new().unwrap();
        transport.media_origin = Some(Url::parse(base).unwrap());
        Self::with_transport(
            transport,
            show_nsfw,
            &format!("{}/", base.trim_end_matches('/')),
            session::Session::parse(
                ".reddit.com\tTRUE\t/\tTRUE\t0\treddit_session\tfixture-session\n",
                0,
            )
            .unwrap(),
        )
        .unwrap()
    }

    pub fn cache_namespace(&self) -> String {
        let identity = self.account().map_or_else(
            || self.0.session.fingerprint.clone(),
            |a| format!("account:{}", a.id),
        );
        format!(
            "reddit-web-v1\n{}\nnsfw={}\nsession={}",
            self.0.api, self.0.show_nsfw, identity
        )
    }

    pub fn listing(
        &self,
        subreddit: &str,
        sort: Sort,
        time: TimeRange,
        after: Option<&str>,
    ) -> Result<Listing> {
        if sort == Sort::Best {
            bail!("Best is available on Home only");
        }
        let url = self.subreddit_url(subreddit, &format!("{}.json", sort.as_str()))?;
        self.fetch_listing(url, sort, time, after)
    }

    pub fn home(&self, sort: Sort, time: TimeRange, after: Option<&str>) -> Result<Listing> {
        if sort == Sort::Best {
            return self.browser_home(after);
        }
        let url = self.0.api.join(&format!("{}.json", sort.as_str()))?;
        self.fetch_listing(url, sort, time, after)
    }

    fn browser_home(&self, after: Option<&str>) -> Result<Listing> {
        let url = home::url(
            &self.0.api,
            after.unwrap_or("/svc/shreddit/feeds/home-feed?sort=BEST"),
        )?;
        let html = transport::html(self.request(url, home::ACCEPT)?)?;
        let page = home::parse(&html, &self.0.api)?;
        let mut details = std::collections::HashMap::new();
        for ids in page.ids.chunks(100) {
            let mut url = self.0.api.join(&format!("by_id/{}.json", ids.join(",")))?;
            url.query_pairs_mut().append_pair("raw_json", "1");
            for post in parse::parse_listing_value(self.get(url)?)?.posts {
                details.insert(format!("t3_{}", post.id), post);
            }
        }
        let posts = page
            .ids
            .into_iter()
            .filter_map(|id| details.remove(&id))
            .filter(|post| self.0.show_nsfw || !post.nsfw)
            .collect();
        Ok(Listing {
            posts,
            after: page.after,
        })
    }

    fn fetch_listing(
        &self,
        mut url: Url,
        sort: Sort,
        time: TimeRange,
        after: Option<&str>,
    ) -> Result<Listing> {
        self.listing_params(&mut url, after);
        if sort.takes_time() {
            url.query_pairs_mut().append_pair("t", time.as_str());
        }
        let mut listing = parse::parse_listing_value(self.get(url)?)?;
        if !self.0.show_nsfw {
            listing.posts.retain(|p| !p.nsfw);
        }
        Ok(listing)
    }

    /// Follow every page before replacing the account's last complete subscription list.
    pub fn subscriptions(&self) -> Result<Vec<String>> {
        let mut names = Vec::new();
        let mut seen_names = HashSet::new();
        let mut cursors = HashSet::new();
        let mut after: Option<String> = None;
        loop {
            let mut url = self.0.api.join("subreddits/mine/subscriber.json")?;
            url.query_pairs_mut()
                .append_pair("limit", "100")
                .append_pair("raw_json", "1");
            if let Some(cursor) = &after {
                url.query_pairs_mut().append_pair("after", cursor);
            }
            let (page, next) = parse::parse_subscriptions(self.get(url)?)?;
            names.extend(
                page.into_iter()
                    .filter(|name| seen_names.insert(name.to_ascii_lowercase())),
            );
            let Some(next) = next else { break };
            if !cursors.insert(next.clone()) {
                bail!("Reddit repeated a subscription cursor");
            }
            after = Some(next);
        }
        names.sort_by_key(|name| name.to_ascii_lowercase());
        Ok(names)
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

    /// The subreddit's icon URL, if it has one.
    pub fn icon(&self, subreddit: &str) -> Result<Option<String>> {
        let mut url = self.subreddit_url(subreddit, "about.json")?;
        url.query_pairs_mut().append_pair("raw_json", "1");
        let about = match self.get(url) {
            // The subreddit's answer, kept like an icon: asking again gets the same 404.
            Err(e) if e.is::<NotFound>() => return Ok(None),
            result => result?,
        };
        Ok(["community_icon", "icon_img"].into_iter().find_map(|key| {
            about["data"][key]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        }))
    }

    pub fn post(&self, permalink: &str) -> Result<PostPage> {
        self.sorted_post(permalink, "")
    }

    pub fn sorted_post(&self, permalink: &str, sort: &str) -> Result<PostPage> {
        self.focused_post(permalink, sort, None)
    }

    pub fn focused_post(
        &self,
        permalink: &str,
        sort: &str,
        comment: Option<&str>,
    ) -> Result<PostPage> {
        if !matches!(
            sort,
            "" | "confidence" | "top" | "new" | "old" | "controversial" | "qa"
        ) {
            bail!("Invalid comment sort");
        }
        // Use only the post id; no slug or external URL can change the authenticated origin.
        let id = parse::post_id(permalink).context("invalid post permalink")?;
        let mut url = self.0.api.join(&format!("comments/{id}.json"))?;
        url.query_pairs_mut()
            .append_pair("raw_json", "1")
            .append_pair("limit", "100")
            .append_pair("depth", "10");
        if !sort.is_empty() {
            url.query_pairs_mut().append_pair("sort", sort);
        }
        if let Some(comment) = comment {
            if comment.is_empty() || !comment.bytes().all(|c| c.is_ascii_alphanumeric()) {
                bail!("Invalid comment identity");
            }
            url.query_pairs_mut()
                .append_pair("comment", comment)
                .append_pair("context", "3");
        }
        let mut page = parse::parse_post_page_value(self.get(url)?)?;
        parse::set_reply_sort(&mut page.comments, sort)?;
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
            if !more.sort.is_empty() {
                url.query_pairs_mut().append_pair("sort", &more.sort);
            }
            let mut page = parse::focused_replies(self.get(url)?, &more)?;
            parse::set_reply_sort(&mut page.comments, &more.sort)?;
            parse::set_more_sort(&mut page.more, &more.sort)?;
            return Ok(page);
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
        if !more.sort.is_empty() {
            url.query_pairs_mut().append_pair("sort", &more.sort);
        }
        let response = {
            let _expansion = self.0.expansion.lock();
            self.get(url)?
        };
        let mut page = parse::expanded_replies(response, &more)?;
        parse::set_reply_sort(&mut page.comments, &more.sort)?;
        parse::set_more_sort(&mut page.more, &more.sort)?;
        Ok(page)
    }

    pub fn image(&self, url: &str) -> Result<Vec<u8>> {
        if self.0.retired.load(Ordering::Acquire) {
            bail!("This Reddit connection was replaced");
        }
        let mut bytes = Vec::new();
        Reader::new(self.0.transport.media(url, None)?, IMAGE_LIMIT).read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    pub fn still_image(&self, url: &str) -> Result<DynamicImage> {
        if self.0.retired.load(Ordering::Acquire) {
            bail!("This Reddit connection was replaced");
        }
        decode_still(Reader::new(self.0.transport.media(url, None)?, IMAGE_LIMIT))
    }

    pub fn media_url(&self, url: &str) -> Result<String> {
        if self.0.retired.load(Ordering::Acquire) {
            bail!("This Reddit connection was replaced");
        }
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
        if self.needs_sign_in() {
            return Health::SignedOut;
        }
        let status = self.0.status.lock();
        match status.up {
            _ if status.rejected => Health::Down,
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
        let listing = self.home(Sort::Best, TimeRange::Day, None);
        match (listing, self.health()) {
            (Err(_), Health::Up) => Health::Down,
            (_, health) => health,
        }
    }

    fn get(&self, url: Url) -> Result<Value> {
        transport::json(self.request(url, "application/json")?)
    }

    fn request(&self, url: Url, accept: &str) -> Result<wreq::Response> {
        self.send(url, accept, |client, url| client.get(url))
    }

    fn send(
        &self,
        url: Url,
        accept: &str,
        request: impl FnOnce(&wreq::Client, &str) -> wreq::RequestBuilder,
    ) -> Result<wreq::Response> {
        if self.0.retired.load(Ordering::Acquire) {
            bail!("This Reddit connection was replaced");
        }
        if url.origin() != self.0.api.origin() {
            bail!("refusing to send Reddit cookies to another origin");
        }
        {
            let status = self.0.status.lock();
            if status.rejected {
                bail!(session::SIGN_IN);
            }
            if status.cooldown.is_some_and(|until| Instant::now() < until) {
                bail!("Reddit requests paused; wait before retrying");
            }
        }
        let cookie = self
            .0
            .session
            .header(url.path(), session::now())
            .inspect_err(|_| {
                self.record(false);
            })?;
        let mut request = request(&self.0.transport.client, url.as_str())
            .header("Cookie", cookie)
            .header("Accept", accept)
            .header("User-Agent", USER_AGENT)
            .timeout(Duration::from_secs(20));
        if accept == home::ACCEPT {
            request = request.header("Referer", self.0.api.as_str());
        }
        let response = RUNTIME.block_on(request.send()).map_err(|_| {
            self.record(false);
            transport::network_error()
        })?;
        let status = response.status().as_u16();
        self.observe_limits(&response);
        self.record(!matches!(status, 401 | 500..));
        match status {
            200..=299 => Ok(response),
            401 => {
                self.0.status.lock().rejected = true;
                bail!(session::SIGN_IN);
            }
            403 => bail!(
                "Reddit denied this request (HTTP 403); check your sign-in or access to this content"
            ),
            404 => Err(NotFound.into()),
            429 => bail!("Reddit rate limit reached; wait before retrying"),
            300..=399 => bail!(
                "Reddit redirected the request (HTTP {status}); reconnect with A or redtab --signin"
            ),
            _ => bail!("Reddit: HTTP {status}"),
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
