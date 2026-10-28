//! Keep graphics uploads out of tmux's pane synchronization hold.

use std::io::{self, Write};
use std::ops::Range;

const BEGIN: &[u8] = b"\x1b[?2026h";
const END: &[u8] = b"\x1b[?2026l";
const TMUX: &[u8] = b"\x1bPtmux;";
const NATIVE_BEGIN: &[u8] = b"\x1b\x1b[?2026h";
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
            self.output.write_all(TMUX)?;
            self.output.write_all(NATIVE_BEGIN)?;
            self.output
                .write_all(&self.frame[range.start + TMUX.len()..range.end])?;
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
