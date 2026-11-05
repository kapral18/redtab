use super::*;

impl Player {
    /// Off screen (scrolled away, an overlay, a dialog): stop, and go on when back.
    pub fn set_hidden(&mut self, hidden: bool, now: Instant) {
        if self.hidden != hidden {
            let was = self.playing();
            self.hidden = hidden;
            self.follow(was, now);
        }
    }

    pub fn toggle_pause(&mut self, now: Instant) {
        let was = self.playing();
        self.paused = !self.paused;
        self.follow(was, now);
    }

    /// Start or stop the clock and the sound after a change of `paused` or `hidden`.
    pub(super) fn follow(&mut self, was: bool, now: Instant) {
        self.waiting_since = None;
        match (was, self.playing()) {
            (true, false) => {
                self.release_audio(now);
            }
            (false, true) => {
                self.clock.start(now);
                self.show_one = false;
                if self.sound {
                    self.play_sound(now);
                }
            }
            _ => {}
        }
    }

    /// Toggles sound; without a sound stream or ffplay it stays off with a notice.
    pub fn toggle_sound(&mut self, now: Instant) {
        self.sound = !self.sound;
        if !self.sound {
            self.release_audio(now);
        } else if self.sources.as_ref().is_some_and(|s| s.audio.is_none()) {
            self.sound = false;
            self.notice = Some(("This video has no sound".into(), false));
        } else if self.playing() && self.sources.is_some() {
            self.play_sound(now);
        }
    }

    pub fn sound(&self) -> bool {
        self.sound
    }

    pub fn waiting_for_audio(&self) -> bool {
        self.audio
            .as_ref()
            .is_some_and(|audio| audio.position(Instant::now()).is_none())
    }

    /// Jump `by` seconds from the current position, within the video.
    pub fn seek(&mut self, by: f64, now: Instant) {
        if self.error.is_some() || self.sources.is_none() {
            return;
        }
        let mut to = (self.position(now) + by).max(0.0);
        // Past the end, or past a point the server always cuts at: stop just before it.
        if let Some(end) = self.duration.or(self.unresumable) {
            to = to.min((end - 1.0).max(0.0));
        }
        let was = self.playing();
        // A new start: its own retry.
        self.retried = false;
        self.run_from(to, now);
        if !was {
            self.clock.stop(now);
            self.show_one = true;
        }
    }

    pub(super) fn start_audio(&mut self, now: Instant) {
        let at = self.position(now);
        self.stop_audio();
        let Some(url) = self.sources.as_ref().and_then(|s| s.audio.as_deref()) else {
            self.sound = false;
            self.notice = Some(("This video has no sound".into(), false));
            return;
        };
        match Audio::start(url, &self.sources.as_ref().unwrap().audio_headers, at, now) {
            Ok(audio) => {
                self.clock.set(at, now);
                self.clock.running = false;
                self.audio = Some(audio);
            }
            Err(e) => {
                self.sound = false;
                let text = if e.kind() == io::ErrorKind::NotFound {
                    "Sound needs ffplay (part of ffmpeg)".to_string()
                } else {
                    e.to_string()
                };
                self.notice = Some((text, true));
            }
        }
    }

    /// Start sound now if this run shows frames, else with its first frame.
    pub(super) fn play_sound(&mut self, now: Instant) {
        if self.shown == 0 {
            self.sound_waits = true;
        } else {
            self.start_audio(now);
        }
    }

    pub(super) fn stop_audio(&mut self) {
        self.sound_waits = false;
        self.audio = None;
    }

    pub(super) fn release_audio(&mut self, now: Instant) {
        let at = self.position(now);
        self.stop_audio();
        self.clock.set(at, now);
        self.clock.running = self.playing();
    }

    /// Frames per second of the frames on screen.
    pub fn rate(&self) -> f64 {
        self.rate
    }

    /// Where a player for a new area takes over: the run's start until its first frame,
    /// not the clock that ran while loading.
    pub fn resume(&self, now: Instant) -> Resume {
        Resume {
            at: if self.shown == 0 {
                self.start
            } else {
                self.position(now)
            },
            paused: self.paused,
            sound: self.sound,
        }
    }

    /// A player for a video without sound, sized for `area`, whose run never gives
    /// a frame.
    #[cfg(test)]
    pub fn silent(url: &str, area: Size) -> Self {
        let spawn: Spawn = Box::new(|_, _| {
            let (tx, frames) = sync_channel(1);
            std::mem::forget(tx);
            Ok(Decoder {
                child: None,
                frames,
                log: Arc::new(Mutex::new(Log::default())),
                progress: Arc::new(DecodeProgress::default()),
            })
        });
        let sources = Sources {
            video: url.into(),
            audio: None,
            audio_in_file: false,
            headers: Vec::new(),
            audio_headers: Vec::new(),
        };
        Self::new(
            url.into(),
            area,
            None,
            Some(sources),
            spawn,
            Resume::default(),
            Instant::now(),
        )
    }

    /// As if its frames came at `rate` per second.
    #[cfg(test)]
    pub fn at_rate(mut self, rate: f64) -> Self {
        self.rate = rate;
        self
    }

    /// ffplay failing to start, as the sound would see it.
    #[cfg(test)]
    pub fn fail_sound(&mut self) {
        self.sound = false;
        self.notice = Some(("Sound needs ffplay (part of ffmpeg)".into(), true));
    }
}
