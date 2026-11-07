use super::*;

#[derive(Clone)]
pub(super) struct Request {
    pub(super) key: Key,
    pub(super) image: Arc<DynamicImage>,
    pub(super) picker: Picker,
    pub(super) buffer: bool,
}

pub(super) struct Completed {
    pub(super) key: Key,
    pub(super) image: Weak<DynamicImage>,
    pub(super) source_size: (u32, u32),
    pub(super) result: Result<Arc<Tile>, String>,
}

#[derive(Default)]
pub(super) struct Work {
    pub(super) pending: Option<Vec<Request>>,
    pub(super) wanted: Vec<(Key, Weak<DynamicImage>)>,
    pub(super) completed: Vec<Completed>,
    pub(super) clear_encoder: bool,
    pub(super) stop: bool,
}

#[derive(Default)]
pub(super) struct Shared {
    pub(super) work: Mutex<Work>,
    pub(super) changed: Condvar,
    pub(super) residency: Arc<Mutex<NativeResidency>>,
}

#[derive(Default)]
pub struct Renderer {
    pub(super) shared: Arc<Shared>,
    pub(super) started: bool,
    pub(super) frame: Vec<Request>,
    pub(super) cache: HashMap<Key, Completed>,
    pub(super) visible: HashSet<Key>,
    pub(super) order: VecDeque<Key>,
    pub(super) video_ready_at: Option<std::time::Instant>,
    /// Frames per second of the playing video; its frames share the output budget.
    pub(super) video_rate: Option<f64>,
    /// A video frame waited for the output budget: it shows once the budget allows,
    /// instead of being skipped for the one after it.
    pub(super) video_waiting: bool,
    pub(super) cache_bytes: CacheBytes,
}

/// Bytes of encoded pictures kept for offscreen reuse.
pub(super) struct CacheBytes(pub(super) usize);

impl Default for CacheBytes {
    fn default() -> Self {
        Self(CACHE_BYTES)
    }
}

impl Completed {
    pub(super) fn bytes(&self, key: &Key) -> usize {
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

    pub(super) fn prune(&mut self) {
        let (mut count, mut bytes) = self
            .cache
            .iter()
            .filter(|(key, _)| !self.visible.contains(*key))
            .fold((0usize, 0usize), |(count, bytes), (key, entry)| {
                (count + 1, bytes + entry.bytes(key))
            });
        while bytes > self.cache_bytes.0 || count > CACHE_TILES {
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

pub(super) fn wanted(
    wanted: &[(Key, Weak<DynamicImage>)],
    key: &Key,
    image: &Weak<DynamicImage>,
) -> bool {
    wanted
        .iter()
        .any(|(next, source)| next == key && Weak::ptr_eq(source, image))
}

pub(super) fn prepare(shared: Arc<Shared>, mut encoder: Encoder) {
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
