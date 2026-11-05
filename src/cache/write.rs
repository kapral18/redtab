use super::*;

pub(super) struct CacheLock<'a> {
    // Release the file lock before another local writer acquires the mutex.
    pub(super) _file: fs::File,
    pub(super) _writer: MutexGuard<'a, ()>,
}

impl Drop for CacheLock<'_> {
    fn drop(&mut self) {
        // Closing isn't enough: descriptors inherited by a spawned process would keep the lock.
        let _ = self._file.unlock();
    }
}

impl Cache {
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

    pub(super) fn lock(&self) -> std::io::Result<CacheLock<'_>> {
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
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(self.root.join("cache.lock"))?;
        if !lock.metadata()?.is_file() {
            return Err(std::io::Error::other("cache lock must be a regular file"));
        }
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
            self.prune_if_due(&target, temp, size, fetched_at)
        })
    }

    pub(super) fn atomic_replace(
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

    pub(super) fn remove_abandoned_writes(&self) -> std::io::Result<bool> {
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

    pub(super) fn write_entry(
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
    pub(super) fn abandoned_write(name: &str) -> bool {
        name.strip_prefix(".tmp-")
            .and_then(|name| name.split_once('-'))
            .is_some_and(|(pid, sequence)| {
                pid.parse::<u32>().is_ok() && sequence.parse::<u64>().is_ok()
            })
    }
}
