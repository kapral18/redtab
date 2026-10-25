//! Shared admission for authenticated HTTP; media has its own transport.
use parking_lot::{Condvar, Mutex};
use std::collections::HashSet;
use std::time::{Duration, Instant};
use url::Url;
use wreq::Response;

#[derive(Clone, Copy, Default)]
pub(crate) enum Priority {
    Background,
    #[default]
    Foreground,
    Listing,
    Selected,
}

const CLASSES: usize = Priority::Selected as usize + 1;
const INTERACTIVE_CAPACITY: f64 = 3.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pause {
    pub seconds: u64,
    pub cause: String,
}

struct Schedule {
    next: Instant,
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
    credited: Instant,
}

impl Budget {
    fn tokens(&self, now: Instant) -> f64 {
        let accrued = if self.interval.is_zero() {
            INTERACTIVE_CAPACITY
        } else {
            now.saturating_duration_since(self.credited).as_secs_f64() / self.interval.as_secs_f64()
        };
        (self.tokens + accrued)
            .min(INTERACTIVE_CAPACITY)
            .min(self.remaining)
    }

    fn ready(&self, now: Instant, base: Duration, rank: usize) -> Instant {
        let Some(until) = self.until else {
            return self.next;
        };
        if until <= now {
            return self.started + base;
        }
        if self.remaining < 1.0 {
            return until;
        }
        let ready = now + self.interval.mul_f64((1.0 - self.tokens(now)).max(0.0));
        if rank == Priority::Background as usize {
            ready.max(self.next).min(until)
        } else {
            ready.min(until)
        }
    }
}

// Home partials and JSON report independent budgets on the same origin.
fn budget_index(url: &Url) -> usize {
    usize::from(url.path() == super::home::PATH)
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
                next: now,
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
                    remaining: INTERACTIVE_CAPACITY,
                    tokens: INTERACTIVE_CAPACITY,
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
                        .map(move |(rank, _)| (budget, budget.ready(now, self.base, rank)))
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
                    budget.remaining = INTERACTIVE_CAPACITY;
                    budget.tokens = INTERACTIVE_CAPACITY;
                    budget.credited = now;
                }
            }
            let cooldown = state.cooldown.filter(|until| *until > now);
            if cooldown.is_some() && !wait_cooldown {
                break Err(anyhow::anyhow!(
                    "Reddit requests paused; wait before retrying"
                ));
            }
            let ready = state
                .next
                .max(state.budgets[index].ready(now, self.base, rank))
                .max(cooldown.unwrap_or(now));
            let eligible: [bool; CLASSES] = std::array::from_fn(|class| {
                state.budgets.iter().enumerate().any(|(index, budget)| {
                    budget.ready(now, self.base, class) <= now && state.waiting[index][class] > 0
                })
            });
            let next = (0..CLASSES).rev().find(|&class| {
                eligible[class] && (state.bursts[class] < 2 || !eligible[..class].contains(&true))
            });
            if now >= ready && next == Some(rank) {
                state.next = now + self.base;
                let budget = &mut state.budgets[index];
                if budget.until.is_some() {
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
                INTERACTIVE_CAPACITY
            } else {
                budget.tokens(now)
            };
            // Keep the admission's older reservations even if they completed while its hint was delayed.
            let newer = budget.sequence.saturating_sub(sequence);
            budget.remaining = (remaining - newer as f64 - older as f64).max(0.0);
            budget.tokens = tokens.min(budget.remaining);
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
        ["svc", "shreddit", ..] => "Home".into(),
        ["api", ..] => "account/action".into(),
        _ => "Reddit content".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quota(remaining: &str, reset: &str) -> (Rate, Url) {
        use crate::reddit::{
            tests::{Fixture, Reply},
            transport::{self, Transport},
        };
        let headers = vec![
            ("X-Ratelimit-Remaining".into(), remaining.to_owned()),
            ("X-Ratelimit-Reset".into(), reset.to_owned()),
        ];
        let fixture = Fixture::new(move |_| Reply {
            status: 200,
            body: String::new(),
            headers: headers.clone(),
        });
        let transport = Transport::new().unwrap();
        let response = transport::wait(transport.client.get(&fixture.base).send(), None)
            .unwrap()
            .unwrap();
        let rate = Rate::new(Duration::ZERO);
        let url = Url::parse(&fixture.base).unwrap();
        rate.observe(&response, &url, Instant::now(), 0, 0);
        (rate, url)
    }

    #[test]
    fn when_interactive_capacity_is_spent_further_navigation_should_wait() {
        let (rate, url) = quota("10", "30");
        rate.admit(&url, Priority::Listing, true, || false).unwrap();
        rate.admit(&url, Priority::Selected, true, || false)
            .unwrap();
        rate.admit(&url, Priority::Foreground, true, || false)
            .unwrap();
        let deadline = Instant::now() + Duration::from_millis(50);

        let actual = rate.admit(&url, Priority::Listing, true, || Instant::now() >= deadline);

        assert!(
            actual.is_err(),
            "quota hints must not refill the burst on every response"
        );
    }

    #[test]
    fn when_background_is_paced_it_should_leave_interactive_capacity_available() {
        let (rate, url) = quota("10", "30");
        let deadline = Instant::now() + Duration::from_millis(50);
        assert!(
            rate.admit(&url, Priority::Background, true, || Instant::now()
                >= deadline)
                .is_err()
        );

        let actual = rate.admit(&url, Priority::Listing, true, || false);

        assert!(actual.is_ok());
    }

    #[test]
    fn when_only_background_work_is_paced_it_should_not_mark_reddit_paused() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc::channel,
        };
        let (rate, url) = quota("10", "30");
        let rate = Arc::new(rate);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_rate = rate.clone();
        let worker_stop = stop.clone();
        let (entered, waiting) = channel();
        let worker = std::thread::spawn(move || {
            worker_rate
                .admit(&url, Priority::Background, true, || {
                    let _ = entered.send(());
                    worker_stop.load(Ordering::Acquire)
                })
                .map(|_| ())
        });
        waiting.recv_timeout(Duration::from_secs(1)).unwrap();

        let actual = rate.pause();
        stop.store(true, Ordering::Release);
        rate.wake();
        assert!(worker.join().unwrap().is_err());

        assert!(
            actual.is_none(),
            "background pacing must not announce a blocked connection"
        );
    }

    #[test]
    fn when_interactive_credit_is_available_starts_should_still_obey_base_spacing() {
        let (mut rate, url) = quota("10", "30");
        rate.base = Duration::from_millis(80);
        rate.admit(&url, Priority::Listing, true, || false).unwrap();
        let started = Instant::now();

        rate.admit(&url, Priority::Selected, true, || false)
            .unwrap();

        assert!(started.elapsed() >= Duration::from_millis(65));
    }

    #[test]
    fn when_an_older_request_finishes_before_a_delayed_newer_hint_its_reservation_should_survive() {
        use crate::reddit::{
            tests::{Fixture, Reply},
            transport::{self, Transport},
        };
        let fixture = Fixture::new(|request| Reply {
            status: 200,
            body: String::new(),
            headers: if request.url == "/no-quota" {
                vec![]
            } else {
                vec![
                    ("X-Ratelimit-Remaining".into(), "1".into()),
                    ("X-Ratelimit-Reset".into(), "30".into()),
                ]
            },
        });
        let transport = Transport::new().unwrap();
        let rate = Rate::new(Duration::ZERO);
        let url = Url::parse(&fixture.base).unwrap();
        let older = rate.admit(&url, Priority::Listing, true, || false).unwrap();
        let old_sent = Instant::now();
        let newer = rate
            .admit(&url, Priority::Selected, true, || false)
            .unwrap();
        let new_sent = Instant::now();
        let delayed = transport::wait(transport.client.get(&fixture.base).send(), None)
            .unwrap()
            .unwrap();
        let no_quota = transport::wait(
            transport
                .client
                .get(url.join("no-quota").unwrap().as_str())
                .send(),
            None,
        )
        .unwrap()
        .unwrap();
        rate.observe(&no_quota, &url, old_sent, older.sequence, older.older);
        drop(older);
        rate.observe(&delayed, &url, new_sent, newer.sequence, newer.older);
        let deadline = Instant::now() + Duration::from_millis(50);

        let actual = rate.admit(&url, Priority::Listing, true, || Instant::now() >= deadline);

        assert!(
            actual.is_err(),
            "an older completion cannot release a reservation belonging to a delayed hint"
        );
    }

    #[test]
    fn when_a_later_request_finishes_without_quota_its_debit_should_survive_an_older_hint() {
        use crate::reddit::{
            tests::{Fixture, Reply},
            transport::{self, Transport},
        };
        let fixture = Fixture::new(|request| Reply {
            status: 200,
            body: String::new(),
            headers: if request.url == "/no-quota" {
                vec![]
            } else {
                vec![
                    ("X-Ratelimit-Remaining".into(), "1".into()),
                    ("X-Ratelimit-Reset".into(), "30".into()),
                ]
            },
        });
        let transport = Transport::new().unwrap();
        let rate = Rate::new(Duration::ZERO);
        let url = Url::parse(&fixture.base).unwrap();
        let older = rate.admit(&url, Priority::Listing, true, || false).unwrap();
        let sent = Instant::now();
        let old_response = transport::wait(transport.client.get(&fixture.base).send(), None)
            .unwrap()
            .unwrap();
        let later = rate
            .admit(&url, Priority::Selected, true, || false)
            .unwrap();
        let no_quota = transport::wait(
            transport
                .client
                .get(url.join("no-quota").unwrap().as_str())
                .send(),
            None,
        )
        .unwrap()
        .unwrap();
        rate.observe(&no_quota, &url, Instant::now(), later.sequence, later.older);
        drop(later);
        rate.observe(&old_response, &url, sent, older.sequence, older.older);
        let deadline = Instant::now() + Duration::from_millis(50);

        let actual = rate.admit(&url, Priority::Listing, true, || Instant::now() >= deadline);

        assert!(
            actual.is_err(),
            "the completed later request already consumed the older hint's last quota slot"
        );
    }

    #[test]
    fn when_an_older_request_is_pending_a_newer_hint_should_reserve_its_quota() {
        use crate::reddit::{
            tests::{Fixture, Reply},
            transport::{self, Transport},
        };
        let fixture = Fixture::new(|_| Reply {
            status: 200,
            body: String::new(),
            headers: vec![
                ("X-Ratelimit-Remaining".into(), "1".into()),
                ("X-Ratelimit-Reset".into(), "30".into()),
            ],
        });
        let transport = Transport::new().unwrap();
        let rate = Rate::new(Duration::ZERO);
        let url = Url::parse(&fixture.base).unwrap();
        let _older = rate.admit(&url, Priority::Listing, true, || false).unwrap();
        let newer = rate
            .admit(&url, Priority::Selected, true, || false)
            .unwrap();
        let response = transport::wait(transport.client.get(&fixture.base).send(), None)
            .unwrap()
            .unwrap();
        rate.observe(&response, &url, Instant::now(), newer.sequence, newer.older);
        let deadline = Instant::now() + Duration::from_millis(50);

        let actual = rate.admit(&url, Priority::Listing, true, || Instant::now() >= deadline);

        assert!(
            actual.is_err(),
            "the older request can still consume the response's last quota slot"
        );
    }

    #[test]
    fn when_later_starts_are_not_in_a_quota_hint_they_should_be_deducted() {
        use crate::reddit::{
            tests::{Fixture, Reply},
            transport::{self, Transport},
        };
        let fixture = Fixture::new(|_| Reply {
            status: 200,
            body: String::new(),
            headers: vec![
                ("X-Ratelimit-Remaining".into(), "1".into()),
                ("X-Ratelimit-Reset".into(), "30".into()),
            ],
        });
        let transport = Transport::new().unwrap();
        let rate = Rate::new(Duration::ZERO);
        let url = Url::parse(&fixture.base).unwrap();
        let first = rate.admit(&url, Priority::Listing, true, || false).unwrap();
        let sent = Instant::now();
        let _second = rate
            .admit(&url, Priority::Selected, true, || false)
            .unwrap();
        let response = transport::wait(transport.client.get(&fixture.base).send(), None)
            .unwrap()
            .unwrap();
        rate.observe(&response, &url, sent, first.sequence, first.older);
        let deadline = Instant::now() + Duration::from_millis(50);

        let actual = rate.admit(&url, Priority::Listing, true, || Instant::now() >= deadline);

        assert!(
            actual.is_err(),
            "the second start already spent the response's remaining request"
        );
    }

    #[test]
    fn when_quota_is_smaller_than_the_burst_it_should_limit_total_admissions() {
        let (rate, url) = quota("1", "30");
        rate.admit(&url, Priority::Listing, true, || false).unwrap();
        let deadline = Instant::now() + Duration::from_millis(50);

        let actual = rate.admit(&url, Priority::Selected, true, || {
            Instant::now() >= deadline
        });

        assert!(
            actual.is_err(),
            "an unacknowledged start already spent the last known request"
        );
    }

    #[test]
    fn when_burst_credit_recovers_background_should_make_progress() {
        let (rate, url) = quota("10", "0.2");
        rate.admit(&url, Priority::Listing, true, || false).unwrap();
        rate.admit(&url, Priority::Listing, true, || false).unwrap();
        rate.admit(&url, Priority::Listing, true, || false).unwrap();
        let deadline = Instant::now() + Duration::from_millis(150);

        let actual = rate.admit(&url, Priority::Background, true, || {
            Instant::now() >= deadline
        });

        assert!(
            actual.is_ok(),
            "paced traffic must resume after one quota interval"
        );
    }

    #[test]
    fn when_quota_is_replenished_continuous_navigation_should_not_postpone_background() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
            mpsc::channel,
        };
        let (rate, url) = quota("100", "3");
        let rate = Arc::new(rate);
        let starts = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let producer_rate = rate.clone();
        let producer_url = url.clone();
        let producer_starts = starts.clone();
        let producer_stop = stop.clone();
        let (entered, waiting) = channel();
        let producer = std::thread::spawn(move || {
            for _ in 0..20 {
                if producer_rate
                    .admit(&producer_url, Priority::Selected, true, || {
                        let _ = entered.send(());
                        producer_stop.load(Ordering::Acquire)
                    })
                    .is_err()
                {
                    break;
                }
                producer_starts.fetch_add(1, Ordering::SeqCst);
            }
        });
        waiting.recv_timeout(Duration::from_secs(1)).unwrap();
        let deadline = Instant::now() + Duration::from_millis(250);

        let actual = rate.admit(&url, Priority::Background, true, || {
            Instant::now() >= deadline
        });
        let before = starts.load(Ordering::SeqCst);
        stop.store(true, Ordering::Release);
        rate.wake();
        producer.join().unwrap();

        assert!(
            actual.is_ok(),
            "navigation repeatedly postponed the background slot"
        );
        assert!(
            before <= 5,
            "background waited for {before} selected starts"
        );
    }

    #[test]
    fn when_budget_replies_arrive_out_of_order_the_older_hint_should_not_restore_slow_pacing() {
        use crate::reddit::{
            tests::{Fixture, Reply},
            transport::{self, Transport},
        };
        let fixture = Fixture::new(|request| Reply {
            status: 200,
            body: String::new(),
            headers: vec![
                (
                    "X-Ratelimit-Remaining".into(),
                    if request.url == "/old" { "2" } else { "100" }.into(),
                ),
                ("X-Ratelimit-Reset".into(), "1".into()),
            ],
        });
        let transport = Transport::new().unwrap();
        let response = |path| {
            transport::wait(
                transport
                    .client
                    .get(format!("{}{path}", fixture.base))
                    .send(),
                None,
            )
            .unwrap()
            .unwrap()
        };
        let old = response("old");
        let new = response("new");
        let rate = Rate::new(Duration::ZERO);
        let url = Url::parse(&fixture.base).unwrap();
        let home = url.join(super::super::home::PATH).unwrap();
        let now = Instant::now();
        rate.observe(&old, &url, now - Duration::from_secs(3), 0, 0);
        rate.observe(&old, &home, now, 0, 0);
        rate.observe(&new, &url, now - Duration::from_secs(1), 0, 0);

        rate.observe(&old, &url, now - Duration::from_secs(2), 0, 0);

        assert_eq!(
            rate.state.lock().budgets[0].interval,
            Duration::from_millis(10)
        );
    }

    #[test]
    fn when_selected_reads_keep_arriving_other_classes_should_still_make_progress() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
            mpsc::channel,
        };
        let url = Url::parse("https://www.reddit.com/hot.json").unwrap();
        let rate = Arc::new(Rate::new(Duration::from_millis(30)));
        let selected_starts = Arc::new(AtomicUsize::new(0));
        rate.state.lock().next = Instant::now() + Duration::from_secs(5);
        let (entered, waiting) = channel();
        let (done, results) = channel();
        let mut threads = Vec::new();
        for (priority, name) in [
            (Priority::Foreground, "feed"),
            (Priority::Background, "background"),
        ] {
            let rate = rate.clone();
            let url = url.clone();
            let selected_starts = selected_starts.clone();
            let entered = entered.clone();
            let done = done.clone();
            threads.push(std::thread::spawn(move || {
                let first = std::sync::atomic::AtomicBool::new(true);
                rate.admit(&url, priority, true, || {
                    if first.swap(false, std::sync::atomic::Ordering::Relaxed) {
                        entered.send(()).unwrap();
                    }
                    false
                })
                .unwrap();
                done.send((name, selected_starts.load(Ordering::SeqCst)))
                    .unwrap();
            }));
            waiting.recv_timeout(Duration::from_secs(1)).unwrap();
        }
        let selected = rate.clone();
        let starts = selected_starts.clone();
        let producer = std::thread::spawn(move || {
            for _ in 0..24 {
                selected
                    .admit(&url, Priority::Selected, true, || false)
                    .unwrap();
                starts.fetch_add(1, Ordering::SeqCst);
            }
        });
        let deadline = Instant::now() + Duration::from_secs(1);
        while rate.state.lock().waiting[0][Priority::Selected as usize] == 0 {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        rate.state.lock().next = Instant::now();
        rate.wake();
        let first = results.recv_timeout(Duration::from_secs(1));
        let second = results.recv_timeout(Duration::from_secs(1));
        producer.join().unwrap();
        for thread in threads {
            thread.join().unwrap();
        }
        let (first, before_feed) = first.unwrap();
        let (second, before_background) = second.unwrap();
        assert_eq!(first, "feed");
        assert_eq!(second, "background");
        assert!(
            before_feed <= 2,
            "feed waited for {before_feed} selected starts"
        );
        assert!(
            before_background <= 4,
            "background waited for {before_background} selected starts"
        );
    }

    #[test]
    fn when_selected_and_feed_reads_are_waiting_selected_should_get_the_next_paced_slot() {
        use std::sync::{Arc, mpsc::channel};
        let url = Url::parse("https://www.reddit.com/hot.json").unwrap();
        let rate = Arc::new(Rate::new(Duration::from_millis(80)));
        rate.state.lock().next = Instant::now() + Duration::from_secs(5);
        let (entered, waiting) = channel();
        let (finished, results) = channel();
        let mut threads = Vec::new();
        for (priority, name) in [
            (Priority::Foreground, "feed"),
            (Priority::Selected, "selected"),
        ] {
            let rate = Arc::clone(&rate);
            let url = url.clone();
            let entered = entered.clone();
            let finished = finished.clone();
            threads.push(std::thread::spawn(move || {
                let first = std::sync::atomic::AtomicBool::new(true);
                rate.admit(&url, priority, true, || {
                    if first.swap(false, std::sync::atomic::Ordering::Relaxed) {
                        entered.send(()).unwrap();
                    }
                    false
                })
                .unwrap();
                finished.send((name, Instant::now())).unwrap();
            }));
            waiting.recv_timeout(Duration::from_secs(1)).unwrap();
        }
        rate.state.lock().next = Instant::now();
        rate.wake();
        let first = results.recv_timeout(Duration::from_secs(1)).unwrap();
        let second = results.recv_timeout(Duration::from_secs(1)).unwrap();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(first.0, "selected");
        assert_eq!(second.0, "feed");
        assert!(second.1.duration_since(first.1) >= Duration::from_millis(65));
    }

    #[test]
    fn when_callers_share_admission_they_should_start_apart_and_cancel_without_consuming_a_slot() {
        let rate = Rate::new(Duration::from_millis(80));
        let home = Url::parse("https://www.reddit.com/svc/shreddit/feeds/home-feed").unwrap();
        let json = Url::parse("https://www.reddit.com/hot.json").unwrap();
        rate.admit(&home, Priority::Foreground, true, || false)
            .unwrap();
        assert!(
            rate.admit(&json, Priority::Foreground, true, || true)
                .is_err()
        );
        let start = Instant::now();
        rate.admit(&json, Priority::Background, true, || false)
            .unwrap();
        assert!(start.elapsed() >= Duration::from_millis(65));
    }

    #[test]
    fn when_a_quota_blocks_one_pool_ready_work_should_still_use_shared_admission() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc::channel,
        };
        for (blocked, ready, burst) in [
            (Priority::Selected, Priority::Listing, 0),
            (Priority::Foreground, Priority::Selected, 2),
        ] {
            let rate = Arc::new(Rate::new(Duration::ZERO));
            let home = Url::parse("https://www.reddit.com/svc/shreddit/feeds/home-feed").unwrap();
            let json = Url::parse("https://www.reddit.com/hot.json").unwrap();
            {
                let mut state = rate.state.lock();
                state.budgets[0].next = Instant::now() + Duration::from_secs(5);
                state.bursts[ready as usize] = burst;
            }
            let cancelled = Arc::new(AtomicBool::new(false));
            let stop = cancelled.clone();
            let worker_rate = rate.clone();
            let (entered, waiting) = channel();
            let worker = std::thread::spawn(move || {
                worker_rate
                    .admit(&json, blocked, true, || {
                        let _ = entered.send(());
                        stop.load(Ordering::Acquire)
                    })
                    .map(|_| ())
            });
            waiting.recv_timeout(Duration::from_secs(1)).unwrap();
            let deadline = Instant::now() + Duration::from_millis(500);

            let actual = rate.admit(&home, ready, true, || Instant::now() >= deadline);

            cancelled.store(true, Ordering::Release);
            rate.wake();
            assert!(worker.join().unwrap().is_err());
            assert!(
                actual.is_ok(),
                "ready Home work was blocked by a JSON waiter"
            );
        }
    }

    #[test]
    fn when_routes_contain_private_data_diagnostics_should_only_name_the_operation() {
        for (url, expected) in [
            (
                "https://www.reddit.com/search.json?q=SECRET",
                "Reddit content",
            ),
            (
                "https://www.reddit.com/comments/abc/SECRET?token=SECRET",
                "post/comments",
            ),
            ("https://www.reddit.com/user/SECRET.json", "Reddit content"),
            (
                "https://www.reddit.com/r/rust/about.json?token=SECRET",
                "r/rust info",
            ),
        ] {
            assert_eq!(operation(&Url::parse(url).unwrap()), expected);
        }
    }
}
