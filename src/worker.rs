//! Background fetching. Three queues feed a few threads: jobs the user is
//! waiting for (and health checks) never queue behind the Home wall's bulk
//! subreddit fetches, and neither waits behind list thumbnails. Results go back
//! to the UI loop, which ignores responses for outdated requests.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;

use image::DynamicImage;

use crate::model::{Comment, Listing, Media, PostPage, Sort, TimeRange};
use crate::redlib::Redlib;

/// Largest image edge sent to the terminal; bigger images only cost bandwidth.
const MAX_IMAGE_EDGE: u32 = 1600;
/// Largest list-thumbnail edge: a few cells wide, so this keeps the cache small.
const MAX_THUMB_EDGE: u32 = 128;
/// Threads reserved for jobs the user waits on.
const PRIORITY_THREADS: usize = 2;
/// Threads for list thumbnails and subreddit icons, apart from the other queues.
const THUMB_THREADS: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
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
        /// The post the images belong to; one download per post at a time.
        permalink: String,
        urls: Vec<String>,
    },
    Health,
    RunCommand {
        argv: Vec<String>,
    },
}

impl Job {
    /// The reply this job gives when it fails with `e`; used when it panics, so
    /// what waits on it (a loading list, a thumbnail) still gets an answer.
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
            Job::Group { subreddit } => Msg::Group {
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
            Job::Image { url } => Msg::Image {
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
    /// A queued thumbnail left the screen before its turn; it was not fetched.
    ThumbSkipped {
        url: String,
    },
    /// A queued full-size image whose post closed before its turn; not fetched.
    ImageSkipped,
    Health(bool),
    CommandDone {
        argv: Vec<String>,
        result: Result<CommandEnd, String>,
    },
}

pub struct Pool {
    priority: Sender<Job>,
    background: Sender<Job>,
    thumbs: Sender<Job>,
    wanted: Arc<Wanted>,
}

type SharedRx = Arc<Mutex<Receiver<Job>>>;

/// URLs whose queued fetches are still worth running; others are skipped.
#[derive(Default)]
struct Wanted {
    /// Thumbnails drawn in the last frame.
    thumbs: Mutex<HashSet<String>>,
    /// Full-size images of the open post.
    images: Mutex<HashSet<String>>,
}

impl Pool {
    pub fn start(redlib: Redlib, threads: usize) -> (Self, Receiver<Msg>) {
        let (priority_tx, priority_rx) = channel::<Job>();
        let (background_tx, background_rx) = channel::<Job>();
        let (thumbs_tx, thumbs_rx) = channel::<Job>();
        let (msg_tx, msg_rx) = channel::<Msg>();
        let wanted = Arc::<Wanted>::default();
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
                );
            }
        }
        (
            Self {
                priority: priority_tx,
                background: background_tx,
                thumbs: thumbs_tx,
                wanted,
            },
            msg_rx,
        )
    }

    pub fn submit(&self, job: Job) {
        let queue = match job {
            Job::Thumb { .. } => &self.thumbs,
            Job::Group { .. } => &self.background,
            _ => &self.priority,
        };
        // A send error means the workers are gone because the app is exiting.
        let _ = queue.send(job);
    }

    /// Record the thumbnails on screen now, so scrolled-away ones are not fetched.
    pub fn set_on_screen(&self, urls: &[&str]) {
        if let Ok(mut set) = self.wanted.thumbs.lock() {
            // Most frames redraw the same rows: leave the set alone then. Spots can
            // repeat a URL (an icon in the sidebar and on a card), so compare both ways.
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

    /// The post closed: its queued image fetches are skipped.
    pub fn forget_images(&self) {
        if let Ok(mut set) = self.wanted.images.lock() {
            set.clear();
        }
    }
}

/// Name of the worker threads, for debuggers. The panic hook keeps the screen for
/// every thread but main; a worker catches its own panic.
const WORKER_THREAD: &str = "redtab-worker";

fn spawn_worker(rx: SharedRx, tx: Sender<Msg>, redlib: Redlib, wanted: Arc<Wanted>) {
    thread::Builder::new()
        .name(WORKER_THREAD.into())
        .spawn(move || {
            loop {
                let job = {
                    let Ok(guard) = rx.lock() else { return };
                    match guard.recv() {
                        Ok(job) => job,
                        Err(_) => return,
                    }
                };
                // A bug in one job (a parser or decoder panic) must not kill this worker
                // or leave its caller waiting: it becomes that job's error.
                let msg = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run(&redlib, &job, &wanted)
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
    /// The panic message of this thread's last panic, set by the panic hook.
    static LAST_PANIC: std::cell::Cell<Option<String>> = const { std::cell::Cell::new(None) };
}

/// Called by the panic hook on a thread other than main: keep the message, so the
/// job's error can name it instead of printing over the screen.
pub fn note_panic(message: String) {
    LAST_PANIC.set(Some(message));
}

/// This thread's last panic message, for a thread that caught its own panic.
pub fn take_panic() -> Option<String> {
    LAST_PANIC.take()
}

fn err_string(e: anyhow::Error) -> String {
    format!("{e:#}")
}

fn run(redlib: &Redlib, job: &Job, wanted: &Wanted) -> Msg {
    match job {
        Job::Feed {
            tab,
            generation,
            subreddit,
            query,
            after,
        } => {
            let result = match query {
                FeedQuery::Listing { sort, time } => {
                    redlib.listing(subreddit, *sort, *time, after.as_deref())
                }
                FeedQuery::Search { query } => redlib.search(subreddit, query, after.as_deref()),
            };
            Msg::Feed {
                tab: *tab,
                generation: *generation,
                append: after.is_some(),
                result: result.map_err(err_string),
            }
        }
        Job::Group { subreddit } => Msg::Group {
            subreddit: subreddit.clone(),
            result: redlib
                .listing(subreddit, Sort::Hot, TimeRange::Day, None)
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
        } => Msg::MoreReplies {
            generation: *generation,
            comment_id: comment_id.clone(),
            result: redlib.more_replies(path).map_err(err_string),
        },
        Job::Image { url } => {
            if !wanted.images.lock().is_ok_and(|set| set.contains(url)) {
                return Msg::ImageSkipped;
            }
            Msg::Image {
                url: url.clone(),
                result: fetch_image(redlib, url, MAX_IMAGE_EDGE),
            }
        }
        Job::Thumb { url } => {
            if !wanted.thumbs.lock().is_ok_and(|set| set.contains(url)) {
                return Msg::ThumbSkipped { url: url.clone() };
            }
            Msg::Thumb {
                url: url.clone(),
                result: fetch_image(redlib, url, MAX_THUMB_EDGE),
            }
        }
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

/// Download and decode an image, shrunk to fit `max_edge`.
fn fetch_image(redlib: &Redlib, url: &str, max_edge: u32) -> Result<DynamicImage, String> {
    redlib
        .image(url)
        .and_then(|bytes| Ok(image::load_from_memory(&bytes)?))
        .map(|img| {
            if img.width() > max_edge || img.height() > max_edge {
                img.thumbnail(max_edge, max_edge)
            } else {
                img
            }
        })
        .map_err(err_string)
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

/// How long `S` waits for its command. A command that is the server itself
/// (`redlib`, `docker compose up`) never exits; past this it counts as started.
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

/// The supervisor reports startup once, then lives until the server closes stderr
/// and exits. A reader thread in the TUI would close its pipe when the TUI quits.
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

/// Invoked before terminal setup in a separate process. Reporting can fail when
/// the TUI has already quit; that must not stop the server's stderr reader.
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

/// Bound both storage and each drain pass: continuous output must not starve the
/// startup deadline. Bytes after the first 64 KiB are discarded.
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
    fn fetches_no_longer_wanted_are_skipped() {
        // Nothing listens on port 9: a fetch comes back as a connection error.
        let redlib = Redlib::new("http://127.0.0.1:9", false);
        let url = "/img/a.jpeg";
        let none = Wanted::default();
        let all = Wanted::default();
        all.thumbs.lock().unwrap().insert(url.into());
        all.images.lock().unwrap().insert(url.into());
        let thumb = || Job::Thumb { url: url.into() };
        let image = || Job::Image { url: url.into() };

        let thumb_skipped = run(&redlib, &thumb(), &none);
        let thumb_fetched = run(&redlib, &thumb(), &all);
        let image_skipped = run(&redlib, &image(), &none);
        let image_fetched = run(&redlib, &image(), &all);

        assert!(matches!(thumb_skipped, Msg::ThumbSkipped { url: u } if u == url));
        assert!(matches!(thumb_fetched, Msg::Thumb { result: Err(_), .. }));
        assert!(
            matches!(image_skipped, Msg::ImageSkipped),
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
}
