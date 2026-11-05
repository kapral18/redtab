//! Direct Reddit access using an exported browser session.

mod accounts;
mod credentials;
mod dash;
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
pub(crate) use dash::Manifest as VideoManifest;
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

mod connection;
mod images;
mod listings;
mod requests;
mod threads;

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
/// Reddit answers every `search.json` on www.reddit.com with HTTP 403, even signed in;
/// api.reddit.com serves the same search listings as JSON.
const CLASSIC_API: &str = "https://api.reddit.com/";
/// www.reddit.com answers `r/all` with 403 and api.reddit.com with an empty listing;
/// old.reddit.com still serves it.
const OLD_API: &str = "https://old.reddit.com/";
const CLASSIC_SESSION: &str =
    "This Reddit API request needs a reddit.com-wide session cookie; reconnect through Accounts";
const CLASSIC_REFUSED: &str =
    "Reddit refused this API request's session (HTTP 401); reconnect through Accounts";
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
    /// Search origin: `CLASSIC_API` beside the real `API`, otherwise the fixture `api`.
    classic: Url,
    /// r/all origin: `OLD_API` beside the real `API`, otherwise the fixture `api`.
    old: Url,
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
