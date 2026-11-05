use super::*;

impl State {
    /// Invalid JSON is moved aside; an unreadable file is left alone and saving is disabled.
    pub fn load(path: &Path) -> (Self, Option<String>) {
        // Act on a symlink's target, never the link.
        Self::load_file(&resolve_link(path), false)
    }

    pub(super) fn load_file(path: &Path, locked: bool) -> (Self, Option<String>) {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Self::default(), None),
            Err(e) => {
                let state = Self {
                    read_only: true,
                    ..Self::default()
                };
                return (
                    state,
                    Some(format!(
                        "cannot read {}: {e}; this session will not save state",
                        path.display()
                    )),
                );
            }
        };
        match serde_json::from_slice(&bytes) {
            Ok(state) => (state, None),
            Err(e) => {
                if !locked {
                    let _lock = match lock(path) {
                        Ok(lock) => lock,
                        Err(lock) => {
                            return (
                                Self {
                                    read_only: true,
                                    ..Self::default()
                                },
                                Some(format!(
                                    "state file was invalid ({e}) and could not be locked ({lock}); not saving"
                                )),
                            );
                        }
                    };
                    // Another instance may have recovered and saved since the first read.
                    return Self::load_file(path, true);
                }
                // Never overwrite an earlier backup.
                match move_corrupt_state(path) {
                    Ok(aside) => (
                        Self::default(),
                        Some(format!(
                            "state file was invalid ({e}); moved to {}",
                            aside.display()
                        )),
                    ),
                    Err(rename) => (
                        Self {
                            read_only: true,
                            ..Self::default()
                        },
                        Some(format!(
                            "state file was invalid ({e}) and could not be moved aside ({rename}); not saving"
                        )),
                    ),
                }
            }
        }
    }
}

pub(super) fn move_corrupt_state(path: &Path) -> std::io::Result<std::path::PathBuf> {
    let mut aside = path.with_extension("json.bad");
    loop {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&aside)
        {
            Ok(reservation) => {
                drop(reservation);
                if let Err(error) = std::fs::rename(path, &aside) {
                    let _ = std::fs::remove_file(&aside);
                    return Err(error);
                }
                return Ok(aside);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                aside = path.with_extension(format!("json.bad-{}", uuid::Uuid::new_v4()));
            }
            Err(error) => return Err(error),
        }
    }
}

/// The file `path` names, following links even to a missing target, so writes never
/// replace a link.
pub(super) fn resolve_link(path: &Path) -> std::path::PathBuf {
    if let Ok(real) = std::fs::canonicalize(path) {
        return real;
    }
    let mut current = path.to_path_buf();
    // Linux's symlink limit (macOS stops at 32), so a link loop can't spin.
    for _ in 0..40 {
        let Ok(target) = std::fs::read_link(&current) else {
            break;
        };
        current = match current.parent() {
            Some(dir) if target.is_relative() => dir.join(target),
            _ => target,
        };
    }
    current
}
