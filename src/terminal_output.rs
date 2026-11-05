//! Keep graphics uploads out of tmux's pane synchronization hold.

use std::io::{self, Write};
use std::ops::Range;
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

mod async_output;
mod tmux;

pub use async_output::*;
pub use tmux::*;

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
        // tmux reports focus-in when the pane comes into view, so image support is
        // checked again then.
        if inside_tmux() {
            crossterm::execute!(self, crossterm::event::EnableFocusChange)?;
        }
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
mod tests;
