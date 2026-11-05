use super::*;

/// Starts a decoder run at a position: ffmpeg, or frames a test feeds.
pub(super) type Spawn = Box<dyn FnMut(&Sources, f64) -> io::Result<Decoder>>;

/// Where a player starts: position, pause and sound.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Resume {
    pub at: f64,
    pub paused: bool,
    pub sound: bool,
}

pub struct Player {
    pub url: String,
    /// The area, in cells, the frames are sized for.
    pub area: Size,
    /// Resolving the stream to its files; `sources` holds the answer.
    pub(super) resolving: Option<Receiver<Result<Sources, String>>>,
    pub(super) resolve_cancel: Option<Arc<youtube::Cancellation>>,
    pub(super) sources: Option<Sources>,
    pub(super) spawn: Spawn,
    pub(super) decoder: Option<Decoder>,
    /// Media time of the current run's first frame.
    pub(super) start: f64,
    pub(super) shown: u64,
    /// Frames per second of the frames shown.
    pub(super) rate: f64,
    pub(super) next: Option<Decoded>,
    pub(super) frame: Option<Frame>,
    /// Counts frames put on screen; the drawing side sends a frame once per number.
    pub frame_number: u64,
    pub(super) clock: Clock,
    pub(super) paused: bool,
    pub(super) hidden: bool,
    /// Stopped after a seek or a paused start: show one frame at the new position.
    pub(super) show_one: bool,
    /// Every frame of this run so far was the same picture.
    pub(super) unchanged: bool,
    /// A silent stream that never changed (a still GIF): it stays on screen and
    /// nothing plays.
    pub(super) still: bool,
    /// No more retries for runs before the next frame: one was used, or a resume just
    /// failed and this run from the start must not retry either.
    pub(super) retried: bool,
    /// Where resuming after a cut failed even when retried, as when the server always
    /// cuts the file at the same byte: a cut there is treated as the end.
    pub(super) unresumable: Option<f64>,
    /// Sound starts with this run's first frame, so a slow start can't start it twice.
    pub(super) sound_waits: bool,
    pub(super) sound: bool,
    pub(super) audio: Option<Audio>,
    pub(super) duration: Option<f64>,
    pub(super) error: Option<String>,
    pub(super) waiting_since: Option<Instant>,
    /// A message for the reader (no sound, no ffplay), and whether it is an error.
    pub(super) notice: Option<(String, bool)>,
}

impl Player {
    /// Plays `url` (absolute) for an `area` cells big at `font`, from `resume`.
    pub fn start(
        url: String,
        area: Size,
        font: FontSize,
        resume: Resume,
        now: Instant,
        fallback: impl FnOnce(
            Arc<dyn Fn() -> bool + Send + Sync>,
        ) -> Result<crate::reddit::VideoManifest, String>
        + Send
        + 'static,
    ) -> Self {
        let frame_box = frame_box(area, font);
        let (tx, rx) = sync_channel(1);
        let target = url.clone();
        let cancelled = Arc::new(youtube::Cancellation::default());
        let cancel = Arc::clone(&cancelled);
        std::thread::spawn(move || {
            let youtube = crate::model::youtube_id(&target).is_some();
            let result = prepare_playback(
                || {
                    if youtube {
                        youtube::resolve(&target, &cancel)
                    } else {
                        let native_cancel = Arc::clone(&cancel);
                        resolve(
                            &target,
                            frame_box,
                            frame_box.1,
                            Some(MAX_VIDEO_PIXELS),
                            || fallback(Arc::new(move || native_cancel.is_cancelled())),
                        )
                    }
                },
                || {
                    LazyLock::force(&FPS_EXPRESSIONS);
                    if youtube {
                        LazyLock::force(&RANGED_REQUESTS);
                    }
                },
            );
            let _ = tx.send(result);
        });
        let spawn: Spawn = Box::new(move |sources: &Sources, at| {
            Decoder::spawn(&sources.video, at, frame_box, &sources.headers)
        });
        let mut player = Self::new(url, area, Some(rx), None, spawn, resume, now);
        player.resolve_cancel = Some(cancelled);
        player
    }

    pub(super) fn new(
        url: String,
        area: Size,
        resolving: Option<Receiver<Result<Sources, String>>>,
        sources: Option<Sources>,
        spawn: Spawn,
        resume: Resume,
        now: Instant,
    ) -> Self {
        let mut clock = Clock::new(resume.at, now);
        if resume.paused {
            clock.stop(now);
        }
        let mut player = Self {
            url,
            area,
            resolving,
            resolve_cancel: None,
            sources,
            spawn,
            decoder: None,
            start: resume.at,
            shown: 0,
            rate: MIN_FPS,
            next: None,
            frame: None,
            frame_number: 0,
            clock,
            paused: resume.paused,
            hidden: false,
            show_one: resume.paused,
            unchanged: true,
            still: false,
            retried: false,
            unresumable: None,
            sound_waits: false,
            sound: resume.sound,
            audio: None,
            duration: None,
            error: None,
            waiting_since: None,
            notice: None,
        };
        if player.sources.is_some() {
            player.run_from(resume.at, now);
        }
        player
    }

    pub(super) fn playing(&self) -> bool {
        !self.paused && !self.hidden && self.error.is_none()
    }

    /// When the next frame of this run is due, in media time.
    pub(super) fn due(&self) -> f64 {
        self.start
            + self
                .next
                .as_ref()
                .map_or(self.shown as f64 / self.rate, |next| {
                    next.index as f64 / next.rate
                })
    }

    /// How long until the next frame is due; None while nothing will change.
    pub fn wake_in(&self, now: Instant) -> Option<Duration> {
        if self.error.is_some() || self.still {
            return None;
        }
        if self.resolving.is_some() {
            return Some(Duration::from_millis(50));
        }
        if !(self.playing() || self.show_one) {
            return None;
        }
        let poll = Duration::from_millis(10);
        if self.next.is_none() || !self.playing() {
            return Some(poll);
        }
        if self.audio.is_some() {
            return Some(poll);
        }
        self.clock
            .when(self.due(), now)
            .map(|at| at.saturating_duration_since(now))
    }

    pub fn loading(&self) -> bool {
        self.error.is_none() && (self.shown == 0 || self.waiting_for_audio())
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    /// The stream is resolved: seeking works.
    pub fn ready(&self) -> bool {
        self.sources.is_some()
    }

    /// The message for the reader, once: text and whether it is an error.
    pub fn take_notice(&mut self) -> Option<(String, bool)> {
        self.notice.take()
    }

    pub fn position(&self, now: Instant) -> f64 {
        if self.still || self.shown == 0 {
            self.start
        } else {
            self.audio
                .as_ref()
                .and_then(|audio| audio.position(now))
                .unwrap_or_else(|| self.clock.position(now))
        }
    }

    pub fn duration(&self) -> Option<f64> {
        self.duration
    }

    pub fn frame(&self) -> Option<&Frame> {
        self.frame.as_ref()
    }

    /// Keep the last display while a replacement decoder waits for its first frame.
    /// This does not advance the new run's clock or start its sound.
    pub fn retain_frame(&mut self, frame: Option<Frame>) {
        if self.frame.is_none() && frame.is_some() {
            self.frame = frame;
            self.frame_number += 1;
        }
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref().map(|error| {
            if crate::model::youtube_id(&self.url).is_some()
                && self.sources.is_some()
                && error != "ffmpeg is not installed"
            {
                "YouTube playback failed; open media externally"
            } else {
                error
            }
        })
    }

    pub fn resolving(&self) -> bool {
        self.resolving.is_some()
    }

    #[cfg(test)]
    pub fn hidden(&self) -> bool {
        self.hidden
    }

    /// A player showing `frame`, with nothing to decode.
    #[cfg(test)]
    pub fn showing(url: &str, frame: Frame) -> Self {
        let spawn: Spawn = Box::new(|_, _| Err(io::Error::other("no decoder in tests")));
        let mut player = Self::new(
            url.into(),
            Size::new(10, 5),
            None,
            None,
            spawn,
            Resume::default(),
            Instant::now(),
        );
        player.show(frame);
        player
    }

    /// The next frame on screen, as if it came due.
    #[cfg(test)]
    pub fn show(&mut self, frame: Frame) {
        self.next = Some(Decoded {
            index: self.shown,
            frame,
            rate: self.rate,
        });
        self.show_next();
    }

    #[cfg(test)]
    pub fn buffered(url: &str, area: Size, frame: Frame) -> Self {
        let mut player = Self::new(
            url.into(),
            area,
            None,
            None,
            Box::new(|_, _| panic!("buffered fixture cannot start a decoder")),
            Resume::default(),
            Instant::now(),
        );
        let (tx, frames) = sync_channel(1);
        tx.send(Decoded {
            index: 0,
            frame,
            rate: MIN_FPS,
        })
        .unwrap();
        player.decoder = Some(Decoder {
            child: None,
            frames,
            log: Arc::new(Mutex::new(Log::default())),
            progress: Arc::new(DecodeProgress::default()),
        });
        player
    }

    #[cfg(test)]
    pub fn pending(url: &str, area: Size) -> Self {
        let (sender, resolving) = sync_channel(1);
        Self::new(
            url.into(),
            area,
            Some(resolving),
            None,
            Box::new(move |_, _| {
                let _keep_open = &sender;
                panic!("pending resolution cannot start a decoder")
            }),
            Resume::default(),
            Instant::now(),
        )
    }

    #[cfg(test)]
    pub fn unavailable(url: &str, area: Size) -> Self {
        Self::new(
            url.into(),
            area,
            None,
            Some(Sources {
                video: url.into(),
                audio: None,
                audio_in_file: false,
                headers: Vec::new(),
                audio_headers: Vec::new(),
            }),
            Box::new(|_, _| Err(io::Error::other("fixture decoder unavailable"))),
            Resume::default(),
            Instant::now(),
        )
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        if let Some(cancelled) = &self.resolve_cancel {
            cancelled.cancel();
        }
        if self.error.is_some() {
            youtube::invalidate(&self.url);
        }
        self.stop_audio();
    }
}
