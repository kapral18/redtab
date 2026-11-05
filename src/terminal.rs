use super::*;

/// Restores the terminal on drop. Errors are ignored: on a closed tty writes fail
/// and `eprintln!` panics.
pub(super) struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

pub(super) fn install_panic_hook() {
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().name() == Some("main") {
            let _ = terminal_output::drain_output();
            restore_terminal();
            let backtrace = std::backtrace::Backtrace::capture();
            if backtrace.status() == std::backtrace::BacktraceStatus::Captured {
                terminal_output::diagnostic(format_args!("thread 'main' {info}\n{backtrace}"));
            } else {
                terminal_output::diagnostic(format_args!("thread 'main' {info}"));
            }
        } else {
            // The panicking thread catches it and reports this text.
            worker::note_panic(info.to_string());
        }
    }));
}

pub(super) fn restore_terminal() {
    let _ = crossterm::terminal::disable_raw_mode();
    let Ok(mut output) = terminal_output::TerminalWriter::open(Duration::from_millis(100)) else {
        return;
    };
    let _ = crossterm::execute!(output, EndSynchronizedUpdate);
    if terminal_output::inside_tmux() {
        let _ = crossterm::execute!(
            output,
            crossterm::style::Print(terminal_output::TMUX_END),
            crossterm::event::DisableFocusChange
        );
    }
    if terminal_output::modify_other_keys() {
        let _ = crossterm::execute!(
            output,
            crossterm::style::Print(terminal_output::MODIFY_OTHER_KEYS_OFF)
        );
    }
    let _ = crossterm::execute!(
        output,
        crossterm::event::PopKeyboardEnhancementFlags,
        crossterm::event::DisableBracketedPaste,
        crossterm::cursor::Show,
        crossterm::terminal::LeaveAlternateScreen
    );
}

pub(super) type CursorQuery = std::sync::mpsc::SyncSender<std::io::Result<()>>;

pub(super) struct InputReader {
    pub(super) events: Receiver<Event>,
    pub(super) failure: Arc<std::sync::Mutex<Option<String>>>,
    pub(super) queries: std::sync::mpsc::Sender<CursorQuery>,
}

/// Bound the frame wait; the input thread owns crossterm's reply parser.
pub(super) fn cursor_barrier(
    queries: &std::sync::mpsc::Sender<CursorQuery>,
) -> std::io::Result<()> {
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
pub(super) fn spawn_input_reader() -> InputReader {
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

#[cfg(test)]
mod tests;
