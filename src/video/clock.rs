use super::*;

/// Media time against wall time.
#[derive(Debug, Clone, Copy)]
pub(super) struct Clock {
    /// Media time at `anchor`.
    pub(super) at: f64,
    pub(super) anchor: Instant,
    pub(super) running: bool,
}

impl Clock {
    pub(super) fn new(at: f64, now: Instant) -> Self {
        Self {
            at,
            anchor: now,
            running: true,
        }
    }

    pub(super) fn position(&self, now: Instant) -> f64 {
        if self.running {
            self.at + now.saturating_duration_since(self.anchor).as_secs_f64()
        } else {
            self.at
        }
    }

    pub(super) fn set(&mut self, at: f64, now: Instant) {
        self.at = at;
        self.anchor = now;
    }

    pub(super) fn stop(&mut self, now: Instant) {
        self.set(self.position(now), now);
        self.running = false;
    }

    pub(super) fn start(&mut self, now: Instant) {
        self.anchor = now;
        self.running = true;
    }

    /// When media time `at` comes, if the clock runs.
    pub(super) fn when(&self, at: f64, now: Instant) -> Option<Instant> {
        let ahead = at - self.position(now);
        self.running
            .then(|| now + Duration::from_secs_f64(ahead.max(0.0)))
    }
}

/// `m:ss` for a time in seconds.
pub fn clock_text(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}
