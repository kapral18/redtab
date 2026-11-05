use super::*;

impl Player {
    /// Start a decoder run (and the sound) at `at` seconds.
    pub(super) fn run_from(&mut self, at: f64, now: Instant) {
        self.stop_audio();
        self.decoder = None;
        self.next = None;
        self.waiting_since = None;
        self.start = at;
        self.shown = 0;
        self.unchanged = true;
        self.still = false;
        self.clock.set(at, now);
        self.clock.running = false;
        let Some(sources) = &self.sources else {
            return;
        };
        match (self.spawn)(sources, at) {
            Ok(decoder) => self.decoder = Some(decoder),
            Err(e) => {
                self.error = Some(if e.kind() == io::ErrorKind::NotFound {
                    "ffmpeg is not installed".into()
                } else {
                    format!("ffmpeg: {e}")
                });
                return;
            }
        }
        if self.playing() && self.sound {
            self.play_sound(now);
        }
    }

    /// Advance to the frame due now; true when the picture or status line changed.
    pub fn tick(&mut self, now: Instant) -> bool {
        let mut changed = false;
        if let Some(resolving) = &self.resolving {
            let answer = match resolving.try_recv() {
                Ok(answer) => answer,
                Err(TryRecvError::Empty) => return false,
                Err(TryRecvError::Disconnected) => Err("the video could not be resolved".into()),
            };
            self.resolving = None;
            changed = true;
            match answer {
                Ok(sources) => {
                    self.sources = Some(sources);
                    self.run_from(self.start, now);
                }
                Err(e) => self.error = Some(e),
            }
        }
        if self.error.is_some() {
            return changed;
        }
        if let Some(succeeded) = self
            .audio
            .as_mut()
            .and_then(|audio| audio.completed(now, self.duration))
        {
            self.release_audio(now);
            if !succeeded {
                self.sound = false;
                self.notice = Some(("Audio unavailable; playing muted".into(), false));
            }
            changed = true;
        }
        if !self.playing() {
            if self.show_one && self.receive(now, &mut changed) {
                self.show_one = false;
                self.show_next();
                changed = true;
            }
            return changed;
        }
        if let Some(decoder) = &self.decoder
            && let Some(at) = self.audio.as_ref().and_then(|audio| audio.position(now))
        {
            decoder.progress.target.store(
                ((at - self.start).max(0.0) * self.rate) as u64,
                Ordering::Relaxed,
            );
        }
        for _ in 0..8 {
            if !self.receive(now, &mut changed) {
                break;
            }
            let due = self.due();
            let position = self.position(now);
            if position < due {
                break;
            }
            if self.audio.is_none() && position - due > STALL {
                self.clock.set(due, now);
            }
            self.show_next();
            if self.shown == 1 {
                self.clock.set(self.start, now);
                self.clock.start(now);
            }
            if self.sound_waits && self.sound {
                self.start_audio(now);
            }
            changed = true;
        }
        changed
    }

    /// Ensure a frame waits in `next`; false if none is ready. A run that ends is judged
    /// once ffmpeg's log has ended:
    /// - cut short (before the known duration, or, with none, on an input error such as
    ///   a dropped connection), it goes on from there;
    /// - an opened input cut before its first frame is retried once; a resume that still
    ///   fails restarts from the start, and a later cut there counts as the end;
    /// - a silent run from the start that never changed is a still and stays;
    /// - any other at the end, or seeked past it, loops; one with no frame is an error.
    pub(super) fn receive(&mut self, now: Instant, changed: &mut bool) -> bool {
        if self.next.is_some() {
            self.waiting_since = None;
            return true;
        }
        let Some(decoder) = &self.decoder else {
            return false;
        };
        let received = decoder.frames.try_recv();
        let log = Arc::clone(&decoder.log);
        let mut log = log.lock();
        match received {
            Ok(frame) => {
                self.waiting_since = None;
                self.next = Some(frame);
                if self.duration.is_none() {
                    self.duration = log.duration;
                }
                let audio = log.streams_read.then_some(log.audio);
                drop(log);
                if let Some(audio) = audio {
                    self.learn_audio(audio, now);
                }
                true
            }
            Err(TryRecvError::Empty) => {
                drop(log);
                self.wait_for_frame(now, changed)
            }
            Err(TryRecvError::Disconnected) => {
                let decoded = decoder.progress.read.load(Ordering::Relaxed);
                // The frames can end before the log says why: wait for the rest of it.
                if !log.ended {
                    drop(log);
                    return self.wait_for_frame(now, changed);
                }
                self.waiting_since = None;
                *changed = true;
                let error = log.error.take();
                let input_failed = log.input_error;
                // A failure before the stream mapping is a link that never opened.
                let opened = log.streams_read;
                // A short still can end before any of its frames learned there is no sound.
                let audio = log.streams_read.then_some(log.audio);
                // Frames read but skipped behind the sound can end a run before one shows.
                let rate = log.rate.unwrap_or(self.rate);
                drop(log);
                if let Some(audio) = audio {
                    self.learn_audio(audio, now);
                }
                let end = self.start + self.shown.max(decoded) as f64 / rate;
                // Cut short: before the known end, or (no duration, as for a streamed GIF)
                // when the input failed, such as a dropped connection. Go on from there,
                // unless resuming there already failed.
                let cut = self.duration.map_or(input_failed, |d| end + 1.0 < d)
                    && !self.unresumable.is_some_and(|at| (at - end).abs() < 0.5);
                if self.shown > 0 && cut {
                    self.run_from(end, now);
                } else if self.shown == 0 && input_failed && opened && !self.retried {
                    // Cut before the first frame (for a resumed run, while decoding up to
                    // its start): once more from there.
                    self.retried = true;
                    self.run_from(self.start, now);
                } else if self.shown == 0 && self.start > 0.0 && input_failed {
                    self.unresumable = Some(self.start);
                    self.run_from(0.0, now);
                } else if self.start == 0.0
                    && self.shown > 0
                    && self.unchanged
                    && self.sources.as_ref().is_some_and(|s| s.audio.is_none())
                {
                    // A silent picture that never changed has nothing to loop: restarting
                    // it would refetch and decode the file over and over. A still GIF
                    // drawn in several zero-delay pieces changes, so it still loops.
                    self.decoder = None;
                    self.still = true;
                } else if self.shown > 0 || self.start > 0.0 {
                    self.run_from(0.0, now);
                } else {
                    self.decoder = None;
                    self.stop_audio();
                    self.error = Some(error.unwrap_or_else(|| "the stream has no video".into()));
                }
                false
            }
        }
    }

    pub(super) fn wait_for_frame(&mut self, now: Instant, changed: &mut bool) -> bool {
        if self.hidden {
            self.waiting_since = None;
            return false;
        }
        let waiting = self.waiting_since.get_or_insert(now);
        if now.saturating_duration_since(*waiting) >= FRAME_WAIT_TIMEOUT {
            self.decoder = None;
            self.stop_audio();
            self.waiting_since = None;
            self.error = Some("Video decoding timed out".into());
            *changed = true;
        }
        false
    }

    /// Learn from ffmpeg's log whether the video's own file has sound.
    pub(super) fn learn_audio(&mut self, has_audio: bool, now: Instant) {
        let Some(sources) = &mut self.sources else {
            return;
        };
        if !sources.audio_in_file {
            return;
        }
        sources.audio_in_file = false;
        if !has_audio {
            sources.audio = None;
            if self.sound {
                self.release_audio(now);
                self.sound = false;
                self.notice = Some(("This video has no sound".into(), false));
            }
        }
    }

    pub(super) fn show_next(&mut self) {
        if let Some(Decoded { index, frame, rate }) = self.next.take() {
            self.rate = rate;
            self.unchanged &= frame.same_as_first;
            self.retried = false;
            self.frame = Some(frame);
            self.shown = index + 1;
            self.frame_number += 1;
            // Played past where resuming failed: that was an outage, not the file's end.
            if self.unresumable.is_some_and(|at| self.due() > at + 0.5) {
                self.unresumable = None;
            }
        }
    }
}
