mod app;
mod config;
mod keys;
mod media;
mod model;
mod redlib;
mod state;
mod ui;
mod worker;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

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
      --check            print config, subscriptions and Redlib status, then exit
  -h, --help             show this help
  -V, --version          show the version

Press ? inside redtab for key bindings.";

const WORKER_THREADS: usize = 6;
/// Hidden flag: the child process that asks the terminal for its image protocol.
const PROBE_ARG: &str = "--probe-image-protocol";
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

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
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            PROBE_ARG => {
                probe_image_protocol();
                return Ok(None);
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("redtab {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "-c" | "--config" => {
                args.config = Some(it.next().context("--config needs a path")?.into())
            }
            "-s" | "--subreddit" => {
                args.subreddit = Some(it.next().context("--subreddit needs a name")?)
            }
            "--check" => args.check = true,
            other => bail!("unknown argument {other:?}\n\n{USAGE}"),
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
        println!("redlib_url     {}", redlib.base());
        println!(
            "redlib         {}",
            if redlib.healthy() {
                "up"
            } else {
                "NOT answering"
            }
        );
        println!("subscriptions  {}", subscriptions.len());
        println!(
            "state          {}",
            config::state_dir().join("state.json").display()
        );
        return Ok(());
    }

    // Enter raw mode before the probe: crossterm records the terminal settings on the
    // first enable only, so exit restores these settings rather than whatever a
    // timed-out probe child leaves behind (no echo, no line mode).
    crossterm::terminal::enable_raw_mode()?;
    let image_picker = if config.image_preview {
        detect_image_picker()
    } else {
        Picker::halfblocks()
    };
    let (pool, messages) = Pool::start(redlib.clone(), WORKER_THREADS);
    let mut terminal = ratatui::init();
    // A terminal can answer the probe late; drop those bytes so they are not read as keys.
    while event::poll(Duration::from_millis(150))? {
        let _ = event::read()?;
    }
    // Closing the terminal (SIGHUP) or `kill` (SIGTERM) quits like `q`, saving state.
    let quit_signal = Arc::new(AtomicBool::new(false));
    for signal in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGHUP] {
        signal_hook::flag::register(signal, Arc::clone(&quit_signal))?;
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
    if let Some(name) = &args.subreddit {
        app.open_subreddit(name);
    }

    let result = (|| -> Result<()> {
        let mut dirty = true;
        while !app.quit {
            if quit_signal.load(Ordering::Relaxed) {
                break;
            }
            if dirty {
                terminal.draw(|f| ui::draw(f, &mut app))?;
                dirty = false;
            }
            if event::poll(Duration::from_millis(100))? {
                match event::read()? {
                    Event::Key(key) => {
                        keys::handle(&mut app, key);
                        dirty = true;
                    }
                    Event::Resize(..) => dirty = true,
                    _ => {}
                }
            }
            while let Ok(msg) = messages.try_recv() {
                app.on_msg(msg);
                dirty = true;
            }
            dirty |= app.tick();
        }
        Ok(())
    })();
    ratatui::restore();
    app.save_state();
    if let Some(status) = app.status.as_ref().filter(|s| s.error) {
        eprintln!("redtab: {}", status.text);
    }
    result
}

/// Child side: query the terminal and report `<protocol> <font width> <font height>` on stderr
/// (stdout stays on the terminal, where the query escapes go).
fn probe_image_protocol() {
    use ratatui_image::picker::ProtocolType;
    if let Ok(picker) = Picker::from_query_stdio() {
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
        Some("kitty") => ProtocolType::Kitty,
        Some("sixel") => ProtocolType::Sixel,
        Some("iterm2") => ProtocolType::Iterm2,
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
    picker.set_protocol_type(protocol);
    picker
}
