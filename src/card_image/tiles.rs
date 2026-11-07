use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub url: String,
    pub size: Size,
    pub pixels: (u32, u32),
    pub start: u16,
    pub rows: u16,
    pub left: u16,
    pub cols: u16,
    pub overlay: Option<Overlay>,
    pub post_id: Option<String>,
    pub video: bool,
    pub fingerprint: Option<[u8; 32]>,
    pub(super) font: (u16, u16),
    pub(super) protocol: u8,
    pub(super) tmux: bool,
}

impl Key {
    pub fn new(
        url: &str,
        picker: &Picker,
        size: Size,
        pixels: (u32, u32),
        start: u16,
        rows: u16,
        column: u16,
    ) -> Self {
        Self {
            url: url.into(),
            size,
            pixels,
            start,
            rows,
            left: column * TILE_COLS,
            cols: TILE_COLS.min(size.width - column * TILE_COLS),
            overlay: None,
            post_id: None,
            video: false,
            fingerprint: None,
            font: (picker.font_size().width, picker.font_size().height),
            protocol: picker.protocol_type() as u8,
            tmux: picker.tmux_detected(),
        }
    }

    pub(super) fn bytes(&self) -> usize {
        usize::from(self.cols)
            * usize::from(self.rows)
            * usize::from(self.font.0.max(1))
            * usize::from(self.font.1.max(1))
            * 16
    }
}

pub enum Tile {
    Image(Protocol),
    KittyImage {
        upload: String,
        size: Size,
        id: u32,
        decoded_bytes: usize,
        residency: Arc<Mutex<NativeResidency>>,
    },
    Kitty {
        upload: String,
        frame: crate::video::Frame,
        size: Size,
        id: u32,
        buffer: bool,
        sent: AtomicBool,
        residency: Arc<Mutex<NativeResidency>>,
    },
}

/// Terminals budget decoded pixels and may discard old image IDs. Keep a smaller
/// transmission window than Ghostty's default so expired IDs can be sent again.
pub struct NativeResidency {
    pub(super) uploads: VecDeque<(u32, usize)>,
    pub(super) bytes: usize,
    pub(super) limit: usize,
}

impl Default for NativeResidency {
    fn default() -> Self {
        Self {
            uploads: VecDeque::new(),
            bytes: 0,
            limit: NATIVE_BYTES,
        }
    }
}

impl NativeResidency {
    pub(super) fn contains(&self, id: u32) -> bool {
        self.uploads.iter().any(|(known, _)| *known == id)
    }

    pub(super) fn record(&mut self, id: u32, bytes: usize) {
        if let Some(index) = self.uploads.iter().position(|(known, _)| *known == id) {
            self.bytes -= self.uploads.remove(index).unwrap().1;
        }
        self.uploads.push_back((id, bytes));
        self.bytes += bytes;
        while self.bytes > self.limit || self.uploads.len() > CACHE_TILES {
            let (_, bytes) = self.uploads.pop_front().unwrap();
            self.bytes -= bytes;
        }
    }
}

impl Tile {
    pub fn size(&self) -> Size {
        match self {
            Self::Image(protocol) => protocol.size(),
            Self::Kitty { size, .. } | Self::KittyImage { size, .. } => *size,
        }
    }

    pub fn take_upload(&self) -> Option<String> {
        match self {
            Self::KittyImage {
                upload,
                id,
                decoded_bytes,
                residency,
                ..
            } => {
                let mut residency = residency.lock();
                if residency.contains(*id) {
                    return None;
                }
                residency.record(*id, *decoded_bytes);
                Some(upload.clone())
            }
            Self::Kitty {
                upload,
                frame,
                id,
                sent,
                residency,
                ..
            } => {
                let mut residency = residency.lock();
                if sent.swap(true, Ordering::Relaxed) && residency.contains(*id) {
                    return None;
                }
                residency.record(*id, frame.width as usize * frame.height as usize * 4);
                Some(upload.clone())
            }
            Self::Image(_) => None,
        }
    }

    pub(super) fn bytes(&self, fallback: usize) -> usize {
        match self {
            Self::KittyImage { upload, .. } => upload.capacity(),
            Self::Kitty {
                frame:
                    crate::video::Frame {
                        pixels: crate::video::Pixels::Png(data),
                        ..
                    },
                upload,
                ..
            } => data.capacity() + upload.capacity(),
            _ => fallback,
        }
    }

    pub fn render_region(&self, area: Rect, source: Position, buf: &mut ratatui::buffer::Buffer) {
        match self {
            Self::Image(protocol) => ratatui_image::Image::new(protocol).render(area, buf),
            Self::KittyImage { id, .. } | Self::Kitty { id, .. } => {
                crate::video::kitty_cells(buf, area, *id, source, self.take_upload())
            }
        }
    }
}

impl Widget for &Tile {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        self.render_region(area, Position::ORIGIN, buf);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Preparation {
    Hold,
    Missing,
    Refresh,
}
