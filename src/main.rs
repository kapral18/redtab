mod app;
mod auth;
mod auth_browser;
mod cache;
mod card_image;
mod config;
mod keys;
mod media;
mod model;
mod reddit;
mod state;
mod terminal_output;
mod ui;
mod video;
mod worker;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use crossterm::event::{self, Event};
use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};
use crossterm::{ExecutableCommand, QueueableCommand};
use ratatui_image::picker::Picker;

use crate::app::App;
use crate::config::Config;
use crate::reddit::{Health, Reddit};
use crate::worker::Pool;

const USAGE: &str = "\
redtab — terminal Reddit reader with tabs and browser-session sign-in

Usage: redtab [--config PATH] [--subreddit NAME]

Options:
  -c, --config PATH      config file (default: $XDG_CONFIG_HOME/redtab/config.toml)
  -s, --subreddit NAME   open this subreddit on start
      --signin           connect a browser session (or import Netscape cookies from stdin)
      --browser NAME     choose a browser with --signin (for example chrome or firefox)
      --profile NAME     choose its profile with --signin --browser
      --check            print config, Home status, account subscription count and state file;
                         exit 1 if Home or subscriptions cannot load
  -h, --help             show this help
  -V, --version          show the version

Sign-in is encrypted in macOS Keychain or Linux Secret Service.
Press ? inside redtab for key bindings.";

const WORKER_THREADS: usize = 6;
/// Hidden flag for the child that queries the terminal's image protocol.
const PROBE_ARG: &str = "--probe-image-protocol";
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
/// The child's inactivity wait in tmux; the parent bounds the whole probe.
const PROBE_QUERY_TIMEOUT: Duration = Duration::from_secs(1);

struct Args {
    config: Option<PathBuf>,
    subreddit: Option<String>,
    check: bool,
    signin: bool,
    browser: Option<String>,
    profile: Option<String>,
}

fn parse_args() -> Result<Option<Args>> {
    parse_args_from(std::env::args_os().skip(1))
}

fn parse_args_from(mut it: impl Iterator<Item = std::ffi::OsString>) -> Result<Option<Args>> {
    let mut args = Args {
        config: None,
        subreddit: None,
        check: false,
        signin: false,
        browser: None,
        profile: None,
    };
    // OS strings: a non-UTF-8 argument errors instead of panicking.
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
            Some("--signin") => args.signin = true,
            Some("--browser") => {
                args.browser = Some(
                    it.next()
                        .context("--browser needs a name")?
                        .into_string()
                        .map_err(|_| anyhow::anyhow!("invalid browser name"))?,
                )
            }
            Some("--profile") => {
                args.profile = Some(
                    it.next()
                        .context("--profile needs a name")?
                        .into_string()
                        .map_err(|_| anyhow::anyhow!("invalid profile name"))?,
                )
            }
            _ => bail!("unknown argument {arg:?}\n\n{USAGE}"),
        }
    }
    if (args.browser.is_some() || args.profile.is_some()) && !args.signin {
        bail!("--browser and --profile require --signin");
    }
    if args.profile.is_some() && args.browser.is_none() {
        bail!("--profile requires --browser");
    }
    if args.signin && (args.check || args.config.is_some() || args.subreddit.is_some()) {
        bail!("--signin cannot be combined with --check, --config or --subreddit");
    }
    Ok(Some(args))
}

fn main() -> Result<()> {
    if std::env::args_os()
        .nth(1)
        .is_some_and(|a| a == auth_browser::HELPER)
    {
        if std::env::args_os().count() != 2 {
            bail!("Invalid browser helper arguments");
        }
        return auth_browser::helper();
    }
    let Some(args) = parse_args()? else {
        return Ok(());
    };
    let quit_signal = auth::shutdown_flag();
    for signal in [
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
        signal_hook::consts::SIGINT,
    ] {
        signal_hook::flag::register(signal, Arc::clone(&quit_signal))?;
    }
    if args.signin {
        use std::io::IsTerminal;
        if !std::io::stdin().is_terminal() {
            if args.browser.is_some() {
                bail!("--browser requires an interactive terminal");
            }
            let account = reddit::sign_in(std::io::stdin().lock())?;
            println!(
                "Connected as u/{}. Reddit sign-in saved in the OS credential store.",
                account.name
            );
            return Ok(());
        }
    }
    let settings_store = if args.check {
        None
    } else {
        Some(config::store::Store::open(args.config.as_deref())?)
    };
    let config = if let Some(store) = &settings_store {
        store.current.clone()
    } else {
        Config::load(args.config.as_deref())?
    };
    let (reddit, signin_error) = match Reddit::new(config.show_nsfw) {
        Ok(reddit) => (reddit, None),
        Err(error) if !args.check => (
            Reddit::disconnected(config.show_nsfw)?,
            Some(error.to_string()),
        ),
        Err(error) => return Err(error),
    };

    if args.check {
        let health = reddit.check();
        let subscriptions = reddit.subscriptions();
        let config_file = if config.path.exists() {
            config.path.display().to_string()
        } else {
            format!("{} (missing; defaults)", config.path.display())
        };
        println!("config         {config_file}");
        println!("sign-in        OS credential store");
        println!(
            "reddit         {}",
            match health {
                Health::Up => "up",
                Health::Paused => "paused",
                Health::Down => "NOT answering",
                Health::Unknown => "unknown",
                Health::SignedOut => "sign-in required",
            }
        );
        match &subscriptions {
            Ok(names) => println!("subscriptions  {} (Reddit account)", names.len()),
            Err(error) => println!("subscriptions  unavailable: {error}"),
        }
        println!(
            "state          {}",
            auth::account_state_path(&config::state_dir(), reddit.account().as_ref()).display()
        );
        if health != Health::Up || subscriptions.is_err() {
            std::process::exit(1);
        }
        return Ok(());
    }

    // Download folders of exited redtabs; off the UI thread, as $TMPDIR can be large.
    std::thread::spawn(worker::remove_stale_media_dirs);

    // SIGHUP, SIGTERM and SIGINT quit like `q`; registered before the probe.
    // crossterm saves termios on the first raw-mode enable, so enabling before the
    // probe makes exit restore the original, not what a killed probe left.
    crossterm::terminal::enable_raw_mode()?;
    let guard = TerminalGuard;
    let image_picker = detect_image_picker();
    let cache = cache::Cache::new(config::cache_dir());
    let (pool, mut messages) = Pool::start(reddit.clone(), WORKER_THREADS, Some(cache));
    // The terminal may have closed during the probe.
    if quit_signal.load(Ordering::Relaxed) {
        return Ok(());
    }
    let input_reader = spawn_input_reader();
    let queries = input_reader.queries.clone();
    crossterm::execute!(std::io::stdout(), crossterm::terminal::EnterAlternateScreen)?;
    // The input thread writes cursor queries through stdout, so keep it unlocked here.
    let backend = ratatui::backend::CrosstermBackend::new(terminal_output::Writer::new(
        std::io::stdout(),
        move || cursor_barrier(&queries),
    ));
    let mut terminal = ratatui::Terminal::new(backend)?;
    // Restore quietly on main; worker panics become job errors.
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
    let InputReader {
        events: input,
        failure: input_failure,
        ..
    } = input_reader;
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
    let state_root = config::state_dir();
    let state_path = if reddit.needs_sign_in() {
        state_root.join("signed-out/state.json")
    } else {
        auth::account_state_path(&state_root, reddit.account().as_ref())
    };
    let mut app = App::new(config, pool, reddit, state_path, image_picker);
    app.settings = app::settings::Settings::new(settings_store.unwrap());
    app.no_color = no_color;
    app.start();
    if args.signin || app.reddit.needs_sign_in() {
        app.auth.cli_selection = args
            .browser
            .clone()
            .map(|browser| (browser, args.profile.clone()));
        app.show_auth();
        if let Some(error) = signin_error {
            app.auth.message = Some(error);
        }
    }
    let mut startup_subreddit = args.subreddit.clone();
    open_startup_subreddit(&mut app, &mut startup_subreddit);

    let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<()> {
        let mut dirty = true;
        while !app.quit {
            if quit_signal.load(Ordering::Relaxed) {
                break;
            }
            if dirty {
                draw_synchronized(&mut terminal, |f| {
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
            dirty |= app.poll_auth();
            dirty |= open_startup_subreddit(&mut app, &mut startup_subreddit);
            if let Some(replacement) = app.take_replacement_messages() {
                messages = replacement;
            }
            dirty |= drain_messages(&mut app, &messages);
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

/// Hold text/placeholder changes until the complete draw has been written.
fn draw_synchronized<W: std::io::Write>(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<W>>,
    draw: impl FnOnce(&mut ratatui::Frame),
) -> std::io::Result<()> {
    terminal.backend_mut().queue(BeginSynchronizedUpdate)?;
    let drawn = terminal.draw(draw).map(|_| ());
    let ended = terminal
        .backend_mut()
        .execute(EndSynchronizedUpdate)
        .map(|_| ());
    drawn.and(ended)
}

fn open_startup_subreddit(app: &mut App, pending: &mut Option<String>) -> bool {
    if app.reddit.needs_sign_in() {
        return false;
    }
    if let Some(name) = pending.take() {
        app.open_subreddit(&name);
        return true;
    }
    false
}

/// Return to input between bursts of worker completions.
fn drain_messages(app: &mut App, messages: &Receiver<worker::Msg>) -> bool {
    let started = Instant::now();
    let mut changed = false;
    for _ in 0..8 {
        let Ok(message) = messages.try_recv() else {
            break;
        };
        app.on_msg(message);
        changed = true;
        if started.elapsed() >= Duration::from_millis(4) {
            break;
        }
    }
    changed
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
        let _ = crossterm::execute!(std::io::stdout(), EndSynchronizedUpdate);
        if Picker::halfblocks().tmux_detected() {
            let _ = crossterm::execute!(
                std::io::stdout(),
                crossterm::style::Print(terminal_output::TMUX_END)
            );
        }
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::event::DisableBracketedPaste,
            crossterm::cursor::Show
        );
        let _ = ratatui::try_restore();
    }
}

type CursorQuery = std::sync::mpsc::SyncSender<std::io::Result<()>>;

struct InputReader {
    events: Receiver<Event>,
    failure: Arc<std::sync::Mutex<Option<String>>>,
    queries: std::sync::mpsc::Sender<CursorQuery>,
}

/// Bound the frame wait; the input thread owns crossterm's reply parser.
fn cursor_barrier(queries: &std::sync::mpsc::Sender<CursorQuery>) -> std::io::Result<()> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    queries.send(tx).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "terminal input stopped before graphics acknowledgment",
        )
    })?;
    rx.recv_timeout(Duration::from_millis(100))
        .map_err(|error| {
            std::io::Error::new(
                if error == RecvTimeoutError::Timeout {
                    std::io::ErrorKind::TimedOut
                } else {
                    std::io::ErrorKind::BrokenPipe
                },
                "tmux did not acknowledge the graphics upload",
            )
        })?
}

/// Keep reads off main because crossterm can spin on tty EOF. Poll between reads
/// so cursor queries run on this same thread without competing for terminal input.
fn spawn_input_reader() -> InputReader {
    let (tx, rx) = std::sync::mpsc::channel();
    let (queries, requests) = std::sync::mpsc::channel::<CursorQuery>();
    let failure = Arc::new(std::sync::Mutex::new(None));
    let report = Arc::clone(&failure);
    std::thread::spawn(move || {
        let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            loop {
                if let Ok(reply) = requests.try_recv() {
                    let _ = reply.send(crossterm::cursor::position().map(|_| ()));
                }
                match event::poll(Duration::from_millis(5)) {
                    Ok(false) => continue,
                    Ok(true) => match event::read() {
                        Ok(event) => {
                            if tx.send(event).is_err() {
                                break;
                            }
                        }
                        _ => break,
                    },
                    Err(_) => break,
                }
            }
        }));
        if read.is_err()
            && let Ok(mut slot) = report.lock()
        {
            *slot = Some(worker::take_panic().unwrap_or_else(|| "a bug in redtab".into()));
        }
    });
    InputReader {
        events: rx,
        failure,
        queries,
    }
}

/// Child side: prints `<protocol> <font width> <font height>` to stderr; stdout is the terminal.
fn probe_image_protocol() {
    use ratatui_image::picker::ProtocolType;
    use ratatui_image::picker::cap_parser::QueryStdioOptions;

    // In tmux replies may never arrive; the parent also handles a killed probe.
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
    let report = image_protocol_report();
    let native_tmux = Picker::halfblocks().tmux_detected() && tmux_pane_gets_kitty();
    let window = crossterm::terminal::window_size().ok();
    let Some((protocol, font)) = image_protocol_choice(report.as_deref(), native_tmux, window)
    else {
        return Picker::halfblocks();
    };
    // `from_fontsize` retains tmux passthrough wrapping.
    #[allow(deprecated)]
    let mut picker = Picker::from_fontsize(font);
    picker.set_protocol_type(protocol);
    picker
}

fn image_protocol_report() -> Option<String> {
    use std::io::Read;
    use std::process::{Command, Stdio};

    let exe = std::env::current_exe().ok()?;
    let mut child = Command::new(exe)
        .arg(PROBE_ARG)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
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
                return None;
            }
        }
    }
    let mut report = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        let _ = stderr.read_to_string(&mut report);
    }
    Some(report)
}

fn image_protocol_choice(
    report: Option<&str>,
    native_tmux: bool,
    window: Option<crossterm::terminal::WindowSize>,
) -> Option<(ratatui_image::picker::ProtocolType, ratatui_image::FontSize)> {
    use ratatui_image::{FontSize, picker::ProtocolType};
    let reported = report.and_then(|report| {
        let mut parts = report.split_whitespace();
        let protocol = match parts.next()? {
            "kitty" => ProtocolType::Kitty,
            "sixel" => ProtocolType::Sixel,
            "iterm2" => ProtocolType::Iterm2,
            "halfblocks" => ProtocolType::Halfblocks,
            _ => return None,
        };
        let width = parts.next()?.parse::<u16>().ok().filter(|n| *n > 0)?;
        let height = parts.next()?.parse::<u16>().ok().filter(|n| *n > 0)?;
        parts
            .next()
            .is_none()
            .then_some((protocol, FontSize::new(width, height)))
    });
    match reported {
        // tmux's own sixel answer does not mean Ghostty can display sixel.
        Some((ProtocolType::Sixel, font)) if native_tmux => Some((ProtocolType::Kitty, font)),
        Some((protocol, font)) if protocol != ProtocolType::Halfblocks => Some((protocol, font)),
        _ if native_tmux => {
            let window = window?;
            let width = window
                .width
                .checked_div(window.columns)
                .filter(|n| *n > 0)?;
            let height = window.height.checked_div(window.rows).filter(|n| *n > 0)?;
            Some((ProtocolType::Kitty, FontSize::new(width, height)))
        }
        _ => None,
    }
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

#[cfg(test)]
mod auth_cli_tests {
    use super::*;

    #[test]
    fn when_first_run_signin_is_needed_it_should_open_the_startup_subreddit_once_after_connecting()
    {
        let directory = tempfile::tempdir().unwrap();
        let make_app = |reddit: Reddit| {
            let (pool, _messages) = Pool::start(reddit.clone(), 1, None);
            App::new(
                Config::default(),
                pool,
                reddit,
                directory.path().join("state.json"),
                Picker::halfblocks(),
            )
        };
        let mut app = make_app(Reddit::disconnected(false).unwrap());
        let mut pending = Some("rust".into());
        assert!(!open_startup_subreddit(&mut app, &mut pending));
        assert_eq!(app.tabs.len(), 3);
        // Initial sign-in installs a new account-owned App. The CLI intent outlives it.
        app = make_app(Reddit::test_at("http://127.0.0.1:9", false));
        assert!(open_startup_subreddit(&mut app, &mut pending));
        assert_eq!(app.active_tab().subreddit(), Some("rust"));
        assert!(pending.is_none());
        assert!(!open_startup_subreddit(&mut app, &mut pending));
        assert_eq!(app.tabs.len(), 4);
    }

    #[test]
    fn when_signin_selects_a_browser_it_should_keep_a_non_ascii_profile_name() {
        let actual = parse_args_from(
            ["--signin", "--browser", "firefox", "--profile", "Wørk"]
                .into_iter()
                .map(Into::into),
        )
        .unwrap()
        .unwrap();
        assert!(actual.signin);
        assert_eq!(actual.browser.as_deref(), Some("firefox"));
        assert_eq!(actual.profile.as_deref(), Some("Wørk"));
    }

    #[test]
    fn when_profile_selection_has_no_browser_it_should_fail_before_authentication() {
        let actual = parse_args_from(
            ["--signin", "--profile", "Work"]
                .into_iter()
                .map(Into::into),
        );
        assert!(actual.is_err());
    }

    #[test]
    fn when_signin_is_combined_with_check_it_should_fail_before_authentication() {
        let actual = parse_args_from(["--signin", "--check"].into_iter().map(Into::into));
        assert!(actual.is_err());
    }
}

#[cfg(test)]
mod event_loop_tests {
    use super::*;

    #[test]
    fn terminal_draw_should_hold_graphics_and_placeholders_until_complete() {
        let mut bytes = Vec::<u8>::new();
        let backend = ratatui::backend::CrosstermBackend::new(&mut bytes);
        let mut terminal = ratatui::Terminal::with_options(
            backend,
            ratatui::TerminalOptions {
                viewport: ratatui::Viewport::Fixed(ratatui::layout::Rect::new(0, 0, 4, 2)),
            },
        )
        .unwrap();
        draw_synchronized(&mut terminal, |frame| {
            frame.buffer_mut()[(0, 0)].set_symbol("frame");
        })
        .unwrap();
        drop(terminal);
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.starts_with("\x1b[?2026h"), "{text:?}");
        assert!(text.contains("frame"));
        assert!(text.rfind("\x1b[?2026l").unwrap() > text.find("frame").unwrap());
    }

    #[test]
    fn a_failed_terminal_draw_should_end_synchronization() {
        #[derive(Default)]
        struct Writer {
            bytes: Vec<u8>,
            fail_draw: bool,
            fail_once: bool,
        }
        impl std::io::Write for Writer {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if std::mem::take(&mut self.fail_once) {
                    return Err(std::io::Error::other("draw failed"));
                }
                self.bytes.extend_from_slice(bytes);
                if self.fail_draw && self.bytes.ends_with(b"\x1b[?2026h") {
                    self.fail_draw = false;
                    self.fail_once = true;
                }
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut writer = Writer {
            fail_draw: true,
            ..Writer::default()
        };
        let backend = ratatui::backend::CrosstermBackend::new(&mut writer);
        let mut terminal = ratatui::Terminal::with_options(
            backend,
            ratatui::TerminalOptions {
                viewport: ratatui::Viewport::Fixed(ratatui::layout::Rect::new(0, 0, 4, 2)),
            },
        )
        .unwrap();
        assert!(
            draw_synchronized(&mut terminal, |frame| {
                frame.buffer_mut()[(0, 0)].set_symbol("frame");
            })
            .is_err()
        );
        drop(terminal);
        assert!(writer.bytes.ends_with(b"\x1b[?2026l"));
    }

    #[test]
    fn cursor_barrier_should_require_a_successful_reply_and_fail_closed() {
        let (tx, rx) = std::sync::mpsc::channel::<CursorQuery>();
        let worker = std::thread::spawn(move || {
            rx.recv()
                .unwrap()
                .send(Err(std::io::Error::other("query failed")))
                .unwrap();
        });
        assert!(cursor_barrier(&tx).is_err());
        worker.join().unwrap();
        let (tx, rx) = std::sync::mpsc::channel::<CursorQuery>();
        drop(rx);
        assert_eq!(
            cursor_barrier(&tx).unwrap_err().kind(),
            std::io::ErrorKind::BrokenPipe
        );
        let (tx, _rx) = std::sync::mpsc::channel::<CursorQuery>();
        assert_eq!(
            cursor_barrier(&tx).unwrap_err().kind(),
            std::io::ErrorKind::TimedOut
        );
    }

    fn graphics_window() -> crossterm::terminal::WindowSize {
        crossterm::terminal::WindowSize {
            columns: 100,
            rows: 50,
            width: 800,
            height: 800,
        }
    }

    #[test]
    fn when_tmux_reports_no_graphics_or_the_probe_fails_it_should_keep_native_kitty_pixels() {
        use ratatui_image::picker::ProtocolType;
        for report in [
            None,
            Some(""),
            Some("invalid"),
            Some("halfblocks 10 20"),
            Some("kitty 0 20"),
            Some("kitty x 20"),
        ] {
            assert_eq!(
                image_protocol_choice(report, true, Some(graphics_window()))
                    .map(|(p, f)| (p, f.width, f.height)),
                Some((ProtocolType::Kitty, 8, 16)),
                "{report:?}"
            );
        }
    }

    #[test]
    fn when_the_probe_reports_native_graphics_it_should_keep_its_measured_cell_size() {
        use ratatui_image::picker::ProtocolType;
        for (report, native_tmux, expected) in [
            ("kitty 9 18", true, ProtocolType::Kitty),
            ("iterm2 9 18", false, ProtocolType::Iterm2),
            ("sixel 9 18", false, ProtocolType::Sixel),
            ("sixel 9 18", true, ProtocolType::Kitty),
        ] {
            assert_eq!(
                image_protocol_choice(Some(report), native_tmux, None)
                    .map(|(p, f)| (p, f.width, f.height)),
                Some((expected, 9, 18)),
                "{report}"
            );
        }
    }

    #[test]
    fn when_client_support_or_cell_size_is_unverified_it_should_not_force_kitty() {
        assert!(image_protocol_choice(None, false, Some(graphics_window())).is_none());
        assert!(image_protocol_choice(None, true, None).is_none());
        for field in 0..4 {
            let mut window = graphics_window();
            match field {
                0 => window.columns = 0,
                1 => window.rows = 0,
                2 => window.width = 0,
                _ => window.height = 0,
            }
            assert!(
                image_protocol_choice(None, true, Some(window)).is_none(),
                "missing dimension {field}"
            );
        }
    }

    #[test]
    fn when_worker_results_arrive_in_a_burst_input_should_get_another_turn() {
        let reddit = Reddit::test_at("http://127.0.0.1:9", false);
        let (pool, _worker_messages) = Pool::start(reddit.clone(), 3, None);
        let mut app = App::new(
            Config::default(),
            pool,
            reddit,
            std::env::temp_dir().join("redtab-event-loop-test.json"),
            Picker::halfblocks(),
        );
        let (sender, messages) = std::sync::mpsc::channel();
        for _ in 0..32 {
            sender
                .send(worker::Msg::CacheWarning("fixture".into()))
                .unwrap();
        }

        assert!(drain_messages(&mut app, &messages));

        assert!(
            (24..32).contains(&messages.try_iter().count()),
            "completion processing must yield after at most eight results"
        );
    }
}
