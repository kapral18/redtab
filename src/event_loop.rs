use super::*;

pub(super) fn run() -> Result<()> {
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
    if args.check {
        let reddit = Reddit::new(config.show_nsfw)?;
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
            Ok(entries) => {
                let communities = entries
                    .iter()
                    .filter(|entry| entry.community().is_some())
                    .count();
                println!(
                    "subscriptions  {communities} communities, {} following (Reddit account)",
                    entries.len() - communities
                );
            }
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

    install_panic_hook();
    // The credential store can take seconds (on macOS, after each rebuild); read it
    // while the terminal is set up and the startup screen is drawn.
    let show_nsfw = config.show_nsfw;
    let signin = std::thread::spawn(move || {
        std::panic::catch_unwind(|| Reddit::new(show_nsfw)).unwrap_or_else(|_| {
            Err(anyhow::anyhow!(
                worker::take_panic().unwrap_or_else(|| "a bug in redtab".into())
            ))
        })
    });
    let mut tmux_passthrough = terminal_output::TmuxPane::for_stdout().and_then(|pane| {
        terminal_output::TmuxPassthrough::capture(move |args| pane.command_for_pane(args))
    });
    // Download folders of exited redtabs; off the UI thread, as $TMPDIR can be large.
    std::thread::spawn(worker::remove_stale_media_dirs);

    // SIGHUP, SIGTERM and SIGINT quit like `q`; registered before the probe.
    // crossterm saves termios on the first raw-mode enable, so enabling before the
    // probe makes exit restore the original, not what a killed probe left.
    crossterm::terminal::enable_raw_mode()?;
    let guard = TerminalGuard;
    let mut image_picker = detect_image_picker();
    if image_picker.tmux_detected()
        && image_picker.protocol_type() == ratatui_image::picker::ProtocolType::Kitty
        && !tmux_passthrough
            .as_mut()
            .is_some_and(|passthrough| passthrough.enable_native())
    {
        image_picker.set_protocol_type(ratatui_image::picker::ProtocolType::Halfblocks);
    }
    let mut graphics_watch = (image_picker.tmux_detected()
        && image_picker.protocol_type() == ratatui_image::picker::ProtocolType::Halfblocks
        && tmux_passthrough.is_some())
    .then(TmuxGraphicsWatch::start);
    // The terminal may have closed during the probe.
    if quit_signal.load(Ordering::Relaxed) {
        return Ok(());
    }
    let input_reader = spawn_input_reader();
    let queries = input_reader.queries.clone();
    let mut terminal_writer = terminal_output::TerminalWriter::open(Duration::from_secs(2))?;
    terminal_writer.enter_screen()?;
    let (output_writer, output) = terminal_output::AsyncWriter::new(terminal_output::Writer::new(
        terminal_writer,
        move || cursor_barrier(&queries),
    ));
    terminal_output::install_output(output.clone());
    let backend = ratatui::backend::CrosstermBackend::new(output_writer);
    let mut terminal = ratatui::Terminal::new(backend)?;
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
    let mut early_events = Vec::new();
    let waited = (|| -> Result<bool> {
        let mut phase = 0;
        while !signin.is_finished() {
            if quit_signal.load(Ordering::Relaxed) {
                return Ok(false);
            }
            if output.ready()?
                && drawn(draw_synchronized(&mut terminal, |f| {
                    ui::draw_startup(f, phase);
                    if no_color {
                        ui::strip_colors(f.buffer_mut());
                    }
                }))?
            {
                phase += 1;
            }
            match input.recv_timeout(Duration::from_millis(80)) {
                // Raw mode turns C-c into a key; it quits here as it does later.
                Ok(event) if quit_key(&event) => return Ok(false),
                Ok(event) => early_events.push(event),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    std::thread::sleep(Duration::from_millis(80))
                }
            }
        }
        Ok(true)
    })();
    if !matches!(waited, Ok(true)) {
        // The shutdown flag stops the credential helper; joining lets it remove its files.
        quit_signal.store(true, Ordering::Relaxed);
        let _ = signin.join();
        let _ = output.wait();
        // As on the main exit: Terminal's drop would only error on a dead tty.
        std::mem::forget(terminal);
        drop(guard);
        return waited.map(drop);
    }
    let (reddit, signin_error) = match signin.join() {
        Ok(Ok(reddit)) => (reddit, None),
        Ok(Err(error)) => (
            Reddit::disconnected(config.show_nsfw)?,
            Some(error.to_string()),
        ),
        Err(panic) => std::panic::resume_unwind(panic),
    };
    let cache = cache::Cache::new(config::cache_dir());
    let (pool, mut messages) = Pool::start(reddit.clone(), WORKER_THREADS, Some(cache));
    let state_root = config::state_dir();
    let state_path = if reddit.needs_sign_in() {
        state_root.join("signed-out/state.json")
    } else {
        auth::account_state_path(&state_root, reddit.account().as_ref())
    };
    let mut app = App::new(config, pool, reddit, state_path, image_picker);
    app.install_settings_store(settings_store.unwrap());
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
        let mut backlog = false;
        while !app.quit {
            if quit_signal.load(Ordering::Relaxed) {
                break;
            }
            let output_ready = output.ready()?;
            if dirty
                && output_ready
                && drawn(draw_synchronized(&mut terminal, |f| {
                    ui::draw(f, &mut app);
                    if no_color {
                        ui::strip_colors(f.buffer_mut());
                    }
                }))?
            {
                dirty = false;
                // Keys typed on the startup screen, once the first draw sized the layout.
                for ev in early_events.drain(..) {
                    if app.quit {
                        break;
                    }
                    handle_input(&mut app, ev, graphics_watch.as_ref());
                    dirty = true;
                }
            }
            if let Some(found) = graphics_watch.as_ref().and_then(TmuxGraphicsWatch::result) {
                graphics_watch = None;
                if let Some(font) = found
                    && tmux_passthrough
                        .as_mut()
                        .is_some_and(|passthrough| passthrough.enable_native())
                {
                    app.replace_image_picker(protocol_picker(
                        ratatui_image::picker::ProtocolType::Kitty,
                        font,
                    ));
                    dirty = true;
                }
            }
            let mut wait = if output_ready {
                app.wake_in()
            } else {
                app.wake_in().min(Duration::from_millis(10))
            };
            if graphics_watch.is_some() {
                wait = wait.min(TmuxGraphicsWatch::POLL);
            }
            if backlog {
                wait = Duration::ZERO;
            }
            match input.recv_timeout(wait) {
                // Keys keep their order behind the startup ones until the first draw.
                Ok(first) if !early_events.is_empty() => {
                    for ev in std::iter::once(first).chain(input.try_iter()) {
                        // C-c quits even while output stalls before the first draw.
                        app.quit |= quit_key(&ev);
                        early_events.push(ev);
                    }
                }
                Ok(first) => {
                    for ev in std::iter::once(first).chain(input.try_iter()) {
                        // Keys after a quit key do nothing.
                        if app.quit {
                            break;
                        }
                        handle_input(&mut app, ev, graphics_watch.as_ref());
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
            let drained = drain_messages(&mut app, &messages);
            dirty |= drained.changed;
            backlog = drained.more;
            dirty |= app.tick();
        }
        Ok(())
    }));
    // Save first: it never touches the terminal, which may be gone. Runs after a panic too.
    app.save_state();
    let output_result = output.wait();
    drop(guard);
    drop(tmux_passthrough);
    // The guard restored the terminal; Terminal's drop would only error on a dead tty.
    std::mem::forget(terminal);
    if let Some(status) = app.status.as_ref().filter(|s| s.error) {
        terminal_output::diagnostic(format_args!("redtab: {}", status.text));
    }
    match run {
        Ok(result) => result.and(output_result.map_err(Into::into)),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

/// A pane out of view at startup gets no probe reply and draws half-blocks. This
/// watches, off the UI thread, for a kitty-capable client to show it: tmux reports
/// focus-in only with `focus-events on`, so the clients are also polled.
pub(super) struct TmuxGraphicsWatch {
    wake: std::sync::mpsc::Sender<()>,
    found: Receiver<ratatui_image::FontSize>,
}

impl TmuxGraphicsWatch {
    pub(super) const POLL: Duration = Duration::from_secs(2);

    pub(super) fn start() -> Self {
        let (wake, woken) = std::sync::mpsc::channel();
        let (send, found) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            loop {
                if let Some((clients, sessions)) = tmux_window_clients() {
                    let sessions: Vec<&str> = sessions.iter().map(String::as_str).collect();
                    if let (true, Some(font)) = tmux_client_graphics(&clients, &sessions) {
                        let _ = send.send(font);
                        return;
                    }
                    // Shown by a terminal without kitty graphics: half-blocks are right.
                    if clients.lines().any(|line| {
                        line.split_whitespace()
                            .next()
                            .is_some_and(|session| sessions.contains(&session))
                    }) {
                        return;
                    }
                }
                if woken.recv_timeout(Self::POLL) == Err(RecvTimeoutError::Disconnected) {
                    return;
                }
            }
        });
        Self { wake, found }
    }

    /// `None` while watching; then the font, or `Some(None)` once the watch gave up.
    pub(super) fn result(&self) -> Option<Option<ratatui_image::FontSize>> {
        match self.found.try_recv() {
            Ok(font) => Some(Some(font)),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(None),
        }
    }
}

pub(super) fn handle_input(app: &mut App, event: Event, watch: Option<&TmuxGraphicsWatch>) {
    if let (Event::FocusGained, Some(watch)) = (&event, watch) {
        let _ = watch.wake.send(());
    }
    handle_event(app, event);
}

/// A terminal command (an OSC 52 copy) queued between the readiness check and the frame's
/// flush holds the frame back; it stays buffered and goes out with the next draw.
pub(super) fn drawn(result: std::io::Result<()>) -> std::io::Result<bool> {
    match result {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(false),
        Err(error) => Err(error),
    }
}

/// Hold text/placeholder changes until the complete draw has been written.
pub(super) fn draw_synchronized<W: std::io::Write>(
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

pub(super) fn open_startup_subreddit(app: &mut App, pending: &mut Option<String>) -> bool {
    if app.reddit.needs_sign_in() {
        return false;
    }
    if let Some(name) = pending.take() {
        app.open_subreddit(&name);
        return true;
    }
    false
}

pub(super) fn quit_key(event: &Event) -> bool {
    matches!(event, Event::Key(key) if key.modifiers == crossterm::event::KeyModifiers::CONTROL
        && key.code == crossterm::event::KeyCode::Char('c'))
}

pub(super) struct Drained {
    pub(super) changed: bool,
    /// The burst ended before the queue did: the next pass does not wait.
    pub(super) more: bool,
}

/// Return to input between bursts of worker completions.
pub(super) fn drain_messages(app: &mut App, messages: &Receiver<worker::Msg>) -> Drained {
    let started = Instant::now();
    let mut drained = Drained {
        changed: false,
        more: false,
    };
    for _ in 0..8 {
        let Ok(message) = messages.try_recv() else {
            return drained;
        };
        app.on_msg(message);
        drained.changed = true;
        if started.elapsed() >= Duration::from_millis(4) {
            break;
        }
    }
    drained.more = true;
    drained
}

pub(super) fn handle_event(app: &mut App, event: Event) {
    match event {
        Event::Key(key) => keys::handle(app, key),
        Event::Paste(text) => keys::paste(app, &text),
        Event::Resize(width, _) => app.resize(width),
        _ => {}
    }
}

#[cfg(test)]
mod tests;
