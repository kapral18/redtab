//! Background fetching. Two queues feed a few threads: jobs the user is
//! waiting for (and health checks) never queue behind the Home wall's bulk
//! subreddit fetches. Results go back to the UI loop, which ignores responses
//! for outdated requests.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;

use image::DynamicImage;

use crate::model::{Comment, Listing, Media, PostPage, Sort, TimeRange};
use crate::redlib::Redlib;

/// Largest image edge sent to the terminal; bigger images only cost bandwidth.
const MAX_IMAGE_EDGE: u32 = 1600;
/// Threads reserved for jobs the user waits on.
const PRIORITY_THREADS: usize = 2;

#[derive(Debug, Clone)]
pub enum FeedQuery {
    Listing { sort: Sort, time: TimeRange },
    Search { query: String },
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
    },
    Post {
        generation: u64,
        permalink: String,
    },
    MoreReplies {
        generation: u64,
        comment_id: String,
        path: String,
    },
    Image {
        url: String,
    },
    /// Fetch the post page to learn full media URLs (galleries, HLS video).
    ResolveMedia {
        permalink: String,
    },
    /// Save images to temp files so a desktop viewer can open them.
    DownloadImages {
        urls: Vec<String>,
    },
    Health,
    RunCommand {
        argv: Vec<String>,
    },
}

impl Job {
    fn is_background(&self) -> bool {
        matches!(self, Job::Group { .. })
    }
}

pub enum Msg {
    Feed {
        tab: u64,
        generation: u64,
        append: bool,
        result: Result<Listing, String>,
    },
    Group {
        subreddit: String,
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
    MediaResolved {
        permalink: String,
        result: Result<Media, String>,
    },
    ImagesDownloaded(Result<Vec<PathBuf>, String>),
    Health(bool),
    CommandDone {
        argv: Vec<String>,
        result: Result<(), String>,
    },
}

pub struct Pool {
    priority: Sender<Job>,
    background: Sender<Job>,
}

type SharedRx = Arc<Mutex<Receiver<Job>>>;

impl Pool {
    pub fn start(redlib: Redlib, threads: usize) -> (Self, Receiver<Msg>) {
        let (priority_tx, priority_rx) = channel::<Job>();
        let (background_tx, background_rx) = channel::<Job>();
        let (msg_tx, msg_rx) = channel::<Msg>();
        let priority_rx: SharedRx = Arc::new(Mutex::new(priority_rx));
        let background_rx: SharedRx = Arc::new(Mutex::new(background_rx));
        let background_threads = threads.saturating_sub(PRIORITY_THREADS).max(1);
        for (rx, count) in [
            (priority_rx, PRIORITY_THREADS),
            (background_rx, background_threads),
        ] {
            for _ in 0..count {
                spawn_worker(Arc::clone(&rx), msg_tx.clone(), redlib.clone());
            }
        }
        (
            Self {
                priority: priority_tx,
                background: background_tx,
            },
            msg_rx,
        )
    }

    pub fn submit(&self, job: Job) {
        let queue = if job.is_background() {
            &self.background
        } else {
            &self.priority
        };
        // A send error means the workers are gone because the app is exiting.
        let _ = queue.send(job);
    }
}

fn spawn_worker(rx: SharedRx, tx: Sender<Msg>, redlib: Redlib) {
    thread::spawn(move || {
        loop {
            let job = {
                let Ok(guard) = rx.lock() else { return };
                match guard.recv() {
                    Ok(job) => job,
                    Err(_) => return,
                }
            };
            if tx.send(run(&redlib, job)).is_err() {
                return;
            }
        }
    });
}

fn err_string(e: anyhow::Error) -> String {
    format!("{e:#}")
}

fn run(redlib: &Redlib, job: Job) -> Msg {
    match job {
        Job::Feed {
            tab,
            generation,
            subreddit,
            query,
            after,
        } => {
            let append = after.is_some();
            let result = match query {
                FeedQuery::Listing { sort, time } => {
                    redlib.listing(&subreddit, sort, time, after.as_deref())
                }
                FeedQuery::Search { query } => redlib.search(&subreddit, &query, after.as_deref()),
            };
            Msg::Feed {
                tab,
                generation,
                append,
                result: result.map_err(err_string),
            }
        }
        Job::Group { subreddit } => {
            let result = redlib
                .listing(&subreddit, Sort::Hot, TimeRange::Day, None)
                .map_err(err_string);
            Msg::Group { subreddit, result }
        }
        Job::Post {
            generation,
            permalink,
        } => Msg::Post {
            generation,
            result: redlib.post(&permalink).map(Box::new).map_err(err_string),
        },
        Job::MoreReplies {
            generation,
            comment_id,
            path,
        } => Msg::MoreReplies {
            generation,
            comment_id,
            result: redlib.more_replies(&path).map_err(err_string),
        },
        Job::Image { url } => {
            let result = redlib
                .image(&url)
                .and_then(|bytes| Ok(image::load_from_memory(&bytes)?))
                .map(|img| {
                    if img.width() > MAX_IMAGE_EDGE || img.height() > MAX_IMAGE_EDGE {
                        img.thumbnail(MAX_IMAGE_EDGE, MAX_IMAGE_EDGE)
                    } else {
                        img
                    }
                })
                .map_err(err_string);
            Msg::Image { url, result }
        }
        Job::ResolveMedia { permalink } => {
            let result = redlib
                .post(&permalink)
                .map(|page| page.post.media)
                .map_err(err_string);
            Msg::MediaResolved { permalink, result }
        }
        Job::DownloadImages { urls } => {
            Msg::ImagesDownloaded(download_images(redlib, &urls).map_err(err_string))
        }
        Job::Health => Msg::Health(redlib.healthy()),
        Job::RunCommand { argv } => {
            let result = run_command(&argv);
            Msg::CommandDone { argv, result }
        }
    }
}

/// Private directory for downloaded images, created by this process with mode
/// 0700. A directory that already exists is never used: a shared, predictable
/// /tmp path would let another local user plant symlinks that `m` writes through.
fn media_dir() -> anyhow::Result<PathBuf> {
    use std::os::unix::fs::DirBuilderExt;
    // Both priority threads may download at once: create under the lock so one
    // directory serves the process. Only a success is stored, so a transient
    // failure (full disk) is retried on the next `m`.
    static DIR: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);
    let mut cached = DIR
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(dir) = cached.as_ref() {
        return Ok(dir.clone());
    }
    let pid = std::process::id();
    for attempt in 0..16u32 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let dir = std::env::temp_dir().join(format!("redtab-media-{pid}-{nanos:x}{attempt}"));
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

fn download_images(redlib: &Redlib, urls: &[String]) -> anyhow::Result<Vec<PathBuf>> {
    let dir = media_dir()?;
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

fn run_command(argv: &[String]) -> Result<(), String> {
    let (program, args) = argv.split_first().ok_or("empty command")?;
    let output = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("{program}: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "{program} exited {}: {}",
            output.status,
            stderr.trim()
        ))
    }
}
