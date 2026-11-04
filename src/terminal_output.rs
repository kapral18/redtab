//! Keep graphics uploads out of tmux's pane synchronization hold.

use std::io::{self, Write};
use std::ops::Range;
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

pub fn inside_tmux() -> bool {
    std::env::var("TERM").is_ok_and(|term| term.starts_with("tmux"))
        || std::env::var("TERM_PROGRAM").is_ok_and(|program| program == "tmux")
}

pub struct TmuxPane {
    socket: String,
    id: String,
}

impl TmuxPane {
    pub fn for_stdout() -> Option<Self> {
        use std::os::unix::fs::MetadataExt;
        if !inside_tmux() {
            return None;
        }
        let tmux = std::env::var("TMUX").ok()?;
        let socket = tmux.rsplitn(3, ',').nth(2)?.to_owned();
        let pane = Self {
            socket,
            id: std::env::var("TMUX_PANE").ok()?,
        };
        let tty = pane.command_for_pane(&["display-message", "-p", "#{pane_tty}"])?;
        let tty = std::fs::metadata(tty.trim()).ok()?;
        let stdout = std::fs::metadata("/dev/stdout").ok()?;
        (tty.rdev() == stdout.rdev()).then_some(pane)
    }

    pub fn command_for_pane(&self, args: &[&str]) -> Option<String> {
        self.command(&[&args[..1], &["-t", &self.id], &args[1..]].concat())
    }

    pub fn command(&self, args: &[&str]) -> Option<String> {
        use std::process::{Command, Stdio};
        let mut child = Command::new("tmux")
            .args(["-S", &self.socket])
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
            }
        }
        let output = child.wait_with_output().ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }
}

pub struct TmuxPassthrough<C: Fn(&[&str]) -> Option<String>> {
    command: C,
    previous: String,
    expected: &'static str,
}

impl<C: Fn(&[&str]) -> Option<String>> TmuxPassthrough<C> {
    pub fn capture(command: C) -> Option<Self> {
        let previous = command(&["show-options", "-pqv", "allow-passthrough"])?;
        Some(Self {
            command,
            previous,
            expected: "on",
        })
    }

    pub fn enable_native(&mut self) -> bool {
        if (self.command)(&["set-option", "-p", "allow-passthrough", "all"]).is_none() {
            return false;
        }
        self.expected = "all";
        true
    }
}

impl<C: Fn(&[&str]) -> Option<String>> Drop for TmuxPassthrough<C> {
    fn drop(&mut self) {
        let current = (self.command)(&["show-options", "-pqv", "allow-passthrough"]);
        if current.as_deref() != Some(self.expected)
            || current.as_deref() == Some(self.previous.as_str())
        {
            return;
        }
        if self.previous.is_empty() {
            (self.command)(&["set-option", "-pu", "allow-passthrough"]);
        } else {
            (self.command)(&["set-option", "-p", "allow-passthrough", &self.previous]);
        }
    }
}

pub struct TerminalWriter {
    file: std::fs::File,
    timeout: Duration,
    deadline: Option<Instant>,
}

impl TerminalWriter {
    pub fn open(timeout: Duration) -> io::Result<Self> {
        Self::for_descriptor(libc::STDOUT_FILENO, timeout)
    }

    pub fn enter_screen(&mut self) -> io::Result<()> {
        crossterm::execute!(
            self,
            crossterm::terminal::EnterAlternateScreen,
            crossterm::event::EnableBracketedPaste,
            crossterm::event::PushKeyboardEnhancementFlags(
                crossterm::event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            )
        )?;
        // tmux ignores the kitty request; it reports C-i apart from Tab only in
        // modifyOtherKeys mode 2, and crossterm parses only its csi-u format.
        if TmuxPane::for_stdout()
            .and_then(|pane| pane.command(&["show-options", "-gv", "extended-keys-format"]))
            .is_some_and(|format| format == "csi-u")
        {
            self.write_all(MODIFY_OTHER_KEYS_ON.as_bytes())?;
            self.flush()?;
            MODIFY_OTHER_KEYS.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        Ok(())
    }

    fn for_descriptor(descriptor: libc::c_int, timeout: Duration) -> io::Result<Self> {
        use std::os::unix::{ffi::OsStrExt, fs::OpenOptionsExt};
        let mut path = [0; 1024];
        let result = unsafe { libc::ttyname_r(descriptor, path.as_mut_ptr(), path.len()) };
        if result != 0 {
            return Err(io::Error::from_raw_os_error(result));
        }
        let path = unsafe { std::ffi::CStr::from_ptr(path.as_ptr()) };
        let file = std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
            .open(std::ffi::OsStr::from_bytes(path.to_bytes()))?;
        Ok(Self {
            file,
            timeout,
            deadline: None,
        })
    }
}

impl Write for TerminalWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let deadline = *self
            .deadline
            .get_or_insert_with(|| Instant::now() + self.timeout);
        loop {
            match self.file.write(bytes) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                result => return result,
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "terminal output stalled",
                ));
            }
            let mut descriptor = libc::pollfd {
                fd: self.file.as_raw_fd(),
                events: libc::POLLOUT,
                revents: 0,
            };
            let result = unsafe {
                libc::poll(
                    &mut descriptor,
                    1,
                    remaining.as_millis().clamp(1, i32::MAX as u128) as i32,
                )
            };
            if result < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                return Err(io::Error::last_os_error());
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.deadline = None;
        Ok(())
    }
}

pub fn diagnostic(message: std::fmt::Arguments<'_>) {
    use std::os::fd::{FromRawFd, OwnedFd};
    let descriptor = unsafe { libc::fcntl(libc::STDERR_FILENO, libc::F_DUPFD_CLOEXEC, 0) };
    if descriptor >= 0 {
        let descriptor = unsafe { OwnedFd::from_raw_fd(descriptor) };
        diagnostic_to(
            std::fs::File::from(descriptor),
            format!("{message}\n"),
            Duration::from_millis(100),
        );
    }
}

fn diagnostic_to(output: impl Write + Send + 'static, message: String, timeout: Duration) {
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    let _ = std::thread::Builder::new()
        .name("redtab-diagnostic".into())
        .spawn(move || {
            let mut output = output;
            let _ = output.write_all(message.as_bytes());
            let _ = send.send(());
        });
    let _ = receive.recv_timeout(timeout);
}

const BEGIN: &[u8] = b"\x1b[?2026h";
const END: &[u8] = b"\x1b[?2026l";
const TMUX: &[u8] = b"\x1bPtmux;";
const NATIVE_BEGIN: &[u8] = b"\x1b\x1b[?2026h";
const MODIFY_OTHER_KEYS_ON: &str = "\x1b[>4;2m";
pub const MODIFY_OTHER_KEYS_OFF: &str = "\x1b[>4;0m";
static MODIFY_OTHER_KEYS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether `enter_screen` asked tmux for modifyOtherKeys, so restoring must undo it.
pub fn modify_other_keys() -> bool {
    MODIFY_OTHER_KEYS.load(std::sync::atomic::Ordering::Relaxed)
}

pub const TMUX_END: &str = "\x1bPtmux;\x1b\x1b[?2026l\x1b\\";

#[derive(Default)]
struct OutputState {
    pending: usize,
    command_pending: bool,
    error: Option<io::Error>,
}

#[derive(Default)]
struct SharedOutput {
    state: parking_lot::Mutex<OutputState>,
    changed: parking_lot::Condvar,
}

enum Output {
    Frame(Vec<u8>),
    Command(Vec<u8>),
}

#[derive(Clone)]
pub struct OutputHandle {
    shared: std::sync::Arc<SharedOutput>,
    send: std::sync::mpsc::SyncSender<Output>,
}

static OUTPUT: std::sync::OnceLock<OutputHandle> = std::sync::OnceLock::new();

pub fn install_output(output: OutputHandle) {
    let _ = OUTPUT.set(output);
}

pub fn drain_output() -> io::Result<()> {
    OUTPUT.get().map_or(Ok(()), OutputHandle::wait)
}

pub fn queue_command(bytes: &[u8]) -> io::Result<()> {
    OUTPUT
        .get()
        .ok_or_else(|| io::Error::other("terminal output is not initialized"))?
        .command(bytes)
}

impl OutputHandle {
    pub fn command(&self, bytes: &[u8]) -> io::Result<()> {
        let mut state = self.shared.state.lock();
        if state.command_pending {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "terminal command still pending",
            ));
        }
        if let Some(error) = state.error.take() {
            return Err(error);
        }
        if self.send.try_send(Output::Command(bytes.to_vec())).is_err() {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "terminal output stopped",
            ));
        }
        state.pending += 1;
        state.command_pending = true;
        Ok(())
    }

    pub fn ready(&self) -> io::Result<bool> {
        let mut state = self.shared.state.lock();
        match state.error.take() {
            Some(error) => Err(error),
            None => Ok(state.pending == 0),
        }
    }

    pub fn wait(&self) -> io::Result<()> {
        self.wait_for(Duration::from_secs(3))
    }

    fn wait_for(&self, timeout: Duration) -> io::Result<()> {
        let deadline = Instant::now() + timeout;
        let mut state = self.shared.state.lock();
        while state.pending != 0 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "terminal output did not drain",
                ));
            }
            self.shared.changed.wait_for(&mut state, remaining);
        }
        match state.error.take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

pub struct AsyncWriter {
    frame: Vec<u8>,
    send: std::sync::mpsc::SyncSender<Output>,
    shared: std::sync::Arc<SharedOutput>,
}

impl AsyncWriter {
    pub fn new(mut output: impl Write + Send + 'static) -> (Self, OutputHandle) {
        let shared = std::sync::Arc::new(SharedOutput::default());
        let worker = shared.clone();
        let (send, receive) = std::sync::mpsc::sync_channel::<Output>(2);
        std::thread::Builder::new()
            .name("redtab-terminal-output".into())
            .spawn(move || {
                while let Ok(message) = receive.recv() {
                    let command = matches!(message, Output::Command(_));
                    let (Output::Frame(frame) | Output::Command(frame)) = message;
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        output.write_all(&frame).and_then(|()| output.flush())
                    }))
                    .unwrap_or_else(|_| Err(io::Error::other("terminal output worker panicked")));
                    let failed = result.is_err();
                    let mut state = worker.state.lock();
                    state.pending -= 1;
                    if command {
                        state.command_pending = false;
                    }
                    state.error = result.err();
                    if failed {
                        state.pending = 0;
                        state.command_pending = false;
                    }
                    worker.changed.notify_all();
                    if failed {
                        break;
                    }
                }
            })
            .expect("cannot start terminal output");
        (
            Self {
                frame: Vec::new(),
                send: send.clone(),
                shared: shared.clone(),
            },
            OutputHandle { shared, send },
        )
    }
}

impl Write for AsyncWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.frame.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.frame.is_empty() || (self.frame.starts_with(BEGIN) && !self.frame.ends_with(END)) {
            return Ok(());
        }
        let mut state = self.shared.state.lock();
        if let Some(error) = state.error.take() {
            return Err(error);
        }
        if state.pending != 0 {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "terminal frame still pending",
            ));
        }
        state.pending = 1;
        if self
            .send
            .try_send(Output::Frame(std::mem::take(&mut self.frame)))
            .is_err()
        {
            state.pending = 0;
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "terminal output stopped",
            ));
        }
        Ok(())
    }
}

pub struct Writer<W, B> {
    output: W,
    frame: Vec<u8>,
    acknowledge: B,
    barrier_enabled: bool,
}

impl<W: Write, B: FnMut() -> io::Result<()>> Writer<W, B> {
    pub fn new(output: W, acknowledge: B) -> Self {
        Self {
            output,
            frame: Vec::new(),
            acknowledge,
            barrier_enabled: true,
        }
    }

    fn present(&mut self) -> io::Result<()> {
        let uploads = kitty_uploads(&self.frame);
        if uploads.is_empty() {
            return self.output.write_all(&self.frame);
        }
        // tmux restores the pane cursor between input batches only with MODE_SYNC off.
        // The native hold covers its raw-passthrough cursor resets during pixel uploads.
        self.output.write_all(b"\x1b[?25l")?;
        for range in &uploads {
            self.output.write_all(
                &[
                    TMUX,
                    NATIVE_BEGIN,
                    &self.frame[range.start + TMUX.len()..range.end],
                ]
                .concat(),
            )?;
        }
        self.output.flush()?;
        // A same-pane DSR reply orders the next input after tmux's cursor restoration.
        // Starting the grid hold earlier would preserve the last raw reset's visible cursor.
        if self.barrier_enabled {
            match (self.acknowledge)() {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::TimedOut => {
                    self.barrier_enabled = false;
                }
                Err(error) => return Err(error),
            }
        }
        // Virtual images have no screen position until their placeholder cells arrive.
        let mut start = 0;
        for range in uploads {
            self.output.write_all(&self.frame[start..range.start])?;
            start = range.end;
        }
        self.output.write_all(&self.frame[start..])?;
        self.output.write_all(TMUX_END.as_bytes())?;
        Ok(())
    }
}

impl<W: Write, B: FnMut() -> io::Result<()>> Write for Writer<W, B> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.frame.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        // Ratatui flushes before the draw wrapper queues its closing marker.
        if self.frame.starts_with(BEGIN) && !self.frame.ends_with(END) {
            return Ok(());
        }
        let result = self.present().and_then(|()| self.output.flush());
        self.frame.clear();
        result
    }
}

fn kitty_uploads(frame: &[u8]) -> Vec<Range<usize>> {
    let mut uploads = Vec::new();
    let mut offset = 0;
    let mut virtual_image = false;
    while let Some(start) = frame[offset..].windows(TMUX.len()).position(|s| s == TMUX) {
        let start = offset + start;
        let mut end = start + TMUX.len();
        while end + 1 < frame.len() {
            match &frame[end..end + 2] {
                b"\x1b\x1b" => end += 2,
                b"\x1b\\" => break,
                _ => end += 1,
            }
        }
        if end + 1 >= frame.len() {
            break;
        }
        end += 2;
        let payload = &frame[start + TMUX.len()..end];
        if let Some(command) = payload.strip_prefix(b"\x1b\x1b_G") {
            let header = command.split(|b| *b == b';').next().unwrap_or_default();
            let fields: Vec<_> = header.split(|b| *b == b',').collect();
            if fields.contains(&b"U=1".as_slice()) {
                virtual_image = true;
            } else if fields.iter().any(|field| field.starts_with(b"a=")) {
                virtual_image = false;
            }
            if virtual_image {
                uploads.push(start..end);
            }
            let last = payload
                .windows(4)
                .rposition(|s| s == b"\x1b\x1b_G")
                .unwrap();
            let last_header = payload[last + 4..]
                .split(|b| *b == b';')
                .next()
                .unwrap_or_default();
            virtual_image &= last_header.split(|b| *b == b',').any(|f| f == b"m=1");
        } else {
            virtual_image = false;
        }
        offset = end;
    }
    uploads
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_passthrough_should_restore_the_original_pane_override_even_after_unwinding() {
        use std::cell::RefCell;
        for original in ["", "off", "on", "all"] {
            let option = RefCell::new(original.to_owned());
            let command = |args: &[&str]| match args {
                ["show-options", "-pqv", "allow-passthrough"] => Some(option.borrow().clone()),
                ["set-option", "-p", "allow-passthrough", value] => {
                    *option.borrow_mut() = (*value).to_owned();
                    Some(String::new())
                }
                ["set-option", "-pu", "allow-passthrough"] => {
                    option.borrow_mut().clear();
                    Some(String::new())
                }
                _ => panic!("unexpected tmux command: {args:?}"),
            };
            let mut passthrough = TmuxPassthrough::capture(command).unwrap();
            *option.borrow_mut() = "on".into();
            assert!(passthrough.enable_native());
            assert_eq!(*option.borrow(), "all");
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                let _passthrough = passthrough;
                panic!("exit fixture");
            }));
            assert!(result.is_err());
            assert_eq!(*option.borrow(), original);
        }
    }

    #[test]
    fn passthrough_cleanup_should_preserve_a_later_user_override() {
        use std::cell::RefCell;
        let option = RefCell::new("on".to_owned());
        let command = |args: &[&str]| {
            if let ["set-option", "-p", "allow-passthrough", value] = args {
                *option.borrow_mut() = (*value).to_owned();
            }
            Some(option.borrow().clone())
        };
        let mut passthrough = TmuxPassthrough::capture(command).unwrap();
        assert!(passthrough.enable_native());
        *option.borrow_mut() = "off".into();

        drop(passthrough);

        assert_eq!(*option.borrow(), "off");
    }

    #[test]
    fn a_stalled_output_drain_returns_without_waiting_for_the_sink() {
        struct Sink(std::sync::mpsc::Receiver<()>);
        impl Write for Sink {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0.recv().unwrap();
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let (release, receive) = std::sync::mpsc::channel();
        let (mut writer, output) = AsyncWriter::new(Sink(receive));
        writer.write_all(b"frame").unwrap();
        writer.flush().unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            release.send(()).unwrap();
        });
        assert_eq!(
            output
                .wait_for(Duration::from_millis(30))
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
        release.join().unwrap();
        output.wait().unwrap();
    }

    #[test]
    fn terminal_writer_reopens_the_requested_tty_and_bounds_screen_entry() {
        use std::os::fd::{FromRawFd, OwnedFd};
        let mut master = -1;
        let mut slave = -1;
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            },
            0
        );
        let master = unsafe { OwnedFd::from_raw_fd(master) };
        let slave = unsafe { OwnedFd::from_raw_fd(slave) };
        let flags = unsafe { libc::fcntl(slave.as_raw_fd(), libc::F_GETFL) };
        let mut output =
            TerminalWriter::for_descriptor(slave.as_raw_fd(), Duration::from_millis(50)).unwrap();
        output.write_all(b"same terminal").unwrap();
        assert_eq!(
            unsafe { libc::fcntl(slave.as_raw_fd(), libc::F_GETFL) },
            flags
        );
        let mut input = std::fs::File::from(master);
        let mut bytes = [0; 13];
        std::io::Read::read_exact(&mut input, &mut bytes).unwrap();
        assert_eq!(&bytes, b"same terminal");
        output.flush().unwrap();
        assert_eq!(unsafe { libc::tcflow(slave.as_raw_fd(), libc::TCOOFF) }, 0);
        loop {
            match output.file.write(&[b'x'; 4096]) {
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => panic!("cannot fill terminal: {error}"),
            }
        }
        let started = Instant::now();
        assert_eq!(
            output.enter_screen().unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn redirected_diagnostics_return_before_a_stalled_pipe_and_finish_when_it_drains() {
        use std::os::fd::{FromRawFd, OwnedFd};
        let mut descriptors = [-1; 2];
        assert_eq!(unsafe { libc::pipe(descriptors.as_mut_ptr()) }, 0);
        let read = unsafe { OwnedFd::from_raw_fd(descriptors[0]) };
        let write = unsafe { OwnedFd::from_raw_fd(descriptors[1]) };
        let message = "x".repeat(4 * 1024 * 1024);
        let started = Instant::now();
        diagnostic_to(
            std::fs::File::from(write),
            message.clone(),
            Duration::from_millis(30),
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        let mut received = String::new();
        std::io::Read::read_to_string(&mut std::fs::File::from(read), &mut received).unwrap();
        assert_eq!(received, message);
    }

    #[test]
    fn terminal_writes_stop_when_the_receiver_never_reads() {
        let (sender, _receiver) = std::os::unix::net::UnixStream::pair().unwrap();
        sender.set_nonblocking(true).unwrap();
        let descriptor: std::os::fd::OwnedFd = sender.into();
        let mut output = TerminalWriter {
            file: descriptor.into(),
            timeout: Duration::from_millis(30),
            deadline: None,
        };
        assert_eq!(
            output
                .write_all(&vec![0; 4 * 1024 * 1024])
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
    }
    use ratatui::{Terminal, TerminalOptions, Viewport, backend::CrosstermBackend, layout::Rect};

    #[test]
    fn slow_terminal_output_keeps_input_usable_and_limits_pending_frames() {
        struct Sink {
            started: std::sync::mpsc::SyncSender<()>,
            release: std::sync::mpsc::Receiver<()>,
            first: bool,
            bytes: std::sync::Arc<parking_lot::Mutex<Vec<u8>>>,
        }
        impl Write for Sink {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if !self.first {
                    return Err(io::Error::other("terminal closed"));
                }
                self.started.send(()).unwrap();
                self.release
                    .recv_timeout(std::time::Duration::from_secs(2))
                    .map_err(|_| {
                        io::Error::new(io::ErrorKind::TimedOut, "test output remained blocked")
                    })?;
                self.first = false;
                self.bytes.lock().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let (started, began) = std::sync::mpsc::sync_channel(1);
        let (release, wait) = std::sync::mpsc::sync_channel(1);
        let bytes = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
        let (mut writer, output) = AsyncWriter::new(Sink {
            started,
            release: wait,
            first: true,
            bytes: bytes.clone(),
        });
        let frame = [BEGIN, b"one complete frame", END].concat();
        writer.write_all(&frame).unwrap();
        writer.flush().unwrap();
        began
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert!(!output.ready().unwrap());

        let reddit = crate::reddit::Reddit::test_at("http://127.0.0.1:9", false);
        let (pool, _messages) = crate::worker::Pool::start(reddit.clone(), 1, None);
        let dir = tempfile::tempdir().unwrap();
        let mut app = crate::app::App::new(
            crate::config::Config::default(),
            pool,
            reddit,
            dir.path().join("state.json"),
            ratatui_image::picker::Picker::halfblocks(),
        );
        crate::handle_event(
            &mut app,
            crossterm::event::Event::Key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('q'),
                crossterm::event::KeyModifiers::NONE,
            )),
        );
        assert!(
            app.quit,
            "input must be handled while the terminal write is blocked"
        );
        writer.write_all(&frame).unwrap();
        assert_eq!(
            writer.flush().unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        release.send(()).unwrap();
        output.wait().unwrap();
        assert_eq!(*bytes.lock(), frame);
        writer.flush().unwrap();
        assert_eq!(output.wait().unwrap_err().to_string(), "terminal closed");
    }

    #[test]
    fn clipboard_output_queues_behind_a_blocked_frame_without_waiting_on_stdout() {
        struct Sink {
            first: bool,
            started: std::sync::mpsc::SyncSender<()>,
            release: std::sync::mpsc::Receiver<()>,
            bytes: std::sync::Arc<parking_lot::Mutex<Vec<u8>>>,
        }
        impl Write for Sink {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if self.first {
                    self.started.send(()).unwrap();
                    self.release
                        .recv_timeout(std::time::Duration::from_secs(2))
                        .unwrap();
                    self.first = false;
                }
                self.bytes.lock().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let (started, began) = std::sync::mpsc::sync_channel(1);
        let (release, wait) = std::sync::mpsc::sync_channel(1);
        let bytes = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
        let (mut writer, output) = AsyncWriter::new(Sink {
            first: true,
            started,
            release: wait,
            bytes: bytes.clone(),
        });
        let frame = [BEGIN, b"complete frame", END].concat();
        writer.write_all(&frame).unwrap();
        writer.flush().unwrap();
        began
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        let clipboard = b"\x1b]52;c;aHR0cHM6Ly9yZWRkaXQuY29tLw==\x07";

        output.command(clipboard).unwrap();
        assert!(!output.ready().unwrap());
        assert_eq!(
            output.command(clipboard).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        release.send(()).unwrap();
        output.wait().unwrap();

        assert_eq!(*bytes.lock(), [frame.as_slice(), clipboard].concat());
    }

    fn draw(editing: bool, frame: &crate::video::Frame) -> Vec<u8> {
        let mut bytes = Vec::new();
        let backend = CrosstermBackend::new(Writer::new(&mut bytes, || Ok(())));
        let mut terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Fixed(Rect::new(0, 0, 12, 4)),
            },
        )
        .unwrap();
        crate::draw_synchronized(&mut terminal, |f| {
            f.buffer_mut()[(0, 0)].set_symbol("video-marker");
            crate::video::kitty_cells(
                f.buffer_mut(),
                Rect::new(0, 1, 4, 2),
                42,
                ratatui::layout::Position::ORIGIN,
                Some(crate::video::kitty_transmit(42, frame, 4, 2, true)),
            );
            if editing {
                f.set_cursor_position((7, 3));
            }
        })
        .unwrap();
        drop(terminal);
        bytes
    }

    #[test]
    fn kitty_uploads_should_precede_the_grid_hold_and_preserve_the_edit_cursor() {
        let frame = crate::video::Frame {
            width: 2,
            height: 1,
            pixels: crate::video::Pixels::Png(vec![7; 5000]),
            same_as_first: false,
        };
        for editing in [false, true] {
            let bytes = draw(editing, &frame);
            assert!(
                bytes.starts_with(b"\x1b[?25l\x1bPtmux;\x1b\x1b[?2026h"),
                "graphics must upload with the pane hold off and the native hold on"
            );
            let text = String::from_utf8(bytes).unwrap();
            assert!(text.contains("video-marker"));
            assert!(text.contains("\x1b[?2026h"));
            let grid = &text[text.rfind("\x1b[?2026h").unwrap()..];
            assert!(grid.contains(if editing { "\x1b[?25h" } else { "\x1b[?25l" }));
            if editing {
                assert!(grid.contains("\x1b[4;8H"));
            }
        }
    }

    #[test]
    fn ordinary_frames_and_positioned_graphics_should_keep_their_exact_bytes() {
        for body in [
            b"plain text".as_slice(),
            b"\x1bPtmux;\x1b\x1bPqSIXEL\x1b\x1b\\\x1b\\",
            b"\x1bPtmux;\x1b\x1b_Ga=T,i=1,m=0;AAAA\x1b\x1b\\\x1b\\",
        ] {
            let frame = [BEGIN, body, END].concat();
            let mut writer = Writer::new(Vec::new(), || Ok(()));
            for chunk in frame.chunks(3) {
                writer.write_all(chunk).unwrap();
            }
            writer.flush().unwrap();
            assert_eq!(writer.output, frame);
        }
    }

    #[test]
    fn an_intermediate_backend_flush_should_not_start_the_pane_hold() {
        let mut writer = Writer::new(Vec::new(), || Ok(()));
        writer.write_all(BEGIN).unwrap();
        writer.write_all(b"part of a frame").unwrap();
        writer.flush().unwrap();
        assert!(writer.output.is_empty());
        writer.write_all(END).unwrap();
        writer.flush().unwrap();
        assert_eq!(writer.output, [BEGIN, b"part of a frame", END].concat());
        writer.write_all(b"exit cursor").unwrap();
        writer.flush().unwrap();
        assert!(writer.output.ends_with(b"exit cursor"));
    }

    #[test]
    fn all_virtual_uploads_should_finish_before_any_placeholder_switches() {
        let first = b"\x1bPtmux;\x1b\x1b_Ga=T,U=1,i=1;AAAA\x1b\x1b\\\x1b\\";
        let second = b"\x1bPtmux;\x1b\x1b_Ga=T,U=1,i=2;BBBB\x1b\x1b\\\x1b\\";
        let frame = [
            BEGIN,
            b"old to new ",
            first,
            b"middle ",
            second,
            b"done",
            END,
        ]
        .concat();
        let mut writer = Writer::new(Vec::new(), || Ok(()));
        writer.write_all(&frame).unwrap();
        writer.flush().unwrap();
        let expected = [
            b"\x1b[?25l".as_slice(),
            TMUX,
            NATIVE_BEGIN,
            &first[TMUX.len()..],
            TMUX,
            NATIVE_BEGIN,
            &second[TMUX.len()..],
            BEGIN,
            b"old to new middle done",
            END,
            TMUX_END.as_bytes(),
        ]
        .concat();
        assert_eq!(writer.output, expected);
    }

    #[test]
    fn short_writes_and_failed_commits_should_follow_the_writer_contract() {
        #[derive(Default)]
        struct Output {
            bytes: Vec<u8>,
            fail: bool,
        }
        impl Write for Output {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if self.fail {
                    return Err(io::Error::other("closed terminal"));
                }
                let size = bytes.len().min(3);
                self.bytes.extend_from_slice(&bytes[..size]);
                Ok(size)
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let frame = [BEGIN, b"complete frame", END].concat();
        let mut writer = Writer::new(Output::default(), || Ok(()));
        writer.write_all(&frame).unwrap();
        writer.flush().unwrap();
        assert_eq!(writer.output.bytes, frame);
        writer.output.fail = true;
        writer.write_all(&frame).unwrap();
        assert!(writer.flush().is_err());
    }
    #[test]
    fn when_acknowledgment_times_out_frames_should_remain_drawable_without_more_queries() {
        let upload = b"\x1bPtmux;\x1b\x1b_Ga=T,U=1,m=0;AAAA\x1b\x1b\\\x1b\\";
        let mut queries = 0;
        let mut writer = Writer::new(Vec::new(), || {
            queries += 1;
            Err(io::Error::new(io::ErrorKind::TimedOut, "no reply"))
        });
        for grid in [b"new grid".as_slice(), b"next grid"] {
            writer
                .write_all(&[BEGIN, upload, grid, END].concat())
                .unwrap();
            writer.flush().unwrap();
            assert!(writer.output.windows(grid.len()).any(|bytes| bytes == grid));
            assert!(writer.output.ends_with(TMUX_END.as_bytes()));
        }
        drop(writer);
        assert_eq!(queries, 1);
    }
}
