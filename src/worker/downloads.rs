use super::*;

/// Download folders in `$TMPDIR`: `redtab-media-<pid>-<hex>`.
pub(super) const MEDIA_PREFIX: &str = "redtab-media-";

/// Downloads older than this go: a running redtab's own on its next download, and the
/// folder of an exited redtab once unchanged this long. A viewer opened just before still
/// has its files.
pub(super) const MEDIA_GRACE: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// Private 0700 directory this process creates for downloads. An existing one is never
/// reused: a predictable /tmp path would let another user plant symlinks.
pub(super) fn media_dir() -> anyhow::Result<PathBuf> {
    use std::os::unix::fs::DirBuilderExt;
    // Created under the lock and replaced if it disappears; only success is stored, so a
    // failure retries on the next `m`.
    static DIR: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);
    let mut cached = DIR
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Reused only while it is still this user's real folder: a tmp cleaner may have
    // removed it, and someone else may have put a symlink at its name.
    if let Some(dir) = cached.as_ref()
        && own_dir(&std::fs::symlink_metadata(dir).ok())
    {
        return Ok(dir.clone());
    }
    let pid = std::process::id();
    for attempt in 0..16u32 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let dir = std::env::temp_dir().join(format!("{MEDIA_PREFIX}{pid}-{nanos:x}{attempt}"));
        match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => return Ok(cached.insert(dir).clone()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    anyhow::bail!(
        "cannot create a private media directory in {}",
        std::env::temp_dir().display()
    )
}

/// A real directory (not a symlink) owned by this user.
pub(super) fn own_dir(meta: &Option<std::fs::Metadata>) -> bool {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    meta.as_ref().is_some_and(|m| m.is_dir() && m.uid() == uid)
}

/// Removes downloads in this process's own folder older than `MEDIA_GRACE`, so a redtab
/// left running for days does not keep every image.
pub(super) fn remove_old_downloads(dir: &std::path::Path, now: std::time::SystemTime) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        let old = meta
            .modified()
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= MEDIA_GRACE);
        if meta.is_file() && old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Removes download folders that redtab processes which have exited left in `$TMPDIR`,
/// once `MEDIA_GRACE` has passed. Folders of running redtabs stay.
pub fn remove_stale_media_dirs() {
    remove_stale_media_dirs_in(
        &std::env::temp_dir(),
        std::time::SystemTime::now(),
        process_running,
    );
}

pub(super) fn remove_stale_media_dirs_in(
    tmp: &std::path::Path,
    now: std::time::SystemTime,
    running: impl Fn(u32) -> bool,
) {
    let Ok(entries) = std::fs::read_dir(tmp) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name();
        let Some((pid, rest)) = name
            .to_str()
            .and_then(|n| n.strip_prefix(MEDIA_PREFIX))
            .and_then(|n| n.split_once('-'))
        else {
            continue;
        };
        let Ok(pid) = pid.parse::<u32>() else {
            continue;
        };
        if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        // A real folder of this user only: never through a symlink.
        let meta = std::fs::symlink_metadata(entry.path()).ok();
        let unchanged = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|modified| now.duration_since(modified).ok())
            .unwrap_or_default();
        if own_dir(&meta) && unchanged >= MEDIA_GRACE && !running(pid) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Whether `pid` is a live process, whoever owns it.
pub(super) fn process_running(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        // Signal 0 to these would address a process group: keep the folder.
        return true;
    }
    // SAFETY: signal 0 only checks that the process exists; nothing is delivered.
    let exists = unsafe { libc::kill(pid, 0) } == 0;
    exists || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

pub(super) fn download_images(reddit: &Reddit, urls: &[String]) -> anyhow::Result<Vec<PathBuf>> {
    let dir = media_dir()?;
    remove_old_downloads(&dir, std::time::SystemTime::now());
    urls.iter()
        .enumerate()
        .map(|(i, url)| {
            let bytes = reddit.image(url)?;
            let ext = image::guess_format(&bytes)
                .ok()
                .and_then(|f| f.extensions_str().first().copied())
                .unwrap_or("img");
            let stem: String = url
                .split('?')
                .next()
                .unwrap_or_default()
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .split('.')
                .next()
                .unwrap_or_default()
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                .collect();
            let stem = if stem.is_empty() {
                format!("image-{i}")
            } else {
                stem
            };
            // A fresh name per download: never open an existing file.
            let mut n = 0;
            let path = loop {
                let name = if n == 0 {
                    format!("{stem}.{ext}")
                } else {
                    format!("{stem}-{n}.{ext}")
                };
                let path = dir.join(name);
                match std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                {
                    Ok(mut file) => {
                        std::io::Write::write_all(&mut file, &bytes)?;
                        break path;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
                    Err(e) => return Err(e.into()),
                }
            };
            Ok(path)
        })
        .collect()
}
