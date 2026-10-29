//! Keep graphics uploads out of tmux's pane synchronization hold.

use std::io::{self, Write};
use std::ops::Range;

const BEGIN: &[u8] = b"\x1b[?2026h";
const END: &[u8] = b"\x1b[?2026l";
const TMUX: &[u8] = b"\x1bPtmux;";
const NATIVE_BEGIN: &[u8] = b"\x1b\x1b[?2026h";
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
        let mut state = self.shared.state.lock();
        while state.pending != 0 {
            self.shared.changed.wait(&mut state);
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
        if !self.barrier_enabled {
            self.output.write_all(TMUX_END.as_bytes())?;
        }
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
