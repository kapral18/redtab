use super::*;

/// The directory as the last scan left it, plus every store since.
pub(super) struct Tally {
    pub(super) bytes: u64,
    pub(super) files: u64,
    pub(super) scanned_at: i64,
}

impl Tally {
    pub(super) fn encode(&self) -> [u8; 24] {
        let mut bytes = [0; 24];
        bytes[..8].copy_from_slice(&self.bytes.to_le_bytes());
        bytes[8..16].copy_from_slice(&self.files.to_le_bytes());
        bytes[16..].copy_from_slice(&self.scanned_at.to_le_bytes());
        bytes
    }

    pub(super) fn decode(bytes: &[u8]) -> Option<Self> {
        let bytes: &[u8; 24] = bytes.try_into().ok()?;
        Some(Self {
            bytes: u64::from_le_bytes(bytes[..8].try_into().ok()?),
            files: u64::from_le_bytes(bytes[8..16].try_into().ok()?),
            scanned_at: i64::from_le_bytes(bytes[16..].try_into().ok()?),
        })
    }
}

impl Cache {
    /// Scans the directory when the tally is missing or unreadable, exceeds a limit,
    /// `SCAN_INTERVAL` passed or the clock went back. Callers hold `cache.lock`, so every
    /// process counts its stores.
    pub(super) fn prune_if_due(
        &self,
        replacing: &Path,
        writing: &Path,
        incoming: u64,
        now: i64,
    ) -> std::io::Result<()> {
        let mut full = false;
        if let Some(mut seen) = self.read_tally() {
            // A replaced entry counts again, so the tally only overestimates.
            seen.bytes = seen.bytes.saturating_add(incoming);
            seen.files = seen.files.saturating_add(1);
            full = seen.bytes > self.byte_limit || seen.files > FILE_LIMIT as u64;
            if !full
                && (seen.scanned_at..seen.scanned_at.saturating_add(SCAN_INTERVAL)).contains(&now)
            {
                self.write_tally(&seen);
                return Ok(());
            }
        }
        let (bytes, files) = self.prune(replacing, writing, incoming, now, full)?;
        self.write_tally(&Tally {
            bytes,
            files: files as u64,
            scanned_at: now,
        });
        Ok(())
    }

    pub(super) fn read_tally(&self) -> Option<Tally> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(self.root.join(TALLY))
            .ok()?;
        if !file.metadata().ok()?.is_file() {
            return None;
        }
        let mut bytes = Vec::with_capacity(24);
        file.take(25).read_to_end(&mut bytes).ok()?;
        Tally::decode(&bytes)
    }

    /// A lost update only forces the next store to scan.
    pub(super) fn write_tally(&self, tally: &Tally) {
        let temp = self.root.join(".tally-next");
        let written = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&temp)
            .and_then(|mut file| file.write_all(&tally.encode()))
            .and_then(|()| fs::rename(&temp, self.root.join(TALLY)));
        if written.is_err() {
            let _ = fs::remove_file(&temp);
            let _ = fs::remove_file(self.root.join(TALLY));
        }
    }

    pub(super) fn prune(
        &self,
        replacing: &Path,
        writing: &Path,
        incoming: u64,
        now: i64,
        full: bool,
    ) -> std::io::Result<(u64, usize)> {
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
        // When the cache or the tally passed either limit, evict to 7/8 of both, so the
        // next stores, replacements included, do not each scan again.
        let (byte_target, file_target) = if full || total > self.byte_limit || count > FILE_LIMIT {
            (self.byte_limit / 8 * 7, FILE_LIMIT / 8 * 7)
        } else {
            (self.byte_limit, FILE_LIMIT)
        };
        for (_, path, size) in entries {
            if total <= byte_target && count <= file_target {
                break;
            }
            fs::remove_file(path)?;
            total -= size;
            count -= 1;
        }
        Ok((total, count))
    }
}
