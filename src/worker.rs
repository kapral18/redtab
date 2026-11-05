//! Background fetching. Separate queues keep jobs the user waits on ahead of All subs'
//! bulk fetches and media. Each class reads cache independently of HTTP workers;
//! the worker that starts a request reserves its cache order.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, channel, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread;

use image::DynamicImage;
use sha2::{Digest, Sha256};

use crate::cache::{CONTENT_TTL, Cache, IMAGE_TTL, Source};
use crate::model::{
    Listing, Media, MembershipTarget, Post, PostPage, ReplyPage, Sort, Subscription, TimeRange,
};
use crate::reddit::{AccessDenied, ListingEvent, Priority, Reddit};

mod cache_reader;
mod cache_write;
mod downloads;
mod jobs;
mod msg;
mod pool;
mod run;
mod threads;

use cache_reader::*;
use cache_write::*;
pub use downloads::*;
pub use jobs::*;
pub use msg::*;
pub use pool::*;
pub use run::*;
use threads::*;

/// Largest image edge sent to the terminal; bigger images only cost bandwidth.
const MAX_IMAGE_EDGE: u32 = crate::model::PREVIEW_EDGE;
/// Largest list-thumbnail edge; keeps the cache small.
const MAX_THUMB_EDGE: u32 = 128;
/// Threads reserved for jobs the user waits on.
const PRIORITY_THREADS: usize = 2;
/// Shared logical clock for disk-read windows and HTTP starts in this process.
static NEXT_EVENT: AtomicU64 = AtomicU64::new(1);
/// Threads for list thumbnails.
const THUMB_THREADS: usize = 2;
/// Thread for subreddit icon lookups: they wait neither behind island loads nor
/// hold up drawn thumbnails.
const ICON_THREADS: usize = 1;

thread_local! {
    /// This thread's last panic message, set by the panic hook.
    static LAST_PANIC: std::cell::Cell<Option<String>> = const { std::cell::Cell::new(None) };
}

/// Panic hook on non-main threads: store the message for the job's error.
pub fn note_panic(message: String) {
    LAST_PANIC.set(Some(message));
}

/// Takes this thread's last panic message.
pub fn take_panic() -> Option<String> {
    LAST_PANIC.take()
}

#[cfg(test)]
mod tests;
