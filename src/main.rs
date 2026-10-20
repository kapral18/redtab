mod app;
mod cache;
mod config;
mod keys;
mod media;
mod model;
mod reddit;
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
use crate::reddit::{Health, Reddit};
use crate::worker::Pool;

const USAGE: &str = "\
redtab — terminal Reddit reader with tabs and direct guest access

Usage: redtab [--config PATH] [--subreddit NAME]

Options:
  -c, --config PATH      config file (default: $XDG_CONFIG_HOME/redtab/config.toml)
  -s, --subreddit NAME   open this subreddit on start
      --check            print the config file, Reddit status, subscriptions, subscriptions
                         file and state file, then exit (status 1 unless Reddit is up)
  -h, --help             show this help
  -V, --version          show the version

Press ? inside redtab for key bindings.";

const WORKER_THREADS: usize = 6;
/// Hidden flag for the child that queries the terminal's image protocol.
const PROBE_ARG: &str = "--probe-image-protocol";
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
/// The child's reply wait in tmux; shorter than `PROBE_TIMEOUT` so it reports before being killed.
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
    // OS strings: a non-UTF-8 argument errors instead of panicking.
    let mut it = std::env::args_os().skip(1);
    while let Some(arg) = it.next() {
        match arg.to_str() {
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
    let reddit = Reddit::new(config.show_nsfw)?;

    if args.check {
        let health = reddit.check();
        let config_file = if config.path.exists() {
            config.path.display().to_string()
        } else {
            format!("{} (missing; defaults)", config.path.display())
        };
        println!("config         {config_file}");
        println!(
            "reddit         {}",
            match health {
                Health::Up => "up",
                Health::Paused => "paused",
                Health::Down => "NOT answering",
                Health::Unknown => "unknown",
            }
        );
        let subscriptions_file = config.subscriptions_path();
        let subscriptions_file = if subscriptions_file.exists() {
            subscriptions_file.display().to_string()
        } else {
            format!("{} (missing)", subscriptions_file.display())
        };
        println!("subscriptions  {}", subscriptions.len());
        println!("subs file      {subscriptions_file}");
        println!(
            "state          {}",
            config::state_dir().join("state.json").display()
        );
        if health != Health::Up {
            std::process::exit(1);
        }
        return Ok(());
    }

    // Download folders of exited redtabs; off the UI thread, as $TMPDIR can be large.
    std::thread::spawn(worker::remove_stale_media_dirs);

    // SIGHUP, SIGTERM and SIGINT quit like `q`; registered before the probe.
    let quit_signal = Arc::new(AtomicBool::new(false));
    for signal in [
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
        signal_hook::consts::SIGINT,
    ] {
        signal_hook::flag::register(signal, Arc::clone(&quit_signal))?;
    }
    // crossterm saves termios on the first raw-mode enable, so enabling before the
    // probe makes exit restore the original, not what a killed probe left.
    crossterm::terminal::enable_raw_mode()?;
    let guard = TerminalGuard;
    let image_picker = if config.image_preview {
        detect_image_picker()
    } else {
        Picker::halfblocks()
    };
    let cache = cache::Cache::new(config::cache_dir());
    let (pool, messages) = Pool::start(reddit.clone(), WORKER_THREADS, Some(cache));
    // The terminal may have closed during the probe.
    if quit_signal.load(Ordering::Relaxed) {
        return Ok(());
    }
    let mut terminal = ratatui::try_init()?;
    // ratatui's hook `eprintln!`s, which panics again on a dead tty before state is
    // saved. Restore quietly, and only on main: worker panics become job errors.
    let _ratatui_hook = std::panic::take_hook();
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().name() == Some("main") {
            let _ = ratatui::try_restore();
            default_hook(info);
        } else {
            // The panicking thread catches it and reports this text.
            worker::note_panic(info.to_string());
        }
    }));
    crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste)?;
    // NO_COLOR is applied per frame (`ui::strip_colors`): crossterm's version also
    // drops bold and the colors kitty placeholders use as image ids.
    let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
    if no_color {
        crossterm::style::force_color_output(true);
    }
    let (input, input_failure) = spawn_input_reader();
    // Drop late probe replies so they aren't read as keys. Bounded, and through the
    // input thread so a closing terminal can't stall startup.
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
        reddit,
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
                        // Keys after a quit key do nothing.
                        if app.quit {
                            break;
                        }
                        handle_event(&mut app, ev);
                    }
                    dirty = true;
                }
                Err(RecvTimeoutError::Timeout) => {}
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
    // Save first: it never touches the terminal, which may be gone. Runs after a panic too.
    app.save_state();
    drop(guard);
    // The guard restored the terminal; Terminal's drop would only error on a dead tty.
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

fn handle_event(app: &mut App, event: Event) {
    match event {
        Event::Key(key) => keys::handle(app, key),
        Event::Paste(text) => keys::paste(app, &text),
        Event::Resize(width, _) => app.resize(width),
        _ => {}
    }
}

/// Restores the terminal on drop. Errors are ignored: on a closed tty writes fail
/// and `eprintln!` panics.
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

/// Reads terminal input on its own thread: crossterm 0.29 spins in `poll`/`read`
/// after tty EOF, which would stall the loop that handles SIGHUP. The returned slot
/// receives the thread's panic message.
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
        // `tx` drops only after the report is stored.
    });
    (rx, failure)
}

/// Child side: prints `<protocol> <font width> <font height>` to stderr; stdout is the terminal.
fn probe_image_protocol() {
    use ratatui_image::picker::ProtocolType;
    use ratatui_image::picker::cap_parser::QueryStdioOptions;

    // In tmux replies may never arrive: answer before the parent's kill so its fallback runs.
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

/// Parent side. Querying in a child means a terminal that never answers can't leave a
/// blocked stdin reader in redtab that swallows the first key.
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
        // tmux answers sixel itself, but Ghostty and kitty can't draw it; kitty graphics
        // pass through.
        Some(ProtocolType::Sixel) if picker.tmux_detected() && tmux_pane_gets_kitty() => {
            picker.set_protocol_type(ProtocolType::Kitty)
        }
        Some(protocol) => picker.set_protocol_type(protocol),
        // Scaling needs a real cell size; without one the probe only guesses.
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

/// In tmux the probe may get no kitty reply even when the outer terminal supports it.
/// Use kitty only if redtab draws straight into its pane (a nested terminal drops
/// passthrough) and every client of every session showing the window is Ghostty or kitty.
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
