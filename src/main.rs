mod app;
mod cache;
mod config;
mod keys;
mod media;
mod model;
mod redlib;
mod state;
mod ui;
mod video;
mod worker;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use crossterm::event::{self, Event};
use ratatui_image::picker::Picker;

use crate::app::App;
use crate::config::Config;
use crate::redlib::Redlib;
use crate::worker::Pool;

const USAGE: &str = "\
redtab — terminal Reddit reader with tabs, built on a local Redlib

Usage: redtab [--config PATH] [--subreddit NAME]

Options:
  -c, --config PATH      config file (default: $XDG_CONFIG_HOME/redtab/config.toml)
  -s, --subreddit NAME   open this subreddit on start
      --check            print the config file, Redlib status, subscriptions and state
                         file, then exit (status 1 when Redlib does not answer)
  -h, --help             show this help
  -V, --version          show the version

Press ? inside redtab for key bindings.";

const WORKER_THREADS: usize = 6;
/// Hidden flag: the child process that asks the terminal for its image protocol.
const PROBE_ARG: &str = "--probe-image-protocol";
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
/// The probe child's own wait for terminal replies inside tmux. It stays under
/// `PROBE_TIMEOUT`, so a silent terminal still yields a report instead of a killed child.
const PROBE_QUERY_TIMEOUT: Duration = Duration::from_secs(1);

struct Args {
    config: Option<PathBuf>,
    subreddit: Option<String>,
    check: bool,
}

fn parse_args() -> Result<Option<Args>> {
    let mut args = Args {
        config: None,
        subreddit: None,
        check: false,
    };
    // OS strings: a non-UTF-8 path is an error message, not a panic.
    let mut it = std::env::args_os().skip(1);
    while let Some(arg) = it.next() {
        match arg.to_str() {
            Some(worker::COMMAND_ARG) => {
                worker::command_process(&it.collect::<Vec<_>>())?;
                return Ok(None);
            }
            Some(PROBE_ARG) => {
                probe_image_protocol();
                return Ok(None);
            }
            Some("-h" | "--help") => {
                println!("{USAGE}");
                return Ok(None);
            }
            Some("-V" | "--version") => {
                println!("redtab {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            Some("-c" | "--config") => {
                args.config = Some(it.next().context("--config needs a path")?.into())
            }
            Some("-s" | "--subreddit") => {
                let name = it.next().context("--subreddit needs a name")?;
                args.subreddit = Some(
                    name.into_string()
                        .map_err(|n| anyhow::anyhow!("{n:?} is not a subreddit name"))?,
                )
            }
            Some("--check") => args.check = true,
            _ => bail!("unknown argument {arg:?}\n\n{USAGE}"),
        }
    }
    Ok(Some(args))
}

fn main() -> Result<()> {
    let Some(args) = parse_args()? else {
        return Ok(());
    };
    let config = Config::load(args.config.as_deref())?;
    let subscriptions = config.subscriptions()?;
    let redlib = Redlib::new(&config.redlib_url, config.show_nsfw);

    if args.check {
        let up = redlib.healthy();
        let config_file = if config.path.exists() {
            config.path.display().to_string()
        } else {
            format!("{} (missing; defaults)", config.path.display())
        };
        println!("config         {config_file}");
        println!("redlib_url     {}", redlib.base());
        println!("redlib         {}", if up { "up" } else { "NOT answering" });
        println!("subscriptions  {}", subscriptions.len());
        println!(
            "state          {}",
            config::state_dir().join("state.json").display()
        );
        if !up {
            std::process::exit(1);
        }
        return Ok(());
    }

    // Closing the terminal (SIGHUP), `kill` (SIGTERM) or SIGINT quit like `q`,
    // saving state. Registered first, so a signal during the probe is not fatal.
    let quit_signal = Arc::new(AtomicBool::new(false));
    for signal in [
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
        signal_hook::consts::SIGINT,
    ] {
        signal_hook::flag::register(signal, Arc::clone(&quit_signal))?;
    }
    // Enter raw mode before the probe: crossterm records the terminal settings on the
    // first enable only, so exit restores these settings rather than whatever a
    // timed-out probe child leaves behind (no echo, no line mode).
    crossterm::terminal::enable_raw_mode()?;
    // From here every exit, early error or panic included, restores the terminal.
    let guard = TerminalGuard;
    let image_picker = if config.image_preview {
        detect_image_picker()
    } else {
        Picker::halfblocks()
    };
    let cache = cache::Cache::new(config::cache_dir());
    let (pool, messages) = Pool::start(redlib.clone(), WORKER_THREADS, Some(cache));
    // The terminal may have closed during the probe (up to 2 s): quit before
    // touching it, as for any other quit signal.
    if quit_signal.load(Ordering::Relaxed) {
        return Ok(());
    }
    let mut terminal = ratatui::try_init()?;
    // ratatui's panic hook reports a failed restore with `eprintln!`, which panics
    // again on a dead tty and aborts before the state is saved. Restore quietly, and
    // only for the main thread: a worker's panic is caught and shown as that job's
    // error, so it must not tear down the screen or print over it.
    let _ratatui_hook = std::panic::take_hook();
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().name() == Some("main") {
            let _ = ratatui::try_restore();
            default_hook(info);
        } else {
            // Workers and the input thread catch their panics and report this text.
            worker::note_panic(info.to_string());
        }
    }));
    crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste)?;
    // NO_COLOR is honored by clearing colors in each frame (`ui::strip_colors`), not by
    // crossterm: its NO_COLOR output resets bold too and drops the colors kitty
    // placeholders use as image ids.
    let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
    if no_color {
        crossterm::style::force_color_output(true);
    }
    let (input, input_failure) = spawn_input_reader();
    // A terminal can answer the probe late; drop those bytes so they are not read as
    // keys. Bounded: a held key or a resize drag must not keep the screen blank.
    // Read through the input thread: a terminal closing now must not stall us.
    let drain_until = Instant::now() + Duration::from_millis(300);
    while let Some(left) = drain_until.checked_duration_since(Instant::now()) {
        if quit_signal.load(Ordering::Relaxed)
            || input
                .recv_timeout(left.min(Duration::from_millis(150)))
                .is_err()
        {
            break;
        }
    }
    let state_path = config::state_dir().join("state.json");
    let mut app = App::new(
        config,
        subscriptions,
        pool,
        redlib,
        state_path,
        image_picker,
    );
    app.no_color = no_color;
    if let Some(name) = &args.subreddit {
        app.open_subreddit(name);
    }

    let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<()> {
        let mut dirty = true;
        while !app.quit {
            if quit_signal.load(Ordering::Relaxed) {
                break;
            }
            if dirty {
                terminal.draw(|f| {
                    ui::draw(f, &mut app);
                    if no_color {
                        ui::strip_colors(f.buffer_mut());
                    }
                })?;
                dirty = false;
            }
            match input.recv_timeout(app.wake_in()) {
                Ok(first) => {
                    for ev in std::iter::once(first).chain(input.try_iter()) {
                        // Keys typed after a quit key must not act any more.
                        if app.quit {
                            break;
                        }
                        match ev {
                            Event::Key(key) => keys::handle(&mut app, key),
                            Event::Paste(text) => keys::paste(&mut app, &text),
                            _ => {}
                        }
                    }
                    dirty = true;
                }
                Err(RecvTimeoutError::Timeout) => {}
                // The terminal is gone or unreadable.
                Err(RecvTimeoutError::Disconnected) => {
                    if let Some(why) = input_failure.lock().ok().and_then(|mut w| w.take()) {
                        app.error(format!("terminal input stopped: {why}"));
                    }
                    break;
                }
            }
            while let Ok(msg) = messages.try_recv() {
                app.on_msg(msg);
                dirty = true;
            }
            dirty |= app.tick();
        }
        Ok(())
    }));
    // Save first: it does not touch the terminal, which may be gone (closed pane),
    // so nothing below can stop it. Saved after a panic too.
    app.save_state();
    drop(guard);
    // The guard showed the cursor; Terminal's drop would only report a dead tty.
    std::mem::forget(terminal);
    if let Some(status) = app.status.as_ref().filter(|s| s.error) {
        use std::io::Write;
        let _ = writeln!(std::io::stderr(), "redtab: {}", status.text);
    }
    match run {
        Ok(result) => result,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

/// Restores the terminal when dropped, on every exit path. Errors are ignored:
/// after a closed terminal every write fails, and a failed `eprintln!` panics.
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::event::DisableBracketedPaste,
            crossterm::cursor::Show
        );
        let _ = ratatui::try_restore();
    }
}

/// Terminal input on its own thread. crossterm 0.29 spins inside `poll`/`read`
/// once the tty reports end of file (its read loop only stops on WouldBlock), so
/// a closed terminal must not stall the loop that watches for SIGHUP and quits.
/// Also returns where the thread leaves its panic message, if it panics.
fn spawn_input_reader() -> (Receiver<Event>, Arc<std::sync::Mutex<Option<String>>>) {
    let (tx, rx) = std::sync::mpsc::channel();
    let failure = Arc::new(std::sync::Mutex::new(None));
    let report = Arc::clone(&failure);
    std::thread::spawn(move || {
        let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            while let Ok(ev) = event::read() {
                if tx.send(ev).is_err() {
                    return;
                }
            }
        }));
        if read.is_err()
            && let Ok(mut slot) = report.lock()
        {
            *slot = Some(worker::take_panic().unwrap_or_else(|| "a bug in redtab".into()));
        }
        // `tx` drops here, after the report: the main loop then sees the input end.
    });
    (rx, failure)
}

/// Child side: query the terminal and report `<protocol> <font width> <font height>` on stderr
/// (stdout stays on the terminal, where the query escapes go).
fn probe_image_protocol() {
    use ratatui_image::picker::ProtocolType;
    use ratatui_image::picker::cap_parser::QueryStdioOptions;

    // Inside tmux the terminal's replies may never reach the pane: answer before the
    // parent's kill so its tmux fallback still runs. Elsewhere keep the library's wait.
    let mut options = QueryStdioOptions::default();
    if Picker::halfblocks().tmux_detected() {
        options.timeout = PROBE_QUERY_TIMEOUT;
    }
    if let Ok(picker) = Picker::from_query_stdio_with_options(options) {
        let kind = match picker.protocol_type() {
            ProtocolType::Kitty => "kitty",
            ProtocolType::Sixel => "sixel",
            ProtocolType::Iterm2 => "iterm2",
            ProtocolType::Halfblocks => "halfblocks",
        };
        let size = picker.font_size();
        eprintln!("{kind} {} {}", size.width, size.height);
    }
}

/// Parent side. The terminal query runs in a child process: when a terminal
/// never answers, the query's stdin reader would otherwise stay blocked
/// inside redtab and swallow the first key press. A hung child is killed.
fn detect_image_picker() -> Picker {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::Instant;

    use ratatui_image::FontSize;
    use ratatui_image::picker::ProtocolType;

    let Ok(exe) = std::env::current_exe() else {
        return Picker::halfblocks();
    };
    let Ok(mut child) = Command::new(exe)
        .arg(PROBE_ARG)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::piped())
        .spawn()
    else {
        return Picker::halfblocks();
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < PROBE_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(20))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Picker::halfblocks();
            }
        }
    }
    let mut report = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        let _ = stderr.read_to_string(&mut report);
    }
    let mut parts = report.split_whitespace();
    let protocol = match parts.next() {
        Some("kitty") => Some(ProtocolType::Kitty),
        Some("sixel") => Some(ProtocolType::Sixel),
        Some("iterm2") => Some(ProtocolType::Iterm2),
        Some("halfblocks") => None,
        _ => return Picker::halfblocks(),
    };
    let (Some(Ok(width)), Some(Ok(height))) =
        (parts.next().map(str::parse), parts.next().map(str::parse))
    else {
        return Picker::halfblocks();
    };
    // `from_fontsize` keeps tmux detection, which wraps escapes for passthrough.
    #[allow(deprecated)]
    let mut picker = Picker::from_fontsize(FontSize::new(width, height));
    match protocol {
        // tmux answers for sixel itself, yet draws sixel only on terminals that take
        // it; Ghostty and kitty do not, while kitty graphics pass through to them.
        Some(ProtocolType::Sixel) if picker.tmux_detected() && tmux_pane_gets_kitty() => {
            picker.set_protocol_type(ProtocolType::Kitty)
        }
        Some(protocol) => picker.set_protocol_type(protocol),
        // A real cell size is needed to scale images; the probe reports a guess without one.
        None if picker.tmux_detected()
            && crossterm::terminal::window_size().is_ok_and(|s| s.width > 0 && s.height > 0)
            && tmux_pane_gets_kitty() =>
        {
            picker.set_protocol_type(ProtocolType::Kitty)
        }
        None => return Picker::halfblocks(),
    }
    picker
}

/// Inside tmux the probe can get no kitty-graphics reply even when the outer terminal
/// supports it. Then the tmux clients decide: every client attached to a session that
/// holds redtab's window (grouped and linked sessions too) must be a Ghostty or kitty
/// terminal, because any of them can show the pane. redtab must also draw straight into
/// its pane: a terminal nested in the pane (an editor's terminal) inherits the tmux
/// environment but drops passthrough.
fn tmux_pane_gets_kitty() -> bool {
    use std::os::unix::fs::MetadataExt;
    use std::process::{Command, Stdio};

    let Some(pane) = std::env::var("TMUX_PANE").ok() else {
        return false;
    };
    let tmux = |args: &[&str]| {
        Command::new("tmux")
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let Some(info) = tmux(&[
        "display-message",
        "-p",
        "-t",
        &pane,
        "#{pane_tty} #{window_id}",
    ]) else {
        return false;
    };
    let [pane_tty, window] = info.split_whitespace().collect::<Vec<_>>()[..] else {
        return false;
    };
    let same_tty = match (
        std::fs::metadata(pane_tty),
        std::fs::metadata("/dev/stdout"),
    ) {
        (Ok(pane), Ok(stdout)) => pane.rdev() == stdout.rdev(),
        _ => false,
    };
    if !same_tty {
        return false;
    }
    let Some(windows) = tmux(&["list-windows", "-a", "-F", "#{window_id} #{session_id}"]) else {
        return false;
    };
    let sessions: Vec<&str> = windows
        .lines()
        .filter_map(|line| line.split_once(' '))
        .filter(|(w, _)| *w == window)
        .map(|(_, s)| s)
        .collect();
    let Some(clients) = tmux(&["list-clients", "-F", "#{session_id} #{client_termname}"]) else {
        return false;
    };
    let mut names = clients
        .lines()
        .filter_map(|line| line.split_once(' '))
        .filter(|(s, _)| sessions.contains(s))
        .map(|(_, name)| name)
        .peekable();
    names.peek().is_some() && names.all(|name| name.contains("ghostty") || name.contains("kitty"))
}
