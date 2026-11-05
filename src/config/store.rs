//! One runtime value and a three-way merge against the last valid file.
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use toml_edit::{DocumentMut, Item, Table, Value};

mod document;

pub(in crate::config) use document::*;

const READ_ONLY_HINT: &str = "Config is read-only. Changes are active for this session.";

use super::{
    Config, config_dir, expand_tilde,
    schema::{Setting, annotated},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Persistence {
    Saved,
    Temporary,
    Unsaved,
}

pub struct Store {
    pub current: Config,
    baseline: Config,
    target: Option<PathBuf>,
    existed: bool,
    reload_failed: bool,
    unverified: Option<super::managed::Verification>,
    pub persistence: Persistence,
    pub error: Option<String>,
    pub conflicts: Vec<String>,
}

impl Store {
    pub fn memory(config: Config) -> Self {
        Self {
            baseline: config.clone(),
            current: config,
            target: None,
            existed: false,
            reload_failed: false,
            unverified: None,
            persistence: Persistence::Temporary,
            error: None,
            conflicts: Vec::new(),
        }
    }

    /// Initialize only absent files. Existing malformed files remain untouched.
    pub fn open(path: Option<&Path>) -> Result<Self> {
        let path = path
            .map(expand_tilde)
            .unwrap_or_else(|| config_dir().join("config.toml"));
        match fs::read_to_string(&path) {
            Ok(text) => {
                let config = parse(&path, &text)?;
                Ok(Self {
                    baseline: config.clone(),
                    current: config,
                    target: Some(resolve(&path)?),
                    existed: true,
                    reload_failed: false,
                    unverified: None,
                    persistence: Persistence::Saved,
                    error: None,
                    conflicts: Vec::new(),
                })
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let config = Config {
                    path,
                    ..Config::default()
                };
                let mut store = Self::memory(config);
                // A failed initialization is visible and retryable, without stopping browsing.
                if let Err(error) = store.initialize() {
                    store.failed(error);
                }
                Ok(store)
            }
            Err(error) => Err(error).context("cannot read config"),
        }
    }

    fn initialize(&mut self) -> Result<()> {
        let path = &self.current.path;
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        let target = resolve(path)?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut temp = tempfile::NamedTempFile::new_in(target.parent().unwrap())?;
        temp.write_all(annotated().as_bytes())?;
        temp.as_file().sync_all()?;
        match temp.persist_noclobber(&target) {
            Ok(_) => {}
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.error.into()),
        }
        let text = fs::read_to_string(&target)?;
        let disk = parse(path, &text)?;
        self.target = Some(target);
        self.existed = true;
        self.merge(&disk)?;
        self.persistence = Persistence::Saved;
        Ok(())
    }

    pub fn change(&mut self, field: &Setting, value: toml::Value) -> Result<()> {
        if field.scope == super::schema::Scope::Reddit {
            bail!("This preference is saved to Reddit, not the config file");
        }
        self.current = field.set(&self.current, value)?;
        self.save();
        Ok(())
    }

    pub fn dirty(&self) -> bool {
        self.current.entries() != self.baseline.entries()
    }

    pub fn select_account(&mut self, username: Option<&str>) {
        self.current.select_account(username);
        self.baseline.select_account(username);
    }

    pub fn managed_request(&self) -> Option<super::managed::Request> {
        ((self.persistence == Persistence::Temporary || self.unverified.is_some())
            && self.target.is_some()
            && self.conflicts.is_empty()
            && (self.dirty() || self.unverified.is_some()))
        .then(|| {
            super::managed::Request::new(
                self.baseline.clone(),
                self.current.clone(),
                self.unverified.clone(),
            )
        })
    }

    pub fn managed_finished(
        &mut self,
        verification: super::managed::Verification,
        result: Result<()>,
    ) {
        match result {
            Ok(()) => {
                self.unverified = None;
                self.reload();
            }
            Err(error) => {
                self.unverified = Some(verification);
                self.failed(error);
            }
        }
    }

    fn failed(&mut self, error: anyhow::Error) {
        self.persistence = Persistence::Unsaved;
        self.error = Some(format!("{error:#}"));
    }

    fn read(&self) -> Result<(PathBuf, String, Config)> {
        let target = resolve(&self.current.path)?;
        if self.target.as_ref().is_some_and(|old| old != &target) {
            bail!("Config symlink target changed; restore it or revert to the new file");
        }
        let text = fs::read_to_string(&target)
            .context("Config is missing or unreadable; restore it and retry")?;
        let disk = parse(&self.current.path, &text)?;
        Ok((target, text, disk))
    }

    fn merge(&mut self, disk: &Config) -> Result<()> {
        let mut disk = disk.clone();
        Config::align_account_names(&mut [&mut disk, &mut self.current, &mut self.baseline]);
        self.conflicts.clear();
        let local = self.current.entries();
        let base = self.baseline.entries();
        let remote = disk.entries();
        let mut current_updates = Vec::new();
        let mut base_updates = Vec::new();
        let keys: std::collections::BTreeSet<_> = local
            .keys()
            .chain(base.keys())
            .chain(remote.keys())
            .collect();
        for key in keys {
            let local = local.get(key);
            let base = base.get(key);
            let remote = remote.get(key);
            if !Config::same_entry(key, local, base)
                && !Config::same_entry(key, remote, base)
                && !Config::same_entry(key, local, remote)
            {
                self.conflicts.push(key.clone());
                continue;
            }
            if Config::same_entry(key, local, base) {
                current_updates.push((key.clone(), remote.cloned()));
            }
            base_updates.push((key.clone(), remote.cloned()));
        }
        self.current = self.current.with_entries(current_updates)?;
        self.baseline = self.baseline.with_entries(base_updates)?;
        if !self.conflicts.is_empty() {
            bail!(
                "Conflicting file changes: {}. Revert to file, then reapply your choice.",
                self.conflicts.join(", ")
            );
        }
        Ok(())
    }

    /// File polling never writes, and keeps locally edited keys until explicit save/revert.
    pub fn reload(&mut self) {
        if self.current.path.as_os_str().is_empty() || !self.existed {
            return;
        }
        let result = self.read().and_then(|(_, _, disk)| self.merge(&disk));
        match result {
            Err(error) => {
                self.failed(error);
                self.reload_failed = true;
            }
            Ok(()) if self.unverified.is_some() => {
                self.persistence = Persistence::Unsaved;
                self.reload_failed = false;
            }
            Ok(()) if !self.dirty() => {
                self.persistence = Persistence::Saved;
                self.error = None;
                self.reload_failed = false;
            }
            Ok(()) => {
                if self.reload_failed {
                    self.error = None;
                }
                self.reload_failed = false;
                if self.persistence == Persistence::Temporary
                    && fs::metadata(&self.current.path)
                        .is_ok_and(|meta| readonly(&self.current.path, &meta))
                {
                    self.error = Some(READ_ONLY_HINT.into());
                    return;
                }
                if self.persistence == Persistence::Temporary {
                    self.error = Some(
                        "Session changes are not saved. Retry to save, or revert to the file."
                            .into(),
                    );
                }
                self.persistence = Persistence::Unsaved;
                self.reload_failed = false;
            }
        }
    }

    pub fn save(&mut self) {
        self.error = None;
        if self.unverified.is_some() {
            match self.read().and_then(|(_, _, disk)| self.merge(&disk)) {
                Ok(()) => {
                    self.persistence = Persistence::Unsaved;
                    self.error = Some(
                        "The managed save still needs verification; review and retry it.".into(),
                    );
                }
                Err(error) => self.failed(error),
            }
            return;
        }
        if self.current.path.as_os_str().is_empty() {
            self.persistence = Persistence::Temporary;
            return;
        }
        if !self.existed
            && let Err(error) = self.initialize()
        {
            self.failed(error);
            return;
        }
        if let Err(error) = self.write() {
            self.failed(error);
        }
    }

    fn write(&mut self) -> Result<()> {
        let (target, _, disk) = self.read()?;
        self.merge(&disk)?;
        if !self.dirty() {
            self.persistence = Persistence::Saved;
            return Ok(());
        }
        let metadata = fs::metadata(&target)?;
        if readonly(&target, &metadata) {
            self.persistence = Persistence::Temporary;
            self.error = Some(READ_ONLY_HINT.into());
            return Ok(());
        }
        let lock_path = target.with_file_name(format!(
            "{}.lock",
            target.file_name().unwrap().to_string_lossy()
        ));
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let lock = options
            .open(lock_path)
            .context("Cannot open config save lock")?;
        lock.try_lock().context("Config is busy; retry saving")?;
        // Re-read while holding the cooperating-instance lock.
        let (_, original, disk) = self.read()?;
        self.merge(&disk)?;
        let metadata = fs::metadata(&target)?;
        if readonly(&target, &metadata) {
            bail!("Config became read-only; changes remain in memory");
        }
        let updated = patch(&original, &disk, &self.current)?;
        let parent = target.parent().unwrap();
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        temp.as_file().set_permissions(metadata.permissions())?;
        temp.write_all(updated.as_bytes())?;
        temp.as_file().sync_all()?;
        if resolve(&self.current.path)? != target
            || fs::read_to_string(&target)? != original
            || readonly(&target, &fs::metadata(&target)?)
        {
            bail!("Config changed while saving; retry to merge the latest file");
        }
        temp.persist(&target).context("Could not replace config")?;
        // Keep the runtime/disk baseline accurate even if directory syncing fails.
        self.baseline = self.current.clone();
        self.persistence = Persistence::Saved;
        self.conflicts.clear();
        File::open(parent)?.sync_all()?;
        Ok(())
    }

    pub fn revert(&mut self) {
        if self.current.path.as_os_str().is_empty() {
            self.unverified = None;
            self.current = self.baseline.clone();
            self.error = None;
            return;
        }
        let result = (|| {
            let target = resolve(&self.current.path)?;
            let mut disk = parse(&self.current.path, &fs::read_to_string(&target)?)?;
            disk.select_account(self.current.active_account.as_deref());
            disk.show_nsfw = self.current.show_nsfw;
            disk.media.video_autoplay = self.current.media.video_autoplay;
            self.target = Some(target);
            self.current = disk.clone();
            self.baseline = disk;
            self.unverified = None;
            self.existed = true;
            self.conflicts.clear();
            self.error = None;
            self.persistence = Persistence::Saved;
            Ok(())
        })();
        if let Err(error) = result {
            self.failed(error);
        }
    }
}

#[cfg(test)]
mod tests;
