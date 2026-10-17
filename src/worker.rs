//! Background fetching. Separate queues keep jobs the user waits on ahead of Home's
//! bulk fetches, and both ahead of thumbnails. Cacheable jobs pass a read-only disk
//! reader first; the worker that runs a job reserves its cache order.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;

use image::DynamicImage;

use crate::cache::{CONTENT_TTL, Cache, IMAGE_TTL, Source};
use crate::model::{Comment, Listing, Media, PostPage, Sort, TimeRange};
use crate::redlib::Redlib;

/// Largest image edge sent to the terminal; bigger images only cost bandwidth.
const MAX_IMAGE_EDGE: u32 = 1600;
/// Largest list-thumbnail edge; keeps the cache small.
const MAX_THUMB_EDGE: u32 = 128;
/// Threads reserved for jobs the user waits on.
const PRIORITY_THREADS: usize = 2;
/// Shared logical clock for disk-read windows and HTTP starts in this process.
static NEXT_EVENT: AtomicU64 = AtomicU64::new(1);
/// Threads for list thumbnails and subreddit icons.
const THUMB_THREADS: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum FeedQuery {
    Listing { sort: Sort, time: TimeRange },
    Search { query: String },
}

impl FeedQuery {
    pub const HOME: Self = Self::Listing {
        sort: Sort::Hot,
        time: TimeRange::Day,
    };

    fn cache_request(&self, subreddit: &str, after: Option<&str>) -> Option<String> {
        serde_json::to_string(&("feed", subreddit.to_lowercase(), self, after)).ok()
    }

    fn fetch(
        &self,
        redlib: &Redlib,
        subreddit: &str,
        after: Option<&str>,
    ) -> anyhow::Result<Listing> {
        match self {
            Self::Listing { sort, time } => redlib.listing(subreddit, *sort, *time, after),
            Self::Search { query } => redlib.search(subreddit, query, after),
        }
    }
}

#[derive(Debug)]
pub enum Job {
    Feed {
        tab: u64,
        generation: u64,
        subreddit: String,
        query: FeedQuery,
        after: Option<String>,
    },
    /// First page of a subreddit for the Home wall (also seeds its tab).
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
        path: String,
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
    Health,
    RunCommand {
        argv: Vec<String>,
    },
}

impl Job {
    /// This job's reply for error `e`; used on panic so whatever waits still gets an answer.
    fn failed(&self, e: String) -> Msg {
        match self {
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
            Job::ResolveMedia { permalink } => Msg::MediaResolved {
                permalink: permalink.clone(),
                result: Err(e),
            },
            Job::DownloadImages { permalink, .. } => Msg::ImagesDownloaded {
                permalink: permalink.clone(),
                result: Err(e),
            },
            Job::Health => Msg::Health(false),
            Job::RunCommand { argv } => Msg::CommandDone {
                argv: argv.clone(),
                result: Err(e),
            },
        }
    }
}

pub enum Msg {
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
        result: Result<Vec<Comment>, String>,
    },
    Image {
        url: String,
        result: Result<DynamicImage, String>,
    },
    Thumb {
        url: String,
        result: Result<DynamicImage, String>,
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
    Health(bool),
    CommandDone {
        argv: Vec<String>,
        result: Result<CommandEnd, String>,
    },
}

pub struct Pool {
    queues: Queues,
    cache_reads: Option<Sender<Work>>,
    wanted: Arc<Wanted>,
}

#[derive(Clone)]
struct Queues {
    priority: Sender<Work>,
    background: Sender<Work>,
    thumbs: Sender<Work>,
}

struct Work {
    job: Job,
    force: bool,
}

impl Queues {
    fn submit(&self, work: Work) {
        let queue = match work.job {
            Job::Thumb { .. } => &self.thumbs,
            Job::Group { .. }
            | Job::Image {
                background: true, ..
            } => &self.background,
            _ => &self.priority,
        };
        let _ = queue.send(work);
    }
}

type SharedRx = Arc<Mutex<Receiver<Work>>>;

/// URLs whose queued fetches are still worth running; others are skipped.
#[derive(Default)]
struct Wanted {
    /// Thumbnails drawn in the last frame.
    thumbs: Mutex<HashSet<String>>,
    /// Full-size images of the open post.
    images: Mutex<HashSet<String>>,
}

impl Pool {
    pub fn start(redlib: Redlib, threads: usize, cache: Option<Cache>) -> (Self, Receiver<Msg>) {
        let (priority_tx, priority_rx) = channel::<Work>();
        let (background_tx, background_rx) = channel::<Work>();
        let (thumbs_tx, thumbs_rx) = channel::<Work>();
        let (msg_tx, msg_rx) = channel::<Msg>();
        let wanted = Arc::<Wanted>::default();
        let cache = cache.map(Arc::new);
        let namespace: Arc<str> = redlib.cache_namespace().into();
        let queues = Queues {
            priority: priority_tx,
            background: background_tx,
            thumbs: thumbs_tx,
        };
        let background_threads = threads.saturating_sub(PRIORITY_THREADS).max(1);
        for (rx, count) in [
            (priority_rx, PRIORITY_THREADS),
            (background_rx, background_threads),
            (thumbs_rx, THUMB_THREADS),
        ] {
            let rx: SharedRx = Arc::new(Mutex::new(rx));
            for _ in 0..count {
                spawn_worker(
                    Arc::clone(&rx),
                    msg_tx.clone(),
                    redlib.clone(),
                    Arc::clone(&wanted),
                    cache.clone(),
                    Arc::clone(&namespace),
                );
            }
        }
        let cache_reads = cache.map(|cache| {
            let (tx, rx) = channel::<Work>();
            let queues = queues.clone();
            let wanted = Arc::clone(&wanted);
            thread::Builder::new()
                .name("redtab-cache".into())
                .spawn(move || {
                    while let Ok(work) = rx.recv() {
                        let job = &work.job;
                        if let Some(msg) = skipped_image(job, &wanted) {
                            let _ = msg_tx.send(msg);
                            continue;
                        }
                        let now = crate::app::now();
                        if !work.force && let Some((key, ttl)) = job.cache_key(&namespace) {
                            let read_start = NEXT_EVENT.fetch_add(1, Ordering::Relaxed);
                            if let Some(entry) = cache.load(&key, now)
                                // Media resolution triggers a viewer: stale entries must
                                // not deliver twice.
                                && (!matches!(job, Job::ResolveMedia { .. })
                                    || now.saturating_sub(entry.fetched_at) < ttl)
                                && let Some(message) = job.cached_message(&entry.bytes)
                            {
                                let source = Source {
                                    order: Some(entry.order),
                                    observed: Some((
                                        read_start,
                                        NEXT_EVENT.fetch_add(1, Ordering::Relaxed),
                                    )),
                                    cached: true,
                                };
                                let refreshing = now.saturating_sub(entry.fetched_at) >= ttl
                                    || matches!(job, Job::MoreReplies { minimum: Some(minimum), .. } if !source.may_replace(*minimum));
                                let _ = msg_tx.send(Msg::Snapshot {
                                    message: Box::new(message),
                                    fetched_at: entry.fetched_at,
                                    refreshing,
                                    source,
                                });
                                if !refreshing {
                                    continue;
                                }
                            }
                        }
                        queues.submit(work);
                    }
                })
                .expect("cannot start cache reader");
            tx
        });
        (
            Self {
                queues,
                cache_reads,
                wanted,
            },
            msg_rx,
        )
    }

    pub fn submit(&self, job: Job) {
        self.enqueue(job, false)
    }

    fn enqueue(&self, job: Job, force: bool) {
        let work = Work { job, force };
        match &self.cache_reads {
            Some(tx) if work.job.uses_cache() => {
                let _ = tx.send(work);
            }
            _ => self.queues.submit(work),
        }
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
}

/// Worker thread name; the panic hook leaves the screen alone on non-main threads.
const WORKER_THREAD: &str = "redtab-worker";

fn spawn_worker(
    rx: SharedRx,
    tx: Sender<Msg>,
    redlib: Redlib,
    wanted: Arc<Wanted>,
    cache: Option<Arc<Cache>>,
    namespace: Arc<str>,
) {
    thread::Builder::new()
        .name(WORKER_THREAD.into())
        .spawn(move || {
            loop {
                let Work { job, .. } = {
                    let Ok(guard) = rx.lock() else { return };
                    match guard.recv() {
                        Ok(job) => job,
                        Err(_) => return,
                    }
                };
                if let Some(msg) = skipped_image(&job, &wanted) {
                    if tx.send(msg).is_err() {
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
                    let msg = run(&redlib, &job, &wanted);
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
                if tx.send(msg).is_err() {
                    return;
                }
            }
        })
        .expect("cannot start a worker thread");
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
    /// Whether disk holds snapshots of this job's response.
    fn uses_cache(&self) -> bool {
        matches!(
            self,
            Job::Feed { .. }
                | Job::Group { .. }
                | Job::Post { .. }
                | Job::ResolveMedia { .. }
                | Job::MoreReplies { .. }
                | Job::Thumb { .. }
        )
    }

    fn cache_key(&self, namespace: &str) -> Option<(String, i64)> {
        let (request, ttl) = match self {
            Job::Feed {
                subreddit,
                query,
                after,
                ..
            } => (
                query.cache_request(subreddit, after.as_deref())?,
                CONTENT_TTL,
            ),
            Job::Group { subreddit, .. } => {
                (FeedQuery::HOME.cache_request(subreddit, None)?, CONTENT_TTL)
            }
            Job::Post { permalink, .. } | Job::ResolveMedia { permalink } => (
                serde_json::to_string(&("post", permalink)).ok()?,
                CONTENT_TTL,
            ),
            Job::MoreReplies { path, .. } => {
                (serde_json::to_string(&("replies", path)).ok()?, CONTENT_TTL)
            }
            Job::Thumb { url } => (serde_json::to_string(&("thumb", url)).ok()?, IMAGE_TTL),
            _ => return None,
        };
        Some((Cache::key(namespace, &request), ttl))
    }

    fn cached_message(&self, bytes: &[u8]) -> Option<Msg> {
        Some(match self {
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
            _ => return None,
        })
    }
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
    tx: &Sender<Msg>,
    source: Source,
) -> Msg {
    let ok = matches!(
        &msg,
        Msg::Feed { result: Ok(_), .. }
            | Msg::Group { result: Ok(_), .. }
            | Msg::Post { result: Ok(_), .. }
            | Msg::MoreReplies { result: Ok(_), .. }
            | Msg::Thumb { result: Ok(_), .. }
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
            Msg::Thumb {
                result: Ok(value), ..
            } => {
                let mut bytes = std::io::Cursor::new(Vec::new());
                value
                    .write_to(&mut bytes, image::ImageFormat::Png)
                    .ok()
                    .map(|()| bytes.into_inner())
            }
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

fn run(redlib: &Redlib, job: &Job, wanted: &Wanted) -> Msg {
    if let Some(message) = skipped_image(job, wanted) {
        return message;
    }
    match job {
        Job::Feed {
            tab,
            generation,
            subreddit,
            query,
            after,
        } => Msg::Feed {
            tab: *tab,
            generation: *generation,
            append: after.is_some(),
            result: query
                .fetch(redlib, subreddit, after.as_deref())
                .map_err(err_string),
        },
        Job::Group {
            subreddit,
            generation,
        } => Msg::Group {
            generation: *generation,
            subreddit: subreddit.clone(),
            result: FeedQuery::HOME
                .fetch(redlib, subreddit, None)
                .map_err(err_string),
        },
        Job::Post {
            generation,
            permalink,
        } => Msg::Post {
            generation: *generation,
            result: redlib.post(permalink).map(Box::new).map_err(err_string),
        },
        Job::MoreReplies {
            generation,
            comment_id,
            path,
            ..
        } => Msg::MoreReplies {
            generation: *generation,
            comment_id: comment_id.clone(),
            result: redlib.more_replies(path).map_err(err_string),
        },
        Job::Image { url, .. } => Msg::Image {
            url: url.clone(),
            result: fetch_image(redlib, url, MAX_IMAGE_EDGE),
        },
        Job::Thumb { url } => Msg::Thumb {
            url: url.clone(),
            result: fetch_image(redlib, url, MAX_THUMB_EDGE),
        },
        Job::ResolveMedia { permalink } => Msg::MediaResolved {
            permalink: permalink.clone(),
            result: redlib
                .post(permalink)
                .map(|page| page.post.media)
                .map_err(err_string),
        },
        Job::DownloadImages { permalink, urls } => Msg::ImagesDownloaded {
            permalink: permalink.clone(),
            result: download_images(redlib, urls).map_err(err_string),
        },
        Job::Health => Msg::Health(redlib.healthy()),
        Job::RunCommand { argv } => Msg::CommandDone {
            argv: argv.clone(),
            result: run_command(argv),
        },
    }
}

/// Download and decode an image, shrunk to fit `max_edge`. A GIF gives its first frame
/// and the rest of the file is never read: animated GIFs play as video and can be huge.
fn fetch_image(redlib: &Redlib, url: &str, max_edge: u32) -> Result<DynamicImage, String> {
    redlib
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

fn download_images(redlib: &Redlib, urls: &[String]) -> anyhow::Result<Vec<PathBuf>> {
    let dir = media_dir()?;
    remove_old_downloads(&dir, std::time::SystemTime::now());
    urls.iter()
        .enumerate()
        .map(|(i, url)| {
            let bytes = redlib.image(url)?;
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

/// How long `S` waits. A command that is the server itself never exits and then counts as started.
const COMMAND_WAIT: std::time::Duration = std::time::Duration::from_secs(20);

/// How the start command ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CommandEnd {
    Exited,
    /// Still running after `COMMAND_WAIT`: it is the server itself.
    Running,
}

/// Hidden process entry: keep draining a server's stderr after the TUI exits.
pub const COMMAND_ARG: &str = "--supervise-start-command";
const COMMAND_STDERR_LIMIT: usize = 64 * 1024;

/// Runs `argv` under a supervisor process (`COMMAND_ARG`) that reports startup once, then
/// drains the server's stderr until it closes; a TUI thread would close the pipe when the
/// TUI quits.
fn run_command(argv: &[String]) -> Result<CommandEnd, String> {
    use std::io::{BufRead, BufReader};
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut supervisor = Command::new(executable)
        .arg(COMMAND_ARG)
        .args(argv)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|e| format!("start command supervisor: {e}"))?;
    let output = supervisor.stdout.take().expect("piped stdout");
    thread::spawn(move || supervisor.wait());
    let mut reply = String::new();
    BufReader::new(output)
        .read_line(&mut reply)
        .map_err(|e| format!("start command supervisor: {e}"))?;
    serde_json::from_str(&reply).map_err(|e| format!("start command supervisor: {e}"))?
}

struct StartedCommand {
    child: std::process::Child,
    stderr: std::io::PipeReader,
    result: Result<CommandEnd, String>,
}

/// The supervisor process, entered before terminal setup. A failed report (TUI gone) must
/// not stop the draining.
pub fn command_process(argv: &[std::ffi::OsString]) -> std::io::Result<()> {
    use std::io::Write;

    let started = start_command(argv);
    let result = match &started {
        Ok(command) => &command.result,
        Err(e) => &Err(e.clone()),
    };
    let mut output = std::io::stdout().lock();
    let _ = serde_json::to_writer(&mut output, result);
    let _ = output.write_all(b"\n");
    let _ = output.flush();
    if let Ok(mut command) = started {
        nonblocking(&command.stderr, false)?;
        let drained = std::io::copy(&mut command.stderr, &mut std::io::sink());
        let _ = command.child.wait();
        drained?;
    }
    Ok(())
}

fn start_command(argv: &[std::ffi::OsString]) -> Result<StartedCommand, String> {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let (program, args) = argv.split_first().ok_or("empty command")?;
    let (mut stderr, writer) = std::io::pipe().map_err(|e| e.to_string())?;
    nonblocking(&stderr, true).map_err(|e| e.to_string())?;
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(writer)
        .spawn()
        .map_err(|e| format!("{}: {e}", program.to_string_lossy()))?;
    let started = Instant::now();
    let mut captured = Vec::new();
    let status = loop {
        let status = child.try_wait().map_err(|e| e.to_string())?;
        // Drain after observing exit so all of the command's final writes count.
        capture_stderr(&mut stderr, &mut captured).map_err(|e| e.to_string())?;
        if status.is_some() || started.elapsed() >= COMMAND_WAIT {
            break status;
        }
        thread::sleep(Duration::from_millis(10));
    };
    let result = match status {
        Some(status) if !status.success() => Err(format!(
            "{} exited {status}: {}",
            program.to_string_lossy(),
            String::from_utf8_lossy(&captured).trim()
        )),
        Some(_) => Ok(CommandEnd::Exited),
        None => Ok(CommandEnd::Running),
    };
    Ok(StartedCommand {
        child,
        stderr,
        result,
    })
}

fn nonblocking(pipe: &std::io::PipeReader, enabled: bool) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;

    // SAFETY: the pipe owns this live descriptor throughout both fcntl calls.
    let flags = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_GETFL) };
    if flags == -1 {
        return Err(std::io::Error::last_os_error());
    }
    let flags = if enabled {
        flags | libc::O_NONBLOCK
    } else {
        flags & !libc::O_NONBLOCK
    };
    if unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, flags) } == -1 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Keeps the first 64 KiB, and bounds each pass so steady output can't starve the startup deadline.
fn capture_stderr(pipe: &mut std::io::PipeReader, captured: &mut Vec<u8>) -> std::io::Result<()> {
    use std::io::{ErrorKind, Read};

    let mut buffer = [0; 8192];
    let mut remaining = COMMAND_STDERR_LIMIT;
    while remaining > 0 {
        match pipe.read(&mut buffer[..remaining.min(8192)]) {
            Ok(0) => break,
            Ok(n) => {
                let keep = n.min(COMMAND_STDERR_LIMIT - captured.len());
                captured.extend_from_slice(&buffer[..keep]);
                remaining -= n;
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_in_text_should_wait_on_the_background_queue_not_the_one_the_reader_waits_on() {
        let (priority, priority_rx) = channel();
        let (background, background_rx) = channel();
        let (thumbs, _thumbs_rx) = channel();
        let queues = Queues {
            priority,
            background,
            thumbs,
        };
        let image = |background| Work {
            job: Job::Image {
                url: "/preview/pre/x.png".into(),
                background,
            },
            force: false,
        };

        queues.submit(image(true));
        queues.submit(image(false));

        assert!(matches!(
            background_rx.try_recv(),
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
    fn fetches_no_longer_wanted_are_skipped() {
        // Nothing listens on port 9: a fetch comes back as a connection error.
        let redlib = Redlib::new("http://127.0.0.1:9", false);
        let url = "/img/a.jpeg";
        let none = Wanted::default();
        let all = Wanted::default();
        all.thumbs.lock().unwrap().insert(url.into());
        all.images.lock().unwrap().insert(url.into());
        let thumb = || Job::Thumb { url: url.into() };
        let image = || Job::Image {
            url: url.into(),
            background: false,
        };

        let thumb_skipped = run(&redlib, &thumb(), &none);
        let thumb_fetched = run(&redlib, &thumb(), &all);
        let image_skipped = run(&redlib, &image(), &none);
        let image_fetched = run(&redlib, &image(), &all);

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

    #[test]
    fn a_start_command_that_leaves_a_background_server_returns_promptly() {
        // A short sleep: the backgrounded child outlives the test run otherwise.
        let argv: Vec<std::ffi::OsString> = ["sh", "-c", "sleep 8 & exit 0"]
            .map(std::ffi::OsString::from)
            .to_vec();
        let started = std::time::Instant::now();

        let result = start_command(&argv).unwrap().result;

        assert_eq!(result, Ok(CommandEnd::Exited));
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    fn a_failing_start_command_reports_its_stderr() {
        let argv: Vec<std::ffi::OsString> = ["sh", "-c", "echo no such container >&2; exit 3"]
            .map(std::ffi::OsString::from)
            .to_vec();

        let err = start_command(&argv).unwrap().result.unwrap_err();

        assert!(err.contains("no such container"), "{err}");
    }

    #[test]
    fn a_failing_start_command_bounds_its_stderr() {
        let argv = ["sh", "-c", "head -c 131072 /dev/zero >&2; exit 3"]
            .map(std::ffi::OsString::from)
            .to_vec();

        let err = start_command(&argv).unwrap().result.unwrap_err();

        assert!(err.len() < 66000, "captured {} bytes", err.len());
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

    #[test]
    fn a_warm_feed_is_readable_after_the_server_shuts_down() {
        use std::io::{BufRead, BufReader, Write};
        let dir = CacheDir::new();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let redlib = Redlib::new(&format!("http://{}", listener.local_addr().unwrap()), false);
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
            let html = include_str!("../tests/fixtures/sub.html");
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                html.len()
            )
            .unwrap();
        });
        let job = || Job::Group {
            subreddit: "rust".into(),
            generation: 0,
        };
        let (cold, messages) = Pool::start(redlib.clone(), 3, Some(Cache::new(dir.0.clone())));
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
        let (warm, messages) = Pool::start(redlib, 3, Some(Cache::new(dir.0.clone())));

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
        let redlib = Redlib::new("http://127.0.0.1:9", false);
        let job = Job::Group {
            subreddit: "rust".into(),
            generation: 0,
        };
        let cache = Cache::new(dir.0.clone());
        let (key, _) = job.cache_key(&redlib.cache_namespace()).unwrap();
        let listing =
            crate::redlib::parse::parse_listing(include_str!("../tests/fixtures/sub.html"))
                .unwrap();
        cache
            .store(
                &key,
                &serde_json::to_vec(&listing).unwrap(),
                crate::app::now() - CONTENT_TTL,
                cache.reserve_request().unwrap(),
            )
            .unwrap();
        let (pool, messages) = Pool::start(redlib, 3, Some(cache));

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
        let redlib = Redlib::new("http://127.0.0.1:9", false);
        let job = Job::Post {
            generation: 1,
            permalink: "/r/rust/comments/p/title/".into(),
        };
        let cache = Cache::new(dir.0.clone());
        let (key, _) = job.cache_key(&redlib.cache_namespace()).unwrap();
        let page =
            crate::redlib::parse::parse_post_page(include_str!("../tests/fixtures/post.html"))
                .unwrap();
        cache
            .store(
                &key,
                &serde_json::to_vec(&page).unwrap(),
                crate::app::now(),
                cache.reserve_request().unwrap(),
            )
            .unwrap();
        let (pool, messages) = Pool::start(redlib, 3, Some(cache));

        pool.refresh(job);

        assert!(matches!(
            next_message(&messages),
            Msg::Post { result: Err(_), .. }
        ));
    }

    #[test]
    fn cached_thumbnail_arrives_before_a_failed_background_refresh() {
        let dir = CacheDir::new();
        let redlib = Redlib::new("http://127.0.0.1:9", false);
        let job = Job::Thumb {
            url: "/img/cached.png".into(),
        };
        let cache = Cache::new(dir.0.clone());
        let (key, _) = job.cache_key(&redlib.cache_namespace()).unwrap();
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
        let (pool, messages) = Pool::start(redlib, 3, Some(cache));
        pool.set_on_screen(&["/img/cached.png"]);

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
        let redlib = Redlib::new("http://127.0.0.1:9", false);
        let job = Job::ResolveMedia {
            permalink: "/r/pics/comments/p/gallery/".into(),
        };
        let cache = Cache::new(dir.0.clone());
        let (key, _) = job.cache_key(&redlib.cache_namespace()).unwrap();
        let page =
            crate::redlib::parse::parse_post_page(include_str!("../tests/fixtures/gallery.html"))
                .unwrap();
        cache
            .store(
                &key,
                &serde_json::to_vec(&page).unwrap(),
                crate::app::now() - CONTENT_TTL,
                cache.reserve_request().unwrap(),
            )
            .unwrap();
        let (pool, messages) = Pool::start(redlib, 3, Some(cache));

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
        let redlib = Redlib::new("http://127.0.0.1:9", false);
        let job = Job::MoreReplies {
            generation: 1,
            comment_id: "parent".into(),
            path: "/r/rust/comments/post/title/parent/".into(),
            minimum: Some(Source {
                order: Some(newer),
                ..Source::default()
            }),
        };
        let (key, _) = job.cache_key(&redlib.cache_namespace()).unwrap();
        cache.store(&key, b"[]", crate::app::now(), older).unwrap();
        let (pool, messages) = Pool::start(redlib, 3, Some(cache));

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
