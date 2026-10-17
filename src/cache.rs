//! Disposable, versioned disk cache; only workers touch it.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::{Mutex, MutexGuard};
use sha2::{Digest, Sha256};

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

fn valid_header(header: &[u8; HEADER as usize]) -> bool {
    &header[..8] == MAGIC && header[HEADER_FIELDS..] == Sha256::digest(&header[..HEADER_FIELDS])[..]
}

/// Where a snapshot came from, for ordering.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Source {
    pub order: Option<RequestOrder>,
    /// Local event window: a disk read interval, or an HTTP start.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestOrder {
    pub generation: [u8; 16],
    pub sequence: u64,
}

impl RequestOrder {
    fn from_header(header: &[u8; HEADER as usize]) -> Option<Self> {
        Some(Self {
            sequence: u64::from_le_bytes(header[16..24].try_into().ok()?),
            generation: header[24..HEADER_FIELDS].try_into().ok()?,
        })
    }
}

pub struct Cache {
    root: PathBuf,
    byte_limit: u64,
    writers: Mutex<()>,
}

struct CacheLock<'a> {
    // Release the file lock before another local writer acquires the mutex.
    _file: fs::File,
    _writer: MutexGuard<'a, ()>,
}

impl Drop for CacheLock<'_> {
    fn drop(&mut self) {
        // Closing isn't enough: descriptors inherited by a spawned process would keep the lock.
        let _ = self._file.unlock();
    }
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

    fn open_entry(&self, key: &str) -> Option<fs::File> {
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(self.path(key))
            .ok()
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

    /// Reserve before HTTP, under the replacement lock. A persisted sequence orders
    /// writers across restarts without wall time.
    pub fn reserve_request(&self) -> std::io::Result<RequestOrder> {
        let _lock = self.lock()?;
        let order = match self
            .latest_request(COUNTER)
            .filter(|previous| (1..u64::MAX).contains(&previous.sequence))
        {
            Some(previous) => RequestOrder {
                sequence: previous.sequence + 1,
                ..previous
            },
            None => {
                // Recovery retires outstanding reservations, not just saved snapshots.
                let mut generation = [0; 16];
                fs::File::open("/dev/urandom")?.read_exact(&mut generation)?;
                RequestOrder {
                    generation,
                    sequence: 1,
                }
            }
        };
        self.atomic_replace(&self.path(COUNTER), |temp| {
            self.write_entry(temp, &[], 0, order)
        })?;
        Ok(order)
    }

    fn lock(&self) -> std::io::Result<CacheLock<'_>> {
        // One shared deadline, so queued threads don't each wait and a stuck holder can't
        // stall the pool.
        let deadline = std::time::Instant::now() + LOCK_WAIT;
        let writer = self.writers.try_lock_until(deadline).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::TimedOut, "cache writer lock timed out")
        })?;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.root)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(self.root.join("cache.lock"))?;
        // A short wait covers another instance's write; a stuck holder means uncached
        // work, not a stall.
        loop {
            match lock.try_lock() {
                Ok(()) => break,
                Err(fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(e) => return Err(std::io::Error::other(e)),
            }
        }
        Ok(CacheLock {
            _file: lock,
            _writer: writer,
        })
    }

    pub fn store(
        &self,
        key: &str,
        bytes: &[u8],
        fetched_at: i64,
        request_order: RequestOrder,
    ) -> std::io::Result<()> {
        let size = bytes.len() as u64 + HEADER;
        if size > ENTRY_LIMIT.min(self.byte_limit) {
            return Ok(());
        }
        let _lock = self.lock()?;
        let Some(current) = self.latest_request(COUNTER) else {
            return Ok(());
        };
        if request_order.generation != current.generation
            || request_order.sequence > current.sequence
        {
            return Ok(());
        }
        if self.latest_request(key).is_some_and(|stored| {
            stored.generation == request_order.generation
                && (1..=current.sequence).contains(&stored.sequence)
                && stored.sequence > request_order.sequence
        }) {
            return Ok(());
        }
        let target = self.path(key);
        self.atomic_replace(&target, |temp| {
            self.write_entry(temp, bytes, fetched_at, request_order)?;
            self.prune(&target, temp, size, fetched_at)
        })
    }

    fn atomic_replace(
        &self,
        target: &Path,
        write: impl Fn(&Path) -> std::io::Result<()>,
    ) -> std::io::Result<()> {
        let temp = self.root.join(format!(
            ".tmp-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let mut result = write(&temp).and_then(|()| fs::rename(&temp, target));
        if let Err(error) = &result {
            let no_space = matches!(
                error.raw_os_error(),
                Some(libc::ENOSPC) | Some(libc::EDQUOT)
            );
            let _ = fs::remove_file(&temp);
            // Reclaim crashed writers' files; retry once only if that freed space.
            if no_space && self.remove_abandoned_writes()? {
                result = write(&temp).and_then(|()| fs::rename(&temp, target));
                if result.is_err() {
                    let _ = fs::remove_file(&temp);
                }
            }
        }
        result
    }

    fn remove_abandoned_writes(&self) -> std::io::Result<bool> {
        let mut removed = false;
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if entry
                .file_name()
                .to_str()
                .is_some_and(Self::abandoned_write)
                && entry.file_type()?.is_file()
            {
                fs::remove_file(entry.path())?;
                removed = true;
            }
        }
        Ok(removed)
    }

    fn write_entry(
        &self,
        path: &Path,
        bytes: &[u8],
        fetched_at: i64,
        request_order: RequestOrder,
    ) -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        let mut header = [0; HEADER as usize];
        header[..8].copy_from_slice(MAGIC);
        header[8..16].copy_from_slice(&fetched_at.to_le_bytes());
        header[16..24].copy_from_slice(&request_order.sequence.to_le_bytes());
        header[24..HEADER_FIELDS].copy_from_slice(&request_order.generation);
        let checksum = Sha256::digest(&header[..HEADER_FIELDS]);
        header[HEADER_FIELDS..].copy_from_slice(&checksum);
        file.write_all(&header)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        let modified = UNIX_EPOCH + Duration::from_secs(fetched_at.max(0) as u64);
        file.set_times(fs::FileTimes::new().set_modified(modified))
    }

    /// Every writer owns cache.lock before creating its temp file, so the only live
    /// one is `writing`; others are left by interrupted processes.
    fn abandoned_write(name: &str) -> bool {
        name.strip_prefix(".tmp-")
            .and_then(|name| name.split_once('-'))
            .is_some_and(|(pid, sequence)| {
                pid.parse::<u32>().is_ok() && sequence.parse::<u64>().is_ok()
            })
    }

    fn prune(
        &self,
        replacing: &Path,
        writing: &Path,
        incoming: u64,
        now: i64,
    ) -> std::io::Result<()> {
        let mut entries = Vec::new();
        let mut total = incoming;
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if Self::abandoned_write(name)
                && entry.path() != writing
                && entry.file_type()?.is_file()
            {
                fs::remove_file(entry.path())?;
                continue;
            }
            let Some(key) = name.strip_suffix(".cache") else {
                continue;
            };
            if key.len() != 64
                || !key.bytes().all(|b| b.is_ascii_hexdigit())
                || !entry.file_type()?.is_file()
            {
                continue;
            }
            let path = entry.path();
            if path == replacing {
                continue;
            }
            let meta = entry.metadata()?;
            let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let age = UNIX_EPOCH
                .checked_add(Duration::from_secs(now.max(0) as u64))
                .and_then(|now| now.duration_since(modified).ok())
                .unwrap_or_default();
            if age.as_secs() > RETENTION as u64 {
                fs::remove_file(path)?;
                continue;
            }
            total += meta.len();
            entries.push((modified, path, meta.len()));
        }
        entries.sort_unstable_by_key(|e| e.0);
        let mut count = entries.len() + 1;
        for (_, path, size) in entries {
            if total <= self.byte_limit && count <= FILE_LIMIT {
                break;
            }
            fs::remove_file(path)?;
            total -= size;
            count -= 1;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "redtab-cache-test-{}-{}",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_new_cache_handle_reads_the_previous_process_snapshot() {
        let dir = Scratch::new();
        let key = Cache::key("instance", "post");
        let cache = Cache::new(dir.0.clone());
        cache
            .store(&key, b"saved body", 1000, cache.reserve_request().unwrap())
            .unwrap();

        let entry = Cache::new(dir.0.clone()).load(&key, 1001).unwrap();

        assert_eq!(entry.bytes, b"saved body");
        assert_eq!(entry.fetched_at, 1000);
    }

    #[test]
    fn eviction_keeps_newer_content_within_the_byte_budget() {
        let dir = Scratch::new();
        let cache = Cache {
            writers: Mutex::new(()),
            root: dir.0.clone(),
            byte_limit: HEADER + b"older body".len() as u64,
        };
        let old = Cache::key("instance", "old");
        let new = Cache::key("instance", "new");
        cache
            .store(&old, b"older body", 1000, cache.reserve_request().unwrap())
            .unwrap();

        cache
            .store(&new, b"new body", 1001, cache.reserve_request().unwrap())
            .unwrap();

        assert!(cache.load(&old, 1001).is_none());
        assert_eq!(cache.load(&new, 1001).unwrap().bytes, b"new body");
    }

    #[test]
    fn a_write_removes_entries_past_the_retention_window_but_not_at_it() {
        let dir = Scratch::new();
        let cache = Cache::new(dir.0.clone());
        let now = 10_000_000;
        let expired = Cache::key("instance", "expired");
        let at_limit = Cache::key("instance", "at limit");
        cache
            .store(
                &expired,
                b"expired",
                now - RETENTION - 1,
                cache.reserve_request().unwrap(),
            )
            .unwrap();
        cache
            .store(
                &at_limit,
                b"at limit",
                now - RETENTION,
                cache.reserve_request().unwrap(),
            )
            .unwrap();

        cache
            .store(
                &Cache::key("instance", "new"),
                b"new",
                now,
                cache.reserve_request().unwrap(),
            )
            .unwrap();

        assert!(!cache.path(&expired).exists());
        assert!(cache.path(&at_limit).exists());
    }

    /// `count` entries named like cache files, all written at `now` except entry 7,
    /// written 10 s earlier.
    fn entries_with_one_oldest(dir: &Path, count: usize, now: i64) {
        for i in 0..count {
            let written = if i == 7 { now - 10 } else { now };
            fs::File::create(dir.join(format!("{i:064x}.cache")))
                .unwrap()
                .set_modified(UNIX_EPOCH + Duration::from_secs(written as u64))
                .unwrap();
        }
    }

    fn entry_count(dir: &Path) -> usize {
        fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|name| name.len() == 64 + ".cache".len())
            .count()
    }

    #[test]
    fn a_write_past_the_file_limit_removes_the_oldest_entry() {
        let dir = Scratch::new();
        let cache = Cache::new(dir.0.clone());
        let now = 10_000_000;
        entries_with_one_oldest(&dir.0, FILE_LIMIT, now);

        cache
            .store(
                &Cache::key("instance", "new"),
                b"new",
                now,
                cache.reserve_request().unwrap(),
            )
            .unwrap();

        assert_eq!(entry_count(&dir.0), FILE_LIMIT);
        assert!(!dir.0.join(format!("{:064x}.cache", 7)).exists());
    }

    #[test]
    fn a_write_reaching_the_file_limit_removes_nothing() {
        let dir = Scratch::new();
        let cache = Cache::new(dir.0.clone());
        let now = 10_000_000;
        entries_with_one_oldest(&dir.0, FILE_LIMIT - 1, now);

        cache
            .store(
                &Cache::key("instance", "new"),
                b"new",
                now,
                cache.reserve_request().unwrap(),
            )
            .unwrap();

        assert_eq!(entry_count(&dir.0), FILE_LIMIT);
        assert!(dir.0.join(format!("{:064x}.cache", 7)).exists());
    }

    #[test]
    fn invalid_header_checksums_and_versions_are_cache_misses() {
        let dir = Scratch::new();
        let cache = Cache::new(dir.0.clone());
        let key = Cache::key("instance", "post");
        for incompatible_version in [false, true] {
            cache
                .store(&key, b"body", 1000, cache.reserve_request().unwrap())
                .unwrap();
            let mut bytes = fs::read(cache.path(&key)).unwrap();
            if incompatible_version {
                bytes[..8].copy_from_slice(b"REDTAB99");
                let checksum = Sha256::digest(&bytes[..HEADER_FIELDS]);
                bytes[HEADER_FIELDS..HEADER as usize].copy_from_slice(&checksum);
            } else {
                bytes[HEADER_FIELDS] ^= 1;
            }
            fs::write(cache.path(&key), bytes).unwrap();
            assert!(cache.load(&key, 1001).is_none());
        }
    }

    #[test]
    fn future_and_expired_snapshots_are_not_served() {
        let dir = Scratch::new();
        let cache = Cache::new(dir.0.clone());
        let key = Cache::key("instance", "post");
        cache
            .store(&key, b"body", 1000, cache.reserve_request().unwrap())
            .unwrap();

        assert!(cache.load(&key, 999).is_none());
        assert_eq!(cache.load(&key, 2_593_000).unwrap().bytes, b"body");
        assert!(cache.load(&key, 2_593_001).is_none());
    }

    #[test]
    fn another_instance_or_settings_cannot_read_the_snapshot() {
        let dir = Scratch::new();
        let cache = Cache::new(dir.0.clone());
        cache
            .store(
                &Cache::key("http://one\nnsfw=off", "feed"),
                b"safe posts",
                1000,
                cache.reserve_request().unwrap(),
            )
            .unwrap();

        assert!(
            cache
                .load(&Cache::key("http://two\nnsfw=off", "feed"), 1001)
                .is_none()
        );
        assert!(
            cache
                .load(&Cache::key("http://one\nnsfw=on", "feed"), 1001)
                .is_none()
        );
        assert!(
            cache
                .load(&Cache::key("http://one\nnsfw=off", "search"), 1001)
                .is_none()
        );
    }

    #[test]
    fn symlink_entries_never_read_or_overwrite_the_target() {
        let dir = Scratch::new();
        let cache = Cache::new(dir.0.clone());
        let key = Cache::key("instance", "post");
        let target = dir.0.join("user-file");
        fs::write(&target, b"keep me").unwrap();
        std::os::unix::fs::symlink(&target, cache.path(&key)).unwrap();
        assert!(cache.load(&key, 1000).is_none());

        cache
            .store(&key, b"cached body", 1000, cache.reserve_request().unwrap())
            .unwrap();

        assert_eq!(fs::read(target).unwrap(), b"keep me");
        assert_eq!(cache.load(&key, 1000).unwrap().bytes, b"cached body");
    }

    #[test]
    fn when_a_fifo_has_no_header_it_should_not_wait_for_data() {
        use std::os::unix::ffi::OsStrExt;

        let dir = Scratch::new();
        let cache = Cache::new(dir.0.clone());
        let path = cache.path("fifo");
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: name is a NUL-terminated path in this test's scratch directory.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let mut held = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
            .unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let reader =
            std::thread::spawn(move || tx.send(cache.load("fifo", 1000).is_none()).unwrap());

        let result = rx.recv_timeout(Duration::from_secs(1));
        // Let even a reader that starts after the timeout open and reject a header.
        held.write_all(&[0; HEADER as usize]).unwrap();
        reader.join().unwrap();
        drop(held);

        assert_eq!(
            result,
            Ok(true),
            "invalid-size entries must be cache misses"
        );
    }

    #[test]
    fn an_older_request_finishing_later_cannot_overwrite_a_newer_snapshot() {
        let dir = Scratch::new();
        let cache = Cache::new(dir.0.clone());
        let key = Cache::key("instance", "post");
        let now = crate::app::now();
        let old = cache.reserve_request().unwrap();
        let new = cache.reserve_request().unwrap();
        cache.store(&key, b"new response", now, new).unwrap();

        Cache::new(dir.0.clone())
            .store(&key, b"old response", now + 1, old)
            .unwrap();

        let actual = cache.load(&key, now + 1).unwrap();
        assert_eq!(actual.bytes, b"new response");
        assert_eq!(actual.fetched_at, now);
    }

    #[test]
    fn the_next_write_removes_abandoned_temporary_files_only() {
        let dir = Scratch::new();
        let cache = Cache::new(dir.0.clone());
        fs::write(dir.0.join(".tmp-123-1"), b"interrupted write").unwrap();
        fs::write(dir.0.join(".tmp-user-note"), b"keep").unwrap();

        cache
            .store(
                &Cache::key("instance", "post"),
                b"body",
                1000,
                cache.reserve_request().unwrap(),
            )
            .unwrap();

        assert!(!dir.0.join(".tmp-123-1").exists());
        assert_eq!(fs::read(dir.0.join(".tmp-user-note")).unwrap(), b"keep");
    }

    #[test]
    fn request_order_survives_restart_and_clock_correction() {
        let dir = Scratch::new();
        let key = Cache::key("instance", "post");
        let now = crate::app::now();
        let cache = Cache::new(dir.0.clone());
        let old = cache.reserve_request().unwrap();
        cache
            .store(&key, b"before clock correction", now + 1000, old)
            .unwrap();
        assert!(cache.load(&key, now).is_none());
        let restarted = Cache::new(dir.0.clone());
        let new = restarted.reserve_request().unwrap();
        assert_eq!(new.generation, old.generation);
        assert!(new.sequence > old.sequence);

        restarted
            .store(&key, b"after clock correction", now, new)
            .unwrap();
        // Even after the wall clock catches up, an older in-flight response cannot win.
        cache
            .store(&key, b"old request finishing late", now + 1001, old)
            .unwrap();

        assert_eq!(
            restarted.load(&key, now + 1001).unwrap().bytes,
            b"after clock correction"
        );
    }

    #[test]
    fn counter_recovery_invalidates_outstanding_reservations() {
        let dir = Scratch::new();
        let cache = Cache::new(dir.0.clone());
        let key = Cache::key("instance", "post");
        let first = cache.reserve_request().unwrap();
        cache.store(&key, b"saved response", 1000, first).unwrap();
        let outstanding = cache.reserve_request().unwrap();
        fs::write(cache.path("sequence"), b"interrupted metadata").unwrap();

        let recovered = Cache::new(dir.0.clone()).reserve_request().unwrap();
        cache
            .store(
                &key,
                b"old completion before replacement",
                1000,
                outstanding,
            )
            .unwrap();
        assert!(cache.load(&key, 1000).is_none());
        cache.store(&key, b"replacement", 1000, recovered).unwrap();
        cache
            .store(&key, b"older completion", 1000, outstanding)
            .unwrap();

        assert_ne!(recovered.generation, outstanding.generation);
        assert_eq!(cache.load(&key, 1000).unwrap().bytes, b"replacement");
    }

    #[test]
    fn concurrent_reservations_and_writes_keep_the_newest_request() {
        let dir = Scratch::new();
        let key = Cache::key("instance", "post");
        let cache = Cache::new(dir.0.clone());
        let initial = cache.reserve_request().unwrap();
        cache
            .store(&key, &initial.sequence.to_le_bytes(), 1000, initial)
            .unwrap();
        let mut orders = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        let order = match cache.reserve_request() {
                            Ok(order) => order,
                            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => return None,
                            Err(e) => panic!("reserve: {e}"),
                        };
                        match cache.store(&key, &order.sequence.to_le_bytes(), 1000, order) {
                            Ok(()) => Some(order.sequence),
                            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => None,
                            Err(e) => panic!("store: {e}"),
                        }
                    })
                })
                .collect();
            handles
                .into_iter()
                .filter_map(|handle| handle.join().unwrap())
                .collect::<Vec<_>>()
        });
        orders.push(initial.sequence);
        let accepted = orders.len();
        orders.sort_unstable();
        orders.dedup();
        assert_eq!(orders.len(), accepted, "accepted reservations are unique");
        let newest = *orders.last().unwrap();
        let cache = Cache::new(dir.0.clone());
        assert_eq!(cache.load(&key, 1000).unwrap().bytes, newest.to_le_bytes());
        assert!(cache.reserve_request().unwrap().sequence > newest);
    }

    #[test]
    fn held_cache_locks_do_not_stall_cache_work() {
        for shared in [false, true] {
            let dir = Scratch::new();
            let holder = std::sync::Arc::new(Cache::new(dir.0.clone()));
            let lock = holder.lock().unwrap();
            // Every worker of one process shares a cache; their waits must overlap,
            // not queue one 100 ms window after another.
            let cache = if shared {
                std::sync::Arc::clone(&holder)
            } else {
                std::sync::Arc::new(Cache::new(dir.0.clone()))
            };
            let (tx, rx) = std::sync::mpsc::channel();
            let workers: Vec<_> = (0..6)
                .map(|_| {
                    let (cache, tx) = (std::sync::Arc::clone(&cache), tx.clone());
                    std::thread::spawn(move || tx.send(cache.reserve_request()).unwrap())
                })
                .collect();
            let deadline = std::time::Instant::now() + Duration::from_millis(350);
            let results: Vec<_> = (0..6)
                .map_while(|_| {
                    rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                        .ok()
                })
                .collect();
            drop(lock);
            workers.into_iter().for_each(|w| w.join().unwrap());

            assert_eq!(
                results.len(),
                6,
                "a held cache lock (shared={shared}) must not delay cache work"
            );
            assert!(results.iter().all(Result::is_err));
        }
    }

    #[test]
    fn a_briefly_held_external_lock_is_waited_for() {
        let dir = Scratch::new();
        let (locked_tx, locked_rx) = std::sync::mpsc::channel();
        let root = dir.0.clone();
        let other_instance = std::thread::spawn(move || {
            let file = fs::File::create(root.join("cache.lock")).unwrap();
            file.lock().unwrap();
            locked_tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(20));
            file.unlock().unwrap();
        });
        locked_rx.recv().unwrap();

        let reserved = Cache::new(dir.0.clone()).reserve_request();
        other_instance.join().unwrap();

        assert!(reserved.is_ok(), "{reserved:?}");
    }
    #[test]
    fn releasing_a_writer_unlocks_inherited_file_descriptors() {
        let dir = Scratch::new();
        let cache = Cache::new(dir.0.clone());
        let lock = cache.lock().unwrap();
        let inherited = lock._file.try_clone().unwrap();

        drop(lock);

        cache.reserve_request().unwrap();
        drop(inherited);
    }

    #[test]
    fn full_disk_recovers_space_from_an_abandoned_writer_before_retrying() {
        for target in ["sequence", "payload"] {
            let dir = Scratch::new();
            let cache = Cache::new(dir.0.clone());
            let _lock = cache.lock().unwrap();
            fs::write(dir.0.join(".tmp-123-1"), b"abandoned space").unwrap();
            fs::write(dir.0.join(".tmp-user-note"), b"keep").unwrap();
            let calls = std::cell::Cell::new(0);
            cache
                .atomic_replace(&cache.path(target), |temp| {
                    calls.set(calls.get() + 1);
                    if dir.0.join(".tmp-123-1").exists() {
                        return Err(std::io::Error::from_raw_os_error(libc::ENOSPC));
                    }
                    fs::write(temp, b"recovered")
                })
                .unwrap();
            assert_eq!(fs::read(cache.path(target)).unwrap(), b"recovered");
            assert_eq!(fs::read(dir.0.join(".tmp-user-note")).unwrap(), b"keep");
            assert_eq!(calls.get(), 2);
        }
    }
    #[test]
    fn an_impossible_payload_sequence_is_ignored_and_repaired() {
        for bad_sequence in [u64::MAX, 0] {
            let dir = Scratch::new();
            let cache = Cache::new(dir.0.clone());
            let key = Cache::key("instance", "feed");
            let first = cache.reserve_request().unwrap();
            cache.store(&key, b"stale content", 1000, first).unwrap();
            let mut bytes = fs::read(cache.path(&key)).unwrap();
            bytes[16..24].copy_from_slice(&bad_sequence.to_le_bytes());
            let checksum = Sha256::digest(&bytes[..HEADER_FIELDS]);
            bytes[HEADER_FIELDS..HEADER as usize].copy_from_slice(&checksum);
            fs::write(cache.path(&key), bytes).unwrap();
            assert!(cache.load(&key, 1001).is_none());
            let next = cache.reserve_request().unwrap();
            cache.store(&key, b"repaired content", 1001, next).unwrap();
            let saved = cache.load(&key, 1001).unwrap();
            assert_eq!(saved.bytes, b"repaired content");
            assert_eq!(saved.order, next);
        }
    }
    #[test]
    fn invalid_counter_values_retire_outstanding_reservations() {
        for (previous, bad, reseal) in [(1, 0, true), (1, u64::MAX, true), (2, 1, false)] {
            let dir = Scratch::new();
            let cache = Cache::new(dir.0.clone());
            let key = Cache::key("instance", "post");
            if previous == 2 {
                cache.reserve_request().unwrap();
            }
            let old = cache.reserve_request().unwrap();
            let mut counter = fs::read(cache.path("sequence")).unwrap();
            counter[16..24].copy_from_slice(&bad.to_le_bytes());
            if reseal {
                let checksum = Sha256::digest(&counter[..HEADER_FIELDS]);
                counter[HEADER_FIELDS..].copy_from_slice(&checksum);
            }
            fs::write(cache.path("sequence"), counter).unwrap();
            let recovered = cache.reserve_request().unwrap();
            cache
                .store(&key, b"newer response", 1000, recovered)
                .unwrap();
            cache.store(&key, b"old completion", 1000, old).unwrap();
            assert_eq!(cache.load(&key, 1000).unwrap().bytes, b"newer response");
            assert_ne!(old.generation, recovered.generation);
        }
    }
}
