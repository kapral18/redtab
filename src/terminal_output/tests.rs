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
    assert_eq!(
        output
            .wait_for(Duration::from_millis(30))
            .unwrap_err()
            .kind(),
        io::ErrorKind::TimedOut
    );
    release.send(()).unwrap();
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
