//! Direct Reddit access using an exported browser session.

mod accounts;
mod credentials;
pub mod experience;
mod home;
mod membership;
pub mod parse;
mod preferences;
mod rate;
mod relay;
mod session;
#[cfg(test)]
pub(crate) mod tests;
mod text;
mod transport;

pub use accounts::SavedAccount;
pub(crate) use accounts::saved_revision;
pub(crate) use membership::is_community;
pub(crate) use parse::merge_reply_continuations;
pub use preferences::{Preference, Preferences};
pub use rate::Pause;

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

use crate::model::{Listing, Post, PostPage, ReplyPage, Sort, TimeRange};
use transport::{Reader, Transport};

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

pub(crate) use rate::Priority;

pub(crate) const REQUEST_CANCELLED: &str = "Reddit request is no longer needed";

pub(crate) enum ListingEvent<'a> {
    Post(&'a Post),
    ContinuationFailed { cursor: &'a str, error: &'a str },
}

const API: &str = "https://www.reddit.com/";
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36";
const IMAGE_LIMIT: u64 = 64 * 1024 * 1024;
const DECODE_LIMIT: u64 = 512 * 1024 * 1024;
const STILL_DECODE_LIMIT: u64 = 128 * 1024 * 1024;
const READ_REUSE_GRACE: Duration = Duration::from_millis(500);

#[derive(Clone)]
pub struct Reddit(Arc<Inner>, bool, RequestContext);

#[derive(Clone, Default)]
struct RequestContext {
    priority: rate::Priority,
    retry_reads: bool,
    cancelled: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    active_cancelled: Option<transport::Cancellation>,
    read_aborted: Arc<AtomicBool>,
}

struct Inner {
    transport: Transport,
    api: Url,
    session: session::Session,
    account: Mutex<Option<Account>>,
    source: Mutex<Option<crate::auth::Source>>,
    saved_key: Mutex<Option<String>>,
    retired: AtomicBool,
    /// Never held across a request, so the UI can read it while one is in flight.
    status: Mutex<Status>,
    rate: rate::Rate,
    relay: OnceLock<relay::Relay>,
}

#[derive(Default)]
struct Status {
    /// Whether the latest request found Reddit up; `None` before the first.
    up: Option<bool>,
    rejected: bool,
}

/// Access refusals carry only an allowlisted reason, never the response body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessDenied {
    PrivateCommunity,
    Forbidden,
}

impl AccessDenied {
    pub fn message(self) -> &'static str {
        match self {
            Self::PrivateCommunity => {
                "This community is private; only approved members can view its posts"
            }
            Self::Forbidden => {
                "Reddit denied this request (HTTP 403); check your sign-in or access to this content"
            }
        }
    }
}

impl std::fmt::Display for AccessDenied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for AccessDenied {}

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
        self.0.rate.wake();
    }

    pub(crate) fn for_worker(
        &self,
        priority: rate::Priority,
        retry_reads: bool,
        cancelled: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Self {
        let cancelled: transport::Cancellation = Arc::new(cancelled);
        let demand = Arc::clone(&cancelled);
        let since = Mutex::new(None);
        let read_aborted = Arc::new(AtomicBool::new(false));
        let aborted = Arc::clone(&read_aborted);
        let active_cancelled: transport::Cancellation = Arc::new(move || {
            if aborted.load(Ordering::Acquire) {
                return true;
            }
            let obsolete = demand();
            let mut since = since.lock();
            if !obsolete {
                *since = None;
                return false;
            }
            let now = Instant::now();
            let expired =
                now.saturating_duration_since(*since.get_or_insert(now)) >= READ_REUSE_GRACE;
            if expired {
                aborted.store(true, Ordering::Release);
            }
            expired
        });
        Self(
            Arc::clone(&self.0),
            self.1,
            RequestContext {
                priority,
                retry_reads,
                cancelled: Some(cancelled),
                active_cancelled: Some(active_cancelled),
                read_aborted,
            },
        )
    }

    pub(crate) fn read_aborted(&self) -> bool {
        self.2.read_aborted.load(Ordering::Acquire)
    }

    pub fn pause(&self) -> Option<Pause> {
        self.0.rate.pause()
    }

    fn read_cancellation(&self) -> transport::Cancellation {
        let inner = Arc::clone(&self.0);
        let cancelled = self.2.active_cancelled.clone();
        Arc::new(move || {
            inner.retired.load(Ordering::Acquire)
                || cancelled.as_ref().is_some_and(|cancel| cancel())
        })
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
        Ok(Self(
            Arc::new(Inner {
                transport,
                api: Url::parse(api)?,
                session,
                account,
                source,
                saved_key,
                retired: AtomicBool::new(false),
                status: Mutex::new(Status::default()),
                rate: rate::Rate::new(if cfg!(test) {
                    Duration::ZERO
                } else {
                    Duration::from_secs(1)
                }),
                relay: OnceLock::new(),
            }),
            show_nsfw,
            RequestContext::default(),
        ))
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

    #[cfg(test)]
    pub fn test_at_paced(base: &str, interval: Duration) -> Self {
        let mut reddit = Self::test_at(base, false);
        Arc::get_mut(&mut reddit.0).unwrap().rate = rate::Rate::new(interval);
        reddit
    }

    /// Reuse the same account, transport and health without rereading credentials.
    pub fn with_nsfw(&self, show_nsfw: bool) -> Self {
        Self(Arc::clone(&self.0), show_nsfw, self.2.clone())
    }

    pub fn cache_namespace(&self) -> String {
        let identity = self.account().map_or_else(
            || self.0.session.fingerprint.clone(),
            |a| format!("account:{}", a.id),
        );
        format!(
            "reddit-web-v1\n{}\nnsfw={}\nsession={}",
            self.0.api, self.1, identity
        )
    }

    #[cfg(test)]
    pub fn listing(
        &self,
        subreddit: &str,
        sort: Sort,
        time: TimeRange,
        after: Option<&str>,
    ) -> Result<Listing> {
        self.listing_progress(subreddit, sort, time, after, &mut |_| {})
    }

    pub(crate) fn listing_progress(
        &self,
        subreddit: &str,
        sort: Sort,
        time: TimeRange,
        after: Option<&str>,
        progress: &mut dyn FnMut(ListingEvent<'_>),
    ) -> Result<Listing> {
        if sort == Sort::Best {
            bail!("Best is available on Home only");
        }
        let url = self.subreddit_url(subreddit, &format!("{}.json", sort.as_str()))?;
        self.fetch_listing(url, sort, time, after, progress)
    }

    pub fn home(&self, sort: Sort, time: TimeRange, after: Option<&str>) -> Result<Listing> {
        self.home_progress(sort, time, after, &mut |_| {})
    }

    pub(crate) fn home_progress(
        &self,
        sort: Sort,
        time: TimeRange,
        after: Option<&str>,
        progress: &mut dyn FnMut(ListingEvent<'_>),
    ) -> Result<Listing> {
        if sort == Sort::Best {
            return self.browser_home(after, progress);
        }
        let url = self.0.api.join(&format!("{}.json", sort.as_str()))?;
        self.fetch_listing(url, sort, time, after, progress)
    }

    fn home_partial(
        &self,
        cursor: Option<&str>,
        progress: &mut dyn FnMut(&Post),
    ) -> Result<home::Page> {
        let url = home::url(
            &self.0.api,
            cursor.unwrap_or("/svc/shreddit/feeds/home-feed?sort=BEST"),
        )?;
        let cancelled = self.read_cancellation();
        home::stream(
            transport::Reader::new(
                self.request(url, home::ACCEPT)?,
                16 * 1024 * 1024,
                Some(cancelled.clone()),
            ),
            &self.0.api,
            progress,
        )
        .map_err(|error| {
            if cancelled() {
                anyhow::anyhow!(REQUEST_CANCELLED)
            } else {
                error
            }
        })
    }

    fn browser_home(
        &self,
        after: Option<&str>,
        progress: &mut dyn FnMut(ListingEvent<'_>),
    ) -> Result<Listing> {
        let mut seen = HashSet::new();
        let mut emit = |post: &Post| {
            if seen.insert(post.id.clone()) && (self.1 || !post.nsfw) {
                progress(ListingEvent::Post(post));
            }
        };
        let mut page = self.home_partial(after, &mut emit)?;
        let mut failure = None;
        if after.is_none()
            && page.posts.len() <= 3
            && let Some(next) = page.after.clone()
        {
            match self
                .home_partial(Some(&next), &mut |_| {})
                .and_then(|continuation| {
                    if continuation.after.as_deref() == Some(&next) {
                        bail!("Reddit repeated a Home cursor");
                    }
                    Ok(continuation)
                }) {
                Ok(continuation) => {
                    for post in &continuation.posts {
                        emit(post);
                    }
                    let mut ids: HashSet<_> = page.posts.iter().map(|p| p.id.clone()).collect();
                    page.posts.extend(
                        continuation
                            .posts
                            .into_iter()
                            .filter(|p| ids.insert(p.id.clone())),
                    );
                    page.after = continuation.after;
                }
                Err(error) if error.to_string() == REQUEST_CANCELLED => return Err(error),
                Err(error) => failure = Some((next, error.to_string())),
            }
        }
        page.posts.retain(|post| self.1 || !post.nsfw);
        if let Some((cursor, error)) = &failure {
            progress(ListingEvent::ContinuationFailed { cursor, error });
        }
        Ok(Listing {
            posts: page.posts,
            after: page.after,
        })
    }

    fn fetch_listing(
        &self,
        mut url: Url,
        sort: Sort,
        time: TimeRange,
        after: Option<&str>,
        progress: &mut dyn FnMut(ListingEvent<'_>),
    ) -> Result<Listing> {
        self.listing_params(&mut url, after);
        if sort.takes_time() {
            url.query_pairs_mut().append_pair("t", time.as_str());
        }
        self.listing_response(url, progress)
    }

    fn listing_response(
        &self,
        url: Url,
        progress: &mut dyn FnMut(ListingEvent<'_>),
    ) -> Result<Listing> {
        let mut listing = transport::listing(
            self.request(url, "application/json")?,
            self.read_cancellation(),
            &mut |post| {
                if self.1 || !post.nsfw {
                    progress(ListingEvent::Post(post));
                }
            },
        )?;
        if !self.1 {
            listing.posts.retain(|post| !post.nsfw);
        }
        Ok(listing)
    }

    /// Follow every page before replacing the account's last complete subscription list.
    pub fn subscriptions(&self) -> Result<Vec<crate::model::Subscription>> {
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
            names.extend(page.into_iter().filter(|entry| {
                seen_names.insert((
                    entry.community().is_none(),
                    entry.name().to_ascii_lowercase(),
                ))
            }));
            let Some(next) = next else { break };
            if !cursors.insert(next.clone()) {
                bail!("Reddit repeated a subscription cursor");
            }
            after = Some(next);
        }
        names.sort_by_key(|entry| entry.name().to_ascii_lowercase());
        Ok(names)
    }

    pub(crate) fn user_posts_progress(
        &self,
        user: &str,
        sort: Sort,
        time: TimeRange,
        after: Option<&str>,
        progress: &mut dyn FnMut(ListingEvent<'_>),
    ) -> Result<Listing> {
        let user = experience::name(user)?;
        if !matches!(
            sort,
            Sort::Hot | Sort::New | Sort::Top | Sort::Controversial
        ) {
            bail!("Unsupported user-post sort");
        }
        let mut url = self.0.api.join(&format!("user/{user}/submitted.json"))?;
        url.query_pairs_mut().append_pair("sort", sort.as_str());
        self.fetch_listing(url, sort, time, after, progress)
    }

    #[cfg(test)]
    pub fn search(&self, subreddit: &str, query: &str, after: Option<&str>) -> Result<Listing> {
        self.search_progress(subreddit, query, after, &mut |_| {})
    }

    pub(crate) fn search_progress(
        &self,
        subreddit: &str,
        query: &str,
        after: Option<&str>,
        progress: &mut dyn FnMut(ListingEvent<'_>),
    ) -> Result<Listing> {
        let mut url = self.subreddit_url(subreddit, "search.json")?;
        self.listing_params(&mut url, after);
        url.query_pairs_mut()
            .append_pair("q", query)
            .append_pair("restrict_sr", "on")
            .append_pair("sort", "relevance")
            .append_pair("t", "all");
        self.listing_response(url, progress)
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
            .append_pair("include_over_18", if self.1 { "on" } else { "off" });
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
        let mut page = parse::parse_post_page_value(self.thread_json(permalink, sort, comment)?)?;
        parse::set_reply_sort(&mut page.comments, sort)?;
        if page.post.nsfw && !self.1 {
            bail!("This is NSFW; press F2 and enable Show NSFW to see it");
        }
        Ok(page)
    }

    fn thread_json(&self, permalink: &str, sort: &str, comment: Option<&str>) -> Result<Value> {
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
        self.get(url)
    }

    pub fn more_replies(&self, cursor: &str) -> Result<ReplyPage> {
        let more: parse::MoreRequest =
            serde_json::from_str(cursor).context("invalid reply cursor")?;
        more.validate()?;
        let deadline = std::cell::Cell::new(Instant::now() + Duration::from_secs(20));
        let fetch = |comment: Option<&str>, context: &str| -> Result<Value> {
            let mut url = self.0.api.join(&format!("comments/{}.json", more.post))?;
            url.query_pairs_mut()
                .append_pair("context", context)
                .append_pair("depth", "10")
                .append_pair("limit", "100")
                .append_pair("raw_json", "1");
            if let Some(comment) = comment {
                url.query_pairs_mut().append_pair("comment", comment);
            }
            if !more.sort.is_empty() {
                url.query_pairs_mut().append_pair("sort", &more.sort);
            }
            let timeout = deadline.get().saturating_duration_since(Instant::now());
            if timeout.is_zero() {
                bail!("Comment expansion timed out; retry to load more replies");
            }
            let (response, waited) =
                self.send_with_timeout(url, "application/json", timeout, |client, url| {
                    client.get(url)
                })?;
            deadline.set(deadline.get() + waited);
            transport::json(response, Some(self.read_cancellation()))
        };
        let mut page = if more.children.is_empty() {
            parse::focused_replies(fetch(more.parent.strip_prefix("t1_"), "0")?, &more)?
        } else {
            parse::expanded_replies(&more, |comment| fetch(Some(comment), "8"))?
        };
        parse::set_reply_sort(&mut page.comments, &more.sort)?;
        parse::set_more_sort(&mut page.more, &more.sort)?;
        Ok(page)
    }

    pub fn image(&self, url: &str) -> Result<Vec<u8>> {
        if self.0.retired.load(Ordering::Acquire) {
            bail!("This Reddit connection was replaced");
        }
        let mut bytes = Vec::new();
        let cancelled = self.read_cancellation();
        Reader::new(
            self.0.transport.media(url, None, Some(&cancelled))?,
            IMAGE_LIMIT,
            Some(cancelled),
        )
        .read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    pub fn still_image(&self, url: &str) -> Result<DynamicImage> {
        if self.0.retired.load(Ordering::Acquire) {
            bail!("This Reddit connection was replaced");
        }
        let cancelled = self.read_cancellation();
        decode_still(Reader::new(
            self.0.transport.media(url, None, Some(&cancelled))?,
            IMAGE_LIMIT,
            Some(cancelled),
        ))
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
            _ if self.pause().is_some() => Health::Paused,
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
        transport::json(
            self.request(url, "application/json")?,
            Some(self.read_cancellation()),
        )
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
        self.send_with_timeout(url, accept, Duration::from_secs(20), request)
            .map(|(response, _)| response)
    }

    fn send_with_timeout(
        &self,
        url: Url,
        accept: &str,
        timeout: Duration,
        request: impl FnOnce(&wreq::Client, &str) -> wreq::RequestBuilder,
    ) -> Result<(wreq::Response, Duration)> {
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
            .timeout(timeout);
        if accept == home::ACCEPT {
            request = request.header("Referer", self.0.api.as_str());
        }
        let request = request.build().map_err(|_| transport::network_error())?;
        let safe = request.method() == wreq::Method::GET;
        let retry = self.2.retry_reads && safe;
        let cancelled = safe.then(|| self.read_cancellation());
        let mut pending = Some(request);
        let mut waited = Duration::ZERO;
        let mut network = Duration::ZERO;
        let response = loop {
            let waiting = Instant::now();
            let admission = self.0.rate.admit(&url, self.2.priority, retry, || {
                self.0.retired.load(Ordering::Acquire)
                    || self.2.cancelled.as_ref().is_some_and(|cancel| cancel())
            })?;
            waited += waiting.elapsed();
            if self.needs_sign_in() {
                bail!(session::SIGN_IN);
            }
            let mut outgoing = if retry {
                pending.as_ref().and_then(wreq::Request::try_clone)
            } else {
                pending.take()
            }
            .context("Cannot repeat this Reddit request")?;
            outgoing
                .headers_mut()
                .insert("Cookie", self.0.session.header(url.path(), session::now())?);
            let available = timeout.saturating_sub(network);
            if available.is_zero() {
                bail!("Reddit request timed out; retry when ready");
            }
            let sent = Instant::now();
            let response = transport::wait(
                wreq::RequestBuilder::from_parts(self.0.transport.client.clone(), outgoing)
                    .timeout(available)
                    .send(),
                cancelled.as_ref(),
            )?
            .map_err(|_| {
                self.record(false);
                transport::network_error()
            })?;
            network += sent.elapsed();
            self.0
                .rate
                .observe(&response, &url, sent, admission.sequence, admission.older);
            self.record(!matches!(response.status().as_u16(), 401 | 500..));
            if response.status().as_u16() == 429 && retry {
                continue;
            }
            break response;
        };
        let status = response.status().as_u16();
        match status {
            200..=299 => Ok((response, waited)),
            401 => {
                self.0.status.lock().rejected = true;
                bail!(session::SIGN_IN);
            }
            403 => Err(transport::access_denied(response, cancelled)?.into()),
            404 => Err(NotFound.into()),
            429 => bail!("Reddit rate limit reached; wait before retrying"),
            300..=399 => bail!(
                "Reddit redirected the request (HTTP {status}); reconnect with A or redtab --signin"
            ),
            _ => bail!("Reddit: HTTP {status}"),
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
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(STILL_DECODE_LIMIT);
    reader.limits(limits);
    reader.decode().map_err(|error| match error {
        image::ImageError::Limits(_) => {
            anyhow::anyhow!("Image too large for inline preview; o opens Reddit")
        }
        error => error.into(),
    })
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
    fn when_an_original_exceeds_inline_memory_limits_it_should_offer_the_reddit_handoff() {
        // RGB8000×6000 header (144MB), with a deliberately short pixel stream: reject
        // the declared allocation before attempting to decode those pixels.
        let png = [
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 31, 64, 0, 0, 23,
            112, 8, 2, 0, 0, 0, 190, 6, 115, 105, 0, 0, 0, 12, 73, 68, 65, 84, 120, 156, 99, 96,
            96, 96, 0, 0, 0, 4, 0, 1, 246, 23, 56, 85, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96,
            130,
        ];

        let error = decode_still(png.as_slice()).unwrap_err().to_string();

        assert_eq!(error, "Image too large for inline preview; o opens Reddit");
    }

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
