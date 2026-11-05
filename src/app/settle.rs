use super::*;

/// Holds navigation loads until moves pause. `arrive` permits the first target
/// immediately; `defer` always waits for the pause.
#[derive(Debug, Default)]
pub(super) struct Settle<T = u64> {
    /// The target held back (a post view's generation, a tab id, or `()` for what is on
    /// screen) and when it loads.
    pub(super) waiting: Option<(T, Instant)>,
    pub(super) last_move: Option<Instant>,
}

impl<T: Copy + PartialEq> Settle<T> {
    /// A move to `target`: true when it loads now, else it waits for the moves to pause.
    pub(super) fn arrive(&mut self, target: T, now: Instant) -> bool {
        let quick = self
            .last_move
            .is_some_and(|last| now.saturating_duration_since(last) < NAV_SETTLE);
        self.last_move = Some(now);
        self.waiting = quick.then(|| (target, now + NAV_SETTLE));
        !quick
    }

    pub(super) fn defer(&mut self, target: T, now: Instant) {
        self.last_move = Some(now);
        self.waiting = Some((target, now + NAV_SETTLE));
    }

    /// The held-back target whose wait is over by `now`.
    pub(super) fn due(&mut self, now: Instant) -> Option<T> {
        if self.waiting.is_some_and(|(_, at)| at <= now) {
            self.take()
        } else {
            None
        }
    }

    /// The held-back target, its wait cut short.
    pub(super) fn take(&mut self) -> Option<T> {
        self.waiting.take().map(|(target, _)| target)
    }

    /// Drop the held-back load of `target`: something else just loaded it.
    pub(super) fn forget(&mut self, target: T) {
        if self.waiting.is_some_and(|(waiting, _)| waiting == target) {
            self.waiting = None;
        }
    }

    /// Whether a quick run of moves still holds its load back at `now`.
    pub(super) fn holding(&self, now: Instant) -> bool {
        self.waiting.is_some_and(|(_, at)| at > now)
    }

    pub(super) fn wake_in(&self, now: Instant) -> Option<Duration> {
        self.waiting
            .map(|(_, at)| at.saturating_duration_since(now))
    }
}
