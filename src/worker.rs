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

use crate::cache::{CONTENT_TTL, Cache, IMAGE_TTL, Source};
use crate::model::{Listing, Media, PostPage, ReplyPage, Sort, TimeRange};
use crate::reddit::Reddit;

/// Largest image edge sent to the terminal; bigger images only cost bandwidth.
const MAX_IMAGE_EDGE: u32 = 1600;
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum FeedQuery {
    Listing { sort: Sort, time: TimeRange },
    Search { query: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedTarget {
    Home,
    Subreddit(String),
}

impl FeedQuery {
    pub const DEFAULT_LISTING: Self = Self::Listing {
        sort: Sort::Hot,
        time: TimeRange::Day,
    };

    fn cache_request(&self, target: &FeedTarget, after: Option<&str>) -> Option<String> {
        match target {
            FeedTarget::Home => serde_json::to_string(&("browser-home-v1", self, after)).ok(),
            FeedTarget::Subreddit(name) => {
                serde_json::to_string(&("feed", name.to_lowercase(), self, after)).ok()
            }
        }
    }

    fn fetch(
        &self,
        reddit: &Reddit,
        target: &FeedTarget,
        after: Option<&str>,
    ) -> anyhow::Result<Listing> {
        match (target, self) {
            (FeedTarget::Home, Self::Listing { sort, time }) => reddit.home(*sort, *time, after),
            (FeedTarget::Subreddit(name), Self::Listing { sort, time }) => {
                reddit.listing(name, *sort, *time, after)
            }
            (FeedTarget::Subreddit(name), Self::Search { query }) => {
                reddit.search(name, query, after)
            }
            (FeedTarget::Home, Self::Search { .. }) => {
                anyhow::bail!("search runs inside a subreddit")
            }
        }
    }
}

/// Shared decoded pixels and a small retained preview, prepared by image workers.
#[derive(Debug, Clone)]
pub struct LoadedImage {
    pub image: Arc<DynamicImage>,
    pub preview: Arc<DynamicImage>,
    pub metadata: ImageMetadata,
}

#[derive(Debug, Clone, Copy)]
pub struct ImageMetadata {
    pub size: (u32, u32),
    pub flat: bool,
}

impl From<DynamicImage> for LoadedImage {
    fn from(image: DynamicImage) -> Self {
        let preview = if image.width() > 256 || image.height() > 256 {
            image.thumbnail(256, 256)
        } else {
            image.clone()
        };
        let preview = preview.to_rgba8();
        let flat = preview.pixels().next().is_none_or(|first| {
            preview.pixels().all(|pixel| pixel == first)
                || preview.pixels().all(|pixel| pixel[3] == 0)
        });
        Self {
            metadata: ImageMetadata {
                size: (image.width(), image.height()),
                flat,
            },
            image: Arc::new(image),
            preview: Arc::new(DynamicImage::ImageRgba8(preview)),
        }
    }
}

#[derive(Debug)]
pub enum Job {
    Experience {
        generation: u64,
        request: crate::reddit::experience::Request,
        invalidations: Vec<Invalidation>,
    },
    SortedPost {
        generation: u64,
        permalink: String,
        sort: String,
    },
    Subscriptions {
        generation: u64,
    },
    Membership {
        generation: u64,
        subreddit: String,
        join: bool,
    },
    Feed {
        tab: u64,
        generation: u64,
        target: FeedTarget,
        query: FeedQuery,
        after: Option<String>,
    },
    /// First page of a subreddit for All subs (also seeds its tab).
    Group {
        subreddit: String,
        generation: u64,
    },
    Post {
        generation: u64,
        permalink: String,
    },
    MoreReplies {
        generation: u64,
        comment_id: String,
        cursor: String,
        minimum: Option<Source>,
    },
    Image {
        url: String,
        /// A picture in post or comment text: fetched on the background queue, so a
        /// thread full of them never delays what the reader waits for.
        background: bool,
    },
    /// Small image for a list row or a subreddit icon.
    Thumb {
        url: String,
    },
    /// A subreddit's icon URL; it rarely changes, so it is cached like images.
    Icon {
        subreddit: String,
    },
    /// Fetch the post page to learn full media URLs (galleries, HLS video).
    ResolveMedia {
        permalink: String,
    },
    /// Save images to temp files so a desktop viewer can open them.
    DownloadImages {
        /// One download per post at a time.
        permalink: String,
        urls: Vec<String>,
    },
}

#[derive(Debug, Clone)]
pub enum Invalidation {
    Feed {
        target: FeedTarget,
        query: FeedQuery,
        after: Option<String>,
    },
    Post(String),
    Replies(String),
}

impl Invalidation {
    fn request(&self) -> Option<String> {
        match self {
            Self::Feed {
                target,
                query,
                after,
            } => query.cache_request(target, after.as_deref()),
            Self::Post(link) => post_cache_request(link),
            Self::Replies(cursor) => serde_json::to_string(&("replies", cursor)).ok(),
        }
    }
}

fn post_cache_request(link: &str) -> Option<String> {
    let (path, _) = crate::reddit::experience::thread_location(link).ok()?;
    serde_json::to_string(&("post-v2", crate::reddit::parse::post_id(&path)?)).ok()
}

impl Job {
    /// This job's reply for error `e`; used on panic so whatever waits still gets an answer.
    fn failed(&self, e: String) -> Msg {
        match self {
            Job::Experience { generation, .. } => Msg::Experience {
                generation: *generation,
                result: Err(e),
            },
            Job::SortedPost {
                generation, sort, ..
            } => Msg::SortedPost {
                generation: *generation,
                sort: sort.clone(),
                result: Err(e),
            },
            Job::Subscriptions { generation } => Msg::Subscriptions {
                generation: *generation,
                result: Err(e),
            },
            Job::Membership { generation, .. } => Msg::Membership {
                generation: *generation,
                result: Err(e),
            },
            Job::Feed {
                tab,
                generation,
                after,
                ..
            } => Msg::Feed {
                tab: *tab,
                generation: *generation,
                append: after.is_some(),
                result: Err(e),
            },
            Job::Group {
                subreddit,
                generation,
            } => Msg::Group {
                generation: *generation,
                subreddit: subreddit.clone(),
                result: Err(e),
            },
            Job::Post { generation, .. } => Msg::Post {
                generation: *generation,
                result: Err(e),
            },
            Job::MoreReplies {
                generation,
                comment_id,
                ..
            } => Msg::MoreReplies {
                generation: *generation,
                comment_id: comment_id.clone(),
                result: Err(e),
            },
            Job::Image { url, .. } => Msg::Image {
                url: url.clone(),
                result: Err(e),
            },
            Job::Thumb { url } => Msg::Thumb {
                url: url.clone(),
                result: Err(e),
            },
            Job::Icon { subreddit } => Msg::Icon {
                subreddit: subreddit.clone(),
                result: Err(e),
            },
            Job::ResolveMedia { permalink } => Msg::MediaResolved {
                permalink: permalink.clone(),
                result: Err(e),
            },
            Job::DownloadImages { permalink, .. } => Msg::ImagesDownloaded {
                permalink: permalink.clone(),
                result: Err(e),
            },
        }
    }
}

pub enum Msg {
    SortedPost {
        generation: u64,
        sort: String,
        result: Result<Box<PostPage>, String>,
    },
    Experience {
        generation: u64,
        result: Result<crate::reddit::experience::Response, String>,
    },
    Membership {
        generation: u64,
        result: Result<(), String>,
    },
    Subscriptions {
        generation: u64,
        result: Result<Vec<String>, String>,
    },
    /// Cached or freshly fetched content, with its original fetch time.
    Snapshot {
        message: Box<Msg>,
        fetched_at: i64,
        refreshing: bool,
        source: Source,
    },
    CacheWarning(String),
    Feed {
        tab: u64,
        generation: u64,
        append: bool,
        result: Result<Listing, String>,
    },
    Group {
        subreddit: String,
        generation: u64,
        result: Result<Listing, String>,
    },
    Post {
        generation: u64,
        result: Result<Box<PostPage>, String>,
    },
    MoreReplies {
        generation: u64,
        comment_id: String,
        result: Result<ReplyPage, String>,
    },
    Image {
        url: String,
        result: Result<LoadedImage, String>,
    },
    Thumb {
        url: String,
        result: Result<DynamicImage, String>,
    },
    Icon {
        subreddit: String,
        result: Result<Option<String>, String>,
    },
    MediaResolved {
        permalink: String,
        result: Result<Media, String>,
    },
    ImagesDownloaded {
        permalink: String,
        result: Result<Vec<PathBuf>, String>,
    },
    /// A queued thumbnail left the screen before its turn.
    ThumbSkipped {
        url: String,
    },
    /// A queued image no longer wanted when its turn came (its post closed, or it was
    /// scrolled far away).
    ImageSkipped {
        url: String,
    },
}

pub struct Pool {
    queues: Queues,
    wanted: Arc<Wanted>,
    cache: Option<Arc<Cache>>,
    threads: usize,
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Clone)]
struct Queues {
    priority: Sender<Work>,
    foreground_images: Sender<Work>,
    background: Sender<Work>,
    images: Sender<Work>,
    thumbs: Sender<Work>,
    icons: Sender<Work>,
}

struct Work {
    job: Job,
    force: bool,
    image_ticket: Option<Arc<AtomicBool>>,
}

impl Queues {
    fn submit(&self, work: Work) {
        let queue = match work.job {
            Job::Thumb { .. } => &self.thumbs,
            Job::Icon { .. } => &self.icons,
            Job::Image {
                background: true, ..
            } => &self.images,
            Job::Image {
                background: false, ..
            } => &self.foreground_images,
            Job::Group { .. } => &self.background,
            _ => &self.priority,
        };
        let _ = queue.send(work);
    }
}

type SharedRx = Arc<Mutex<Receiver<Work>>>;

/// URLs and pages whose queued fetches are still worth running; others are skipped.
#[derive(Default)]
struct Wanted {
    stopped: AtomicBool,
    /// Thumbnails drawn in the last frame.
    thumbs: Mutex<HashSet<String>>,
    /// Full-size images of the open post.
    images: Mutex<HashSet<String>>,
    image_tickets: Mutex<HashMap<String, Arc<AtomicBool>>>,
    /// Generation of the page load the open post waits for.
    post: Mutex<Option<u64>>,
}

impl Pool {
    pub fn stop(&self) {
        self.wanted.stopped.store(true, Ordering::Release);
    }
    pub fn start(reddit: Reddit, threads: usize, cache: Option<Cache>) -> (Self, Receiver<Msg>) {
        Self::start_shared(reddit, threads, cache.map(Arc::new))
    }

    pub fn restart(&self, reddit: Reddit) -> (Self, Receiver<Msg>) {
        Self::start_shared(reddit, self.threads, self.cache.clone())
    }

    fn start_shared(
        reddit: Reddit,
        threads: usize,
        cache: Option<Arc<Cache>>,
    ) -> (Self, Receiver<Msg>) {
        let (priority_tx, priority_rx) = channel::<Work>();
        let (background_tx, background_rx) = channel::<Work>();
        let (images_tx, images_rx) = channel::<Work>();
        let (thumbs_tx, thumbs_rx) = channel::<Work>();
        let (icons_tx, icons_rx) = channel::<Work>();
        let (msg_tx, msg_rx) = sync_channel::<Msg>(8);
        let wanted = Arc::<Wanted>::default();
        let namespace: Arc<str> = reddit.cache_namespace().into();
        let cached = |queue| {
            cache_reader(
                queue,
                msg_tx.clone(),
                Arc::clone(&wanted),
                cache.clone(),
                Arc::clone(&namespace),
            )
        };
        let queues = Queues {
            foreground_images: cached(priority_tx.clone()),
            priority: cached(priority_tx),
            background: cached(background_tx),
            images: cached(images_tx),
            thumbs: cached(thumbs_tx),
            icons: cached(icons_tx),
        };
        let background_threads = threads.saturating_sub(PRIORITY_THREADS).max(1);
        for (rx, count) in [
            (priority_rx, PRIORITY_THREADS),
            (background_rx, background_threads),
            (images_rx, 2),
            (thumbs_rx, THUMB_THREADS),
            (icons_rx, ICON_THREADS),
        ] {
            let rx: SharedRx = Arc::new(Mutex::new(rx));
            for _ in 0..count {
                spawn_worker(
                    Arc::clone(&rx),
                    msg_tx.clone(),
                    reddit.clone(),
                    Arc::clone(&wanted),
                    cache.clone(),
                    Arc::clone(&namespace),
                );
            }
        }
        (
            Self {
                queues,
                wanted,
                cache,
                threads,
            },
            msg_rx,
        )
    }

    pub fn submit(&self, job: Job) {
        self.enqueue(job, false)
    }

    fn enqueue(&self, job: Job, force: bool) {
        let image_ticket = match &job {
            Job::Image { url, .. } => self.wanted.image_tickets.lock().ok().map(|mut tickets| {
                Arc::clone(
                    tickets
                        .entry(url.clone())
                        .or_insert_with(|| Arc::new(AtomicBool::new(false))),
                )
            }),
            _ => None,
        };
        self.queues.submit(Work {
            job,
            force,
            image_ticket,
        });
    }

    /// Explicit reload bypasses cached content, leaving the displayed view in place.
    pub fn refresh(&self, job: Job) {
        self.enqueue(job, true)
    }

    /// Record the thumbnails on screen now, so scrolled-away ones are not fetched.
    pub fn set_on_screen(&self, urls: &[&str]) {
        if let Ok(mut set) = self.wanted.thumbs.lock() {
            // Usually unchanged. Spots can repeat a URL, so compare both ways.
            if urls.iter().all(|u| set.contains(*u))
                && set.iter().all(|s| urls.contains(&s.as_str()))
            {
                return;
            }
            set.clear();
            set.extend(urls.iter().map(|u| u.to_string()));
        }
    }

    /// An image the open post shows; call before submitting its `Job::Image`.
    pub fn want_image(&self, url: &str) {
        if let Ok(mut set) = self.wanted.images.lock() {
            set.insert(url.to_string());
        }
    }

    /// An image no longer worth fetching: its queued fetch is skipped.
    pub fn unwant_image(&self, url: &str) {
        if let Ok(mut set) = self.wanted.images.lock() {
            set.remove(url);
        }
    }

    /// The post closed: its queued image fetches are skipped.
    pub fn forget_images(&self) {
        if let Ok(mut set) = self.wanted.images.lock() {
            set.clear();
        }
    }

    /// The page load the open post waits for; queued loads of other generations are
    /// dropped. `None` once no post is open.
    pub fn want_post(&self, generation: Option<u64>) {
        if let Ok(mut post) = self.wanted.post.lock() {
            *post = generation;
        }
    }
}

/// Worker thread name; the panic hook leaves the screen alone on non-main threads.
const WORKER_THREAD: &str = "redtab-worker";

/// Cache hits remain usable while HTTP is busy, with media decoding isolated.
fn cache_reader(
    next: Sender<Work>,
    tx: SyncSender<Msg>,
    wanted: Arc<Wanted>,
    cache: Option<Arc<Cache>>,
    namespace: Arc<str>,
) -> Sender<Work> {
    let Some(cache) = cache else { return next };
    let (sender, receiver) = channel::<Work>();
    thread::Builder::new()
        .name("redtab-cache".into())
        .spawn(move || {
            while let Ok(work) = receiver.recv() {
                if wanted.stopped.load(Ordering::Acquire) {
                    return;
                }
                let job = &work.job;
                if superseded_post(job, &wanted)
                    || work
                        .image_ticket
                        .as_ref()
                        .is_some_and(|ticket| ticket.load(Ordering::Acquire))
                {
                    continue;
                }
                {
                    let mut claim = ImageClaim {
                        wanted: &wanted,
                        url: match job {
                            Job::Image { url, .. } => Some(url),
                            _ => None,
                        },
                        ticket: work.image_ticket.as_ref(),
                        claimed: false,
                    };
                    if let Some(message) = skipped_image(job, &wanted) {
                        if !claim.finish(&tx, message) {
                            return;
                        }
                        continue;
                    }
                    if !work.force
                        && cached_reply(job, Some(&cache), &namespace, &tx, &wanted, &mut claim)
                    {
                        continue;
                    }
                }
                // A miss/stale image stays unclaimed until HTTP starts, allowing promotion.
                if next.send(work).is_err() {
                    return;
                }
            }
        })
        .expect("cannot start cache reader");
    sender
}

fn spawn_worker(
    rx: SharedRx,
    tx: SyncSender<Msg>,
    reddit: Reddit,
    wanted: Arc<Wanted>,
    cache: Option<Arc<Cache>>,
    namespace: Arc<str>,
) {
    thread::Builder::new()
        .name(WORKER_THREAD.into())
        .spawn(move || {
            loop {
                let Work {
                    job,
                    force: _,
                    image_ticket,
                } = {
                    let Ok(guard) = rx.lock() else { return };
                    match guard.recv() {
                        Ok(job) => job,
                        Err(_) => return,
                    }
                };
                if wanted.stopped.load(Ordering::Acquire) {
                    return;
                }
                let mut claim = ImageClaim {
                    wanted: &wanted,
                    url: match &job {
                        Job::Image { url, .. } => Some(url),
                        _ => None,
                    },
                    ticket: image_ticket.as_ref(),
                    claimed: false,
                };
                if !claim.acquire() {
                    continue;
                }
                if superseded_post(&job, &wanted) {
                    continue;
                }
                if let Some(msg) = skipped_image(&job, &wanted) {
                    if !claim.finish(&tx, msg) {
                        return;
                    }
                    continue;
                }
                // Reserved as the request starts, so a later start orders after it.
                let request_order = cache
                    .as_deref()
                    .filter(|_| job.uses_cache() && !matches!(job, Job::ResolveMedia { .. }))
                    .and_then(|cache| match cache.reserve_request() {
                        Ok(order) => Some(order),
                        Err(e) => {
                            let _ =
                                tx.send(Msg::CacheWarning(format!("Could not prepare cache: {e}")));
                            None
                        }
                    });
                let started = NEXT_EVENT.fetch_add(1, Ordering::Relaxed);
                // A job's panic becomes its error instead of killing the worker.
                let msg = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let msg = run(&reddit, &job, &wanted);
                    if wanted.stopped.load(Ordering::Acquire) {
                        return job.failed("Connection replaced".into());
                    }
                    cache_result(
                        msg,
                        &job,
                        cache.as_deref(),
                        &namespace,
                        &tx,
                        Source {
                            order: request_order,
                            observed: Some((started, started)),
                            cached: false,
                        },
                    )
                }))
                .unwrap_or_else(|_| {
                    // "panicked at file:line:col:\nmessage" → one status line.
                    let detail = take_panic().unwrap_or_default().replace('\n', " ");
                    job.failed(format!("internal error in redtab, please report: {detail}"))
                });
                if !claim.finish(&tx, msg) {
                    return;
                }
            }
        })
        .expect("cannot start a worker thread");
}

/// Promoted foreground work and its old queued copy share one claim.
struct ImageClaim<'a> {
    wanted: &'a Wanted,
    url: Option<&'a String>,
    ticket: Option<&'a Arc<AtomicBool>>,
    claimed: bool,
}

impl ImageClaim<'_> {
    fn acquire(&mut self) -> bool {
        if self.claimed {
            return true;
        }
        if self
            .ticket
            .is_some_and(|ticket| ticket.swap(true, Ordering::AcqRel))
        {
            return false;
        }
        self.claimed = true;
        true
    }

    /// A receiver may immediately retry this URL, before this worker runs again.
    fn finish(&mut self, tx: &SyncSender<Msg>, message: Msg) -> bool {
        if !self.acquire() {
            return true;
        }
        self.release();
        tx.send(message).is_ok()
    }

    fn release(&mut self) {
        if self.claimed
            && let (Some(url), Some(ticket)) = (self.url, self.ticket)
            && let Ok(mut tickets) = self.wanted.image_tickets.lock()
            && tickets
                .get(url)
                .is_some_and(|current| Arc::ptr_eq(current, ticket))
        {
            tickets.remove(url);
        }
        self.ticket = None;
        self.claimed = false;
    }
}

impl Drop for ImageClaim<'_> {
    fn drop(&mut self) {
        self.release();
    }
}

/// Cache readers deliver snapshots before dispatching misses/stale refreshes to HTTP.
fn cached_reply(
    job: &Job,
    cache: Option<&Cache>,
    namespace: &str,
    tx: &SyncSender<Msg>,
    wanted: &Wanted,
    claim: &mut ImageClaim<'_>,
) -> bool {
    let Some(cache) = cache else {
        return false;
    };
    let Some((key, ttl)) = job.cache_key(namespace) else {
        return false;
    };
    let now = crate::app::now();
    let read_start = NEXT_EVENT.fetch_add(1, Ordering::Relaxed);
    let Some(entry) = cache.load(&key, now) else {
        return false;
    };
    let fresh = now.saturating_sub(entry.fetched_at) < ttl;
    if matches!(job, Job::ResolveMedia { .. }) && !fresh {
        return false;
    }
    let Some(message) = job.cached_message(&entry.bytes) else {
        return false;
    };
    if let Some(skipped) = skipped_image(job, wanted) {
        let _ = claim.finish(tx, skipped);
        return true;
    }
    let source = Source {
        order: Some(entry.order),
        observed: Some((read_start, NEXT_EVENT.fetch_add(1, Ordering::Relaxed))),
        cached: true,
    };
    let refreshing = !fresh
        || matches!(job, Job::MoreReplies { minimum: Some(minimum), .. } if !source.may_replace(*minimum));
    let message = Msg::Snapshot {
        message: Box::new(message),
        fetched_at: entry.fetched_at,
        refreshing,
        source,
    };
    let delivered = if refreshing {
        tx.send(message).is_ok()
    } else {
        claim.finish(tx, message)
    };
    if !delivered {
        return true;
    }
    if !refreshing {
        cache.touch(&key, now);
    }
    !refreshing
}

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

fn err_string(e: anyhow::Error) -> String {
    format!("{e:#}")
}

impl Job {
    /// Whether this job can be answered by a cached snapshot.
    fn uses_cache(&self) -> bool {
        matches!(
            self,
            Job::Subscriptions { .. }
                | Job::Feed { .. }
                | Job::Group { .. }
                | Job::Post { .. }
                | Job::ResolveMedia { .. }
                | Job::MoreReplies { .. }
                | Job::Image { .. }
                | Job::Thumb { .. }
                | Job::Icon { .. }
        )
    }

    fn cache_key(&self, namespace: &str) -> Option<(String, i64)> {
        let (request, ttl) = match self {
            Job::Subscriptions { .. } | Job::Membership { .. } => {
                ("subscriptions".to_string(), CONTENT_TTL)
            }
            Job::Feed {
                target,
                query,
                after,
                ..
            } => (query.cache_request(target, after.as_deref())?, CONTENT_TTL),
            Job::Group { subreddit, .. } => (
                FeedQuery::DEFAULT_LISTING
                    .cache_request(&FeedTarget::Subreddit(subreddit.clone()), None)?,
                CONTENT_TTL,
            ),
            Job::Post { permalink, .. } | Job::ResolveMedia { permalink } => {
                (post_cache_request(permalink)?, CONTENT_TTL)
            }
            Job::MoreReplies { cursor, .. } => (
                serde_json::to_string(&("replies", cursor)).ok()?,
                CONTENT_TTL,
            ),
            Job::Image { url, .. } => (
                serde_json::to_string(&("image", MAX_IMAGE_EDGE, url)).ok()?,
                IMAGE_TTL,
            ),
            Job::Thumb { url } => (serde_json::to_string(&("thumb", url)).ok()?, IMAGE_TTL),
            Job::Icon { subreddit } => (
                serde_json::to_string(&("icon", subreddit.to_ascii_lowercase())).ok()?,
                IMAGE_TTL,
            ),
            _ => return None,
        };
        Some((Cache::key(namespace, &request), ttl))
    }

    fn cached_message(&self, bytes: &[u8]) -> Option<Msg> {
        Some(match self {
            Job::Subscriptions { generation } => Msg::Subscriptions {
                generation: *generation,
                result: Ok(serde_json::from_slice(bytes).ok()?),
            },
            Job::Feed {
                tab,
                generation,
                after,
                ..
            } => Msg::Feed {
                tab: *tab,
                generation: *generation,
                append: after.is_some(),
                result: Ok(serde_json::from_slice(bytes).ok()?),
            },
            Job::Group {
                subreddit,
                generation,
            } => Msg::Group {
                generation: *generation,
                subreddit: subreddit.clone(),
                result: Ok(serde_json::from_slice(bytes).ok()?),
            },
            Job::Post { generation, .. } => Msg::Post {
                generation: *generation,
                result: Ok(serde_json::from_slice(bytes).ok()?),
            },
            Job::MoreReplies {
                generation,
                comment_id,
                ..
            } => Msg::MoreReplies {
                generation: *generation,
                comment_id: comment_id.clone(),
                result: Ok(serde_json::from_slice(bytes).ok()?),
            },
            Job::ResolveMedia { permalink } => Msg::MediaResolved {
                permalink: permalink.clone(),
                result: Ok(serde_json::from_slice::<PostPage>(bytes).ok()?.post.media),
            },
            Job::Thumb { url } => Msg::Thumb {
                url: url.clone(),
                result: Ok(image::load_from_memory(bytes).ok()?),
            },
            Job::Image { url, .. } => Msg::Image {
                url: url.clone(),
                result: Ok(image::load_from_memory(bytes).ok()?.into()),
            },
            Job::Icon { subreddit } => Msg::Icon {
                subreddit: subreddit.clone(),
                result: Ok(serde_json::from_slice(bytes).ok()?),
            },
            _ => return None,
        })
    }
}

/// A post page load for a post no longer shown (moved past in the list, or closed): queued
/// behind other loads it would only delay the one you wait for, so it is dropped unanswered.
fn superseded_post(job: &Job, wanted: &Wanted) -> bool {
    matches!(job, Job::Post { generation, .. } | Job::SortedPost { generation, .. }
        if wanted.post.lock().is_ok_and(|post| *post != Some(*generation)))
}

fn skipped_image(job: &Job, wanted: &Wanted) -> Option<Msg> {
    match job {
        Job::Thumb { url } if !wanted.thumbs.lock().is_ok_and(|set| set.contains(url)) => {
            Some(Msg::ThumbSkipped { url: url.clone() })
        }
        Job::Image { url, .. } if !wanted.images.lock().is_ok_and(|set| set.contains(url)) => {
            Some(Msg::ImageSkipped { url: url.clone() })
        }
        _ => None,
    }
}

fn cache_result(
    msg: Msg,
    job: &Job,
    cache: Option<&Cache>,
    namespace: &str,
    tx: &SyncSender<Msg>,
    source: Source,
) -> Msg {
    if let Job::Experience {
        request: crate::reddit::experience::Request::Write(_),
        invalidations,
        ..
    } = job
    {
        if let Some(cache) = cache {
            let result = cache.reserve_request().and_then(|order| {
                for key in invalidations.iter().filter_map(Invalidation::request) {
                    cache.store(&Cache::key(namespace, &key), &[], crate::app::now(), order)?;
                }
                Ok(())
            });
            if let Err(error) = result {
                let _ = tx.send(Msg::CacheWarning(format!(
                    "Could not invalidate changed content: {error}"
                )));
            }
        }
        return msg;
    }
    if matches!(msg, Msg::Membership { .. }) {
        if let Some(cache) = cache
            && let Some((key, _)) = job.cache_key(namespace)
        {
            // Reserve after the write finishes: reads begun during it must also lose.
            // Empty bytes are an ordered tombstone, never a valid subscription list.
            let result = cache
                .reserve_request()
                .and_then(|order| cache.store(&key, &[], crate::app::now(), order));
            if let Err(error) = result {
                let _ = tx.send(Msg::CacheWarning(format!(
                    "Could not invalidate subscription cache: {error}"
                )));
            }
        }
        return msg;
    }
    let ok = matches!(
        &msg,
        Msg::Subscriptions { result: Ok(_), .. }
            | Msg::Feed { result: Ok(_), .. }
            | Msg::Group { result: Ok(_), .. }
            | Msg::Post { result: Ok(_), .. }
            | Msg::MoreReplies { result: Ok(_), .. }
            | Msg::Image { result: Ok(_), .. }
            | Msg::Thumb { result: Ok(_), .. }
            | Msg::Icon { result: Ok(_), .. }
    );
    // Feed failures keep their order too, so an older failure can't mark a newer page failed.
    if !ok && !matches!(msg, Msg::Feed { .. }) {
        return msg;
    }
    let fetched_at = crate::app::now();
    if ok
        && let Some(cache) = cache
        && let Some(order) = source.order
        && let Some((key, _)) = job.cache_key(namespace)
    {
        let bytes = match &msg {
            Msg::Subscriptions {
                result: Ok(value), ..
            } => serde_json::to_vec(value).ok(),
            Msg::Feed {
                result: Ok(value), ..
            }
            | Msg::Group {
                result: Ok(value), ..
            } => serde_json::to_vec(value).ok(),
            Msg::Post {
                result: Ok(value), ..
            } => serde_json::to_vec(value).ok(),
            Msg::MoreReplies {
                result: Ok(value), ..
            } => serde_json::to_vec(value).ok(),
            Msg::Icon {
                result: Ok(value), ..
            } => serde_json::to_vec(value).ok(),
            Msg::Image {
                result: Ok(value), ..
            } => encode_png(&value.image),
            Msg::Thumb {
                result: Ok(value), ..
            } => encode_png(value),
            _ => None,
        };
        if let Some(bytes) = bytes
            && let Err(e) = cache.store(&key, &bytes, fetched_at, order)
        {
            let _ = tx.send(Msg::CacheWarning(format!("Could not save cache: {e}")));
        }
    }
    Msg::Snapshot {
        message: Box::new(msg),
        fetched_at,
        refreshing: false,
        source,
    }
}

fn encode_png(value: &DynamicImage) -> Option<Vec<u8>> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    value
        .write_to(&mut bytes, image::ImageFormat::Png)
        .ok()
        .map(|()| bytes.into_inner())
}

fn run(reddit: &Reddit, job: &Job, wanted: &Wanted) -> Msg {
    if let Some(message) = skipped_image(job, wanted) {
        return message;
    }
    match job {
        Job::Experience {
            generation,
            request,
            ..
        } => Msg::Experience {
            generation: *generation,
            result: reddit.experience(request).map_err(err_string),
        },
        Job::SortedPost {
            generation,
            permalink,
            sort,
        } => Msg::SortedPost {
            generation: *generation,
            sort: sort.clone(),
            result: reddit
                .sorted_post(permalink, sort)
                .map(Box::new)
                .map_err(err_string),
        },
        Job::Membership {
            generation,
            subreddit,
            join,
        } => Msg::Membership {
            generation: *generation,
            result: reddit
                .set_subscription(subreddit, *join)
                .map_err(err_string),
        },
        Job::Subscriptions { generation } => Msg::Subscriptions {
            generation: *generation,
            result: reddit.subscriptions().map_err(err_string),
        },
        Job::Feed {
            tab,
            generation,
            target,
            query,
            after,
        } => Msg::Feed {
            tab: *tab,
            generation: *generation,
            append: after.is_some(),
            result: query
                .fetch(reddit, target, after.as_deref())
                .map_err(err_string),
        },
        Job::Group {
            subreddit,
            generation,
        } => Msg::Group {
            generation: *generation,
            subreddit: subreddit.clone(),
            result: FeedQuery::DEFAULT_LISTING
                .fetch(reddit, &FeedTarget::Subreddit(subreddit.clone()), None)
                .map_err(err_string),
        },
        Job::Post {
            generation,
            permalink,
        } => Msg::Post {
            generation: *generation,
            result: reddit.post(permalink).map(Box::new).map_err(err_string),
        },
        Job::MoreReplies {
            generation,
            comment_id,
            cursor,
            ..
        } => Msg::MoreReplies {
            generation: *generation,
            comment_id: comment_id.clone(),
            result: reddit.more_replies(cursor).map_err(err_string),
        },
        Job::Image { url, .. } => {
            let result = fetch_image(reddit, url, MAX_IMAGE_EDGE);
            if let Some(skipped) = skipped_image(job, wanted) {
                return skipped;
            }
            Msg::Image {
                url: url.clone(),
                result: result.map(LoadedImage::from),
            }
        }
        Job::Thumb { url } => Msg::Thumb {
            url: url.clone(),
            result: fetch_image(reddit, url, MAX_THUMB_EDGE),
        },
        Job::Icon { subreddit } => Msg::Icon {
            subreddit: subreddit.clone(),
            result: reddit.icon(subreddit).map_err(err_string),
        },
        Job::ResolveMedia { permalink } => Msg::MediaResolved {
            permalink: permalink.clone(),
            result: reddit
                .post(permalink)
                .map(|page| page.post.media)
                .map_err(err_string),
        },
        Job::DownloadImages { permalink, urls } => Msg::ImagesDownloaded {
            permalink: permalink.clone(),
            result: download_images(reddit, urls).map_err(err_string),
        },
    }
}

/// Download and decode an image, shrunk to fit `max_edge`. A GIF gives its first frame
/// and the rest of the file is never read: animated GIFs play as video and can be huge.
fn fetch_image(reddit: &Reddit, url: &str, max_edge: u32) -> Result<DynamicImage, String> {
    reddit
        .still_image(url)
        .map(|img| {
            if img.width() > max_edge || img.height() > max_edge {
                img.thumbnail(max_edge, max_edge)
            } else {
                img
            }
        })
        .map_err(err_string)
}

/// Download folders in `$TMPDIR`: `redtab-media-<pid>-<hex>`.
const MEDIA_PREFIX: &str = "redtab-media-";
/// Downloads older than this go: a running redtab's own on its next download, and the
/// folder of an exited redtab once unchanged this long. A viewer opened just before still
/// has its files.
const MEDIA_GRACE: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// Private 0700 directory this process creates for downloads. An existing one is never
/// reused: a predictable /tmp path would let another user plant symlinks.
fn media_dir() -> anyhow::Result<PathBuf> {
    use std::os::unix::fs::DirBuilderExt;
    // Created under the lock and replaced if it disappears; only success is stored, so a
    // failure retries on the next `m`.
    static DIR: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);
    let mut cached = DIR
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Reused only while it is still this user's real folder: a tmp cleaner may have
    // removed it, and someone else may have put a symlink at its name.
    if let Some(dir) = cached.as_ref()
        && own_dir(&std::fs::symlink_metadata(dir).ok())
    {
        return Ok(dir.clone());
    }
    let pid = std::process::id();
    for attempt in 0..16u32 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let dir = std::env::temp_dir().join(format!("{MEDIA_PREFIX}{pid}-{nanos:x}{attempt}"));
        match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => return Ok(cached.insert(dir).clone()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    anyhow::bail!(
        "cannot create a private media directory in {}",
        std::env::temp_dir().display()
    )
}

/// A real directory (not a symlink) owned by this user.
fn own_dir(meta: &Option<std::fs::Metadata>) -> bool {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    meta.as_ref().is_some_and(|m| m.is_dir() && m.uid() == uid)
}

/// Removes downloads in this process's own folder older than `MEDIA_GRACE`, so a redtab
/// left running for days does not keep every image.
fn remove_old_downloads(dir: &std::path::Path, now: std::time::SystemTime) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        let old = meta
            .modified()
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= MEDIA_GRACE);
        if meta.is_file() && old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Removes download folders that redtab processes which have exited left in `$TMPDIR`,
/// once `MEDIA_GRACE` has passed. Folders of running redtabs stay.
pub fn remove_stale_media_dirs() {
    remove_stale_media_dirs_in(
        &std::env::temp_dir(),
        std::time::SystemTime::now(),
        process_running,
    );
}

fn remove_stale_media_dirs_in(
    tmp: &std::path::Path,
    now: std::time::SystemTime,
    running: impl Fn(u32) -> bool,
) {
    let Ok(entries) = std::fs::read_dir(tmp) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name();
        let Some((pid, rest)) = name
            .to_str()
            .and_then(|n| n.strip_prefix(MEDIA_PREFIX))
            .and_then(|n| n.split_once('-'))
        else {
            continue;
        };
        let Ok(pid) = pid.parse::<u32>() else {
            continue;
        };
        if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        // A real folder of this user only: never through a symlink.
        let meta = std::fs::symlink_metadata(entry.path()).ok();
        let unchanged = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|modified| now.duration_since(modified).ok())
            .unwrap_or_default();
        if own_dir(&meta) && unchanged >= MEDIA_GRACE && !running(pid) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Whether `pid` is a live process, whoever owns it.
fn process_running(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        // Signal 0 to these would address a process group: keep the folder.
        return true;
    }
    // SAFETY: signal 0 only checks that the process exists; nothing is delivered.
    let exists = unsafe { libc::kill(pid, 0) } == 0;
    exists || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn download_images(reddit: &Reddit, urls: &[String]) -> anyhow::Result<Vec<PathBuf>> {
    let dir = media_dir()?;
    remove_old_downloads(&dir, std::time::SystemTime::now());
    urls.iter()
        .enumerate()
        .map(|(i, url)| {
            let bytes = reddit.image(url)?;
            let ext = image::guess_format(&bytes)
                .ok()
                .and_then(|f| f.extensions_str().first().copied())
                .unwrap_or("img");
            let stem: String = url
                .split('?')
                .next()
                .unwrap_or_default()
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .split('.')
                .next()
                .unwrap_or_default()
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                .collect();
            let stem = if stem.is_empty() {
                format!("image-{i}")
            } else {
                stem
            };
            // A fresh name per download: never open an existing file.
            let mut n = 0;
            let path = loop {
                let name = if n == 0 {
                    format!("{stem}.{ext}")
                } else {
                    format!("{stem}-{n}.{ext}")
                };
                let path = dir.join(name);
                match std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                {
                    Ok(mut file) => {
                        std::io::Write::write_all(&mut file, &bytes)?;
                        break path;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
                    Err(e) => return Err(e.into()),
                }
            };
            Ok(path)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn when_a_write_finishes_a_cached_target_post_should_be_invalidated_even_after_an_error() {
        let directory = tempfile::tempdir().unwrap();
        let cache = Cache::new(directory.path().to_path_buf());
        let post = Job::Post {
            generation: 1,
            permalink: "/r/rust/comments/post/title/".into(),
        };
        let key = post.cache_key("account").unwrap().0;
        let old_order = cache.reserve_request().unwrap();
        cache
            .store(&key, b"old post", crate::app::now(), old_order)
            .unwrap();
        let job = Job::Experience {
            generation: 2,
            request: crate::reddit::experience::Request::Write(
                crate::reddit::experience::Action::Delete {
                    id: "t3_post".into(),
                },
            ),
            invalidations: vec![Invalidation::Post("/comments/post/_/comment/".into())],
        };
        let (sender, _) = std::sync::mpsc::sync_channel(1);
        cache_result(
            Msg::Experience {
                generation: 2,
                result: Err("ambiguous write".into()),
            },
            &job,
            Some(&cache),
            "account",
            &sender,
            Source::default(),
        );
        assert!(
            cache
                .load(&key, crate::app::now())
                .unwrap()
                .bytes
                .is_empty()
        );
        cache
            .store(&key, b"late old response", crate::app::now(), old_order)
            .unwrap();
        assert!(
            cache
                .load(&key, crate::app::now())
                .unwrap()
                .bytes
                .is_empty()
        );
    }

    #[test]
    fn when_a_comment_link_invalidates_a_post_it_should_use_the_same_key_as_the_canonical_post_link()
     {
        let original = Job::Post {
            generation: 1,
            permalink: "/r/rust/comments/post/title/".into(),
        }
        .cache_key("account")
        .unwrap()
        .0;
        let invalidation =
            Invalidation::Post("https://www.reddit.com/comments/post/_/comment/?context=3".into());
        let actual = Cache::key("account", &invalidation.request().unwrap());
        assert_eq!(actual, original);
    }

    #[test]
    fn pictures_in_text_should_wait_on_the_background_queue_not_the_one_the_reader_waits_on() {
        let (priority, priority_rx) = channel();
        let (background, _background_rx) = channel();
        let (thumbs, _thumbs_rx) = channel();
        let (icons, _icons_rx) = channel();
        let (images, images_rx) = channel();
        let queues = Queues {
            foreground_images: priority.clone(),
            images,
            priority,
            background,
            thumbs,
            icons,
        };
        let image = |background| Work {
            job: Job::Image {
                url: "https://preview.redd.it/x.png".into(),
                background,
            },
            force: false,
            image_ticket: None,
        };

        queues.submit(image(true));
        queues.submit(image(false));

        assert!(matches!(
            images_rx.try_recv(),
            Ok(Work {
                job: Job::Image {
                    background: true,
                    ..
                },
                ..
            })
        ));
        assert!(matches!(
            priority_rx.try_recv(),
            Ok(Work {
                job: Job::Image {
                    background: false,
                    ..
                },
                ..
            })
        ));
        assert!(
            priority_rx.try_recv().is_err(),
            "the text picture is not on it"
        );
    }

    #[test]
    fn an_icon_lookup_waits_on_neither_home_group_loads_nor_thumbnails() {
        let (priority, _priority_rx) = channel();
        let (background, background_rx) = channel();
        let (thumbs, thumbs_rx) = channel();
        let (icons, icons_rx) = channel();
        let (images, _images_rx) = channel();
        let queues = Queues {
            foreground_images: priority.clone(),
            images,
            priority,
            background,
            thumbs,
            icons,
        };

        queues.submit(Work {
            job: Job::Icon {
                subreddit: "s".into(),
            },
            force: false,
            image_ticket: None,
        });

        assert!(matches!(
            icons_rx.try_recv(),
            Ok(Work {
                job: Job::Icon { .. },
                ..
            })
        ));
        assert!(background_rx.try_recv().is_err());
        assert!(thumbs_rx.try_recv().is_err());
    }

    #[test]
    fn fetches_no_longer_wanted_are_skipped() {
        // The fixture client refuses Reddit media hosts: a fetch comes back as an error.
        let reddit = Reddit::test_at("http://127.0.0.1:9", false);
        let url = "https://i.redd.it/a.jpeg";
        let none = Wanted::default();
        let all = Wanted::default();
        all.thumbs.lock().unwrap().insert(url.into());
        all.images.lock().unwrap().insert(url.into());
        let thumb = || Job::Thumb { url: url.into() };
        let image = || Job::Image {
            url: url.into(),
            background: false,
        };

        let thumb_skipped = run(&reddit, &thumb(), &none);
        let thumb_fetched = run(&reddit, &thumb(), &all);
        let image_skipped = run(&reddit, &image(), &none);
        let image_fetched = run(&reddit, &image(), &all);

        assert!(matches!(thumb_skipped, Msg::ThumbSkipped { url: u } if u == url));
        assert!(matches!(thumb_fetched, Msg::Thumb { result: Err(_), .. }));
        assert!(
            matches!(image_skipped, Msg::ImageSkipped { url: u } if u == url),
            "a closed post's image is not fetched"
        );
        assert!(
            matches!(image_fetched, Msg::Image { result: Err(_), .. }),
            "the open post's image is fetched"
        );
    }

    struct CacheDir(PathBuf);
    impl CacheDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "redtab-worker-cache-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for CacheDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn when_redtab_starts_it_should_remove_only_old_media_folders_of_exited_processes() {
        let tmp = CacheDir::new();
        let folder = |name: &str| {
            let path = tmp.0.join(name);
            std::fs::create_dir(&path).unwrap();
            std::fs::write(path.join("a.jpg"), b"x").unwrap();
            path
        };
        let exited_old = folder("redtab-media-100-1a2b0");
        let exited_recent = folder("redtab-media-101-1a2b0");
        let running_old = folder("redtab-media-200-1a2b0");
        let other_name = folder("redtab-media-102-notes");
        let target = folder("unrelated");
        let link = tmp.0.join("redtab-media-103-1a2b0");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let file = tmp.0.join("redtab-media-104-1a2b0");
        std::fs::write(&file, b"x").unwrap();
        // Everything above was last changed a day and an hour before `now`; one folder
        // a minute short of a day.
        let now = std::time::SystemTime::now() + MEDIA_GRACE + std::time::Duration::from_secs(3600);
        std::fs::File::open(&exited_recent)
            .unwrap()
            .set_modified(now - MEDIA_GRACE + std::time::Duration::from_secs(60))
            .unwrap();

        remove_stale_media_dirs_in(&tmp.0, now, |pid| pid == 200);

        assert!(
            !exited_old.exists(),
            "an old folder of an exited redtab goes"
        );
        assert!(exited_recent.exists(), "within the grace period it stays");
        assert!(running_old.exists(), "a running redtab keeps its folder");
        assert!(other_name.exists(), "not a name redtab makes");
        assert!(link.symlink_metadata().is_ok() && target.join("a.jpg").exists());
        assert!(file.exists(), "not a folder");
    }

    #[test]
    fn a_running_process_counts_as_running_and_an_exited_one_does_not() {
        let mut exited = std::process::Command::new("true").spawn().unwrap();
        exited.wait().unwrap();

        assert!(process_running(std::process::id()));
        assert!(!process_running(exited.id()));
    }

    #[test]
    fn when_a_redtab_downloads_again_it_should_drop_its_own_downloads_older_than_a_day() {
        let dir = CacheDir::new();
        let old = dir.0.join("old.jpg");
        let recent = dir.0.join("recent.jpg");
        std::fs::write(&old, b"x").unwrap();
        std::fs::write(&recent, b"x").unwrap();
        let now = std::time::SystemTime::now() + MEDIA_GRACE;
        std::fs::File::options()
            .write(true)
            .open(&recent)
            .unwrap()
            .set_modified(now - MEDIA_GRACE + std::time::Duration::from_secs(60))
            .unwrap();

        remove_old_downloads(&dir.0, now);

        assert!(!old.exists(), "a day old: gone");
        assert!(recent.exists(), "a minute short of a day: kept");
    }

    fn next_message(messages: &Receiver<Msg>) -> Msg {
        messages
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("worker response")
    }

    fn data_request(server: &tiny_http::Server) -> Option<tiny_http::Request> {
        loop {
            let request = server
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap()?;
            if request.method() == &tiny_http::Method::Head {
                request.respond(tiny_http::Response::empty(200)).unwrap();
            } else {
                return Some(request);
            }
        }
    }

    fn poster_reply(request: tiny_http::Request) {
        let png = encode_png(&DynamicImage::new_rgb8(2, 2)).unwrap();
        request
            .respond(tiny_http::Response::from_data(png))
            .unwrap();
    }

    fn next_image(messages: &Receiver<Msg>) {
        loop {
            let message = match next_message(messages) {
                Msg::Snapshot { message, .. } => *message,
                message => message,
            };
            if let Msg::Image { result, .. } = message {
                assert!(result.is_ok(), "image result: {result:?}");
                return;
            }
        }
    }

    fn queued_image_pool() -> (Pool, Receiver<Work>) {
        let (sender, receiver) = channel();
        (
            Pool {
                queues: Queues {
                    foreground_images: sender.clone(),
                    priority: sender.clone(),
                    background: sender.clone(),
                    images: sender.clone(),
                    thumbs: sender.clone(),
                    icons: sender,
                },
                wanted: Arc::default(),
                cache: None,
                threads: 1,
            },
            receiver,
        )
    }

    #[test]
    fn when_a_terminal_image_is_received_it_should_allow_retry_before_the_old_worker_returns() {
        let (pool, work) = queued_image_pool();
        let url = "https://preview.redd.it/retry.png".to_owned();
        let job = || Job::Image {
            url: url.clone(),
            background: true,
        };
        pool.submit(job());
        let old_ticket = work.recv().unwrap().image_ticket.unwrap();
        assert!(!old_ticket.swap(true, Ordering::AcqRel));
        let mut claim = ImageClaim {
            wanted: &pool.wanted,
            url: Some(&url),
            ticket: Some(&old_ticket),
            claimed: true,
        };
        let (sender, receiver) = sync_channel(1);

        assert!(claim.finish(&sender, Msg::ImageSkipped { url: url.clone() }));
        assert!(matches!(next_message(&receiver), Msg::ImageSkipped { .. }));
        // The sender's guard is deliberately still alive when the receiver retries.
        pool.submit(job());
        let retry_ticket = work.recv().unwrap().image_ticket.unwrap();

        assert!(
            !retry_ticket.load(Ordering::Acquire),
            "a retry must have unclaimed work, even before the old guard drops"
        );
        assert!(
            old_ticket.load(Ordering::Acquire),
            "obsolete promotion copies must remain claimed"
        );
        drop(claim);
        assert!(Arc::ptr_eq(
            pool.wanted.image_tickets.lock().unwrap().get(&url).unwrap(),
            &retry_ticket,
        ));
    }

    #[test]
    fn when_cached_images_are_delivered_they_should_release_only_terminal_claims() {
        let dir = CacheDir::new();
        let cache = Cache::new(dir.0.clone());
        let (pool, work) = queued_image_pool();
        for (name, fresh, wanted) in [
            ("fresh", true, true),
            ("stale", false, true),
            ("skip", true, false),
        ] {
            let url = format!("https://preview.redd.it/{name}.png");
            let job = Job::Image {
                url: url.clone(),
                background: true,
            };
            let (key, _) = job.cache_key("test").unwrap();
            let now = crate::app::now();
            cache
                .store(
                    &key,
                    &encode_png(&DynamicImage::new_rgb8(2, 2)).unwrap(),
                    if fresh { now } else { now - IMAGE_TTL - 1 },
                    cache.reserve_request().unwrap(),
                )
                .unwrap();
            if wanted {
                pool.want_image(&url);
            }
            pool.submit(Job::Image {
                url: url.clone(),
                background: true,
            });
            let ticket = work.recv().unwrap().image_ticket.unwrap();
            let mut claim = ImageClaim {
                wanted: &pool.wanted,
                url: Some(&url),
                ticket: Some(&ticket),
                claimed: false,
            };
            let (sender, receiver) = sync_channel(1);

            let complete = cached_reply(
                &job,
                Some(&cache),
                "test",
                &sender,
                &pool.wanted,
                &mut claim,
            );
            let message = next_message(&receiver);

            assert_eq!(complete, fresh || !wanted, "{name}");
            assert_eq!(
                pool.wanted.image_tickets.lock().unwrap().contains_key(&url),
                !complete,
                "{name}"
            );
            assert!(
                matches!(message,
                    Msg::Snapshot { refreshing, .. } if wanted && refreshing != fresh
                ) || matches!(message, Msg::ImageSkipped { .. } if !wanted),
                "{name}"
            );
        }
    }

    #[test]
    fn when_subscription_preloads_are_blocked_a_card_image_should_still_start() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.server_addr());
        let (pool, messages) = Pool::start(Reddit::test_at(&base, false), 4, None);
        pool.submit(Job::Group {
            subreddit: "slow_a".into(),
            generation: 1,
        });
        pool.submit(Job::Group {
            subreddit: "slow_b".into(),
            generation: 1,
        });
        let first = data_request(&server).unwrap();
        let second = data_request(&server).unwrap();
        let url = format!("{base}/poster.png");
        pool.want_image(&url);

        pool.submit(Job::Image {
            url: url.clone(),
            background: true,
        });
        let poster = data_request(&server);

        first
            .respond(tiny_http::Response::from_string(include_str!(
                "../tests/fixtures/sub.json"
            )))
            .unwrap();
        second
            .respond(tiny_http::Response::from_string(include_str!(
                "../tests/fixtures/sub.json"
            )))
            .unwrap();
        let poster = poster.expect("visible media must not wait for subscription group requests");
        assert_eq!(poster.url(), "/poster.png");
        poster_reply(poster);
        next_image(&messages);
    }

    #[test]
    fn when_a_queued_card_image_is_promoted_it_should_start_once_on_the_priority_pool() {
        assert_queued_image_can_promote(None);
    }

    #[test]
    fn when_cache_lookup_misses_a_queued_card_image_should_still_allow_foreground_promotion() {
        let dir = CacheDir::new();
        assert_queued_image_can_promote(Some(Cache::new(dir.0.clone())));
    }

    fn assert_queued_image_can_promote(cache: Option<Cache>) {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.server_addr());
        let (pool, messages) = Pool::start(Reddit::test_at(&base, false), 3, cache);
        for name in ["first", "second"] {
            let url = format!("{base}/{name}.png");
            pool.want_image(&url);
            pool.submit(Job::Image {
                url,
                background: true,
            });
        }
        let first = data_request(&server).unwrap();
        let second = data_request(&server).unwrap();
        let url = format!("{base}/promoted.png");
        pool.want_image(&url);
        pool.submit(Job::Image {
            url: url.clone(),
            background: true,
        });

        pool.submit(Job::Image {
            url,
            background: false,
        });
        let promoted = data_request(&server);

        poster_reply(first);
        poster_reply(second);
        let promoted = promoted.expect("foreground image must bypass queued background images");
        assert_eq!(promoted.url(), "/promoted.png");
        poster_reply(promoted);
        for _ in 0..3 {
            next_image(&messages);
        }
        let extra = data_request(&server)
            .map(|request| (request.method().to_string(), request.url().to_string()));
        assert!(
            extra.is_none(),
            "promotion fetched again from its old queue: {extra:?}"
        );
    }

    #[test]
    fn when_priority_http_is_busy_warm_posts_feeds_and_images_should_arrive_from_cache() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.server_addr());
        let dir = CacheDir::new();
        let cache = Cache::new(dir.0.clone());
        let reddit = Reddit::test_at(&base, false);
        let post = Job::Post {
            generation: 7,
            permalink: "/r/rust/comments/p/title/".into(),
        };
        let feed = Job::Feed {
            tab: 3,
            generation: 1,
            target: FeedTarget::Subreddit("rust".into()),
            query: FeedQuery::DEFAULT_LISTING,
            after: None,
        };
        let image_url = format!("{base}/cached.png");
        let image = Job::Image {
            url: image_url.clone(),
            background: false,
        };
        let page =
            crate::reddit::parse::parse_post_page(include_str!("../tests/fixtures/post.json"))
                .unwrap();
        let listing =
            crate::reddit::parse::parse_listing(include_str!("../tests/fixtures/sub.json"))
                .unwrap();
        for (job, bytes) in [
            (&post, serde_json::to_vec(&page).unwrap()),
            (&feed, serde_json::to_vec(&listing).unwrap()),
            (&image, encode_png(&DynamicImage::new_rgb8(8, 4)).unwrap()),
        ] {
            let (key, _) = job.cache_key(&reddit.cache_namespace()).unwrap();
            cache
                .store(
                    &key,
                    &bytes,
                    crate::app::now(),
                    cache.reserve_request().unwrap(),
                )
                .unwrap();
        }
        let (pool, messages) = Pool::start(reddit, 3, Some(cache));
        for name in ["slow_a", "slow_b"] {
            let url = format!("{base}/{name}.png");
            pool.want_image(&url);
            pool.submit(Job::Image {
                url,
                background: false,
            });
        }
        let first = data_request(&server).unwrap();
        let second = data_request(&server).unwrap();
        pool.want_post(Some(7));
        pool.want_image(&image_url);

        pool.submit(post);
        pool.submit(feed);
        pool.submit(image);
        let snapshots: Vec<_> = (0..3)
            .map(|_| messages.recv_timeout(std::time::Duration::from_secs(1)))
            .collect();

        poster_reply(first);
        poster_reply(second);
        assert!(
            snapshots.iter().all(|message| matches!(
                message,
                Ok(Msg::Snapshot {
                    refreshing: false,
                    source: Source { cached: true, .. },
                    ..
                })
            )),
            "all three warm views must arrive before either occupied HTTP worker is released"
        );
        let variants: Vec<_> = snapshots
            .into_iter()
            .map(|message| match message.unwrap() {
                Msg::Snapshot { message, .. } => match *message {
                    Msg::Post { .. } => "post",
                    Msg::Feed { .. } => "feed",
                    Msg::Image { .. } => "image",
                    _ => panic!("expected a cached post, feed or image"),
                },
                _ => panic!("expected a cached snapshot"),
            })
            .collect();
        for expected in ["post", "feed", "image"] {
            assert!(variants.contains(&expected), "missing cached {expected}");
        }
    }

    #[test]
    fn when_the_account_list_is_cached_it_should_load_without_waiting_for_reddit() {
        let dir = CacheDir::new();
        let cache = Cache::new(dir.0.clone());
        let reddit = Reddit::test_at("http://127.0.0.1:9", false);
        let job = Job::Subscriptions { generation: 7 };
        let (key, _) = job.cache_key(&reddit.cache_namespace()).unwrap();
        cache
            .store(
                &key,
                br#"["neovim","rust"]"#,
                crate::app::now(),
                cache.reserve_request().unwrap(),
            )
            .unwrap();
        let (pool, messages) = Pool::start(reddit, 3, Some(cache));

        pool.submit(job);

        let Msg::Snapshot {
            message,
            source: Source { cached: true, .. },
            refreshing: false,
            ..
        } = next_message(&messages)
        else {
            panic!("fresh cached subscriptions")
        };
        let Msg::Subscriptions {
            generation: 7,
            result: Ok(names),
        } = *message
        else {
            panic!("subscription result")
        };
        assert_eq!(names, ["neovim", "rust"]);
    }

    #[test]
    fn when_membership_finishes_it_should_invalidate_reads_started_during_the_write() {
        let dir = CacheDir::new();
        let cache = Cache::new(dir.0.clone());
        let namespace = "membership-fixture";
        let read = Job::Subscriptions { generation: 1 };
        let job = Job::Membership {
            generation: 2,
            subreddit: "armenia".into(),
            join: true,
        };
        let (key, _) = read.cache_key(namespace).unwrap();
        let (tx, _rx) = sync_channel(8);
        for result in [Ok(()), Err("ambiguous write failure".into())] {
            let write_start = cache.reserve_request().unwrap();
            let late_read = cache.reserve_request().unwrap();
            cache
                .store(&key, br#"["rust"]"#, crate::app::now(), late_read)
                .unwrap();
            cache_result(
                Msg::Membership {
                    generation: 2,
                    result,
                },
                &job,
                Some(&cache),
                namespace,
                &tx,
                Source {
                    order: Some(write_start),
                    ..Source::default()
                },
            );
            cache
                .store(&key, br#"["rust"]"#, crate::app::now(), late_read)
                .unwrap();
            let entry = cache.load(&key, crate::app::now()).unwrap();
            assert!(
                entry.bytes.is_empty(),
                "a late read cannot resurrect pre-write membership"
            );
            assert!(read.cached_message(&entry.bytes).is_none());
            assert!(entry.order.sequence > late_read.sequence);
            let confirmed = cache.reserve_request().unwrap();
            cache
                .store(&key, br#"["armenia","rust"]"#, crate::app::now(), confirmed)
                .unwrap();
            cache
                .store(&key, br#"["rust"]"#, crate::app::now(), late_read)
                .unwrap();
            assert_eq!(
                cache.load(&key, crate::app::now()).unwrap().bytes,
                br#"["armenia","rust"]"#
            );
        }
    }

    #[test]
    fn when_membership_has_a_cached_list_it_should_still_write_then_fetch_the_new_account_list() {
        use crate::reddit::tests::{Fixture, Reply};
        use serde_json::json;
        let joined = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let actual = joined.clone();
        let fixture = Fixture::new(
            move |request| match request.url.split('?').next().unwrap() {
                "/api/me.json" => {
                    Reply::json(json!({"data":{"name":"reader","modhash":"fixture-csrf"}}))
                }
                "/api/subscribe" => {
                    assert_eq!(request.method, "POST");
                    actual.store(true, Ordering::SeqCst);
                    Reply::json(json!({}))
                }
                "/subreddits/mine/subscriber.json" => {
                    assert!(actual.load(Ordering::SeqCst));
                    Reply::json(
                        json!({"kind":"Listing","data":{"children":[{"kind":"t5","data":{"display_name":"armenia"}}],"after":null}}),
                    )
                }
                _ => panic!("unexpected membership job request"),
            },
        );
        let dir = CacheDir::new();
        let cache = Cache::new(dir.0.clone());
        let reddit = fixture.reddit();
        let (key, _) = (Job::Subscriptions { generation: 1 })
            .cache_key(&reddit.cache_namespace())
            .unwrap();
        cache
            .store(
                &key,
                br#"["rust"]"#,
                crate::app::now(),
                cache.reserve_request().unwrap(),
            )
            .unwrap();
        let (pool, messages) = Pool::start(reddit, 3, Some(cache));
        pool.submit(Job::Membership {
            generation: 2,
            subreddit: "armenia".into(),
            join: true,
        });
        assert!(matches!(
            next_message(&messages),
            Msg::Membership {
                generation: 2,
                result: Ok(())
            }
        ));
        assert!(joined.load(Ordering::SeqCst));
        pool.submit(Job::Subscriptions { generation: 3 });
        let Msg::Snapshot {
            message, source, ..
        } = next_message(&messages)
        else {
            panic!("fresh list")
        };
        assert!(!source.cached);
        assert!(
            matches!(*message, Msg::Subscriptions { generation: 3, result: Ok(names) } if names == ["armenia"])
        );
    }

    #[test]
    fn a_days_old_icon_answers_from_disk_without_asking_reddit() {
        let dir = CacheDir::new();
        let cache = Cache::new(dir.0.clone());
        let reddit = Reddit::test_at("http://127.0.0.1:9", false);
        let icons = [("withicon", Some("/i.png")), ("plain", None)];
        for (name, icon) in icons {
            let (key, _) = Job::Icon {
                subreddit: name.into(),
            }
            .cache_key(&reddit.cache_namespace())
            .unwrap();
            let order = cache.reserve_request().unwrap();
            let day_old = crate::app::now() - 86_400;
            let bytes = serde_json::to_vec(&icon).unwrap();
            cache.store(&key, &bytes, day_old, order).unwrap();
        }
        let namespace = reddit.cache_namespace();
        let (pool, messages) = Pool::start(reddit, 3, Some(cache));

        for (name, icon) in icons {
            pool.submit(Job::Icon {
                subreddit: name.to_uppercase(),
            });
            let Msg::Snapshot {
                message,
                refreshing: false,
                ..
            } = next_message(&messages)
            else {
                panic!("fresh disk snapshot")
            };
            let Msg::Icon { result, .. } = *message else {
                panic!("icon reply")
            };
            assert_eq!(result.unwrap().as_deref(), icon);
        }
        assert!(
            messages
                .recv_timeout(std::time::Duration::from_millis(300))
                .is_err()
        );
        // Read again counts as used: pruning keeps it over entries never read.
        let (key, _) = Job::Icon {
            subreddit: "plain".into(),
        }
        .cache_key(&namespace)
        .unwrap();
        let used = std::fs::metadata(dir.0.join(format!("{key}.cache")))
            .unwrap()
            .modified()
            .unwrap();
        assert!(used.elapsed().unwrap() < std::time::Duration::from_secs(60));
    }

    #[test]
    fn a_warm_feed_is_readable_after_the_server_shuts_down() {
        use std::io::{BufRead, BufReader, Write};
        let dir = CacheDir::new();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let reddit = Reddit::test_at(&format!("http://{}", listener.local_addr().unwrap()), false);
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(&mut stream);
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap() != 0 {
                if line == "\r\n" {
                    break;
                }
                line.clear();
            }
            let body = include_str!("../tests/fixtures/sub.json");
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        let job = || Job::Group {
            subreddit: "popular".into(),
            generation: 0,
        };
        let (cold, messages) = Pool::start(reddit.clone(), 3, Some(Cache::new(dir.0.clone())));
        cold.submit(job());
        let Msg::Snapshot {
            message,
            source: Source { cached: false, .. },
            ..
        } = next_message(&messages)
        else {
            panic!("network snapshot")
        };
        let Msg::Group {
            result: Ok(expected),
            ..
        } = *message
        else {
            panic!("parsed listing")
        };
        server.join().unwrap();
        drop(cold);
        let (warm, messages) = Pool::start(reddit, 3, Some(Cache::new(dir.0.clone())));

        warm.submit(job());

        let Msg::Snapshot {
            message,
            source: Source { cached: true, .. },
            refreshing: false,
            ..
        } = next_message(&messages)
        else {
            panic!("fresh disk snapshot")
        };
        let Msg::Group {
            result: Ok(actual), ..
        } = *message
        else {
            panic!("cached listing")
        };
        assert_eq!(actual, expected);
    }

    #[test]
    fn stale_content_arrives_before_a_failed_refresh() {
        let dir = CacheDir::new();
        let reddit = Reddit::test_at("http://127.0.0.1:9", false);
        let job = Job::Group {
            subreddit: "rust".into(),
            generation: 0,
        };
        let cache = Cache::new(dir.0.clone());
        let (key, _) = job.cache_key(&reddit.cache_namespace()).unwrap();
        let listing =
            crate::reddit::parse::parse_listing(include_str!("../tests/fixtures/sub.json"))
                .unwrap();
        cache
            .store(
                &key,
                &serde_json::to_vec(&listing).unwrap(),
                crate::app::now() - CONTENT_TTL,
                cache.reserve_request().unwrap(),
            )
            .unwrap();
        let (pool, messages) = Pool::start(reddit, 3, Some(cache));

        pool.submit(job);

        let Msg::Snapshot {
            message,
            source: Source { cached: true, .. },
            refreshing: true,
            ..
        } = next_message(&messages)
        else {
            panic!("stale disk snapshot first")
        };
        let Msg::Group {
            result: Ok(actual), ..
        } = *message
        else {
            panic!("cached listing")
        };
        assert_eq!(actual, listing);
        assert!(matches!(
            next_message(&messages),
            Msg::Group { result: Err(_), .. }
        ));
    }

    #[test]
    fn force_refresh_does_not_accept_a_fresh_disk_snapshot() {
        let dir = CacheDir::new();
        let reddit = Reddit::test_at("http://127.0.0.1:9", false);
        let job = Job::Post {
            generation: 1,
            permalink: "/r/rust/comments/p/title/".into(),
        };
        let cache = Cache::new(dir.0.clone());
        let (key, _) = job.cache_key(&reddit.cache_namespace()).unwrap();
        let page =
            crate::reddit::parse::parse_post_page(include_str!("../tests/fixtures/post.json"))
                .unwrap();
        cache
            .store(
                &key,
                &serde_json::to_vec(&page).unwrap(),
                crate::app::now(),
                cache.reserve_request().unwrap(),
            )
            .unwrap();
        let (pool, messages) = Pool::start(reddit, 3, Some(cache));
        pool.want_post(Some(1));

        pool.refresh(job);

        assert!(matches!(
            next_message(&messages),
            Msg::Post { result: Err(_), .. }
        ));
    }

    #[test]
    fn a_thumbnail_read_from_disk_counts_as_used() {
        let dir = CacheDir::new();
        let reddit = Reddit::test_at("http://127.0.0.1:9", false);
        let url = "https://i.redd.it/used.png";
        let job = Job::Thumb { url: url.into() };
        let cache = Cache::new(dir.0.clone());
        let (key, _) = job.cache_key(&reddit.cache_namespace()).unwrap();
        let image = DynamicImage::ImageRgb8(image::RgbImage::new(2, 2));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        let day_old = crate::app::now() - 86_400;
        let order = cache.reserve_request().unwrap();
        // Stored with its modification time at `day_old`.
        cache.store(&key, bytes.get_ref(), day_old, order).unwrap();
        let path = dir.0.join(format!("{key}.cache"));
        let (pool, messages) = Pool::start(reddit, 3, Some(cache));
        pool.set_on_screen(&[url]);

        pool.submit(job);

        assert!(matches!(
            next_message(&messages),
            Msg::Snapshot {
                refreshing: false,
                ..
            }
        ));
        // The reader touches the entry after it answers.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let used = || {
            let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
            modified.elapsed().unwrap() < std::time::Duration::from_secs(60)
        };
        while !used() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(used());
    }

    #[test]
    fn when_a_full_preview_leaves_memory_it_should_return_from_disk_without_http() {
        let dir = CacheDir::new();
        let reddit = Reddit::test_at("http://127.0.0.1:9", false);
        let url = "https://i.redd.it/preview.png";
        let job = Job::Image {
            url: url.into(),
            background: true,
        };
        let cache = Cache::new(dir.0.clone());
        let order = cache.reserve_request().unwrap();
        let (tx, _messages) = std::sync::mpsc::sync_channel(8);
        cache_result(
            Msg::Image {
                url: url.into(),
                result: Ok(DynamicImage::new_rgb8(8, 4).into()),
            },
            &job,
            Some(&cache),
            &reddit.cache_namespace(),
            &tx,
            Source {
                order: Some(order),
                ..Source::default()
            },
        );
        let (pool, messages) = Pool::start(reddit, 3, Some(cache));
        pool.want_image(url);

        pool.submit(job);

        let Msg::Snapshot {
            message,
            refreshing: false,
            source,
            ..
        } = next_message(&messages)
        else {
            panic!("fresh cached image");
        };
        assert!(source.cached);
        assert!(
            matches!(*message, Msg::Image { result: Ok(ref image), .. } if (image.image.width(), image.image.height()) == (8, 4))
        );
    }

    #[test]
    fn cached_thumbnail_arrives_before_a_failed_background_refresh() {
        let dir = CacheDir::new();
        let reddit = Reddit::test_at("http://127.0.0.1:9", false);
        let job = Job::Thumb {
            url: "https://i.redd.it/cached.png".into(),
        };
        let cache = Cache::new(dir.0.clone());
        let (key, _) = job.cache_key(&reddit.cache_namespace()).unwrap();
        let image =
            DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 2, image::Rgb([10, 20, 30])));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        cache
            .store(
                &key,
                bytes.get_ref(),
                crate::app::now() - IMAGE_TTL,
                cache.reserve_request().unwrap(),
            )
            .unwrap();
        let (pool, messages) = Pool::start(reddit, 3, Some(cache));
        pool.set_on_screen(&["https://i.redd.it/cached.png"]);

        pool.submit(job);

        let Msg::Snapshot {
            message,
            source: Source { cached: true, .. },
            ..
        } = next_message(&messages)
        else {
            panic!("cached thumbnail")
        };
        let Msg::Thumb {
            result: Ok(actual), ..
        } = *message
        else {
            panic!("decoded thumbnail")
        };
        assert_eq!(actual.to_rgb8().get_pixel(0, 0).0, [10, 20, 30]);
        assert!(matches!(
            next_message(&messages),
            Msg::Thumb { result: Err(_), .. }
        ));
    }

    #[test]
    fn stale_media_resolution_does_not_trigger_a_cached_viewer_action() {
        let dir = CacheDir::new();
        let reddit = Reddit::test_at("http://127.0.0.1:9", false);
        let job = Job::ResolveMedia {
            permalink: "/r/pics/comments/p/gallery/".into(),
        };
        let cache = Cache::new(dir.0.clone());
        let (key, _) = job.cache_key(&reddit.cache_namespace()).unwrap();
        let page =
            crate::reddit::parse::parse_post_page(include_str!("../tests/fixtures/gallery.json"))
                .unwrap();
        cache
            .store(
                &key,
                &serde_json::to_vec(&page).unwrap(),
                crate::app::now() - CONTENT_TTL,
                cache.reserve_request().unwrap(),
            )
            .unwrap();
        let (pool, messages) = Pool::start(reddit, 3, Some(cache));

        pool.submit(job);

        assert!(matches!(
            next_message(&messages),
            Msg::MediaResolved { result: Err(_), .. }
        ));
    }
    #[test]
    fn fresh_cached_replies_revalidate_when_the_parent_is_newer() {
        let dir = CacheDir::new();
        let cache = Cache::new(dir.0.clone());
        let older = cache.reserve_request().unwrap();
        let newer = cache.reserve_request().unwrap();
        let reddit = Reddit::test_at("http://127.0.0.1:9", false);
        let job = Job::MoreReplies {
            generation: 1,
            comment_id: "parent".into(),
            cursor: "parent".into(),
            minimum: Some(Source {
                order: Some(newer),
                ..Source::default()
            }),
        };
        let (key, _) = job.cache_key(&reddit.cache_namespace()).unwrap();
        cache
            .store(
                &key,
                &serde_json::to_vec(&ReplyPage::complete(Vec::new())).unwrap(),
                crate::app::now(),
                older,
            )
            .unwrap();
        let (pool, messages) = Pool::start(reddit, 3, Some(cache));

        pool.submit(job);

        assert!(matches!(
            next_message(&messages),
            Msg::Snapshot {
                refreshing: true,
                ..
            }
        ));
        assert!(matches!(
            next_message(&messages),
            Msg::MoreReplies { result: Err(_), .. }
        ));
    }
}
#[test]
fn when_home_and_a_subreddit_share_a_name_their_cache_keys_should_stay_separate() {
    let query = FeedQuery::Listing {
        sort: Sort::Best,
        time: TimeRange::Day,
    };
    assert_ne!(
        query.cache_request(&FeedTarget::Home, None),
        query.cache_request(&FeedTarget::Subreddit("home".into()), None)
    );
    assert_ne!(
        query.cache_request(&FeedTarget::Home, None),
        query.cache_request(&FeedTarget::Home, Some("t3_next"))
    );
}

#[test]
fn when_home_uses_the_browser_feed_it_should_not_reuse_legacy_json_snapshots() {
    let query = FeedQuery::Listing {
        sort: Sort::Best,
        time: TimeRange::Day,
    };
    let legacy = serde_json::to_string(&("home", &query, Option::<&str>::None)).unwrap();
    assert_ne!(
        query.cache_request(&FeedTarget::Home, None).unwrap(),
        legacy
    );
    assert_eq!(
        FeedQuery::DEFAULT_LISTING
            .cache_request(&FeedTarget::Subreddit("rust".into()), None)
            .unwrap(),
        r#"["feed","rust",{"Listing":{"sort":"Hot","time":"Day"}},null]"#
    );
}
