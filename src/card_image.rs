//! Card graphics preparation; the UI only queues work and paints completed tiles.

use ratatui::layout::{Position, Rect};
use ratatui::widgets::Widget;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Weak};

use image::DynamicImage;
use parking_lot::{Condvar, Mutex};
use ratatui::layout::Size;
use ratatui_image::Resize;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;

/// Kitty placeholders address at most 297 columns per image.
pub const TILE_COLS: u16 = 256;
/// Small strips let scrolling reuse the unchanged middle of the viewport.
pub const TILE_ROWS: u16 = 16;
const CACHE_BYTES: usize = 256 * 1024 * 1024;
const CACHE_TILES: usize = 512;
const NATIVE_BYTES: usize = 128 * 1024 * 1024;
const BADGE_MIN_PIXELS: f64 = 80.0;
const VIDEO_BYTES_PER_SECOND: f64 = 12.0 * 1024.0 * 1024.0;
static NEXT_IMAGE_ID: LazyLock<AtomicU32> = LazyLock::new(|| {
    let seed = *uuid::Uuid::new_v4().as_bytes();
    AtomicU32::new(u32::from_be_bytes([seed[0], seed[1], seed[2], seed[3]]))
});

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Indicator {
    Video { gif: bool, play: bool },
    Link,
    Gallery(usize),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Overlay {
    pub indicator: Indicator,
    /// Full-image rows visible in this viewport, independent of the encoded tile.
    pub top: u16,
    pub rows: u16,
}

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
    font: (u16, u16),
    protocol: u8,
    tmux: bool,
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

    fn bytes(&self) -> usize {
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
#[derive(Default)]
pub struct NativeResidency {
    uploads: VecDeque<(u32, usize)>,
    bytes: usize,
}

impl NativeResidency {
    fn contains(&self, id: u32) -> bool {
        self.uploads.iter().any(|(known, _)| *known == id)
    }

    fn record(&mut self, id: u32, bytes: usize) {
        if let Some(index) = self.uploads.iter().position(|(known, _)| *known == id) {
            self.bytes -= self.uploads.remove(index).unwrap().1;
        }
        self.uploads.push_back((id, bytes));
        self.bytes += bytes;
        while self.bytes > NATIVE_BYTES || self.uploads.len() > CACHE_TILES {
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

    fn bytes(&self, fallback: usize) -> usize {
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

#[derive(Clone)]
struct Request {
    key: Key,
    image: Arc<DynamicImage>,
    picker: Picker,
    buffer: bool,
}

struct Completed {
    key: Key,
    image: Weak<DynamicImage>,
    source_size: (u32, u32),
    result: Result<Arc<Tile>, String>,
}

#[derive(Default)]
struct Work {
    pending: Option<Vec<Request>>,
    wanted: Vec<(Key, Weak<DynamicImage>)>,
    completed: Vec<Completed>,
    clear_encoder: bool,
    stop: bool,
}

#[derive(Default)]
struct Shared {
    work: Mutex<Work>,
    changed: Condvar,
    residency: Arc<Mutex<NativeResidency>>,
}

#[derive(Default)]
pub struct Renderer {
    shared: Arc<Shared>,
    started: bool,
    frame: Vec<Request>,
    cache: HashMap<Key, Completed>,
    visible: HashSet<Key>,
    order: VecDeque<Key>,
    video_ready_at: Option<std::time::Instant>,
    /// Frames per second of the playing video; its frames share the output budget.
    video_rate: Option<f64>,
    /// A video frame waited for the output budget: it shows once the budget allows,
    /// instead of being skipped for the one after it.
    video_waiting: bool,
}

impl Completed {
    fn bytes(&self, key: &Key) -> usize {
        std::mem::size_of::<Self>()
            + key.url.capacity()
            + self
                .result
                .as_ref()
                .map_or_else(|error| error.capacity(), |tile| tile.bytes(key.bytes()))
    }
}

impl Renderer {
    #[cfg(test)]
    pub fn fail_cached_tiles(&mut self, error: &str) {
        for entry in self.cache.values_mut() {
            entry.result = Err(error.into());
        }
    }

    pub fn set_video_rate(&mut self, rate: f64) {
        self.video_rate = Some(rate);
    }

    pub fn begin(&mut self) {
        self.frame.clear();
        self.poll();
        self.visible.clear();
    }

    /// A prior encoding remains drawable while new pixels for this URL are prepared.
    /// Disabling preparation returns cached results without queuing new work.
    pub fn tile(
        &mut self,
        key: Key,
        image: &Arc<DynamicImage>,
        picker: &Picker,
        preparation: Preparation,
    ) -> Option<Result<Arc<Tile>, String>> {
        self.visible.insert(key.clone());
        let cached = self.cache.get(&key);
        let current = cached.is_some_and(|entry| {
            Weak::ptr_eq(&entry.image, &Arc::downgrade(image))
                || (!key.video
                    && key.fingerprint.is_some()
                    && entry.source_size == (image.width(), image.height()))
        });
        let preserve =
            preparation == Preparation::Missing && cached.is_some_and(|entry| entry.result.is_ok());
        if !current
            && !preserve
            && preparation != Preparation::Hold
            && !self
                .frame
                .iter()
                .any(|request| request.key == key && Arc::ptr_eq(&request.image, image))
        {
            self.frame.push(Request {
                key: key.clone(),
                image: Arc::clone(image),
                picker: picker.clone(),
                buffer: cached
                    .and_then(|entry| entry.result.as_ref().ok())
                    .is_some_and(|tile| matches!(tile.as_ref(), Tile::Kitty { buffer: false, .. })),
            });
        }
        let result = cached
            .and_then(|entry| (current || entry.result.is_ok()).then(|| entry.result.clone()));
        if result.is_some() {
            self.order.retain(|old| old != &key);
            self.order.push_back(key);
        }
        result
    }

    pub fn finish(&mut self) {
        self.prune();
        let mut next: Vec<_> = self
            .frame
            .iter()
            .map(|request| (request.key.clone(), Arc::downgrade(&request.image)))
            .collect();
        let mut work = self.shared.work.lock();
        if self
            .video_ready_at
            .is_some_and(|at| std::time::Instant::now() < at)
            && work.wanted.is_empty()
            && work.completed.is_empty()
            && self
                .frame
                .iter()
                .filter(|request| request.key.video)
                .all(|request| self.cache.contains_key(&request.key))
        {
            self.video_waiting |= self.frame.iter().any(|request| request.key.video);
            self.frame.retain(|request| !request.key.video);
            next.retain(|(key, _)| !key.video);
        }
        // Finish this picture despite newer pixels, including completions racing this draw.
        if (work.wanted.iter().any(|(key, _)| key.video)
            || work.completed.iter().any(|done| done.key.video))
            && next.len() == work.wanted.len() + work.completed.len()
            && next.iter().all(|(key, image)| {
                work.wanted
                    .iter()
                    .map(|(key, source)| (key, source))
                    .chain(work.completed.iter().map(|done| (&done.key, &done.image)))
                    .any(|(old, source)| old == key && (key.video || Weak::ptr_eq(source, image)))
            })
        {
            return;
        }
        work.completed
            .retain(|done| wanted(&next, &done.key, &done.image));
        next.retain(|(key, image)| {
            !work
                .completed
                .iter()
                .any(|done| &done.key == key && Weak::ptr_eq(&done.image, image))
        });
        if work.wanted.len() == next.len()
            && work
                .wanted
                .iter()
                .zip(&next)
                .all(|((a, source), (b, other))| a == b && Weak::ptr_eq(source, other))
        {
            return;
        }
        self.frame
            .retain(|request| wanted(&next, &request.key, &Arc::downgrade(&request.image)));
        work.wanted = next;
        work.pending = (!self.frame.is_empty()).then(|| std::mem::take(&mut self.frame));
        if !self.started && work.pending.is_some() {
            let shared = Arc::clone(&self.shared);
            let encoder = Encoder {
                residency: Arc::clone(&shared.residency),
                ..Encoder::default()
            };
            std::thread::Builder::new()
                .name("redtab-card-render".into())
                .spawn(move || prepare(shared, encoder))
                .expect("cannot start card renderer");
            self.started = true;
        }
        self.shared.changed.notify_one();
    }

    pub fn poll(&mut self) -> bool {
        let now = std::time::Instant::now();
        let resumed = self.video_waiting && self.video_ready_at.is_none_or(|at| now >= at);
        if resumed {
            self.video_waiting = false;
        }
        let completed = {
            let mut work = self.shared.work.lock();
            // Keep the old video visible until every requested strip is ready.
            if work.wanted.iter().any(|(key, _)| key.video) {
                return resumed;
            }
            std::mem::take(&mut work.completed)
        };
        let changed = resumed || !completed.is_empty();
        let video_bytes: usize = completed
            .iter()
            .filter(|done| done.key.video)
            .filter_map(|done| done.result.as_ref().ok())
            .map(|tile| match tile.as_ref() {
                Tile::Kitty { upload, .. } => upload.len(),
                _ => tile.size().width as usize * tile.size().height as usize * 16 * 32,
            })
            .sum();
        let rate = self.video_rate.unwrap_or(crate::video::MIN_FPS);
        if video_bytes as f64 > VIDEO_BYTES_PER_SECOND / rate {
            self.video_ready_at = Some(
                std::time::Instant::now()
                    + std::time::Duration::from_secs_f64(
                        video_bytes as f64 / VIDEO_BYTES_PER_SECOND,
                    ),
            );
        }
        for done in completed {
            let key = done.key.clone();
            self.cache.insert(key.clone(), done);
            self.order.retain(|old| old != &key);
            self.order.push_back(key);
        }
        self.prune();
        changed
    }

    fn prune(&mut self) {
        let (mut count, mut bytes) = self
            .cache
            .iter()
            .filter(|(key, _)| !self.visible.contains(*key))
            .fold((0usize, 0usize), |(count, bytes), (key, entry)| {
                (count + 1, bytes + entry.bytes(key))
            });
        while bytes > CACHE_BYTES || count > CACHE_TILES {
            let Some(key) = self.order.pop_front() else {
                break;
            };
            if self.visible.contains(&key) {
                self.order.push_back(key);
                continue;
            }
            if let Some(entry) = self.cache.remove(&key) {
                bytes -= entry.bytes(&key);
                count -= 1;
            }
        }
    }

    /// How long until a video frame held for the output budget can go out.
    pub fn video_wake_in(&self, now: std::time::Instant) -> Option<std::time::Duration> {
        self.video_waiting
            .then(|| {
                self.video_ready_at
                    .map(|at| at.saturating_duration_since(now))
            })
            .flatten()
    }

    pub fn pending(&self) -> bool {
        let work = self.shared.work.lock();
        !work.wanted.is_empty() || !work.completed.is_empty()
    }

    /// Video frames keep the renderer busy during ordinary playback, not loading.
    pub fn images_pending(&self) -> bool {
        let work = self.shared.work.lock();
        work.wanted.iter().any(|(key, _)| !key.video)
            || work.completed.iter().any(|done| !done.key.video)
    }

    #[cfg(test)]
    pub(crate) fn wait_for_completion(&self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut work = self.shared.work.lock();
        while !work.wanted.is_empty()
            && (work.completed.is_empty() || work.wanted.iter().any(|(key, _)| key.video))
        {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            assert!(!left.is_zero(), "card renderer did not complete");
            self.shared.changed.wait_for(&mut work, left);
        }
    }

    pub fn clear(&mut self) {
        self.frame.clear();
        self.video_ready_at = None;
        self.video_waiting = false;
        self.cache.clear();
        self.visible.clear();
        self.order.clear();
        let mut work = self.shared.work.lock();
        work.pending = None;
        work.wanted.clear();
        work.completed.clear();
        work.clear_encoder = true;
        self.shared.changed.notify_one();
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        self.shared.work.lock().stop = true;
        self.shared.changed.notify_one();
    }
}

fn wanted(wanted: &[(Key, Weak<DynamicImage>)], key: &Key, image: &Weak<DynamicImage>) -> bool {
    wanted
        .iter()
        .any(|(next, source)| next == key && Weak::ptr_eq(source, image))
}

fn prepare(shared: Arc<Shared>, mut encoder: Encoder) {
    loop {
        let requests = {
            let mut work = shared.work.lock();
            loop {
                if work.stop {
                    return;
                }
                if work.clear_encoder {
                    encoder.downscaled = None;
                    work.clear_encoder = false;
                    shared.changed.notify_all();
                }
                if let Some(requests) = work.pending.take() {
                    break requests;
                }
                shared.changed.wait(&mut work);
            }
        };
        for request in requests {
            let image = Arc::downgrade(&request.image);
            if !wanted(&shared.work.lock().wanted, &request.key, &image) {
                continue;
            }
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| encoder.encode(&request)))
                    .unwrap_or_else(|_| {
                        Err(format!(
                            "card image preparation failed: {}",
                            crate::worker::take_panic().unwrap_or_default()
                        ))
                    })
                    .map(Arc::new);
            let mut work = shared.work.lock();
            if work.stop {
                return;
            }
            if wanted(&work.wanted, &request.key, &image) {
                work.wanted
                    .retain(|(key, source)| key != &request.key || !Weak::ptr_eq(source, &image));
                work.completed.push(Completed {
                    key: request.key,
                    image,
                    source_size: (request.image.width(), request.image.height()),
                    result,
                });
                shared.changed.notify_all();
            }
        }
    }
}

#[derive(Default)]
struct Encoder {
    downscaled: Option<(Weak<DynamicImage>, u32, Arc<DynamicImage>)>,
    residency: Arc<Mutex<NativeResidency>>,
}

impl Encoder {
    fn encode(&mut self, request: &Request) -> Result<Tile, String> {
        let width = u32::from(request.key.size.width) * u32::from(request.key.font.0.max(1));
        let sample = if width < request.image.width() {
            if self
                .downscaled
                .as_ref()
                .is_none_or(|(source, old_width, _)| {
                    !Weak::ptr_eq(source, &Arc::downgrade(&request.image)) || *old_width != width
                })
            {
                self.downscaled = Some((
                    Arc::downgrade(&request.image),
                    width,
                    Arc::new(request.image.resize(
                        width,
                        request.image.height(),
                        ratatui_image::FilterType::Triangle,
                    )),
                ));
            }
            self.downscaled
                .as_ref()
                .map(|(_, _, pixels)| pixels.as_ref())
        } else {
            None
        };
        let kitty_video = request.key.video
            && request.picker.protocol_type() == ratatui_image::picker::ProtocolType::Kitty;
        let scale = if kitty_video {
            let (fit, _, _) = picture_fit(&request.image, &request.key);
            let height = f64::from(request.key.size.height) * f64::from(request.key.font.1.max(1));
            let badge_scale = if request.key.overlay.is_some() {
                BADGE_MIN_PIXELS / f64::from(width)
            } else {
                0.0
            };
            fit.recip()
                .max(badge_scale)
                .min(1.0)
                .min((crate::video::MAX_VIDEO_PIXELS / (f64::from(width) * height)).sqrt())
        } else {
            1.0
        };
        let pixels = tile_pixels(&request.image, sample, &request.key, scale);
        let cols = request.key.cols;
        let size = Size::new(cols, request.key.rows);
        if !request.key.video
            && request.picker.protocol_type() == ratatui_image::picker::ProtocolType::Kitty
        {
            let id = 0x8000_0000 | (NEXT_IMAGE_ID.fetch_add(1, Ordering::Relaxed) & 0x7fff_ffff);
            let frame = png_frame(DynamicImage::ImageRgba8(pixels))?;
            let upload =
                crate::video::kitty_transmit(id, &frame, size.width, size.height, request.key.tmux);
            return Ok(Tile::KittyImage {
                upload,
                size,
                id,
                decoded_bytes: frame.width as usize * frame.height as usize * 4,
                residency: Arc::clone(&self.residency),
            });
        }
        if kitty_video {
            let mut hash = std::collections::hash_map::DefaultHasher::new();
            request.key.hash(&mut hash);
            // Upload into the hidden buffer; synchronized placeholders reveal it later.
            request.buffer.hash(&mut hash);
            std::process::id().hash(&mut hash);
            let id = 0x0080_0000 | (hash.finish() as u32 & 0x007f_ffff);
            let frame = png_frame(DynamicImage::ImageRgb8(
                DynamicImage::ImageRgba8(pixels).to_rgb8(),
            ))?;
            let upload =
                crate::video::kitty_transmit(id, &frame, size.width, size.height, request.key.tmux);
            return Ok(Tile::Kitty {
                upload,
                frame,
                size,
                id,
                buffer: request.buffer,
                sent: AtomicBool::new(false),
                residency: Arc::clone(&self.residency),
            });
        }
        request
            .picker
            .new_protocol(
                DynamicImage::ImageRgba8(pixels),
                size,
                Resize::Scale(Some(ratatui_image::FilterType::Triangle)),
            )
            .map(Tile::Image)
            .map_err(|error| error.to_string())
    }
}

fn png_frame(image: DynamicImage) -> Result<crate::video::Frame, String> {
    let mut png = Vec::new();
    image::ImageEncoder::write_image(
        image::codecs::png::PngEncoder::new_with_quality(
            &mut png,
            image::codecs::png::CompressionType::Fast,
            image::codecs::png::FilterType::Paeth,
        ),
        image.as_bytes(),
        image.width(),
        image.height(),
        image.color().into(),
    )
    .map_err(|error| error.to_string())?;
    Ok(crate::video::Frame {
        width: image.width(),
        height: image.height(),
        pixels: crate::video::Pixels::Png(png),
        same_as_first: false,
    })
}

fn tile_pixels(
    image: &DynamicImage,
    sample: Option<&DynamicImage>,
    key: &Key,
    scale: f64,
) -> image::RgbaImage {
    let cols = key.cols;
    let full_width = f64::from(key.size.width) * f64::from(key.font.0.max(1));
    let full_height = f64::from(key.size.height) * f64::from(key.font.1.max(1));
    let top = f64::from(key.start) * f64::from(key.font.1.max(1));
    let left = f64::from(key.left) * f64::from(key.font.0.max(1));
    let width = (f64::from(cols) * f64::from(key.font.0.max(1)) * scale)
        .ceil()
        .max(1.0) as u32;
    let height = (f64::from(key.rows) * f64::from(key.font.1.max(1)) * scale)
        .ceil()
        .max(1.0) as u32;
    let sample = sample.unwrap_or(image);
    let rgb = key.video.then(|| sample.as_rgb8()).flatten();
    let (_, content_width, content_height) = picture_fit(image, key);
    // Retain at least one encoded pixel for extreme aspect ratios.
    let content_width = (content_width * scale).ceil().max(1.0) / scale;
    let content_height = (content_height * scale).ceil().max(1.0) / scale;
    let pad_left = ((full_width - content_width) * scale / 2.0).floor() / scale;
    let pad_top = ((full_height - content_height) * scale / 2.0).floor() / scale;
    let mut pixels = image::RgbaImage::from_fn(width, height, |x, y| {
        let x = left + (f64::from(x) + 0.5) / scale;
        let y = top + (f64::from(y) + 0.5) / scale;
        let u = (x - pad_left) / content_width;
        let v = (y - pad_top) / content_height;
        if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
            return if key.video {
                image::Rgba([17, 17, 27, 255])
            } else {
                image::Rgba([0, 0, 0, 0])
            };
        }
        match rgb {
            Some(rgb) => image::imageops::sample_bilinear(rgb, u as f32, v as f32)
                .map(|pixel| image::Rgba([pixel[0], pixel[1], pixel[2], 255])),
            None => image::imageops::sample_bilinear(sample, u as f32, v as f32),
        }
        .unwrap_or(image::Rgba([0, 0, 0, 0]))
    });
    if let Some(overlay) = &key.overlay {
        overlay.paint(
            &mut pixels,
            ((left * scale) as u32, (top * scale) as u32),
            OverlayGeometry {
                width: (full_width * scale).ceil() as u32,
                top: (f64::from(overlay.top) * f64::from(key.font.1.max(1)) * scale) as u32,
                height: (f64::from(overlay.rows) * f64::from(key.font.1.max(1)) * scale).ceil()
                    as u32,
                font_height: (f64::from(key.font.1.max(1)) * scale).round().max(1.0) as u32,
            },
        );
    }
    pixels
}

/// Fit the source into its stable cell reservation, including rounded cell edges.
fn picture_fit(image: &DynamicImage, key: &Key) -> (f64, f64, f64) {
    let width = f64::from(key.size.width) * f64::from(key.font.0.max(1));
    let height = f64::from(key.size.height) * f64::from(key.font.1.max(1));
    let pixels = if key.video {
        (image.width(), image.height())
    } else {
        key.pixels
    };
    let (w, h) = (f64::from(pixels.0.max(1)), f64::from(pixels.1.max(1)));
    let scale = (width / w).min(height / h);
    (scale, w * scale, h * scale)
}

struct OverlayGeometry {
    width: u32,
    top: u32,
    height: u32,
    font_height: u32,
}

#[cfg(test)]
pub(crate) fn encode_tile(
    picker: &Picker,
    image: &DynamicImage,
    sample: Option<&DynamicImage>,
    (size, pixels, overlay): (Size, (u32, u32), Option<&Overlay>),
    start: u16,
    rows: u16,
    column: u16,
) -> Result<Protocol, String> {
    let mut key = Key::new("test-image", picker, size, pixels, start, rows, column);
    key.overlay = overlay.cloned();
    let pixels = tile_pixels(image, sample, &key, 1.0);
    picker
        .new_protocol(
            DynamicImage::ImageRgba8(pixels),
            Size::new(TILE_COLS.min(size.width - column * TILE_COLS), rows),
            Resize::Scale(Some(ratatui_image::FilterType::Triangle)),
        )
        .map_err(|error| error.to_string())
}

impl Overlay {
    fn paint(&self, image: &mut image::RgbaImage, origin: (u32, u32), geometry: OverlayGeometry) {
        let OverlayGeometry {
            width,
            top,
            height,
            font_height,
        } = geometry;
        let scale = (font_height / 9).max(1);
        let pad = 3 * scale;
        let center = (width / 2, top + height / 2);
        let radius = (width.min(height) / 5).clamp(1, 28 * scale);
        let (label, symbol) = match &self.indicator {
            Indicator::Video { gif, play } => (
                if *gif { "GIF".into() } else { "VIDEO".into() },
                play.then_some(false),
            ),
            Indicator::Link => {
                let radius = (5 * scale).min(width.min(height) / 4).max(1);
                let corner = (width.saturating_sub(pad + radius), top + pad + radius);
                paint_symbol(image, origin, corner, radius, true);
                return;
            }
            Indicator::Gallery(count) => (format!("1/{count}"), None),
        };
        if let Some(link) = symbol {
            paint_symbol(image, origin, center, radius, link);
        }
        if width >= 40 * scale && height >= 18 * scale {
            paint_label(
                image,
                origin,
                (pad, top + pad),
                &label,
                width.saturating_sub(pad * 2),
                scale,
            );
        }
    }
}

fn paint_symbol(
    image: &mut image::RgbaImage,
    origin: (u32, u32),
    center: (u32, u32),
    radius: u32,
    link: bool,
) {
    let radius = i64::from(radius);
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let dx = i64::from(origin.0 + x) - i64::from(center.0);
        let dy = i64::from(origin.1 + y) - i64::from(center.1);
        if dx * dx + dy * dy > radius * radius {
            continue;
        }
        let stroke = (radius / 9).max(1);
        let mark = if link {
            let a = radius / 2;
            ((dx + dy).abs() <= stroke && dx.abs() <= a && dy.abs() <= a)
                || ((dy + a).abs() <= stroke && dx >= 0 && dx <= a)
                || ((dx - a).abs() <= stroke && dy <= 0 && dy >= -a)
                || ((dx + a).abs() <= stroke && dy >= -a / 2 && dy <= a)
                || ((dy - a).abs() <= stroke && dx >= -a && dx <= a / 2)
        } else {
            dx >= -radius / 4 && dx <= radius / 2 && dy.abs() <= (radius / 2 - dx) * 2 / 3
        };
        *pixel = image::Rgba(if mark {
            [235, 237, 250, 255]
        } else {
            [17, 17, 27, 255]
        });
    }
}

fn paint_label(
    image: &mut image::RgbaImage,
    origin: (u32, u32),
    at: (u32, u32),
    text: &str,
    width: u32,
    scale: u32,
) {
    let limit = width.saturating_sub(4 * scale) / (6 * scale);
    let mut chars: Vec<_> = text.chars().take(limit as usize).collect();
    if text.chars().count() > limit as usize && chars.len() >= 3 {
        let end = chars.len();
        chars[end - 3..].fill('.');
    }
    let cols = chars.len() as u32 * 6 * scale + 4 * scale;
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let Some(x) = (origin.0 + x).checked_sub(at.0) else {
            continue;
        };
        let Some(y) = (origin.1 + y).checked_sub(at.1) else {
            continue;
        };
        if x >= cols || y >= 11 * scale {
            continue;
        }
        *pixel = image::Rgba([17, 17, 27, 255]);
        let Some(x) = x.checked_sub(2 * scale) else {
            continue;
        };
        let Some(y) = y.checked_sub(2 * scale) else {
            continue;
        };
        let (col, row) = (x / scale, y / scale);
        if row < 7
            && col % 6 < 5
            && let Some(ch) = chars.get((col / 6) as usize)
            && glyph(*ch)[row as usize] & (1 << (4 - col % 6)) != 0
        {
            *pixel = image::Rgba([235, 237, 250, 255]);
        }
    }
}

/// Small bitmap labels remain part of the picture on kitty, sixel, iTerm2 and halfblocks.
fn glyph(ch: char) -> [u8; 7] {
    match ch.to_ascii_uppercase() {
        'A' => [14, 17, 17, 31, 17, 17, 17],
        'B' => [30, 17, 17, 30, 17, 17, 30],
        'C' => [14, 17, 16, 16, 16, 17, 14],
        'D' => [30, 17, 17, 17, 17, 17, 30],
        'E' => [31, 16, 16, 30, 16, 16, 31],
        'F' => [31, 16, 16, 30, 16, 16, 16],
        'G' => [14, 17, 16, 23, 17, 17, 15],
        'H' => [17, 17, 17, 31, 17, 17, 17],
        'I' => [14, 4, 4, 4, 4, 4, 14],
        'J' => [7, 2, 2, 2, 18, 18, 12],
        'K' => [17, 18, 20, 24, 20, 18, 17],
        'L' => [16, 16, 16, 16, 16, 16, 31],
        'M' => [17, 27, 21, 21, 17, 17, 17],
        'N' => [17, 25, 21, 19, 17, 17, 17],
        'O' => [14, 17, 17, 17, 17, 17, 14],
        'P' => [30, 17, 17, 30, 16, 16, 16],
        'Q' => [14, 17, 17, 17, 21, 18, 13],
        'R' => [30, 17, 17, 30, 20, 18, 17],
        'S' => [15, 16, 16, 14, 1, 1, 30],
        'T' => [31, 4, 4, 4, 4, 4, 4],
        'U' => [17, 17, 17, 17, 17, 17, 14],
        'V' => [17, 17, 17, 17, 17, 10, 4],
        'W' => [17, 17, 17, 21, 21, 21, 10],
        'X' => [17, 17, 10, 4, 10, 17, 17],
        'Y' => [17, 17, 10, 4, 4, 4, 4],
        'Z' => [31, 1, 2, 4, 8, 16, 31],
        '0' => [14, 17, 19, 21, 25, 17, 14],
        '1' => [4, 12, 4, 4, 4, 4, 14],
        '2' => [14, 17, 1, 2, 4, 8, 31],
        '3' => [30, 1, 1, 14, 1, 1, 30],
        '4' => [2, 6, 10, 18, 31, 2, 2],
        '5' => [31, 16, 16, 30, 1, 1, 30],
        '6' => [14, 16, 16, 30, 17, 17, 14],
        '7' => [31, 1, 2, 4, 8, 8, 8],
        '8' => [14, 17, 17, 14, 17, 17, 14],
        '9' => [14, 17, 17, 15, 1, 1, 14],
        '.' => [0, 0, 0, 0, 0, 12, 12],
        '-' => [0, 0, 0, 31, 0, 0, 0],
        '/' => [1, 2, 2, 4, 8, 8, 16],
        ':' => [0, 12, 12, 0, 12, 12, 0],
        _ => [0; 7],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picker() -> Picker {
        #[allow(deprecated)]
        let mut picker = Picker::from_fontsize(ratatui_image::FontSize::new(1, 1));
        picker.set_protocol_type(ratatui_image::picker::ProtocolType::Iterm2);
        picker
    }

    fn complete(renderer: &mut Renderer) {
        while renderer.pending() {
            renderer.wait_for_completion();
            renderer.poll();
        }
    }

    fn decoded(protocol: &Tile) -> image::RgbaImage {
        use base64::Engine;
        let Tile::Image(Protocol::ITerm2(protocol)) = protocol else {
            panic!("iTerm2 tile");
        };
        let encoded = protocol.data.split("doNotMoveCursor=1:").nth(1).unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded.split('\u{7}').next().unwrap())
            .unwrap();
        image::load_from_memory(&bytes).unwrap().to_rgba8()
    }

    fn pixel(protocol: &Tile) -> image::Rgba<u8> {
        *decoded(protocol).get_pixel(0, 0)
    }

    fn indicator_pixels(indicator: Option<Indicator>, top: u16, rows: u16) -> image::RgbaImage {
        let picker = picker();
        let source = Arc::new(DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            200,
            100,
            image::Rgba([75, 130, 190, 255]),
        )));
        let mut key = Key::new(
            "preview",
            &picker,
            Size::new(200, 100),
            (200, 100),
            top,
            rows,
            0,
        );
        key.overlay = indicator.map(|indicator| Overlay {
            indicator,
            top,
            rows,
        });
        let mut renderer = Renderer::default();
        renderer.begin();
        renderer.tile(key.clone(), &source, &picker, Preparation::Refresh);
        renderer.finish();
        complete(&mut renderer);
        decoded(
            &renderer
                .tile(key, &source, &picker, Preparation::Refresh)
                .unwrap()
                .unwrap(),
        )
    }

    fn kitty_video_tile() -> Tile {
        #[allow(deprecated)]
        let mut picker = Picker::from_fontsize(ratatui_image::FontSize::new(8, 16));
        picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
        let image = Arc::new(DynamicImage::new_rgb8(160, 90));
        let mut key = Key::new("video", &picker, Size::new(68, 20), (320, 180), 0, 16, 0);
        key.video = true;
        Encoder::default()
            .encode(&Request {
                key,
                image,
                picker,
                buffer: false,
            })
            .unwrap()
    }

    #[test]
    fn when_terminal_cells_change_a_still_strip_should_keep_its_fitted_grid_size() {
        #[allow(deprecated)]
        let mut picker = Picker::from_fontsize(ratatui_image::FontSize::new(8, 16));
        picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
        let image = Arc::new(DynamicImage::new_rgb8(160, 512));
        for tmux in [false, true] {
            let mut key = Key::new(
                "portrait",
                &picker,
                Size::new(20, 32),
                (160, 512),
                16,
                16,
                0,
            );
            key.tmux = tmux;
            let tile = Encoder::default()
                .encode(&Request {
                    key,
                    image: Arc::clone(&image),
                    picker: picker.clone(),
                    buffer: false,
                })
                .unwrap();
            let Tile::KittyImage { upload, .. } = tile else {
                panic!("native still");
            };
            let header = upload
                .split("_G")
                .nth(1)
                .unwrap()
                .split(';')
                .next()
                .unwrap();
            let values: HashMap<_, u32> = header
                .split(',')
                .filter_map(|field| {
                    let (name, value) = field.split_once('=')?;
                    Some((name, value.parse().ok()?))
                })
                .collect();

            // Ghostty Placement.gridSize uses c/r directly; omitted values derive
            // cells from the uploaded pixel dimensions and the current font size.
            for (cell_width, cell_height) in [(8, 16), (16, 32), (4, 8)] {
                let cols = values
                    .get("c")
                    .copied()
                    .unwrap_or_else(|| values["s"].div_ceil(cell_width));
                let rows = values
                    .get("r")
                    .copied()
                    .unwrap_or_else(|| values["v"].div_ceil(cell_height));
                assert_eq!(
                    (cols, rows),
                    (20, 16),
                    "cell size {cell_width}x{cell_height}; {header}"
                );
            }
        }
    }

    #[test]
    fn when_native_stills_are_prepared_the_upload_should_compress_the_pixels() {
        let mut picker = picker();
        picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
        let tile = Encoder::default()
            .encode(&Request {
                key: Key::new("still", &picker, Size::new(128, 16), (1, 1), 0, 16, 0),
                image: Arc::new(DynamicImage::new_rgba8(1, 1)),
                picker,
                buffer: false,
            })
            .unwrap();
        let Tile::KittyImage { upload, .. } = tile else {
            panic!("native still");
        };
        assert!(
            upload.len() < 128 * 16 * 4,
            "compress still uploads off the UI thread"
        );
    }

    #[test]
    fn when_compressed_native_tiles_fit_the_budget_it_should_keep_them_offscreen() {
        #[allow(deprecated)]
        let mut picker = Picker::from_fontsize(ratatui_image::FontSize::new(17, 37));
        picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
        let image = Arc::new(DynamicImage::new_rgba8(1, 1));
        let mut renderer = Renderer::default();
        renderer.begin();
        for n in 0..16 {
            renderer.tile(
                Key::new(
                    &format!("still{n}"),
                    &picker,
                    Size::new(128, 16),
                    (1, 1),
                    0,
                    16,
                    0,
                ),
                &image,
                &picker,
                Preparation::Refresh,
            );
        }
        renderer.finish();
        complete(&mut renderer);
        renderer.begin();
        renderer.finish();
        assert_eq!(
            renderer.cache.len(),
            16,
            "budget the encoded payload, not an uncompressed pixel multiple"
        );
    }

    #[test]
    fn when_native_uploads_exceed_terminal_residency_a_warm_still_should_upload_again() {
        #[allow(deprecated)]
        let mut picker = Picker::from_fontsize(ratatui_image::FontSize::new(17, 37));
        picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
        let image = Arc::new(DynamicImage::new_rgba8(1, 1));
        let mut encoder = Encoder::default();
        let mut video_key = Key::new("paused", &picker, Size::new(128, 16), (1, 1), 0, 16, 0);
        video_key.video = true;
        let video = encoder
            .encode(&Request {
                key: video_key,
                image: Arc::clone(&image),
                picker: picker.clone(),
                buffer: false,
            })
            .unwrap();
        assert!(video.take_upload().is_some());
        assert!(video.take_upload().is_none());
        let mut first = None;
        for n in 0..28 {
            let tile = encoder
                .encode(&Request {
                    key: Key::new(
                        &format!("still{n}"),
                        &picker,
                        Size::new(128, 16),
                        (1, 1),
                        0,
                        16,
                        0,
                    ),
                    image: Arc::clone(&image),
                    picker: picker.clone(),
                    buffer: false,
                })
                .unwrap();
            assert!(tile.take_upload().is_some());
            assert!(tile.take_upload().is_none());
            if first.is_none() {
                first = Some(tile);
            }
        }
        assert!(
            first.unwrap().take_upload().is_some(),
            "keep the prepared PNG, but resend IDs beyond the terminal residency budget"
        );
        assert!(
            video.take_upload().is_some(),
            "a retained paused frame must also survive terminal eviction"
        );
    }

    #[test]
    fn a_height_fitted_media_canvas_should_show_both_ends_of_a_tall_source() {
        for video in [false, true] {
            let picker = picker();
            let image = Arc::new(DynamicImage::ImageRgba8(image::RgbaImage::from_fn(
                2,
                1600,
                |_, y| {
                    if y < 800 {
                        image::Rgba([255, 0, 0, 255])
                    } else {
                        image::Rgba([0, 0, 255, 255])
                    }
                },
            )));
            let mut key = Key::new("portrait", &picker, Size::new(1, 10), (2, 1600), 0, 10, 0);
            key.video = video;
            let tile = Encoder::default()
                .encode(&Request {
                    key,
                    image,
                    picker,
                    buffer: false,
                })
                .unwrap();
            let actual = decoded(&tile);
            assert!(
                actual.pixels().any(|p| p.0 == [255, 0, 0, 255]),
                "top must remain visible"
            );
            assert!(
                actual.pixels().any(|p| p.0 == [0, 0, 255, 255]),
                "bottom must remain visible, video={video}"
            );
        }
    }

    #[test]
    fn rgb_video_sampling_should_match_the_generic_rgba_sampler() {
        let rgb = image::RgbImage::from_fn(7, 13, |x, y| {
            image::Rgb([
                (x * 57 + y * 13) as u8,
                (x * 11 + y * 41) as u8,
                (x * 31 + y * 7) as u8,
            ])
        });
        let source = DynamicImage::ImageRgb8(rgb);
        let reference = DynamicImage::ImageRgba8(source.to_rgba8());
        #[allow(deprecated)]
        let picker = Picker::from_fontsize(ratatui_image::FontSize::new(3, 5));
        for scale in [1.0, 0.63] {
            for top in [0, 2, 5] {
                let mut key = Key::new("video", &picker, Size::new(9, 7), (7, 13), top, 2, 0);
                key.video = true;
                key.left = 2;
                key.cols = 7;
                assert_eq!(
                    tile_pixels(&source, None, &key, scale),
                    tile_pixels(&reference, None, &key, scale)
                );
            }
        }
    }

    #[test]
    fn clip_native_frames_should_use_lossless_png_without_outer_compression() {
        let Tile::Kitty { frame, upload, .. } = kitty_video_tile() else {
            panic!("native tile");
        };
        let crate::video::Pixels::Png(data) = &frame.pixels else {
            panic!("encoded payload");
        };
        let decoded = image::load_from_memory(data).expect("native video payload must be PNG");
        assert_eq!(
            (decoded.width(), decoded.height()),
            (frame.width, frame.height)
        );
        assert!(upload.contains("f=100,"));
        assert!(!upload.contains("o=z,"));
    }

    #[test]
    fn video_width_regression_native_tiles_should_retain_display_resolution() {
        #[allow(deprecated)]
        let mut picker = Picker::from_fontsize(ratatui_image::FontSize::new(8, 16));
        picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
        let image = Arc::new(DynamicImage::new_rgb8(960, 540));
        let mut key = Key::new("video", &picker, Size::new(120, 34), (960, 540), 0, 16, 0);
        key.video = true;

        let actual = Encoder::default()
            .encode(&Request {
                key,
                image,
                picker,
                buffer: false,
            })
            .unwrap();

        let Tile::Kitty { frame, .. } = actual else {
            panic!("native video tile");
        };
        assert_eq!((frame.width, frame.height), (960, 256));
    }

    #[test]
    fn card_media_tiny_native_gif_should_keep_enough_pixels_for_its_type_badge() {
        #[allow(deprecated)]
        let mut picker = Picker::from_fontsize(ratatui_image::FontSize::new(8, 16));
        picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
        let image = Arc::new(DynamicImage::new_rgb8(16, 16));
        let mut key = Key::new("tiny-gif", &picker, Size::new(68, 34), (16, 16), 0, 16, 0);
        key.video = true;
        key.overlay = Some(Overlay {
            indicator: Indicator::Video {
                gif: true,
                play: false,
            },
            top: 0,
            rows: 16,
        });

        let actual = Encoder::default()
            .encode(&Request {
                key,
                image,
                picker,
                buffer: false,
            })
            .unwrap();

        let Tile::Kitty { frame, .. } = actual else {
            panic!("native video tile");
        };
        assert_eq!(frame.width, 80);
    }

    #[test]
    fn card_media_portrait_frames_should_fit_without_stretching_into_a_wide_reservation() {
        let picker = picker();
        let image = Arc::new(DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            20,
            40,
            image::Rgba([180, 50, 90, 255]),
        )));
        let mut key = Key::new(
            "portrait",
            &picker,
            Size::new(200, 100),
            (200, 100),
            0,
            100,
            0,
        );
        key.video = true;

        let encoded = Encoder::default()
            .encode(&Request {
                key,
                image,
                picker,
                buffer: false,
            })
            .unwrap();
        let actual = decoded(&encoded);

        assert_eq!(actual.get_pixel(25, 50).0, [17, 17, 27, 255]);
        assert_eq!(actual.get_pixel(100, 50).0, [180, 50, 90, 255]);
        assert_eq!(actual.get_pixel(175, 50).0, [17, 17, 27, 255]);
    }

    #[test]
    fn card_media_kitty_video_should_transmit_decoded_resolution_and_scale_in_the_terminal() {
        let actual = kitty_video_tile();
        let Tile::Kitty { frame, size, .. } = actual else {
            panic!("native video tile");
        };
        assert_eq!((frame.width, frame.height), (160, 76));
        assert_eq!(size, Size::new(68, 16));
    }

    #[test]
    fn card_media_kitty_video_should_transmit_a_frame_only_once_with_a_stable_id() {
        let actual = kitty_video_tile();
        let next = kitty_video_tile();
        let area = Rect::new(0, 0, 68, 16);
        let mut first = ratatui::buffer::Buffer::empty(area);
        let mut second = ratatui::buffer::Buffer::empty(area);
        (&actual).render(area, &mut first);
        (&actual).render(area, &mut second);
        assert!(first[(0, 0)].symbol().contains("_G"));
        assert!(!second[(0, 0)].symbol().contains("_G"));
        let (Tile::Kitty { id: first, .. }, Tile::Kitty { id: second, .. }) = (actual, next) else {
            panic!("native tiles");
        };
        assert_eq!(first, second);
    }

    #[test]
    fn card_media_video_should_have_a_centered_play_symbol_with_dark_backing() {
        let actual = indicator_pixels(
            Some(Indicator::Video {
                gif: false,
                play: true,
            }),
            0,
            100,
        );
        assert_eq!(actual.get_pixel(100, 50).0, [235, 237, 250, 255]);
        assert_eq!(actual.get_pixel(83, 50).0, [17, 17, 27, 255]);
        assert_eq!(actual.get_pixel(0, 50).0, [75, 130, 190, 255]);
    }

    #[test]
    fn card_media_playing_video_should_keep_the_badge_and_remove_the_play_symbol() {
        let actual = indicator_pixels(
            Some(Indicator::Video {
                gif: false,
                play: false,
            }),
            0,
            100,
        );
        assert_eq!(actual.get_pixel(100, 50).0, [75, 130, 190, 255]);
        assert_eq!(actual.get_pixel(5, 5).0, [235, 237, 250, 255]);
    }

    #[test]
    fn card_media_link_should_keep_its_center_clear_and_put_the_arrow_in_a_corner() {
        let actual = indicator_pixels(Some(Indicator::Link), 0, 100);
        assert_eq!(actual.get_pixel(100, 50).0, [75, 130, 190, 255]);
        assert!(
            (180..200).any(|x| (0..20).any(|y| actual.get_pixel(x, y).0 != [75, 130, 190, 255]))
        );
    }

    #[test]
    fn card_media_gallery_should_keep_the_middle_clear() {
        let actual = indicator_pixels(Some(Indicator::Gallery(4)), 0, 100);
        assert_eq!(actual.get_pixel(100, 50).0, [75, 130, 190, 255]);
        assert_eq!(actual.get_pixel(7, 5).0, [235, 237, 250, 255]);
    }

    #[test]
    fn card_media_scrolled_preview_should_keep_the_play_symbol_centered_in_the_visible_slice() {
        let actual = indicator_pixels(
            Some(Indicator::Video {
                gif: true,
                play: true,
            }),
            50,
            10,
        );
        assert_eq!(actual.dimensions(), (200, 10));
        assert_eq!(actual.get_pixel(100, 5).0, [235, 237, 250, 255]);
    }

    #[test]
    fn card_media_plain_image_should_keep_its_original_pixels() {
        let actual = indicator_pixels(None, 0, 100);
        assert!(actual.pixels().all(|pixel| pixel.0 == [75, 130, 190, 255]));
    }

    #[test]
    fn video_strips_should_publish_together_after_the_last_completion() {
        let picker = picker();
        let red = Arc::new(DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            2,
            4,
            image::Rgba([255, 0, 0, 255]),
        )));
        let blue = Arc::new(DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            2,
            4,
            image::Rgba([0, 0, 255, 255]),
        )));
        let keys: Vec<_> = [0, 2]
            .into_iter()
            .map(|top| {
                let mut key = Key::new("video", &picker, Size::new(2, 4), (2, 4), top, 2, 0);
                key.video = true;
                key
            })
            .collect();
        let mut renderer = Renderer::default();
        renderer.started = true;
        let mut encoder = Encoder::default();
        for key in &keys {
            let done = Completed {
                key: key.clone(),
                image: Arc::downgrade(&red),
                source_size: (red.width(), red.height()),
                result: encoder
                    .encode(&Request {
                        key: key.clone(),
                        image: Arc::clone(&red),
                        picker: picker.clone(),
                        buffer: false,
                    })
                    .map(Arc::new),
            };
            renderer.shared.work.lock().completed.push(done);
        }
        assert!(renderer.poll());

        // Stop at the real worker's between-strips state, with actual encoded pixels.
        let mut encoder = Encoder::default();
        let completion = |encoder: &mut Encoder, key: &Key| Completed {
            key: key.clone(),
            image: Arc::downgrade(&blue),
            source_size: (blue.width(), blue.height()),
            result: encoder
                .encode(&Request {
                    key: key.clone(),
                    image: Arc::clone(&blue),
                    picker: picker.clone(),
                    buffer: true,
                })
                .map(Arc::new),
        };
        {
            let mut work = renderer.shared.work.lock();
            work.wanted = vec![(keys[1].clone(), Arc::downgrade(&blue))];
            work.completed.push(completion(&mut encoder, &keys[0]));
        }
        assert!(
            !renderer.poll(),
            "one finished strip must not expose a partial frame"
        );
        renderer.begin();
        for key in &keys {
            let tile = renderer
                .tile(key.clone(), &blue, &picker, Preparation::Refresh)
                .unwrap()
                .unwrap();
            assert_eq!(pixel(&tile), image::Rgba([255, 0, 0, 255]));
        }
        renderer.finish();
        {
            let mut work = renderer.shared.work.lock();
            assert_eq!(
                work.wanted.len(),
                1,
                "do not requeue a finished but unpublished strip"
            );
            work.pending = None;
            work.wanted.clear();
            work.completed.push(completion(&mut encoder, &keys[1]));
        }
        assert!(renderer.poll());
        for key in keys {
            let tile = renderer
                .tile(key, &blue, &picker, Preparation::Hold)
                .unwrap()
                .unwrap();
            assert_eq!(pixel(&tile), image::Rgba([0, 0, 255, 255]));
        }
    }

    #[test]
    fn large_video_frames_keep_the_current_picture_while_the_output_budget_recovers() {
        #[allow(deprecated)]
        let mut picker = Picker::from_fontsize(ratatui_image::FontSize::new(8, 16));
        picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
        let mut seed = 1u32;
        let image = Arc::new(DynamicImage::ImageRgb8(image::RgbImage::from_fn(
            960,
            540,
            |_, _| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                image::Rgb([(seed >> 16) as u8, (seed >> 8) as u8, seed as u8])
            },
        )));
        let mut key = Key::new("video", &picker, Size::new(120, 34), (960, 540), 0, 16, 0);
        key.video = true;
        let mut renderer = Renderer::default();
        renderer.begin();
        renderer.tile(key.clone(), &image, &picker, Preparation::Refresh);
        renderer.finish();
        complete(&mut renderer);
        let current = renderer
            .tile(key.clone(), &image, &picker, Preparation::Hold)
            .unwrap()
            .unwrap();
        let next = Arc::new(DynamicImage::new_rgb8(960, 540));

        renderer.begin();
        let actual = renderer
            .tile(key, &next, &picker, Preparation::Refresh)
            .unwrap()
            .unwrap();
        renderer.finish();

        assert!(
            Arc::ptr_eq(&current, &actual),
            "retain the displayed frame during backpressure"
        );
        assert!(
            !renderer.pending(),
            "do not encode and queue another large frame immediately"
        );
    }

    #[test]
    fn video_frames_over_their_share_at_the_playing_rate_wait_for_the_output_budget() {
        #[allow(deprecated)]
        let mut picker = Picker::from_fontsize(ratatui_image::FontSize::new(8, 16));
        picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
        let mut seed = 7u32;
        let image = Arc::new(DynamicImage::ImageRgb8(image::RgbImage::from_fn(
            344,
            352,
            |_, _| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                image::Rgb([(seed >> 16) as u8, (seed >> 8) as u8, seed as u8])
            },
        )));
        let mut key = Key::new("video", &picker, Size::new(43, 22), (344, 352), 0, 22, 0);
        key.video = true;
        // Whether the frame after this one waits, at `fps`.
        let next_waits = |fps: f64| {
            let mut renderer = Renderer::default();
            renderer.set_video_rate(fps);
            renderer.begin();
            renderer.tile(key.clone(), &image, &picker, Preparation::Refresh);
            renderer.finish();
            complete(&mut renderer);
            let current = renderer
                .tile(key.clone(), &image, &picker, Preparation::Hold)
                .unwrap()
                .unwrap();
            let Tile::Kitty { upload, .. } = current.as_ref() else {
                panic!("a kitty video frame");
            };
            let share = |fps: f64| (VIDEO_BYTES_PER_SECOND / fps) as usize;
            assert!(
                (share(30.0)..share(24.0)).contains(&upload.len()),
                "a frame within a 24 fps share but over a 30 fps one: {}",
                upload.len()
            );
            renderer.begin();
            renderer.tile(
                key.clone(),
                &Arc::new(DynamicImage::new_rgb8(344, 352)),
                &picker,
                Preparation::Refresh,
            );
            renderer.finish();
            !renderer.pending()
        };

        assert!(next_waits(30.0), "30 a second would exceed the budget");
        assert!(!next_waits(24.0), "24 a second fit it: every frame shows");
    }

    #[test]
    fn kitty_video_should_upload_to_a_hidden_buffer_before_reusing_the_previous_one() {
        #[allow(deprecated)]
        let mut picker = Picker::from_fontsize(ratatui_image::FontSize::new(1, 1));
        picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
        let sources: Vec<_> = (0..3)
            .map(|_| Arc::new(DynamicImage::new_rgb8(4, 4)))
            .collect();
        let mut renderer = Renderer::default();
        let mut ids = Vec::new();
        for image in sources {
            renderer.begin();
            let keys: Vec<_> = [0, 2]
                .into_iter()
                .map(|top| {
                    let mut key = Key::new("video", &picker, Size::new(4, 4), (4, 4), top, 2, 0);
                    key.video = true;
                    renderer.tile(key.clone(), &image, &picker, Preparation::Refresh);
                    key
                })
                .collect();
            renderer.finish();
            complete(&mut renderer);
            let frame_ids: Vec<_> = keys
                .into_iter()
                .map(|key| {
                    let tile = renderer
                        .tile(key, &image, &picker, Preparation::Hold)
                        .unwrap()
                        .unwrap();
                    let Tile::Kitty { id, .. } = tile.as_ref() else {
                        panic!("native video");
                    };
                    *id
                })
                .collect();
            ids.push(frame_ids);
        }
        assert!(
            ids[0].iter().all(|id| !ids[1].contains(id)),
            "uploads must not replace visible strips"
        );
        assert_eq!(ids[0], ids[2], "two buffers bound native image storage");
    }

    #[test]
    fn scrolling_video_should_keep_each_visible_strips_upload_buffer_hidden() {
        #[allow(deprecated)]
        let mut picker = Picker::from_fontsize(ratatui_image::FontSize::new(1, 1));
        picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
        let old = Arc::new(DynamicImage::new_rgb8(4, 6));
        let next = Arc::new(DynamicImage::new_rgb8(4, 6));
        let mut renderer = Renderer::default();
        let mut previous = HashMap::new();
        for (image, tops) in [(&old, [0, 2]), (&old, [2, 4]), (&next, [2, 4])] {
            renderer.begin();
            let keys: Vec<_> = tops
                .into_iter()
                .map(|top| {
                    let mut key = Key::new("video", &picker, Size::new(4, 6), (4, 6), top, 2, 0);
                    key.video = true;
                    renderer.tile(key.clone(), image, &picker, Preparation::Refresh);
                    key
                })
                .collect();
            renderer.finish();
            complete(&mut renderer);
            for key in keys {
                let tile = renderer
                    .tile(key.clone(), image, &picker, Preparation::Hold)
                    .unwrap()
                    .unwrap();
                let Tile::Kitty { id, .. } = tile.as_ref() else {
                    panic!("native video");
                };
                if Arc::ptr_eq(image, &next) {
                    assert_ne!(
                        previous[&key.start], *id,
                        "a retained strip must not be overwritten during upload"
                    );
                }
                previous.insert(key.start, *id);
            }
        }
    }

    #[test]
    fn when_video_advances_during_encoding_the_inflight_picture_should_finish() {
        let picker = picker();
        let old = Arc::new(DynamicImage::new_rgb8(4, 4));
        let next = Arc::new(DynamicImage::new_rgb8(4, 4));
        let mut key = Key::new("video", &picker, Size::new(4, 4), (4, 4), 0, 4, 0);
        key.video = true;
        let mut renderer = Renderer::default();
        renderer.started = true;
        renderer.begin();
        renderer.tile(key.clone(), &old, &picker, Preparation::Refresh);
        renderer.finish();

        renderer.begin();
        renderer.tile(key.clone(), &next, &picker, Preparation::Refresh);
        renderer.finish();

        let work = renderer.shared.work.lock();
        assert!(Weak::ptr_eq(&work.wanted[0].1, &Arc::downgrade(&old)));
        assert!(Arc::ptr_eq(&work.pending.as_ref().unwrap()[0].image, &old));
    }

    #[test]
    fn when_video_finishes_during_a_draw_the_completed_picture_should_still_publish() {
        let picker = picker();
        let old = Arc::new(DynamicImage::new_rgb8(4, 4));
        let next = Arc::new(DynamicImage::new_rgb8(4, 4));
        let mut key = Key::new("video", &picker, Size::new(4, 4), (4, 4), 0, 4, 0);
        key.video = true;
        let mut renderer = Renderer::default();
        renderer.started = true;
        renderer.begin();
        renderer.tile(key.clone(), &old, &picker, Preparation::Refresh);
        renderer.finish();
        renderer.begin();
        renderer.tile(key, &next, &picker, Preparation::Refresh);
        // The real worker completes after begin's poll and before finish.
        let request = renderer
            .shared
            .work
            .lock()
            .pending
            .take()
            .unwrap()
            .remove(0);
        let result = Encoder::default().encode(&request).map(Arc::new);
        {
            let mut work = renderer.shared.work.lock();
            work.wanted.clear();
            work.completed.push(Completed {
                key: request.key,
                image: Arc::downgrade(&old),
                source_size: (4, 4),
                result,
            });
        }

        renderer.finish();

        assert!(renderer.poll(), "publish the picture that just finished");
        assert!(!renderer.pending());
    }

    #[test]
    fn superseding_a_partial_video_batch_should_discard_its_pixels_and_finish_the_new_viewport() {
        let picker = picker();
        let old = Arc::new(DynamicImage::new_rgb8(4, 6));
        let next = Arc::new(DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            4,
            6,
            image::Rgba([0, 0, 255, 255]),
        )));
        let key = |top| {
            let mut key = Key::new("video", &picker, Size::new(4, 6), (4, 6), top, 2, 0);
            key.video = true;
            key
        };
        let mut renderer = Renderer::default();
        renderer.started = true;
        renderer.begin();
        for top in [0, 2] {
            renderer.tile(key(top), &old, &picker, Preparation::Refresh);
        }
        renderer.finish();
        // Complete exactly one real encoding before the source and viewport change.
        let request = renderer
            .shared
            .work
            .lock()
            .pending
            .as_mut()
            .unwrap()
            .remove(0);
        let result = Encoder::default().encode(&request).map(Arc::new);
        {
            let mut work = renderer.shared.work.lock();
            work.wanted.remove(0);
            work.completed.push(Completed {
                key: request.key,
                image: Arc::downgrade(&old),
                source_size: (old.width(), old.height()),
                result,
            });
        }
        renderer.begin();
        for top in [2, 4] {
            renderer.tile(key(top), &next, &picker, Preparation::Refresh);
        }
        renderer.finish();
        assert!(renderer.shared.work.lock().completed.is_empty());
        let shared = Arc::clone(&renderer.shared);
        let worker = std::thread::spawn(move || prepare(shared, Encoder::default()));
        complete(&mut renderer);
        for top in [2, 4] {
            let tile = renderer
                .tile(key(top), &next, &picker, Preparation::Hold)
                .unwrap()
                .unwrap();
            assert_eq!(pixel(&tile), image::Rgba([0, 0, 255, 255]));
        }
        assert!(!renderer.pending());
        drop(renderer);
        worker.join().unwrap();
    }

    #[test]
    fn native_still_ids_should_be_independent_across_processes() {
        if std::env::var_os("REDTAB_NATIVE_ID_TEST_CHILD").is_some() {
            let mut picker = picker();
            picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
            let key = Key::new("same-image", &picker, Size::new(2, 2), (2, 2), 0, 2, 0);
            let encoded = Encoder::default()
                .encode(&Request {
                    key,
                    image: Arc::new(DynamicImage::new_rgb8(2, 2)),
                    picker,
                    buffer: false,
                })
                .unwrap();
            let Tile::KittyImage { id, .. } = encoded else {
                panic!("native image");
            };
            println!("NATIVE_ID={id}");
            return;
        }
        let allocate = || {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "card_image::tests::native_still_ids_should_be_independent_across_processes",
                    "--nocapture",
                ])
                .env("REDTAB_NATIVE_ID_TEST_CHILD", "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout)
                .unwrap()
                .lines()
                .find_map(|line| line.strip_prefix("NATIVE_ID=").map(str::to_owned))
                .unwrap()
        };
        assert_ne!(
            allocate(),
            allocate(),
            "two tmux panes must not start with the same native image ID"
        );
    }

    #[test]
    fn a_smaller_authoritative_refresh_should_replace_a_warm_encoding() {
        let picker = picker();
        let key = Key::new("same-url", &picker, Size::new(4, 4), (4, 4), 0, 4, 0);
        let old = Arc::new(DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            4,
            4,
            image::Rgba([255, 0, 0, 255]),
        )));
        let fresh = Arc::new(DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            2,
            2,
            image::Rgba([0, 0, 255, 255]),
        )));
        let mut renderer = Renderer::default();
        renderer.begin();
        renderer.tile(key.clone(), &old, &picker, Preparation::Refresh);
        renderer.finish();
        complete(&mut renderer);
        renderer.begin();
        renderer.tile(key.clone(), &fresh, &picker, Preparation::Refresh);
        renderer.finish();
        complete(&mut renderer);
        let shown = renderer
            .tile(key, &fresh, &picker, Preparation::Hold)
            .unwrap()
            .unwrap();
        assert_eq!(pixel(&shown), image::Rgba([0, 0, 255, 255]));
    }

    #[test]
    fn a_retained_thumbnail_should_not_replace_a_warm_full_resolution_encoding() {
        let picker = picker();
        let key = Key::new("same-url", &picker, Size::new(4, 4), (4, 4), 0, 4, 0);
        let full = Arc::new(DynamicImage::new_rgb8(4, 4));
        let preview = Arc::new(DynamicImage::new_rgb8(2, 2));
        let mut renderer = Renderer::default();
        renderer.begin();
        renderer.tile(key.clone(), &full, &picker, Preparation::Refresh);
        renderer.finish();
        complete(&mut renderer);
        renderer.begin();
        let old = renderer
            .tile(key.clone(), &full, &picker, Preparation::Hold)
            .unwrap()
            .unwrap();
        renderer.finish();
        drop(full);
        renderer.begin();
        let shown = renderer
            .tile(key, &preview, &picker, Preparation::Missing)
            .unwrap()
            .unwrap();
        renderer.finish();
        assert!(Arc::ptr_eq(&old, &shown));
        assert!(
            !renderer.pending(),
            "a cached full image must not be re-encoded as its thumbnail"
        );
    }

    #[test]
    fn when_new_pixels_are_preparing_the_previous_encoding_should_remain_visible() {
        let picker = picker();
        let key = Key::new("image", &picker, Size::new(2, 2), (2, 2), 0, 2, 0);
        let red = Arc::new(DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            2,
            2,
            image::Rgba([255, 0, 0, 255]),
        )));
        let blue = Arc::new(DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            2,
            2,
            image::Rgba([0, 0, 255, 255]),
        )));
        let mut renderer = Renderer::default();
        renderer.begin();
        assert!(
            renderer
                .tile(key.clone(), &red, &picker, Preparation::Refresh)
                .is_none()
        );
        renderer.finish();
        complete(&mut renderer);

        renderer.begin();
        let previous = renderer
            .tile(key.clone(), &blue, &picker, Preparation::Refresh)
            .unwrap()
            .unwrap();
        renderer.finish();

        assert_eq!(pixel(&previous), image::Rgba([255, 0, 0, 255]));
        complete(&mut renderer);
        renderer.begin();
        let current = renderer
            .tile(key, &blue, &picker, Preparation::Refresh)
            .unwrap()
            .unwrap();
        renderer.finish();
        assert_eq!(pixel(&current), image::Rgba([0, 0, 255, 255]));
    }

    #[test]
    fn when_viewports_change_quickly_only_the_latest_batch_should_remain_queued() {
        let picker = picker();
        let image = Arc::new(DynamicImage::new_rgb8(8, 8));
        let mut renderer = Renderer::default();
        for width in 1..=100 {
            renderer.begin();
            renderer.tile(
                Key::new("image", &picker, Size::new(width, 8), (8, 8), 0, 8, 0),
                &image,
                &picker,
                Preparation::Refresh,
            );
            renderer.finish();
        }
        complete(&mut renderer);

        renderer.begin();
        let current = renderer
            .tile(
                Key::new("image", &picker, Size::new(100, 8), (8, 8), 0, 8, 0),
                &image,
                &picker,
                Preparation::Refresh,
            )
            .unwrap()
            .unwrap();
        renderer.finish();

        assert_eq!(current.size(), Size::new(100, 8));
        assert!(!renderer.pending());
        assert!(
            renderer
                .shared
                .work
                .lock()
                .pending
                .as_ref()
                .is_none_or(|batch| batch.len() <= 1)
        );
    }

    #[test]
    fn when_images_are_cleared_queued_work_and_encodings_should_be_released() {
        let picker = picker();
        let image = Arc::new(DynamicImage::new_rgb8(8, 8));
        let mut renderer = Renderer::default();
        renderer.begin();
        renderer.tile(
            Key::new("image", &picker, Size::new(8, 8), (8, 8), 0, 8, 0),
            &image,
            &picker,
            Preparation::Refresh,
        );
        renderer.finish();

        renderer.clear();
        renderer.poll();

        assert!(!renderer.pending());
        assert!(renderer.cache.is_empty());
        assert!(renderer.shared.work.lock().wanted.is_empty());
    }

    #[test]
    fn when_images_are_cleared_the_idle_workers_downscaled_pixels_should_be_released() {
        let picker = picker();
        let image = Arc::new(DynamicImage::new_rgb8(16, 16));
        let mut encoder = Encoder::default();
        encoder
            .encode(&Request {
                key: Key::new("image", &picker, Size::new(2, 2), (16, 16), 0, 2, 0),
                image,
                picker,
                buffer: false,
            })
            .unwrap();
        let pixels = Arc::downgrade(&encoder.downscaled.as_ref().unwrap().2);
        let mut renderer = Renderer::default();
        let shared = Arc::clone(&renderer.shared);
        let worker = std::thread::spawn(move || prepare(shared, encoder));

        renderer.clear();
        {
            let mut work = renderer.shared.work.lock();
            while work.clear_encoder {
                assert!(
                    !renderer
                        .shared
                        .changed
                        .wait_for(&mut work, std::time::Duration::from_secs(5))
                        .timed_out(),
                    "idle renderer did not process clear"
                );
            }
        }

        assert!(
            pixels.upgrade().is_none(),
            "images-off must release derived pixels while idle"
        );
        drop(renderer);
        worker.join().unwrap();
    }

    #[test]
    fn when_offscreen_encodings_exceed_the_count_limit_the_oldest_should_be_released() {
        let picker = picker();
        let image = Arc::new(DynamicImage::new_rgb8(1, 1));
        let mut renderer = Renderer::default();
        renderer.begin();
        for number in 0..513 {
            renderer.tile(
                Key::new(
                    &number.to_string(),
                    &picker,
                    Size::new(1, 1),
                    (1, 1),
                    0,
                    1,
                    0,
                ),
                &image,
                &picker,
                Preparation::Refresh,
            );
        }
        renderer.finish();
        complete(&mut renderer);
        assert_eq!(renderer.cache.len(), 513, "visible encodings are retained");

        renderer.begin();
        renderer.finish();

        assert_eq!(renderer.cache.len(), 512);
        assert!(!renderer.cache.contains_key(&Key::new(
            "0",
            &picker,
            Size::new(1, 1),
            (1, 1),
            0,
            1,
            0
        )));
    }
}
