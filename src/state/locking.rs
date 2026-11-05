use super::*;

/// How long a save waits for another instance's save before giving up.
pub(super) const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// A lockdir older than this is likely a crash's leftover; the error says how to clear it.
pub(super) const STALE_LOCK: std::time::Duration = std::time::Duration::from_secs(30);

/// Held from reading to replacing the state file, so concurrent instances' saves take turns.
pub(super) enum SaveLock {
    /// `flock(2)` on `state.json.lock`; the file stays.
    Flock(#[allow(dead_code)] std::fs::File),
    /// `state.json.lockdir` where `flock` never works (some network mounts). It can't see
    /// `flock` holders, so it's safe only when every instance falls back.
    Dir(std::path::PathBuf),
}

impl Drop for SaveLock {
    fn drop(&mut self) {
        if let SaveLock::Dir(path) = self {
            let _ = std::fs::remove_dir(&*path);
        }
    }
}

/// What a failed non-blocking `flock` means for the save.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum FlockFailure {
    /// Held elsewhere or interrupted: retry until the deadline.
    Busy,
    /// This filesystem has no `flock`: use the directory lock.
    Unsupported,
    Error,
}

pub(super) fn flock_failure(errno: Option<i32>) -> FlockFailure {
    match errno {
        Some(libc::EWOULDBLOCK | libc::EINTR) => FlockFailure::Busy,
        // ENOTSUP and EOPNOTSUPP differ on macOS; ENOSYS comes from mounts without flock (Lustre).
        Some(code)
            if [libc::ENOTSUP, libc::EOPNOTSUPP, libc::ENOLCK, libc::ENOSYS].contains(&code) =>
        {
            FlockFailure::Unsupported
        }
        _ => FlockFailure::Error,
    }
}

pub(super) fn lock(target: &Path) -> Result<SaveLock> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;

    let path = target.with_extension("json.lock");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&path)
        .with_context(|| format!("cannot open {}", path.display()))?;
    // Bounded: a stalled instance must not freeze this one's quit.
    let deadline = std::time::Instant::now() + LOCK_WAIT;
    loop {
        // SAFETY: flock on a descriptor this function owns; no memory is involved.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(SaveLock::Flock(file));
        }
        let e = std::io::Error::last_os_error();
        match flock_failure(e.raw_os_error()) {
            FlockFailure::Busy if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            FlockFailure::Busy => return Err(busy(target)),
            FlockFailure::Unsupported => return dir_lock(target),
            FlockFailure::Error => {
                return Err(e).with_context(|| format!("cannot lock {}", path.display()));
            }
        }
    }
}

/// Lock for filesystems without `flock`: the creator of `state.json.lockdir` holds it and
/// only it removes it. Never broken automatically (check-then-remove isn't atomic), so a
/// crash's leftover blocks saves until deleted.
pub(super) fn dir_lock(target: &Path) -> Result<SaveLock> {
    let path = target.with_extension("json.lockdir");
    let deadline = std::time::Instant::now() + LOCK_WAIT;
    loop {
        match std::fs::create_dir(&path) {
            Ok(()) => return Ok(SaveLock::Dir(path)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e).with_context(|| format!("cannot lock {}", path.display())),
        }
        // A lock this old isn't a save in progress: fail now rather than freeze 2 s per save.
        let age = std::fs::symlink_metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok());
        if let Some(age) = age.filter(|&age| age > STALE_LOCK) {
            return Err(anyhow::anyhow!(
                "not saving: lock {} is {} s old; if no redtab is running, delete it",
                path.display(),
                age.as_secs()
            ));
        }
        if std::time::Instant::now() >= deadline {
            return Err(busy(target));
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

pub(super) fn busy(target: &Path) -> anyhow::Error {
    anyhow::anyhow!(
        "not saving: another redtab is still saving {}",
        target.display()
    )
}
