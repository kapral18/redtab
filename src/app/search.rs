//! The Search tab: live search with a debounce, result rows and a feed preview.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::experience::Field;
use super::{App, Feed, FeedUpdate, SEARCH_TAB, TabKind};
use crate::cache::{Freshness, Source};
use crate::config::Config;
use crate::model::{Listing, Post, SubscriptionIcon, UserProfile};
use crate::reddit::experience::{ListSearch, Query, Response, SearchKind};
use crate::worker::{FeedError, FeedQuery, FeedTarget, Job};

mod input;
mod preview;
mod rows;
mod types;

pub use types::*;

/// Typing pauses this long before the query is sent.
pub(crate) const DEBOUNCE: Duration = Duration::from_millis(450);
/// The selected row rests this long before its feed preview loads.
pub(crate) const PREVIEW_SETTLE: Duration = Duration::from_millis(300);
/// Reddit's search sometimes answers a matching query with nothing; an empty first
/// page is asked for again after these pauses before it counts as no matches.
pub(crate) const EMPTY_RETRY_DELAYS: [Duration; 2] =
    [Duration::from_secs(1), Duration::from_secs(3)];
