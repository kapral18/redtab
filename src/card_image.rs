//! Card graphics preparation; the UI only queues work and paints completed tiles.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Weak};

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
const CACHE_BYTES: usize = 64 * 1024 * 1024;
const CACHE_TILES: usize = 512;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub url: String,
    pub size: Size,
    pub pixels: (u32, u32),
    pub start: u16,
    pub rows: u16,
    pub column: u16,
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
            column,
            font: (picker.font_size().width, picker.font_size().height),
            protocol: picker.protocol_type() as u8,
            tmux: picker.tmux_detected(),
        }
    }

    fn bytes(&self) -> usize {
        usize::from(TILE_COLS.min(self.size.width - self.column * TILE_COLS))
            * usize::from(self.rows)
            * usize::from(self.font.0.max(1))
            * usize::from(self.font.1.max(1))
            * 16
    }
}

#[derive(Clone)]
struct Request {
    key: Key,
    image: Arc<DynamicImage>,
    picker: Picker,
}

struct Completed {
    key: Key,
    image: Weak<DynamicImage>,
    result: Result<Arc<Protocol>, String>,
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
}

#[derive(Default)]
pub struct Renderer {
    shared: Arc<Shared>,
    started: bool,
    frame: Vec<Request>,
    cache: HashMap<Key, Completed>,
    visible: HashSet<Key>,
    order: VecDeque<Key>,
}

impl Renderer {
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
        allow_prepare: bool,
    ) -> Option<Result<Arc<Protocol>, String>> {
        self.visible.insert(key.clone());
        let cached = self.cache.get(&key);
        let current =
            cached.is_some_and(|entry| Weak::ptr_eq(&entry.image, &Arc::downgrade(image)));
        if !current && allow_prepare {
            self.frame.push(Request {
                key: key.clone(),
                image: Arc::clone(image),
                picker: picker.clone(),
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
        let next: Vec<_> = self
            .frame
            .iter()
            .map(|request| (request.key.clone(), Arc::downgrade(&request.image)))
            .collect();
        let mut work = self.shared.work.lock();
        if work.wanted.len() == next.len()
            && work
                .wanted
                .iter()
                .zip(&next)
                .all(|((a, source), (b, other))| a == b && Weak::ptr_eq(source, other))
        {
            return;
        }
        work.completed
            .retain(|done| wanted(&next, &done.key, &done.image));
        work.wanted = next;
        work.pending = (!self.frame.is_empty()).then(|| std::mem::take(&mut self.frame));
        if !self.started && work.pending.is_some() {
            let shared = Arc::clone(&self.shared);
            std::thread::Builder::new()
                .name("redtab-card-render".into())
                .spawn(move || prepare(shared, Encoder::default()))
                .expect("cannot start card renderer");
            self.started = true;
        }
        self.shared.changed.notify_one();
    }

    pub fn poll(&mut self) -> bool {
        let completed = std::mem::take(&mut self.shared.work.lock().completed);
        let changed = !completed.is_empty();
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
            .keys()
            .filter(|key| !self.visible.contains(*key))
            .fold((0usize, 0usize), |(count, bytes), key| {
                (count + 1, bytes + key.bytes())
            });
        while bytes > CACHE_BYTES || count > CACHE_TILES {
            let Some(key) = self.order.pop_front() else {
                break;
            };
            if self.visible.contains(&key) {
                self.order.push_back(key);
                continue;
            }
            if self.cache.remove(&key).is_some() {
                bytes -= key.bytes();
                count -= 1;
            }
        }
    }

    pub fn pending(&self) -> bool {
        let work = self.shared.work.lock();
        !work.wanted.is_empty() || !work.completed.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn wait_for_completion(&self) {
        let mut work = self.shared.work.lock();
        if work.completed.is_empty() && !work.wanted.is_empty() {
            assert!(
                !self
                    .shared
                    .changed
                    .wait_for(&mut work, std::time::Duration::from_secs(10))
                    .timed_out(),
                "card renderer did not complete"
            );
        }
    }

    pub fn clear(&mut self) {
        self.frame.clear();
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
}

impl Encoder {
    fn encode(&mut self, request: &Request) -> Result<Protocol, String> {
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
        encode_tile(
            &request.picker,
            &request.image,
            sample,
            (request.key.size, request.key.pixels),
            request.key.start,
            request.key.rows,
            request.key.column,
        )
    }
}

pub(crate) fn encode_tile(
    picker: &Picker,
    image: &DynamicImage,
    downscaled: Option<&DynamicImage>,
    (size, source_pixels): (Size, (u32, u32)),
    start: u16,
    rows: u16,
    column: u16,
) -> Result<Protocol, String> {
    let x = column * TILE_COLS;
    let cols = TILE_COLS.min(size.width - x);
    let font = picker.font_size();
    let cell_width = u32::from(font.width.max(1));
    let cell_height = u32::from(font.height.max(1));
    let full_width = f64::from(size.width) * f64::from(cell_width);
    let top = u32::from(start) * cell_height;
    let left = u32::from(x) * cell_width;
    let sample = downscaled.unwrap_or(image);
    let pixels = image::RgbaImage::from_fn(
        cols as u32 * cell_width,
        u32::from(rows) * cell_height,
        |x, y| {
            let u = (f64::from(left + x) + 0.5) / full_width;
            let v = (f64::from(top + y) + 0.5) * f64::from(source_pixels.0)
                / (full_width * f64::from(source_pixels.1));
            image::imageops::sample_bilinear(sample, u as f32, v as f32)
                .unwrap_or(image::Rgba([0, 0, 0, 0]))
        },
    );
    picker
        .new_protocol(
            DynamicImage::ImageRgba8(pixels),
            Size::new(cols, rows),
            Resize::Scale(Some(ratatui_image::FilterType::Triangle)),
        )
        .map_err(|error| error.to_string())
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

    fn pixel(protocol: &Protocol) -> image::Rgba<u8> {
        use base64::Engine;
        let Protocol::ITerm2(protocol) = protocol else {
            panic!("iTerm2 tile");
        };
        let encoded = protocol.data.split("doNotMoveCursor=1:").nth(1).unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded.split('\u{7}').next().unwrap())
            .unwrap();
        image::load_from_memory(&bytes)
            .unwrap()
            .to_rgba8()
            .get_pixel(0, 0)
            .to_owned()
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
        assert!(renderer.tile(key.clone(), &red, &picker, true).is_none());
        renderer.finish();
        complete(&mut renderer);

        renderer.begin();
        let previous = renderer
            .tile(key.clone(), &blue, &picker, true)
            .unwrap()
            .unwrap();
        renderer.finish();

        assert_eq!(pixel(&previous), image::Rgba([255, 0, 0, 255]));
        complete(&mut renderer);
        renderer.begin();
        let current = renderer.tile(key, &blue, &picker, true).unwrap().unwrap();
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
                true,
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
                true,
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
            true,
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
                true,
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
