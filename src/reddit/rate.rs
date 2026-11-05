//! Shared admission for authenticated HTTP; media has its own transport.
use parking_lot::{Condvar, Mutex};
use std::collections::HashSet;
use std::time::{Duration, Instant};
use url::Url;
use wreq::Response;

#[derive(Clone, Copy, Default)]
pub(crate) enum Priority {
    Background,
    Browsing,
    #[default]
    Foreground,
    Listing,
    Selected,
}

const CLASSES: usize = Priority::Selected as usize + 1;
const INTERACTIVE_RESERVE: f64 = 3.0;
const BACKGROUND_BURST: f64 = 16.0;
const MAX_START_SPACING: Duration = Duration::from_millis(500);
const KNOWN_START_SPACING: Duration = Duration::from_millis(100);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pause {
    pub seconds: u64,
    pub cause: String,
}

struct Schedule {
    last_start: Option<Instant>,
    last_background_start: Option<Instant>,
    cooldown: Option<Instant>,
    cause: String,
    waiting: [[usize; CLASSES]; 2],
    bursts: [u8; CLASSES],
    budgets: [Budget; 2],
}

struct Budget {
    started: Instant,
    next: Instant,
    interval: Duration,
    until: Option<Instant>,
    request: Option<Instant>,
    cause: String,
    sequence: u64,
    outstanding: HashSet<u64>,
    remaining: f64,
    tokens: f64,
    background_tokens: f64,
    credited: Instant,
}

impl Budget {
    fn tokens(&self, now: Instant) -> f64 {
        (self.tokens + self.accrued(now)).min(self.remaining)
    }

    fn background_tokens(&self, now: Instant) -> f64 {
        (self.background_tokens + self.accrued(now))
            .min(BACKGROUND_BURST)
            .min(self.remaining)
    }

    fn accrued(&self, now: Instant) -> f64 {
        if self.interval.is_zero() {
            self.remaining
        } else {
            now.saturating_duration_since(self.credited).as_secs_f64() / self.interval.as_secs_f64()
        }
    }

    fn ready(&self, now: Instant, base: Duration, rank: usize, background_due: bool) -> Instant {
        let Some(until) = self.until else {
            return self.next;
        };
        if until <= now {
            return self.started + base;
        }
        let browsing = rank <= Priority::Browsing as usize;
        let needed = if browsing {
            INTERACTIVE_RESERVE + 1.0
        } else {
            1.0
        };
        if self.remaining < needed {
            return until;
        }
        let missing = (needed - self.tokens(now)).max(0.0);
        let missing = if rank == Priority::Background as usize {
            missing.max(1.0 - self.background_tokens(now))
        } else {
            missing
        };
        let ready = now + self.interval.mul_f64(missing);
        if rank == Priority::Background as usize && !background_due {
            ready.max(self.next).min(until)
        } else {
            ready.min(until)
        }
    }
}

// Native HTML/GraphQL and classic JSON report independent quotas on the same origin.
fn budget_index(url: &Url) -> usize {
    usize::from(
        matches!(
            url.path(),
            "/settings/preferences" | "/svc/shreddit/graphql"
        ) || url.path() == super::home::PATH,
    )
}

pub(super) struct Rate {
    state: Mutex<Schedule>,
    changed: Condvar,
    base: Duration,
}

pub(super) struct Admission<'a> {
    rate: &'a Rate,
    index: usize,
    pub sequence: u64,
    pub older: usize,
}

impl Drop for Admission<'_> {
    fn drop(&mut self) {
        self.rate.state.lock().budgets[self.index]
            .outstanding
            .remove(&self.sequence);
    }
}

impl Rate {
    pub fn new(base: Duration) -> Self {
        let now = Instant::now();
        Self {
            state: Mutex::new(Schedule {
                last_start: None,
                last_background_start: None,
                cooldown: None,
                cause: String::new(),
                waiting: [[0; CLASSES]; 2],
                bursts: [0; CLASSES],
                budgets: std::array::from_fn(|_| Budget {
                    started: now,
                    next: now,
                    interval: base,
                    until: None,
                    request: None,
                    cause: String::new(),
                    sequence: 0,
                    outstanding: HashSet::new(),
                    remaining: 0.0,
                    tokens: 0.0,
                    background_tokens: 0.0,
                    credited: now,
                }),
            }),
            changed: Condvar::new(),
            base,
        }
    }

    pub fn pause(&self) -> Option<Pause> {
        let state = self.state.lock();
        let now = Instant::now();
        let (until, cause) = if let Some(until) = state.cooldown.filter(|until| *until > now) {
            (until, state.cause.clone())
        } else {
            let budget = state
                .budgets
                .iter()
                .zip(&state.waiting)
                .flat_map(|(budget, waiting)| {
                    waiting
                        .iter()
                        .enumerate()
                        .filter(|(rank, count)| {
                            *rank > Priority::Background as usize && **count > 0
                        })
                        .map(move |(rank, _)| (budget, budget.ready(now, self.base, rank, false)))
                })
                .min_by_key(|(_, ready)| *ready)?;
            let (budget, ready) = budget;
            if budget.interval <= self.base
                || budget.interval < Duration::from_secs(1)
                || budget.until.is_none_or(|until| until <= now)
            {
                return None;
            }
            (ready, budget.cause.clone())
        };
        let left = until.saturating_duration_since(now);
        (!left.is_zero()).then(|| Pause {
            seconds: left.as_secs() + u64::from(left.subsec_nanos() != 0),
            cause,
        })
    }

    pub fn wake(&self) {
        self.changed.notify_all();
    }

    /// Prefer selected work, yielding after two starts while a lower class waits.
    pub fn admit(
        &self,
        url: &Url,
        priority: Priority,
        wait_cooldown: bool,
        cancelled: impl Fn() -> bool,
    ) -> anyhow::Result<Admission<'_>> {
        let mut state = self.state.lock();
        let index = budget_index(url);
        let rank = priority as usize;
        state.waiting[index][rank] += 1;
        let result = loop {
            if cancelled() {
                break Err(anyhow::anyhow!(super::REQUEST_CANCELLED));
            }
            let now = Instant::now();
            for budget in &mut state.budgets {
                if budget.until.is_some_and(|until| now >= until) {
                    budget.interval = self.base;
                    budget.until = None;
                    budget.next = budget.started + self.base;
                    budget.remaining = 0.0;
                    budget.tokens = 0.0;
                    budget.background_tokens = 0.0;
                    budget.credited = now;
                }
            }
            let cooldown = state.cooldown.filter(|until| *until > now);
            if cooldown.is_some() && !wait_cooldown {
                break Err(anyhow::anyhow!(
                    "Reddit requests paused; wait before retrying"
                ));
            }
            let background_ready = state
                .last_background_start
                .map_or(now, |started| started + self.base.min(MAX_START_SPACING));
            let mut eligible: [bool; CLASSES] = std::array::from_fn(|class| {
                class > Priority::Background as usize
                    && state.budgets.iter().enumerate().any(|(index, budget)| {
                        budget.ready(now, self.base, class, false) <= now
                            && state.waiting[index][class] > 0
                    })
            });
            let background_due = eligible
                .iter()
                .enumerate()
                .skip(1)
                .any(|(class, ready)| *ready && state.bursts[class] >= 2);
            eligible[Priority::Background as usize] = background_ready <= now
                && state.budgets.iter().enumerate().any(|(index, budget)| {
                    budget.ready(
                        now,
                        self.base,
                        Priority::Background as usize,
                        background_due,
                    ) <= now
                        && state.waiting[index][Priority::Background as usize] > 0
                });
            let spacing = if state.budgets[index].until.is_some() {
                KNOWN_START_SPACING
            } else {
                MAX_START_SPACING
            };
            let ready = state
                .last_start
                .map_or(now, |started| started + self.base.min(spacing))
                .max(state.budgets[index].ready(now, self.base, rank, background_due))
                .max(if rank == Priority::Background as usize {
                    background_ready
                } else {
                    now
                })
                .max(cooldown.unwrap_or(now));
            let next = (0..CLASSES).rev().find(|&class| {
                eligible[class] && (state.bursts[class] < 2 || !eligible[..class].contains(&true))
            });
            if now >= ready && next == Some(rank) {
                state.last_start = Some(now);
                if rank == Priority::Background as usize {
                    state.last_background_start = Some(now);
                }
                let budget = &mut state.budgets[index];
                if budget.until.is_some() {
                    budget.background_tokens = (budget.background_tokens(now)
                        - if rank <= Priority::Browsing as usize {
                            1.0
                        } else {
                            0.0
                        })
                    .max(0.0);
                    budget.tokens = (budget.tokens(now) - 1.0).max(0.0);
                    budget.remaining = (budget.remaining - 1.0).max(0.0);
                    budget.credited = now;
                }
                budget.sequence += 1;
                let sequence = budget.sequence;
                let older = budget.outstanding.len();
                budget.outstanding.insert(sequence);
                budget.started = now;
                budget.next = now + budget.interval;
                state.bursts[rank] = (state.bursts[rank] + 1).min(2);
                state.bursts[rank + 1..].fill(0);
                break Ok(Admission {
                    rate: self,
                    index,
                    sequence,
                    older,
                });
            }
            let wait = ready
                .saturating_duration_since(now)
                .max(Duration::from_millis(10))
                .min(super::transport::CANCEL_POLL);
            self.changed.wait_for(&mut state, wait);
        };
        state.waiting[index][rank] -= 1;
        self.changed.notify_all();
        result
    }

    pub fn observe(
        &self,
        response: &Response,
        url: &Url,
        sent: Instant,
        sequence: u64,
        older: usize,
    ) {
        let number = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|n| n.is_finite())
        };
        let remaining = number("x-ratelimit-remaining");
        let reset = number("x-ratelimit-reset").filter(|n| *n >= 0.0);
        let status = response.status().as_u16();
        let mut state = self.state.lock();
        let now = Instant::now();
        let budget = &mut state.budgets[budget_index(url)];
        budget.outstanding.remove(&sequence);
        let current = budget.request.is_none_or(|previous| sent >= previous);
        if current && remaining.is_some() && reset.is_some() {
            budget.request = Some(sent);
        }
        if status == 429 || (current && remaining.is_some_and(|n| n < 1.0)) {
            let date_wait = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| chrono::DateTime::parse_from_rfc2822(v).ok())
                .map(|at| (at.timestamp() - chrono::Utc::now().timestamp()).max(1) as f64);
            let seconds = number("retry-after")
                .filter(|n| *n >= 0.0)
                .or(date_wait)
                .or(reset)
                .unwrap_or(60.0)
                .clamp(1.0, 86400.0);
            let until = now + Duration::from_secs_f64(seconds);
            if state.cooldown.is_none_or(|old| until >= old) {
                state.cooldown = Some(until);
                state.cause = format!("{} · HTTP {status}", operation(url));
                if let Some(remaining) = remaining {
                    state.cause.push_str(&format!(" · quota {remaining:.1}"));
                }
            }
        }
        if let (Some(remaining), Some(reset)) = (remaining, reset)
            && remaining >= 1.0
            && current
        {
            let budget = &mut state.budgets[budget_index(url)];
            let tokens = if budget.until.is_none_or(|until| until <= now) {
                remaining
            } else {
                budget.tokens(now)
            };
            let background_tokens = if budget.until.is_none_or(|until| until <= now) {
                BACKGROUND_BURST
            } else {
                budget.background_tokens(now)
            };
            // Keep the admission's older reservations even if they completed while its hint was delayed.
            let newer = budget.sequence.saturating_sub(sequence);
            budget.remaining = (remaining - newer as f64 - older as f64).max(0.0);
            budget.tokens = tokens.min(budget.remaining);
            budget.background_tokens = background_tokens.min(budget.remaining);
            budget.credited = now;
            budget.until = Some(now + Duration::from_secs_f64(reset.clamp(0.0, 86400.0)));
            budget.interval = self.base.max(Duration::from_secs_f64(
                (reset / remaining).clamp(0.0, 86400.0),
            ));
            budget.next = budget.started.max(sent) + budget.interval;
            budget.cause = format!("quota pacing · {} · quota {remaining:.1}", operation(url));
        }
        self.changed.notify_all();
    }
}

/// Labels contain no query, title, username, submitted text or credentials.
fn operation(url: &Url) -> String {
    let parts: Vec<_> = url.path().split('/').filter(|p| !p.is_empty()).collect();
    match parts.as_slice() {
        ["r", name, rest @ ..] if crate::config::is_subreddit_name(name) => {
            format!(
                "r/{name} {}",
                if rest.first() == Some(&"about.json") {
                    "info"
                } else {
                    "listing"
                }
            )
        }
        ["subreddits", "mine", ..] => "subscriptions".into(),
        ["comments", ..] => "post/comments".into(),
        ["by_id", ..] => "post details".into(),
        ["settings", "preferences"] | ["svc", "shreddit", "graphql"] => "account settings".into(),
        ["svc", "shreddit", ..] => "Home".into(),
        ["api", ..] => "account/action".into(),
        _ => "Reddit content".into(),
    }
}

#[cfg(test)]
mod tests;
