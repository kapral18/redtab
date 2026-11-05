use super::*;

/// Where a snapshot came from, for ordering.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Source {
    pub order: Option<RequestOrder>,
    /// Local event window: a disk read interval, or an HTTP start.
    #[serde(skip)]
    pub observed: Option<(u64, u64)>,
    pub cached: bool,
}

impl Source {
    pub fn compare(self, other: Self) -> Option<std::cmp::Ordering> {
        self.compare_requests(other).or_else(|| {
            // Direct in-memory messages carry no timing.
            (self.observed.is_none() || other.observed.is_none())
                .then(|| (!self.cached).cmp(&!other.cached))
        })
    }

    /// Unordered observations cannot displace content already shown.
    pub fn may_replace(self, other: Self) -> bool {
        self.compare(other).is_some_and(|order| !order.is_lt())
    }

    pub fn compare_requests(self, other: Self) -> Option<std::cmp::Ordering> {
        use std::cmp::Ordering::*;
        if let (Some(a), Some(b)) = (self.order, other.order)
            && a.generation == b.generation
        {
            return Some(a.sequence.cmp(&b.sequence));
        }
        if !self.cached
            && !other.cached
            && let (Some(a), Some(b)) = (self.observed, other.observed)
        {
            return Some(a.0.cmp(&b.0));
        }
        if self.cached != other.cached {
            let (disk, live) = if self.cached {
                (self, other)
            } else {
                (other, self)
            };
            // Reservations may come from retired generations; a later read doesn't prove a
            // later fetch.
            return (disk.observed?.1 < live.observed?.0).then_some(if self.cached {
                Less
            } else {
                Greater
            });
        }
        let (a, b) = self.observed.zip(other.observed)?;
        if a.1 < b.0 {
            Some(Less)
        } else if b.1 < a.0 {
            Some(Greater)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Freshness {
    pub fetched_at: i64,
    pub source: Source,
}

impl Freshness {
    pub fn label(self, now: i64, loading: bool, failed: bool, offline: bool) -> String {
        let age = now.saturating_sub(self.fetched_at).max(0);
        let age = match age {
            0..60 => "just now".into(),
            60..3600 => format!("{}m ago", age / 60),
            3600..86400 => format!("{}h ago", age / 3600),
            _ => format!("{}d ago", age / 86400),
        };
        let state = if offline {
            "offline"
        } else if failed {
            "refresh failed"
        } else if loading {
            "refreshing"
        } else if now.saturating_sub(self.fetched_at) >= CONTENT_TTL {
            "stale"
        } else if self.source.cached {
            "cached"
        } else {
            "updated"
        };
        format!("{state} · {age}")
    }
}

pub struct Entry {
    pub fetched_at: i64,
    pub order: RequestOrder,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RequestOrder {
    pub generation: [u8; 16],
    pub sequence: u64,
}

impl RequestOrder {
    pub(super) fn from_header(header: &[u8; HEADER as usize]) -> Option<Self> {
        Some(Self {
            sequence: u64::from_le_bytes(header[16..24].try_into().ok()?),
            generation: header[24..HEADER_FIELDS].try_into().ok()?,
        })
    }
}
