//! Disposable, versioned disk cache; only workers touch it.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::{Mutex, MutexGuard};
use sha2::{Digest, Sha256};

mod prune;
mod source;
mod write;

pub use source::*;

pub const CONTENT_TTL: i64 = 5 * 60;
pub const IMAGE_TTL: i64 = 7 * 86_400;
const RETENTION: i64 = 30 * 86_400;
const MAGIC: &[u8; 8] = b"REDTAB05";
const HEADER_FIELDS: usize = 40;
const HEADER: u64 = (HEADER_FIELDS + 32) as u64;
const COUNTER: &str = "sequence";
const ENTRY_LIMIT: u64 = 32 * 1024 * 1024;
const BYTE_LIMIT: u64 = 256 * 1024 * 1024;
const FILE_LIMIT: usize = 4096;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
/// One wait window shared by local and cross-process lock contention.
const LOCK_WAIT: Duration = Duration::from_millis(100);
/// Seconds between directory scans while the shared tally stays within both limits.
const SCAN_INTERVAL: i64 = 60;
/// Every redtab writing this directory updates this tally under `cache.lock`.
const TALLY: &str = "tally";

fn valid_header(header: &[u8; HEADER as usize]) -> bool {
    &header[..8] == MAGIC && header[HEADER_FIELDS..] == Sha256::digest(&header[..HEADER_FIELDS])[..]
}

pub struct Cache {
    root: PathBuf,
    byte_limit: u64,
    writers: Mutex<()>,
}

impl Cache {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            byte_limit: BYTE_LIMIT,
            writers: Mutex::new(()),
        }
    }

    pub fn key(namespace: &str, request: &str) -> String {
        let mut hash = Sha256::new();
        hash.update(namespace.as_bytes());
        hash.update([0]);
        hash.update(request.as_bytes());
        format!("{:x}", hash.finalize())
    }

    fn path(&self, key: &str) -> PathBuf {
        self.root.join(format!("{key}.cache"))
    }

    pub fn load(&self, key: &str, now: i64) -> Option<Entry> {
        let mut file = self.open_entry(key)?;
        let size = file.metadata().ok()?.len();
        if !(HEADER..=ENTRY_LIMIT).contains(&size) {
            return None;
        }
        let header = Self::read_header(&mut file)?;
        let fetched_at = i64::from_le_bytes(header[8..16].try_into().ok()?);
        if fetched_at > now || now.saturating_sub(fetched_at) > RETENTION {
            return None;
        }
        let order = RequestOrder::from_header(&header)?;
        let current = self.latest_request(COUNTER)?;
        if current.generation != order.generation
            || !(1..=current.sequence).contains(&order.sequence)
        {
            return None;
        }
        let mut bytes = Vec::with_capacity((size - HEADER) as usize);
        file.take(ENTRY_LIMIT).read_to_end(&mut bytes).ok()?;
        if bytes.len() as u64 != size - HEADER {
            return None;
        }
        Some(Entry {
            fetched_at,
            order,
            bytes,
        })
    }

    /// Mark an entry used at `now`: pruning removes the oldest modification time first, so
    /// a touched entry outlives ones written before `now`.
    pub fn touch(&self, key: &str, now: i64) {
        if let Some(file) = self.open_entry(key) {
            let used = UNIX_EPOCH + Duration::from_secs(now.max(0) as u64);
            let _ = file.set_times(fs::FileTimes::new().set_modified(used));
        }
    }

    fn open_entry(&self, key: &str) -> Option<fs::File> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(self.path(key))
            .ok()?;
        file.metadata().ok()?.is_file().then_some(file)
    }

    fn read_header(file: &mut fs::File) -> Option<[u8; HEADER as usize]> {
        let mut header = [0; HEADER as usize];
        file.read_exact(&mut header).ok()?;
        if !valid_header(&header) {
            return None;
        }
        Some(header)
    }

    fn latest_request(&self, key: &str) -> Option<RequestOrder> {
        let mut file = self.open_entry(key)?;
        let header = Self::read_header(&mut file)?;
        RequestOrder::from_header(&header)
    }
}

#[cfg(test)]
mod tests;
