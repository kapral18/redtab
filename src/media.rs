//! Handing URLs to other programs: browser, video player, image viewer, clipboard.

use std::io::Write;
use std::os::{fd::AsRawFd, unix::process::CommandExt};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use base64::Engine;

/// Spawns `argv + extra` with no shared stdio and reaps it in the background.
pub fn spawn_detached(argv: &[String], extra: &[String]) -> Result<()> {
    spawn(argv, extra, |program| Command::new(program))
}

/// `spawn_detached` for video players, through [`local_command`].
pub fn spawn_media(argv: &[String], extra: &[String]) -> Result<()> {
    spawn(argv, extra, local_command)
}

/// Media subprocesses receive loopback URLs. Ignore inherited system proxies for
/// that local hop; the relay alone owns upstream routing.
pub fn local_command(program: &str) -> Command {
    let mut command = Command::new(program);
    for name in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        command.env_remove(name);
    }
    command.env("NO_PROXY", "*").env("no_proxy", "*");
    command
}

fn spawn(argv: &[String], extra: &[String], command: impl FnOnce(&str) -> Command) -> Result<()> {
    let (program, args) = argv.split_first().context("empty command in config")?;
    let mut child = command(program)
        .args(args)
        .args(extra)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("cannot start {program}"))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

pub enum CopyStatus {
    Copied(&'static str),
    Queued,
}

pub struct CopyTask {
    result: std::sync::mpsc::Receiver<std::result::Result<CopyStatus, String>>,
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl CopyTask {
    pub fn start(text: String) -> Self {
        let (send, receive) = std::sync::mpsc::channel();
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop = cancelled.clone();
        let thread = std::thread::spawn(move || {
            let _ = send.send(copy(&text, &stop).map_err(|error| error.to_string()));
        });
        Self {
            result: receive,
            cancelled,
            thread: Some(thread),
        }
    }

    pub fn poll(&self) -> Option<std::result::Result<CopyStatus, String>> {
        match self.result.try_recv() {
            Ok(result) => Some(result),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(_) => Some(Err("clipboard worker stopped".into())),
        }
    }
}

impl Drop for CopyTask {
    fn drop(&mut self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn copy(text: &str, cancelled: &std::sync::atomic::AtomicBool) -> Result<CopyStatus> {
    let candidates: &[(&str, &[&str])] = &[
        ("pbcopy", &[]),
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
    ];
    let deadline = Instant::now() + Duration::from_secs(2);
    for (program, args) in candidates {
        if cancelled.load(std::sync::atomic::Ordering::Acquire) {
            bail!("copy cancelled");
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        if copy_with_deadline(program, args, text, remaining, cancelled) {
            return Ok(CopyStatus::Copied(program));
        }
    }
    if cancelled.load(std::sync::atomic::Ordering::Acquire) {
        bail!("copy cancelled");
    }
    osc52(text)
}

/// Whether `program` took all of `text` and succeeded; always waited for.
#[cfg(test)]
fn copy_with(program: &str, args: &[&str], text: &str) -> bool {
    copy_with_timeout(program, args, text, Duration::from_secs(2))
}

#[cfg(test)]
fn copy_with_timeout(program: &str, args: &[&str], text: &str, timeout: Duration) -> bool {
    copy_with_deadline(
        program,
        args,
        text,
        timeout,
        &std::sync::atomic::AtomicBool::new(false),
    )
}

fn copy_with_deadline(
    program: &str,
    args: &[&str],
    text: &str,
    timeout: Duration,
    cancelled: &std::sync::atomic::AtomicBool,
) -> bool {
    let Ok(mut child) = Command::new(program)
        .args(args)
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let mut input = child.stdin.take();
    let configured = input.as_ref().is_some_and(|stdin| unsafe {
        let flags = libc::fcntl(stdin.as_raw_fd(), libc::F_GETFL);
        flags >= 0 && libc::fcntl(stdin.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) == 0
    });
    let deadline = Instant::now() + timeout;
    let mut written = 0;
    while configured
        && Instant::now() < deadline
        && !cancelled.load(std::sync::atomic::Ordering::Acquire)
    {
        let mut writing = false;
        if let Some(stdin) = &mut input {
            writing = true;
            match stdin.write(&text.as_bytes()[written..]) {
                Ok(count) => written += count,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    let mut ready = libc::pollfd {
                        fd: stdin.as_raw_fd(),
                        events: libc::POLLOUT,
                        revents: 0,
                    };
                    unsafe { libc::poll(&mut ready, 1, 5) };
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
            if written == text.len() {
                input = None;
            }
        }
        match child.try_wait() {
            Ok(Some(status)) if status.success() && written == text.len() => return true,
            Ok(Some(_)) => break,
            Ok(None) if !writing => std::thread::sleep(Duration::from_millis(5)),
            Ok(None) => {}
            Err(_) => break,
        }
    }
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
    false
}

fn osc52(text: &str) -> Result<CopyStatus> {
    if text.len() > 74_994 {
        bail!("text too long for OSC 52");
    }
    let command = format!(
        "\x1b]52;c;{}\x07",
        base64::engine::general_purpose::STANDARD.encode(text)
    );
    crate::terminal_output::queue_command(command.as_bytes())?;
    Ok(CopyStatus::Queued)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stalled_clipboard_helpers_are_killed_without_waiting_for_stdin_or_exit() {
        let started = Instant::now();
        assert!(!copy_with_timeout(
            "sh",
            &["-c", "sleep 10"],
            &"x".repeat(4 * 1024 * 1024),
            Duration::from_millis(30)
        ));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn dropping_a_copy_task_cancels_and_joins_its_helper() {
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop = cancelled.clone();
        let (send, result) = std::sync::mpsc::channel();
        let (started, ready) = std::sync::mpsc::channel();
        let (finished, done) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            started.send(()).unwrap();
            assert!(!copy_with_deadline(
                "sh",
                &["-c", "sleep 10"],
                &"x".repeat(4 * 1024 * 1024),
                Duration::from_secs(2),
                &stop
            ));
            finished.send(()).unwrap();
            drop(send);
        });
        let task = CopyTask {
            result,
            cancelled,
            thread: Some(thread),
        };
        ready.recv_timeout(Duration::from_secs(1)).unwrap();
        let started = Instant::now();
        drop(task);
        done.try_recv().unwrap();
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn media_children_ignore_system_proxies() {
        use std::ffi::OsStr;

        let command = local_command("player");

        // `get_envs` lists a variable the child will not inherit with no value.
        let envs: std::collections::HashMap<_, _> = command.get_envs().collect();
        for name in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            assert_eq!(envs.get(OsStr::new(name)), Some(&None), "{name}");
        }
        assert_eq!(envs[OsStr::new("no_proxy")], Some(OsStr::new("*")));
    }

    #[test]
    fn when_the_tool_reads_all_input_and_succeeds_the_copy_should_count() {
        assert!(copy_with(
            "sh",
            &["-c", "cat > /dev/null"],
            &"x".repeat(1 << 22)
        ));
    }

    #[test]
    fn when_the_tool_stops_reading_the_copy_should_fail_so_the_next_way_is_tried() {
        // More than any default pipe holds, so the write fails.
        assert!(!copy_with(
            "sh",
            &["-c", "exec 0<&-; exit 0"],
            &"x".repeat(1 << 22)
        ));
    }
}
