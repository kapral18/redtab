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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum FeedQuery {
    Listing { sort: Sort, time: TimeRange },
    Search { query: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedTarget {
    Home,
    Subreddit(String),
    User(String),
}

impl FeedQuery {
    pub const DEFAULT_LISTING: Self = Self::Listing {
        sort: Sort::Hot,
        time: TimeRange::Day,
    };

    fn cache_request(&self, target: &FeedTarget, after: Option<&str>) -> Option<String> {
        match target {
            FeedTarget::Home => serde_json::to_string(&("browser-home-v2", self, after)).ok(),
            FeedTarget::User(name) => {
                serde_json::to_string(&("user-submitted", name.to_lowercase(), self, after)).ok()
            }
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
        progress: &mut dyn FnMut(ListingEvent<'_>),
    ) -> anyhow::Result<Listing> {
        match (target, self) {
            (FeedTarget::Home, Self::Listing { sort, time }) => {
                reddit.home_progress(*sort, *time, after, progress)
            }
            (FeedTarget::Subreddit(name), Self::Listing { sort, time }) => {
                reddit.listing_progress(name, *sort, *time, after, progress)
            }
            (FeedTarget::Subreddit(name), Self::Search { query }) => {
                reddit.search_progress(name, query, after, progress)
            }
            (FeedTarget::User(name), Self::Listing { sort, time }) => {
                reddit.user_posts_progress(name, *sort, *time, after, progress)
            }
            (FeedTarget::Home | FeedTarget::User(_), Self::Search { .. }) => {
                anyhow::bail!("search runs inside a subreddit")
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FeedError {
    Access(AccessDenied),
    Other(String),
}

impl FeedError {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Access(access) => access.message(),
            Self::Other(message) => message,
        }
    }

    pub fn access(&self) -> Option<AccessDenied> {
        match self {
            Self::Access(access) => Some(*access),
            Self::Other(_) => None,
        }
    }
}

impl std::fmt::Display for FeedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<anyhow::Error> for FeedError {
    fn from(error: anyhow::Error) -> Self {
        match error.downcast_ref::<AccessDenied>() {
            Some(access) => Self::Access(*access),
            None => Self::Other(err_string(error)),
        }
    }
}

impl From<String> for FeedError {
    fn from(message: String) -> Self {
        Self::Other(message)
    }
}

impl From<&str> for FeedError {
    fn from(message: &str) -> Self {
        Self::Other(message.into())
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
    pub fingerprint: [u8; 32],
}

impl From<DynamicImage> for LoadedImage {
    fn from(image: DynamicImage) -> Self {
        let mut hash = Sha256::new();
        hash.update(image.width().to_le_bytes());
        hash.update(image.height().to_le_bytes());
        hash.update(image.to_rgba8().as_raw());
        let fingerprint = hash.finalize().into();
        let preview = if image.width() > 256 || image.height() > 256 {
            image.thumbnail(256, 256)
        } else {
            image.clone()
        };
        let preview = preview.to_rgba8();
        Self {
            metadata: ImageMetadata {
                size: (image.width(), image.height()),
                fingerprint,
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
        target: MembershipTarget,
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
    VideoFrame {
        url: String,
        background: bool,
    },
    VideoThumb {
        url: String,
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
            Self::Replies(cursor) => serde_json::to_string(&("replies-v3", cursor)).ok(),
        }
    }
}

fn post_cache_request(link: &str) -> Option<String> {
    let (path, _) = crate::reddit::experience::thread_location(link).ok()?;
    serde_json::to_string(&("post-v4", crate::reddit::parse::post_id(&path)?)).ok()
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
                result: Err(e.into()),
            },
            Job::Group {
                subreddit,
                generation,
            } => Msg::Group {
                generation: *generation,
                subreddit: subreddit.clone(),
                result: Err(e.into()),
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
            Job::Image { url, .. } | Job::VideoFrame { url, .. } => Msg::Image {
                url: url.clone(),
                result: Err(e),
            },
            Job::Thumb { url } | Job::VideoThumb { url } => Msg::Thumb {
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
    FeedFallback {
        tab: u64,
        generation: u64,
        listing: Listing,
        cursor: String,
        error: String,
    },
    FeedProgress {
        tab: u64,
        generation: u64,
        post: Box<Post>,
        source: Source,
    },
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
        result: Result<Vec<Subscription>, String>,
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
        result: Result<Listing, FeedError>,
    },
    Group {
        subreddit: String,
        generation: u64,
        result: Result<Listing, FeedError>,
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
    feeds: Sender<Work>,
    selected: Sender<Work>,
    foreground_images: Sender<Work>,
    background: Sender<Work>,
    images: Sender<Work>,
    thumbs: Sender<Work>,
    icons: Sender<Work>,
}

struct Work {
    job: Job,
    force: bool,
    cache_only: bool,
    image_ticket: Option<Arc<AtomicBool>>,
}

impl Queues {
    fn submit(&self, work: Work) {
        let queue = match work.job {
            Job::Thumb { .. } | Job::VideoThumb { .. } => &self.thumbs,
            Job::VideoFrame {
                background: true, ..
            } => &self.images,
            Job::VideoFrame {
                background: false, ..
            } => &self.foreground_images,
            Job::Icon { .. } => &self.icons,
            Job::Image {
                background: true, ..
            } => &self.images,
            Job::Image {
                background: false, ..
            } => &self.foreground_images,
            Job::Group { .. } => &self.background,
            Job::Feed { .. } => &self.feeds,
            Job::Post { .. } | Job::SortedPost { .. } | Job::MoreReplies { .. } => &self.selected,
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
    // None until a view binds demand; each entry tracks its latest dispatched generation.
    groups: Mutex<Option<HashMap<String, Option<u64>>>>,
    icons: Mutex<Option<HashSet<String>>>,
    feeds: Mutex<Option<FeedDemand>>,
}

struct FeedDemand {
    active: Option<u64>,
    requests: HashMap<u64, HashSet<u64>>,
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
        let (feeds_tx, feeds_rx) = channel::<Work>();
        let (selected_tx, selected_rx) = channel::<Work>();
        let (foreground_images_tx, foreground_images_rx) = channel::<Work>();
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
            feeds: cached(feeds_tx),
            foreground_images: cached(foreground_images_tx),
            selected: cached(selected_tx),
            priority: cached(priority_tx),
            background: cached(background_tx),
            images: cached(images_tx),
            thumbs: cached(thumbs_tx),
            icons: cached(icons_tx),
        };
        let background_threads = threads.saturating_sub(PRIORITY_THREADS).max(1);
        for (rx, count) in [
            (priority_rx, PRIORITY_THREADS),
            (feeds_rx, 2),
            (selected_rx, 2),
            (foreground_images_rx, 2),
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
        self.enqueue_policy(job, force, false);
    }

    pub fn cached(&self, job: Job) {
        if self.cache.is_some() {
            self.enqueue_policy(job, false, true);
        }
    }

    fn enqueue_policy(&self, job: Job, force: bool, cache_only: bool) {
        let image_ticket = match &job {
            Job::Image { url, .. } | Job::VideoFrame { url, .. } => {
                self.wanted.image_tickets.lock().ok().map(|mut tickets| {
                    Arc::clone(
                        tickets
                            .entry(url.clone())
                            .or_insert_with(|| Arc::new(AtomicBool::new(false))),
                    )
                })
            }
            _ => None,
        };
        if !cache_only
            && let Job::Group {
                subreddit,
                generation,
            } = &job
        {
            self.want_group(subreddit, *generation);
        }
        self.queues.submit(Work {
            job,
            force,
            cache_only,
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

    /// Replace demand atomically so shared card URLs are never briefly unwanted.
    pub fn retain_images<'a>(&self, urls: impl Iterator<Item = &'a String>) {
        if let Ok(mut set) = self.wanted.images.lock() {
            *set = urls.cloned().collect();
        }
    }

    pub fn set_visible_icons(&self, names: &[&str]) {
        if let Ok(mut icons) = self.wanted.icons.lock() {
            *icons = Some(names.iter().map(|name| name.to_ascii_lowercase()).collect());
        }
    }

    pub fn set_visible_groups(&self, names: &[&str]) {
        if let Ok(mut groups) = self.wanted.groups.lock() {
            let previous = groups.take().unwrap_or_default();
            *groups = Some(
                names
                    .iter()
                    .map(|name| ((*name).to_string(), previous.get(*name).copied().flatten()))
                    .collect(),
            );
        }
    }

    pub fn want_group(&self, name: &str, generation: u64) {
        if let Ok(mut groups) = self.wanted.groups.lock()
            && let Some(current) = groups.as_mut().and_then(|groups| groups.get_mut(name))
        {
            *current = Some(current.unwrap_or(generation).max(generation));
        }
    }

    /// The page load the open post waits for; queued loads of other generations are
    /// dropped. `None` once no post is open.
    pub fn want_post(&self, generation: Option<u64>) {
        if let Ok(mut post) = self.wanted.post.lock() {
            *post = generation;
        }
    }

    pub fn want_feeds(&self, active: Option<u64>, requests: HashMap<u64, HashSet<u64>>) {
        if let Ok(mut feeds) = self.wanted.feeds.lock() {
            *feeds = Some(FeedDemand { active, requests });
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
                if superseded_post(job, &wanted) {
                    if matches!(job, Job::Post { .. })
                        && tx
                            .send(job.failed(crate::reddit::REQUEST_CANCELLED.into()))
                            .is_err()
                    {
                        return;
                    }
                    continue;
                }
                if work
                    .image_ticket
                    .as_ref()
                    .is_some_and(|ticket| ticket.load(Ordering::Acquire))
                {
                    continue;
                }
                if !work.cache_only && background_unwanted(job, &wanted) {
                    if tx
                        .send(job.failed(crate::reddit::REQUEST_CANCELLED.into()))
                        .is_err()
                    {
                        return;
                    }
                    continue;
                }
                {
                    let mut claim = ImageClaim {
                        wanted: &wanted,
                        url: match job {
                            Job::Image { url, .. } | Job::VideoFrame { url, .. } => Some(url),
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
                        && cached_reply(
                            job,
                            Some(&cache),
                            &namespace,
                            &tx,
                            &wanted,
                            &mut claim,
                            work.cache_only,
                        )
                    {
                        continue;
                    }
                }
                if work.cache_only {
                    continue;
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
                    force,
                    cache_only: _,
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
                        Job::Image { url, .. } | Job::VideoFrame { url, .. } => Some(url),
                        _ => None,
                    },
                    ticket: image_ticket.as_ref(),
                    claimed: false,
                };
                if !claim.acquire() {
                    continue;
                }
                if superseded_post(&job, &wanted) {
                    if matches!(job, Job::Post { .. })
                        && tx
                            .send(job.failed(crate::reddit::REQUEST_CANCELLED.into()))
                            .is_err()
                    {
                        return;
                    }
                    continue;
                }
                if background_unwanted(&job, &wanted) {
                    if tx
                        .send(job.failed(crate::reddit::REQUEST_CANCELLED.into()))
                        .is_err()
                    {
                        return;
                    }
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
                    let generation = job.post_generation();
                    let group = match &job {
                        Job::Group {
                            subreddit,
                            generation,
                        } => Some((subreddit.clone(), *generation)),
                        _ => None,
                    };
                    let icon = match &job {
                        Job::Icon { subreddit } => Some(subreddit.clone()),
                        _ => None,
                    };
                    let cancelled = Arc::clone(&wanted);
                    let image = match &job {
                        Job::Image { url, .. } | Job::VideoFrame { url, .. } => Some(url.clone()),
                        _ => None,
                    };
                    let feed = match &job {
                        Job::Feed {
                            tab, generation, ..
                        } => Some((*tab, *generation)),
                        _ => None,
                    };
                    let retry_reads = !matches!(
                        &job,
                        Job::Membership { .. }
                            | Job::Experience {
                                request: crate::reddit::experience::Request::Write(_),
                                ..
                            }
                    );
                    let scoped = reddit.for_worker(
                        match job {
                            Job::Post { .. }
                            | Job::SortedPost { .. }
                            | Job::MoreReplies { .. }
                            | Job::Image {
                                background: false, ..
                            }
                            | Job::VideoFrame {
                                background: false, ..
                            } => Priority::Selected,
                            Job::Icon { .. } => Priority::Background,
                            Job::Subscriptions { .. } if !force => Priority::Background,
                            Job::Feed { .. } | Job::Group { .. } => Priority::Listing,
                            _ => Priority::Foreground,
                        },
                        retry_reads,
                        move || {
                            cancelled.stopped.load(Ordering::Acquire)
                                || feed.is_some_and(|(tab, generation)| {
                                    feed_unwanted(tab, generation, &cancelled)
                                })
                                || group.as_ref().is_some_and(|(name, generation)| {
                                    group_unwanted(name, *generation, &cancelled)
                                })
                                || image.as_ref().is_some_and(|url| {
                                    cancelled
                                        .images
                                        .lock()
                                        .is_ok_and(|urls| !urls.contains(url))
                                })
                                || icon
                                    .as_ref()
                                    .is_some_and(|name| icon_unwanted(name, &cancelled))
                                || generation.is_some_and(|generation| {
                                    cancelled
                                        .post
                                        .lock()
                                        .is_ok_and(|current| *current != Some(generation))
                                })
                        },
                    );
                    let source = Source {
                        order: request_order,
                        observed: Some((started, started)),
                        cached: false,
                    };
                    let mut fallback = None;
                    let msg = run(&scoped, &job, &wanted, &mut |event| {
                        if let Job::Feed {
                            tab, generation, ..
                        } = &job
                        {
                            match event {
                                ListingEvent::Post(post) => {
                                    let _ = tx.send(Msg::FeedProgress {
                                        tab: *tab,
                                        generation: *generation,
                                        post: Box::new(post.clone()),
                                        source,
                                    });
                                }
                                ListingEvent::ContinuationFailed { cursor, error } => {
                                    fallback = Some((cursor.to_string(), error.to_string()));
                                }
                            }
                        }
                    });
                    let msg = match (msg, fallback) {
                        (
                            Msg::Feed {
                                tab,
                                generation,
                                result: Ok(listing),
                                ..
                            },
                            Some((cursor, error)),
                        ) => Msg::FeedFallback {
                            tab,
                            generation,
                            listing,
                            cursor,
                            error,
                        },
                        (msg, _) => msg,
                    };
                    let msg = match &job {
                        Job::Image { url, .. } | Job::VideoFrame { url, .. }
                            if scoped.read_aborted() =>
                        {
                            Msg::ImageSkipped { url: url.clone() }
                        }
                        _ => msg,
                    };
                    if wanted.stopped.load(Ordering::Acquire) {
                        return job.failed("Connection replaced".into());
                    }
                    cache_result(msg, &job, cache.as_deref(), &namespace, &tx, source)
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
    cache_only: bool,
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
    let refreshing = !cache_only
        && (!fresh
            || matches!(job, Job::MoreReplies { minimum: Some(minimum), .. } if !source.may_replace(*minimum)));
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
                | Job::VideoFrame { .. }
                | Job::Thumb { .. }
                | Job::VideoThumb { .. }
                | Job::Icon { .. }
        )
    }

    fn cache_key(&self, namespace: &str) -> Option<(String, i64)> {
        let (request, ttl) = match self {
            Job::Subscriptions { .. } | Job::Membership { .. } => {
                ("subscriptions-v3".to_string(), CONTENT_TTL)
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
                serde_json::to_string(&("replies-v3", cursor)).ok()?,
                CONTENT_TTL,
            ),
            Job::Image { url, .. } | Job::VideoFrame { url, .. } => (
                serde_json::to_string(&("image", MAX_IMAGE_EDGE, url)).ok()?,
                IMAGE_TTL,
            ),
            Job::Thumb { url } | Job::VideoThumb { url } => {
                (serde_json::to_string(&("thumb", url)).ok()?, IMAGE_TTL)
            }
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
            Job::Thumb { url } | Job::VideoThumb { url } => Msg::Thumb {
                url: url.clone(),
                result: Ok(image::load_from_memory(bytes).ok()?),
            },
            Job::Image { url, .. } | Job::VideoFrame { url, .. } => Msg::Image {
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

impl Job {
    fn post_generation(&self) -> Option<u64> {
        match self {
            Self::Post { generation, .. }
            | Self::SortedPost { generation, .. }
            | Self::MoreReplies { generation, .. } => Some(*generation),
            _ => None,
        }
    }
}

fn superseded_post(job: &Job, wanted: &Wanted) -> bool {
    job.post_generation().is_some_and(|generation| {
        wanted
            .post
            .lock()
            .is_ok_and(|post| *post != Some(generation))
    })
}

fn background_unwanted(job: &Job, wanted: &Wanted) -> bool {
    match job {
        Job::Feed {
            tab, generation, ..
        } => feed_unwanted(*tab, *generation, wanted),
        Job::Group {
            subreddit,
            generation,
        } => group_unwanted(subreddit, *generation, wanted),
        Job::Icon { subreddit } => icon_unwanted(subreddit, wanted),
        _ => false,
    }
}

fn feed_unwanted(tab: u64, generation: u64, wanted: &Wanted) -> bool {
    wanted.feeds.lock().is_ok_and(|feeds| {
        feeds.as_ref().is_some_and(|feeds| {
            feeds.active != Some(tab)
                || !feeds
                    .requests
                    .get(&tab)
                    .is_some_and(|requests| requests.contains(&generation))
        })
    })
}

fn group_unwanted(name: &str, generation: u64, wanted: &Wanted) -> bool {
    wanted.groups.lock().is_ok_and(|groups| {
        groups.as_ref().is_some_and(|groups| {
            groups
                .get(name)
                .is_none_or(|current| current.is_some_and(|current| current != generation))
        })
    })
}

fn icon_unwanted(name: &str, wanted: &Wanted) -> bool {
    wanted
        .icons
        .lock()
        .is_ok_and(|icons| icons.as_ref().is_some_and(|icons| !icons.contains(name)))
}

fn skipped_image(job: &Job, wanted: &Wanted) -> Option<Msg> {
    match job {
        Job::Thumb { url } | Job::VideoThumb { url }
            if !wanted.thumbs.lock().is_ok_and(|set| set.contains(url)) =>
        {
            Some(Msg::ThumbSkipped { url: url.clone() })
        }
        Job::Image { url, .. } | Job::VideoFrame { url, .. }
            if !wanted.images.lock().is_ok_and(|set| set.contains(url)) =>
        {
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
    // Listing failures retain order so late refusals cannot undo newer access or pages.
    if !ok
        && !matches!(
            msg,
            Msg::Feed { .. } | Msg::FeedFallback { .. } | Msg::Group { .. }
        )
    {
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

fn run(
    reddit: &Reddit,
    job: &Job,
    wanted: &Wanted,
    progress: &mut dyn FnMut(ListingEvent<'_>),
) -> Msg {
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
            target,
            join,
        } => Msg::Membership {
            generation: *generation,
            result: reddit.set_membership(target, *join).map_err(err_string),
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
                .fetch(reddit, target, after.as_deref(), progress)
                .map_err(FeedError::from),
        },
        Job::Group {
            subreddit,
            generation,
        } => Msg::Group {
            generation: *generation,
            subreddit: subreddit.clone(),
            result: FeedQuery::DEFAULT_LISTING
                .fetch(
                    reddit,
                    &FeedTarget::Subreddit(subreddit.clone()),
                    None,
                    &mut |_| {},
                )
                .map_err(FeedError::from),
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
        Job::Image { url, .. } | Job::VideoFrame { url, .. } => {
            let result = if matches!(job, Job::VideoFrame { .. }) {
                fetch_video_frame(reddit, url, job, wanted, MAX_IMAGE_EDGE)
            } else {
                fetch_image(reddit, url, MAX_IMAGE_EDGE)
            };
            if result.is_err()
                && let Some(skipped) = skipped_image(job, wanted)
            {
                return skipped;
            }
            Msg::Image {
                url: url.clone(),
                result: result.map(LoadedImage::from),
            }
        }
        Job::Thumb { url } | Job::VideoThumb { url } => {
            let result = if matches!(job, Job::VideoThumb { .. }) {
                fetch_video_frame(reddit, url, job, wanted, MAX_THUMB_EDGE)
            } else {
                fetch_image(reddit, url, MAX_THUMB_EDGE)
            };
            if result.is_err()
                && let Some(skipped) = skipped_image(job, wanted)
            {
                return skipped;
            }
            Msg::Thumb {
                url: url.clone(),
                result,
            }
        }
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

fn fetch_video_frame(
    reddit: &Reddit,
    url: &str,
    job: &Job,
    wanted: &Wanted,
    max_edge: u32,
) -> Result<DynamicImage, String> {
    let source = reddit.media_url(url).map_err(err_string)?;
    crate::video::first_frame_preview(&source, || {
        wanted.stopped.load(Ordering::Acquire) || skipped_image(job, wanted).is_some()
    })
    .map(|image| {
        if image.width() > max_edge || image.height() > max_edge {
            image.thumbnail(max_edge, max_edge)
        } else {
            image
        }
    })
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
    #[test]
    fn a_first_frame_job_cancels_when_its_video_leaves_the_viewport() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", server.server_addr());
        let (started, began) = std::sync::mpsc::sync_channel(1);
        let (release, wait) = std::sync::mpsc::sync_channel(1);
        let serving = std::thread::spawn(move || {
            loop {
                let request = server
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap()
                    .unwrap();
                if request.url() != "/slow.mp4" {
                    let _ = request.respond(tiny_http::Response::empty(200));
                    continue;
                }
                started.send(()).unwrap();
                let _ = wait.recv_timeout(std::time::Duration::from_secs(2));
                let _ = request.respond(tiny_http::Response::empty(200));
                break;
            }
        });
        let reddit = Reddit::test_at(&base, false);
        let (pool, messages) = Pool::start(reddit, 1, None);
        let url = format!("{base}slow.mp4");
        pool.want_image(&url);
        pool.submit(Job::VideoFrame {
            url: url.clone(),
            background: true,
        });
        began
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();

        pool.unwant_image(&url);
        let result = messages.recv_timeout(std::time::Duration::from_secs(1));
        let _ = release.send(());
        serving.join().unwrap();

        assert!(
            matches!(result, Ok(Msg::ImageSkipped { .. })),
            "decoder should retire before the stalled request completes"
        );
    }

    use super::*;

    #[test]
    fn when_a_paused_community_is_refreshed_the_old_generation_should_release_the_worker() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let reddit = Reddit::test_at(&format!("http://{}", server.server_addr()), false);
        let (pool, messages) = Pool::start(reddit, 3, None);
        pool.set_visible_groups(&["same"]);
        pool.submit(Job::Group {
            subreddit: "same".into(),
            generation: 1,
        });
        data_request(&server)
            .unwrap()
            .respond(
                tiny_http::Response::empty(429)
                    .with_header(tiny_http::Header::from_bytes("Retry-After", "1").unwrap()),
            )
            .unwrap();
        pool.submit(Job::Group {
            subreddit: "same".into(),
            generation: 2,
        });
        let message = messages
            .recv_timeout(std::time::Duration::from_millis(500))
            .expect("new generation should release the old paused read");
        let message = if let Msg::Snapshot { message, .. } = message {
            *message
        } else {
            message
        };
        assert!(
            matches!(message, Msg::Group { generation: 1, result: Err(ref error), .. } if error.as_str() == crate::reddit::REQUEST_CANCELLED)
        );
        data_request(&server)
            .unwrap()
            .respond(tiny_http::Response::from_string(include_str!(
                "../tests/fixtures/sub.json"
            )))
            .unwrap();
        assert!(
            matches!(next_message(&messages), Msg::Snapshot { message, .. } if matches!(*message, Msg::Group { generation: 2, result: Ok(_), .. }))
        );
    }

    #[test]
    fn when_missing_icons_leave_the_view_queued_and_waiting_lookups_should_be_released() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let reddit = Reddit::test_at(&format!("http://{}", server.server_addr()), false);
        let (pool, messages) = Pool::start(reddit, 2, None);
        pool.set_visible_icons(&["old_a", "old_b"]);
        for name in ["old_a", "old_b"] {
            pool.submit(Job::Icon {
                subreddit: name.into(),
            });
        }
        let first = data_request(&server).unwrap();
        first
            .respond(
                tiny_http::Response::empty(429)
                    .with_header(tiny_http::Header::from_bytes("Retry-After", "1").unwrap()),
            )
            .unwrap();
        pool.set_visible_icons(&["new"]);
        pool.submit(Job::Icon {
            subreddit: "new".into(),
        });
        for _ in 0..2 {
            let message = messages
                .recv_timeout(std::time::Duration::from_millis(500))
                .expect("unused icon work should leave the queue during cooldown");
            let message = if let Msg::Snapshot { message, .. } = message {
                *message
            } else {
                message
            };
            assert!(
                matches!(message, Msg::Icon { result: Err(ref error), .. } if error == crate::reddit::REQUEST_CANCELLED)
            );
        }
        let next = data_request(&server).unwrap();
        assert!(
            next.url().starts_with("/r/new/about.json"),
            "{}",
            next.url()
        );
        next.respond(tiny_http::Response::from_string(
            "{\"data\":{\"icon_img\":\"https://styles.redditmedia.com/new.png\"}}",
        ))
        .unwrap();
        assert!(
            matches!(next_message(&messages), Msg::Snapshot { message, .. } if matches!(*message, Msg::Icon { result: Ok(Some(_)), .. }))
        );
    }

    #[test]
    fn when_waiting_communities_leave_the_view_the_current_group_should_get_the_workers() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let reddit = Reddit::test_at(&format!("http://{}", server.server_addr()), false);
        let (pool, messages) = Pool::start(reddit, 4, None);
        pool.set_visible_groups(&["old_a", "old_b"]);
        for name in ["old_a", "old_b"] {
            pool.submit(Job::Group {
                subreddit: name.into(),
                generation: 1,
            });
        }
        let first = data_request(&server).unwrap();
        let second = data_request(&server).unwrap();
        for request in [first, second] {
            request
                .respond(
                    tiny_http::Response::empty(429)
                        .with_header(tiny_http::Header::from_bytes("Retry-After", "1").unwrap()),
                )
                .unwrap();
        }
        pool.set_visible_groups(&["new"]);
        pool.submit(Job::Group {
            subreddit: "new".into(),
            generation: 2,
        });
        for _ in 0..2 {
            let message = messages
                .recv_timeout(std::time::Duration::from_millis(500))
                .expect("offscreen waits should release workers during cooldown");
            let message = if let Msg::Snapshot { message, .. } = message {
                *message
            } else {
                message
            };
            assert!(
                matches!(message, Msg::Group { generation: 1, result: Err(ref error), .. } if error.as_str() == crate::reddit::REQUEST_CANCELLED)
            );
        }
        let next = data_request(&server).unwrap();
        assert!(next.url().starts_with("/r/new/"), "{}", next.url());
        next.respond(tiny_http::Response::from_string(include_str!(
            "../tests/fixtures/sub.json"
        )))
        .unwrap();
        assert!(
            matches!(next_message(&messages), Msg::Snapshot { message, .. } if matches!(*message, Msg::Group { generation: 2, result: Ok(_), .. }))
        );
    }

    #[test]
    fn when_reply_expansions_are_replaced_they_should_release_workers_during_cooldown() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let reddit = Reddit::test_at(&format!("http://{}", server.server_addr()), false);
        let (pool, messages) = Pool::start(reddit, 2, None);
        pool.want_post(Some(1));
        for id in ["a", "b"] {
            pool.submit(Job::MoreReplies {
                generation: 1,
                comment_id: id.into(),
                cursor: serde_json::json!({"post":"old","parent":"t3_old","children":[]})
                    .to_string(),
                minimum: None,
            });
        }
        let first = data_request(&server).unwrap();
        let second = data_request(&server).unwrap();
        for request in [first, second] {
            request
                .respond(
                    tiny_http::Response::empty(429)
                        .with_header(tiny_http::Header::from_bytes("Retry-After", "1").unwrap()),
                )
                .unwrap();
        }
        pool.want_post(Some(2));
        pool.submit(Job::Post {
            generation: 2,
            permalink: "/comments/new".into(),
        });
        for _ in 0..2 {
            let message = messages
                .recv_timeout(std::time::Duration::from_millis(500))
                .expect("obsolete reads should release workers before cooldown ends");
            let message = if let Msg::Snapshot { message, .. } = message {
                *message
            } else {
                message
            };
            assert!(matches!(
                message,
                Msg::MoreReplies {
                    generation: 1,
                    result: Err(_),
                    ..
                }
            ));
        }
        let next = data_request(&server).unwrap();
        assert!(
            next.url().starts_with("/comments/new.json"),
            "{}",
            next.url()
        );
        next.respond(tiny_http::Response::from_string(include_str!(
            "../tests/fixtures/post.json"
        )))
        .unwrap();
        let message = next_message(&messages);
        assert!(
            matches!(message, Msg::Snapshot { message, .. } if matches!(*message, Msg::Post { generation: 2, result: Ok(_), .. }))
        );
    }

    #[test]
    fn when_offscreen_cache_is_requested_it_should_deliver_stale_data_without_any_http() {
        use crate::reddit::tests::{Fixture, Reply};
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = calls.clone();
        let fixture = Fixture::new(move |_| {
            count.fetch_add(1, Ordering::SeqCst);
            Reply::json(serde_json::from_str(include_str!("../tests/fixtures/sub.json")).unwrap())
        });
        let reddit = fixture.reddit();
        let directory = tempfile::tempdir().unwrap();
        let cache = Cache::new(directory.path().to_path_buf());
        let job = Job::Group {
            subreddit: "rust".into(),
            generation: 1,
        };
        let key = job.cache_key(&reddit.cache_namespace()).unwrap().0;
        let listing = reddit
            .listing(
                "rust",
                crate::model::Sort::Hot,
                crate::model::TimeRange::Day,
                None,
            )
            .unwrap();
        cache
            .store(
                &key,
                &serde_json::to_vec(&listing).unwrap(),
                crate::app::now() - 600,
                cache.reserve_request().unwrap(),
            )
            .unwrap();
        calls.store(0, Ordering::SeqCst);
        let (pool, messages) = Pool::start(reddit, 2, Some(cache));
        pool.cached(job);
        assert!(matches!(next_message(&messages), Msg::Snapshot {
            refreshing: false, message, ..
        } if matches!(*message, Msg::Group { result: Ok(ref listing), .. } if !listing.posts.is_empty())));
        pool.cached(Job::Group {
            subreddit: "missing".into(),
            generation: 2,
        });
        assert!(
            messages
                .recv_timeout(std::time::Duration::from_millis(150))
                .is_err()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

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
                    target: crate::reddit::experience::Target {
                        id: "t3_post".into(),
                        author: "reader".into(),
                        subreddit: "rust".into(),
                        permalink: "/r/rust/comments/post/title/".into(),
                    },
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
            feeds: priority.clone(),
            selected: priority.clone(),
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
            cache_only: false,
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
            feeds: priority.clone(),
            selected: priority.clone(),
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
            cache_only: false,
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

        let thumb_skipped = run(&reddit, &thumb(), &none, &mut |_| {});
        let thumb_fetched = run(&reddit, &thumb(), &all, &mut |_| {});
        let image_skipped = run(&reddit, &image(), &none, &mut |_| {});
        let image_fetched = run(&reddit, &image(), &all, &mut |_| {});

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
                    feeds: sender.clone(),
                    selected: sender.clone(),
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
                false,
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
    fn when_active_detail_reads_are_superseded_the_next_post_should_not_wait_for_them() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.server_addr());
        let (pool, messages) = Pool::start(Reddit::test_at(&base, false), 3, None);
        pool.want_post(Some(1));
        for _ in 0..2 {
            pool.submit(Job::Post {
                generation: 1,
                permalink: "/comments/old/".into(),
            });
        }
        let held = [
            data_request(&server).unwrap(),
            data_request(&server).unwrap(),
        ];
        pool.want_post(Some(2));
        pool.submit(Job::Post {
            generation: 2,
            permalink: "/comments/new/".into(),
        });
        let selected = data_request(&server);
        for request in held {
            let _ = request.respond(tiny_http::Response::empty(200));
        }
        let selected = selected.expect("superseded active reads must release selected workers");
        assert!(selected.url().starts_with("/comments/new.json"));
        selected
            .respond(tiny_http::Response::from_string(include_str!(
                "../tests/fixtures/post.json"
            )))
            .unwrap();
        for _ in 0..3 {
            let message = match next_message(&messages) {
                Msg::Snapshot { message, .. } => *message,
                message => message,
            };
            if let Msg::Post {
                generation: 2,
                result,
            } = message
            {
                assert!(result.is_ok());
                return;
            }
        }
        panic!("selected post result missing");
    }

    #[test]
    fn when_feed_workers_are_occupied_selected_post_and_media_should_start_independently() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.server_addr());
        let (pool, messages) = Pool::start(Reddit::test_at(&base, false), 3, None);
        for tab in 0..2 {
            pool.submit(Job::Feed {
                tab,
                generation: 1,
                target: FeedTarget::Subreddit(format!("held{tab}")),
                query: FeedQuery::DEFAULT_LISTING,
                after: None,
            });
        }
        let first = data_request(&server).unwrap();
        let second = data_request(&server).unwrap();
        pool.want_post(Some(7));
        pool.submit(Job::Post {
            generation: 7,
            permalink: "/r/rust/comments/p/title/".into(),
        });
        let url = format!("{base}/selected.png");
        pool.want_image(&url);
        pool.submit(Job::Image {
            url,
            background: false,
        });
        let selected: Vec<_> = (0..2).filter_map(|_| data_request(&server)).collect();
        for request in &selected {
            assert!(request.url().contains("/comments/p.json") || request.url() == "/selected.png");
        }
        let count = selected.len();
        for request in selected {
            if request.url() == "/selected.png" {
                poster_reply(request);
            } else {
                request
                    .respond(tiny_http::Response::from_string(include_str!(
                        "../tests/fixtures/post.json"
                    )))
                    .unwrap();
            }
        }
        for request in [first, second] {
            request
                .respond(tiny_http::Response::from_string(include_str!(
                    "../tests/fixtures/sub.json"
                )))
                .unwrap();
        }
        assert_eq!(
            count, 2,
            "cold selected content and media must bypass occupied feed workers"
        );
        for _ in 0..4 {
            next_message(&messages);
        }
    }

    #[test]
    fn when_an_image_finishes_just_after_leaving_view_it_should_remain_reusable_from_cache() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.server_addr());
        let dir = CacheDir::new();
        let (pool, messages) = Pool::start(
            Reddit::test_at(&base, false),
            3,
            Some(Cache::new(dir.0.clone())),
        );
        let url = format!("{base}/revisit.png");
        pool.want_image(&url);
        pool.submit(Job::Image {
            url: url.clone(),
            background: true,
        });
        let request = data_request(&server).unwrap();
        pool.unwant_image(&url);
        poster_reply(request);

        assert!(
            matches!(next_message(&messages), Msg::Snapshot { message, .. }
            if matches!(*message, Msg::Image { result: Ok(_), .. })),
            "a completed image should survive a brief departure"
        );
        pool.want_image(&url);
        pool.submit(Job::Image {
            url,
            background: true,
        });
        assert!(
            matches!(
                messages.recv_timeout(std::time::Duration::from_secs(1)),
                Ok(Msg::Snapshot {
                    source: Source { cached: true, .. },
                    refreshing: false,
                    ..
                })
            ),
            "returning must reuse the completed image without HTTP"
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
                br#"[{"community":{"name":"neovim","icon":{"url":"https://i.redd.it/neovim.png"}}},{"community":{"name":"rust","icon":"missing"}}]"#,
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
        assert_eq!(
            names,
            [
                Subscription::Community {
                    name: "neovim".into(),
                    icon: crate::model::SubscriptionIcon::Url(
                        "https://i.redd.it/neovim.png".into()
                    )
                },
                Subscription::Community {
                    name: "rust".into(),
                    icon: crate::model::SubscriptionIcon::Missing
                }
            ]
        );
    }

    #[test]
    fn when_membership_finishes_it_should_invalidate_reads_started_during_the_write() {
        let dir = CacheDir::new();
        let cache = Cache::new(dir.0.clone());
        let namespace = "membership-fixture";
        let read = Job::Subscriptions { generation: 1 };
        let job = Job::Membership {
            generation: 2,
            target: crate::model::MembershipTarget::Community("armenia".into()),
            join: true,
        };
        let (key, _) = read.cache_key(namespace).unwrap();
        let (tx, _rx) = sync_channel(8);
        for result in [Ok(()), Err("ambiguous write failure".into())] {
            let write_start = cache.reserve_request().unwrap();
            let late_read = cache.reserve_request().unwrap();
            cache
                .store(
                    &key,
                    br#"[{"community":{"name":"rust","icon":"unknown"}}]"#,
                    crate::app::now(),
                    late_read,
                )
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
                .store(
                    &key,
                    br#"[{"community":{"name":"rust","icon":"unknown"}}]"#,
                    crate::app::now(),
                    late_read,
                )
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
                .store(
                    &key,
                    br#"[{"community":{"name":"armenia","icon":"unknown"}},{"community":{"name":"rust","icon":"unknown"}}]"#,
                    crate::app::now(),
                    confirmed,
                )
                .unwrap();
            cache
                .store(
                    &key,
                    br#"[{"community":{"name":"rust","icon":"unknown"}}]"#,
                    crate::app::now(),
                    late_read,
                )
                .unwrap();
            assert_eq!(
                cache.load(&key, crate::app::now()).unwrap().bytes,
                br#"[{"community":{"name":"armenia","icon":"unknown"}},{"community":{"name":"rust","icon":"unknown"}}]"#
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
                br#"[{"community":{"name":"rust","icon":"unknown"}}]"#,
                crate::app::now(),
                cache.reserve_request().unwrap(),
            )
            .unwrap();
        let (pool, messages) = Pool::start(reddit, 3, Some(cache));
        pool.submit(Job::Membership {
            generation: 2,
            target: crate::model::MembershipTarget::Community("armenia".into()),
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
            matches!(*message, Msg::Subscriptions { generation: 3, result: Ok(names) } if names == [Subscription::Community { name: "armenia".into(), icon: crate::model::SubscriptionIcon::Unknown }])
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
    fn when_a_feed_ends_with_incomplete_json_it_should_publish_posts_without_caching_a_partial_page()
     {
        use std::io::Write;
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.server_addr());
        let dir = CacheDir::new();
        let reddit = Reddit::test_at(&base, false);
        let job = Job::Feed {
            tab: 1,
            generation: 1,
            target: FeedTarget::Subreddit("s".into()),
            query: FeedQuery::DEFAULT_LISTING,
            after: None,
        };
        let (key, _) = job.cache_key(&reddit.cache_namespace()).unwrap();
        let (pool, messages) = Pool::start(reddit, 3, Some(Cache::new(dir.0.clone())));
        pool.submit(job);
        let request = data_request(&server).unwrap();
        let mut writer = request.into_writer();
        let body = r#"{"kind":"Listing","data":{"children":[{"kind":"t3","data":{"id":"arrived","title":"arrived","subreddit":"s","permalink":"/comments/arrived/"}},"#;
        write!(
            writer,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        writer.flush().unwrap();
        drop(writer);

        let progress = next_message(&messages);
        let terminal = next_message(&messages);

        assert!(matches!(progress,Msg::FeedProgress{post,..} if post.id=="arrived"));
        assert!(
            matches!(terminal,Msg::Snapshot{message,..} if matches!(*message,Msg::Feed{result:Err(_),..}))
        );
        assert!(
            Cache::new(dir.0.clone())
                .load(&key, crate::app::now())
                .is_none()
        );
    }

    #[test]
    fn when_home_has_an_old_cached_primer_it_should_not_replay_that_incomplete_start() {
        let dir = CacheDir::new();
        let reddit = Reddit::test_at("http://127.0.0.1:9", false);
        let cache = Cache::new(dir.0.clone());
        let legacy = r#"["browser-home-v1",{"Listing":{"sort":"Best","time":"Day"}},null]"#;
        let key = Cache::key(&reddit.cache_namespace(), legacy);
        let mut listing =
            crate::reddit::parse::parse_listing(include_str!("../tests/fixtures/sub.json"))
                .unwrap();
        let template = listing.posts[0].clone();
        listing.posts = ["a", "b", "c"]
            .into_iter()
            .map(|id| {
                let mut p = template.clone();
                p.id = id.into();
                p.permalink = format!("/comments/{id}/");
                p
            })
            .collect();
        cache
            .store(
                &key,
                &serde_json::to_vec(&listing).unwrap(),
                crate::app::now(),
                cache.reserve_request().unwrap(),
            )
            .unwrap();
        let (pool, messages) = Pool::start(reddit, 3, Some(cache));

        pool.submit(Job::Feed {
            tab: 1,
            generation: 1,
            target: FeedTarget::Home,
            query: FeedQuery::Listing {
                sort: Sort::Best,
                time: TimeRange::Day,
            },
            after: None,
        });

        assert!(
            matches!(next_message(&messages),Msg::Snapshot{message,source,..} if !source.cached && matches!(*message,Msg::Feed{result:Err(_),..}))
        );
    }

    #[test]
    fn denied_group_responses_should_keep_their_order_after_a_newer_feed_succeeds() {
        for disk_cache in [false, true] {
            let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
            let base = format!("http://{}", server.server_addr());
            let dir = CacheDir::new();
            let (pool, messages) = Pool::start(
                Reddit::test_at(&base, false),
                4,
                disk_cache.then(|| Cache::new(dir.0.clone())),
            );
            pool.submit(Job::Group {
                subreddit: "private".into(),
                generation: 1,
            });
            let group = data_request(&server).unwrap();
            pool.submit(Job::Feed {
                tab: 1,
                generation: 2,
                target: FeedTarget::Subreddit("private".into()),
                query: FeedQuery::DEFAULT_LISTING,
                after: None,
            });
            let feed = data_request(&server).unwrap();
            feed.respond(tiny_http::Response::from_string(include_str!(
                "../tests/fixtures/sub.json"
            )))
            .unwrap();
            let successful = loop {
                let message = next_message(&messages);
                if !matches!(message, Msg::FeedProgress { .. }) {
                    break message;
                }
            };
            let Msg::Snapshot {
                message,
                source: newer,
                ..
            } = successful
            else {
                panic!("successful feed snapshot")
            };
            assert!(matches!(*message, Msg::Feed { result: Ok(_), .. }));
            group
                .respond(
                    tiny_http::Response::from_string(r#"{"reason":"private"}"#)
                        .with_status_code(403),
                )
                .unwrap();

            let Msg::Snapshot {
                message,
                source: older,
                ..
            } = next_message(&messages)
            else {
                panic!("denied group must retain its request source")
            };
            assert!(matches!(
                *message,
                Msg::Group {
                    result: Err(FeedError::Access(AccessDenied::PrivateCommunity)),
                    ..
                }
            ));
            assert!(!older.cached);
            assert_eq!(
                older.compare_requests(newer),
                Some(std::cmp::Ordering::Less),
                "disk_cache={disk_cache}"
            );
        }
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
        let Msg::Snapshot {
            message, source, ..
        } = next_message(&messages)
        else {
            panic!("failed revalidation snapshot")
        };
        assert!(!source.cached);
        assert!(source.order.is_some());
        assert!(matches!(*message, Msg::Group { result: Err(_), .. }));
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

        pool.want_post(Some(1));
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
fn user_submitted_cache_keys_should_not_collide_with_a_same_named_community_or_cursor() {
    let query = FeedQuery::Listing {
        sort: Sort::New,
        time: TimeRange::Day,
    };
    let user = query
        .cache_request(&FeedTarget::User("Same".into()), None)
        .unwrap();
    assert_ne!(
        user,
        query
            .cache_request(&FeedTarget::Subreddit("same".into()), None)
            .unwrap()
    );
    assert_ne!(
        user,
        query
            .cache_request(&FeedTarget::User("same".into()), Some("next"))
            .unwrap()
    );
    assert_eq!(
        user,
        query
            .cache_request(&FeedTarget::User("same".into()), None)
            .unwrap()
    );
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

#[test]
fn when_bodies_retain_markup_they_should_not_reuse_flattened_cached_documents() {
    let directory = tempfile::tempdir().unwrap();
    let cache = Cache::new(directory.path().to_path_buf());
    for (job, legacy, invalidation) in [
        (
            Job::Post {
                generation: 1,
                permalink: "/comments/post/title/".into(),
            },
            r#"["post-v3","post"]"#,
            Invalidation::Post("/comments/post/title/".into()),
        ),
        (
            Job::MoreReplies {
                generation: 1,
                comment_id: "comment".into(),
                cursor: "cursor".into(),
                minimum: None,
            },
            r#"["replies-v2","cursor"]"#,
            Invalidation::Replies("cursor".into()),
        ),
    ] {
        cache
            .store(
                &Cache::key("account", legacy),
                b"flattened document",
                crate::app::now(),
                cache.reserve_request().unwrap(),
            )
            .unwrap();
        let (actual, _) = job.cache_key("account").unwrap();
        assert!(cache.load(&actual, crate::app::now()).is_none());
        assert_eq!(
            actual,
            Cache::key("account", &invalidation.request().unwrap())
        );
    }
}
