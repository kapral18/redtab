//! One runtime value and a three-way merge against the last valid file.
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use toml_edit::{DocumentMut, Item, Table, Value};

const READ_ONLY_HINT: &str = "Config is read-only. Changes are active for this session.";

use super::{
    Config, config_dir, expand_tilde,
    schema::{SETTINGS, Setting, annotated},
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
    pub conflicts: Vec<&'static str>,
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
        self.current = field.set(&self.current, value)?;
        self.save();
        Ok(())
    }

    pub fn dirty(&self) -> bool {
        SETTINGS
            .iter()
            .any(|f| f.value(&self.current) != f.value(&self.baseline))
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
        self.conflicts.clear();
        for field in SETTINGS {
            let local = field.value(&self.current);
            let base = field.value(&self.baseline);
            let remote = field.value(disk);
            if local != base && remote != base && local != remote {
                self.conflicts.push(field.key);
                continue;
            }
            if local == base {
                self.current = field.set(&self.current, remote.clone())?;
            }
            self.baseline = field.set(&self.baseline, remote)?;
        }
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
            let disk = parse(&self.current.path, &fs::read_to_string(&target)?)?;
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

pub(super) fn patch(original: &str, before: &Config, after: &Config) -> Result<String> {
    // A blank line separates a file header from field or section annotations.
    let mut leading = 0;
    let mut header_end = 0;
    let mut comments = false;
    for line in original.split_inclusive('\n') {
        if !line.trim().is_empty() && !line.trim_start().starts_with('#') {
            break;
        }
        leading += line.len();
        if line.trim_start().starts_with('#') {
            comments = true;
        } else if comments && header_end == 0 {
            header_end = leading;
        }
    }
    if leading == original.len() || header_end == 0 {
        header_end = leading;
    }
    let (header, body) = original.split_at(header_end);
    let mut document: DocumentMut = body
        .parse()
        .context("Invalid TOML; file was left untouched")?;
    for field in SETTINGS {
        if field.value(before) == field.value(after) {
            continue;
        }
        let mut item = document.as_item_mut();
        let mut parts = field.key.split('.').peekable();
        while let Some(part) = parts.next() {
            if parts.peek().is_some() && item.get(part).is_none() {
                item[part] = Item::Table(Table::new());
            }
            item = &mut item[part];
        }
        let mut value: Value = field.value(after).to_string().parse()?;
        if let Some(old) = item.as_value() {
            preserve_format(old, &mut value);
        }
        *item = Item::Value(value);
    }
    let body = document.to_string();
    let separator = if !header.is_empty() && !header.ends_with('\n') && !body.is_empty() {
        "\n"
    } else {
        ""
    };
    Ok(format!("{header}{separator}{body}"))
}

/// Keep annotations inside edited arrays, including those on removed elements.
fn preserve_format(old: &Value, next: &mut Value) {
    *next.decor_mut() = old.decor().clone();
    let (Some(previous), Some(values)) = (old.as_array(), next.as_array()) else {
        return;
    };
    let mut array = previous.clone();
    for (index, value) in values.iter().enumerate() {
        if index < array.len() {
            if array.get(index).and_then(Value::as_str) != value.as_str() {
                array.replace(index, value.clone());
            }
        } else {
            array.push(value.clone());
        }
    }
    let mut orphaned = String::new();
    while array.len() > values.len() {
        let removed = array.remove(values.len());
        for raw in [removed.decor().prefix(), removed.decor().suffix()]
            .into_iter()
            .flatten()
        {
            orphaned.push_str(raw.as_str().unwrap_or_default());
        }
    }
    if !orphaned.is_empty() {
        orphaned.push_str(array.trailing().as_str().unwrap_or_default());
        array.set_trailing(orphaned);
    }
    *next = Value::Array(array);
}

pub(super) fn parse(path: &Path, text: &str) -> Result<Config> {
    let mut config: Config = toml::from_str(text)
        .map_err(|error| anyhow::anyhow!("Invalid config: {}", error.message()))?;
    config.validate()?;
    config.path = path.to_path_buf();
    Ok(config)
}

pub(super) fn readonly(path: &Path, metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::PermissionsExt;
        let writable = std::ffi::CString::new(path.as_os_str().as_bytes())
            .is_ok_and(|path| unsafe { libc::access(path.as_ptr(), libc::W_OK) == 0 });
        metadata.permissions().mode() & 0o222 == 0 || !writable
    }
    #[cfg(not(unix))]
    {
        metadata.permissions().readonly()
    }
}

fn resolve(path: &Path) -> Result<PathBuf> {
    let mut path = std::path::absolute(path)?;
    for _ in 0..32 {
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                let link = fs::read_link(&path)?;
                path = if link.is_absolute() {
                    link
                } else {
                    path.parent().unwrap().join(link)
                };
            }
            Ok(_) => return fs::canonicalize(path).map_err(Into::into),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(
                    fs::canonicalize(path.parent().unwrap())?.join(path.file_name().unwrap())
                );
            }
            Err(error) => return Err(error.into()),
        }
    }
    bail!("Config symlink cycle or excessive nesting")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn change(store: &mut Store, key: &str, input: &str) {
        let value: toml::Table = toml::from_str(&format!("value = {input}")).unwrap();
        store
            .change(
                SETTINGS.iter().find(|f| f.key == key).unwrap(),
                value["value"].clone(),
            )
            .unwrap();
    }

    #[test]
    fn grouped_settings_should_initialize_active_media_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        Store::open(Some(&path)).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        let values: toml::Value = toml::from_str(&text).unwrap();
        assert!(text.contains("[media]"), "{text}");
        assert_eq!(values["media"]["image_preview"].as_bool(), Some(true));
        assert_eq!(values["media"]["video_autoplay"].as_bool(), Some(true));
    }

    #[test]
    fn grouped_settings_should_keep_comment_only_headers_above_new_values() {
        for original in ["# réglages\n", "# réglages", "# réglages\r\n\r\n"] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("config.toml");
            fs::write(&path, original).unwrap();
            let mut store = Store::open(Some(&path)).unwrap();
            change(&mut store, "show_nsfw", "true");
            let actual = fs::read_to_string(&path).unwrap();
            assert!(actual.starts_with(original), "{actual}");
            assert!(actual.contains("\nshow_nsfw = true"), "{actual}");
            assert!(Config::load(Some(&path)).unwrap().show_nsfw);
        }
    }

    #[test]
    fn grouped_settings_should_create_missing_media_tables_without_moving_headers() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(&path, "# settings\n\nshow_nsfw = false # keep\n").unwrap();
        let mut store = Store::open(Some(&path)).unwrap();
        change(&mut store, "media.video_autoplay", "false");
        let actual = fs::read_to_string(&path).unwrap();
        assert!(
            actual.starts_with("# settings\n\nshow_nsfw = false # keep\n"),
            "{actual}"
        );
        assert!(actual.contains("[media]\n"), "{actual}");
    }

    #[test]
    fn grouped_settings_should_keep_section_annotations_when_inserting_root_keys() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(
            &path,
            "# file header\n\n# media section\n\n[media]\nvideo_autoplay = true # keep\n",
        )
        .unwrap();
        let mut store = Store::open(Some(&path)).unwrap();
        change(&mut store, "show_nsfw", "true");
        let actual = fs::read_to_string(&path).unwrap();
        assert!(
            actual.starts_with("# file header\n\nshow_nsfw = true\n"),
            "{actual}"
        );
        assert!(
            actual.contains("# media section\n\n[media]\nvideo_autoplay = true # keep\n"),
            "{actual}"
        );
    }

    #[test]
    fn grouped_settings_should_keep_a_single_leading_header_above_new_root_keys() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(
            &path,
            "\n# personal settings\n[media]\nvideo_autoplay = true\n",
        )
        .unwrap();
        let mut store = Store::open(Some(&path)).unwrap();
        change(&mut store, "show_nsfw", "true");
        let actual = fs::read_to_string(&path).unwrap();
        assert!(
            actual.starts_with("\n# personal settings\nshow_nsfw = true\n"),
            "{actual}"
        );
    }

    #[test]
    fn grouped_settings_should_keep_existing_inline_and_dotted_media_styles() {
        for (original, prefix) in [
            ("media = { video_autoplay = false }\n", "media = {"),
            (
                "media.video_autoplay = false\n",
                "media.video_autoplay = false\n",
            ),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("config.toml");
            fs::write(&path, original).unwrap();
            let mut store = Store::open(Some(&path)).unwrap();
            change(&mut store, "media.image_preview", "false");
            let actual = fs::read_to_string(&path).unwrap();
            assert!(actual.starts_with(prefix), "{actual}");
            let values: toml::Value = toml::from_str(&actual).unwrap();
            assert_eq!(values["media"]["image_preview"].as_bool(), Some(false));
            assert_eq!(values["media"]["video_autoplay"].as_bool(), Some(false));
        }
    }
    #[test]
    fn initializes_once_and_preserves_comments_and_unrelated_external_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut first = Store::open(Some(&path)).unwrap();
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains("# redtab settings")
        );
        fs::write(
            &path,
            "# mine\nshow_nsfw = false # keep this\n\n[filters]\nkeywords = [\"news\"] # custom\n",
        )
        .unwrap();
        let mut second = Store::open(Some(&path)).unwrap();
        change(&mut second, "media.video_autoplay", "false");
        change(&mut first, "show_nsfw", "true");
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("show_nsfw = true # keep this"), "{text}");
        assert!(text.contains("keywords = [\"news\"] # custom"));
        assert!(!first.current.media.video_autoplay);
        assert_eq!(first.current.filters.keywords, ["news"]);
        assert_eq!(first.persistence, Persistence::Saved);
        second.reload();
        assert!(second.current.show_nsfw);
        change(&mut first, "show_nsfw", "false");
        change(&mut first, "show_nsfw", "true");
        assert!(Config::load(Some(&path)).unwrap().show_nsfw);
    }
    #[test]
    fn concurrent_same_key_conflicts_until_revert_then_reapply() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut local = Store::open(Some(&path)).unwrap();
        // Unsaved local intent plus an independent editor's change to the same key.
        local.current.all_subs_posts_per_group = 4;
        fs::write(
            &path,
            "all_subs_posts_per_group = 5\n[media]\nimage_preview = false\n",
        )
        .unwrap();
        local.save();
        assert_eq!(local.persistence, Persistence::Unsaved);
        assert_eq!(local.conflicts, ["all_subs_posts_per_group"]);
        assert_eq!(local.current.all_subs_posts_per_group, 4);
        assert!(!local.current.media.image_preview);
        local.reload();
        assert_eq!(local.conflicts, ["all_subs_posts_per_group"]);
        assert_eq!(
            Config::load(Some(&path)).unwrap().all_subs_posts_per_group,
            5
        );
        local.revert();
        assert_eq!(local.current.all_subs_posts_per_group, 5);
        change(&mut local, "all_subs_posts_per_group", "4");
        assert_eq!(
            Config::load(Some(&path)).unwrap().all_subs_posts_per_group,
            4
        );
    }
    #[test]
    fn invalid_deleted_and_retargeted_files_keep_last_working_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut store = Store::open(Some(&path)).unwrap();
        fs::write(&path, "show_nsfw = true\n").unwrap();
        store.reload();
        assert!(store.current.show_nsfw);
        fs::write(&path, "[media]\nimage_preview = \"invalid\"\n").unwrap();
        store.reload();
        assert!(store.current.show_nsfw);
        assert_eq!(store.persistence, Persistence::Unsaved);
        store.save();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "[media]\nimage_preview = \"invalid\"\n"
        );
        fs::remove_file(&path).unwrap();
        store.save();
        assert!(!path.exists());
        fs::write(&path, "show_nsfw = false\n").unwrap();
        store.revert();
        assert!(!store.current.show_nsfw);
        assert_eq!(store.persistence, Persistence::Saved);
    }
    #[test]
    fn read_only_is_temporary_and_retry_keeps_mode_and_symlink() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("réglages.toml");
        fs::write(&real, "# user\nshow_nsfw = false\n").unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o444)).unwrap();
        let link = dir.path().join("config.toml");
        symlink(&real, &link).unwrap();
        let mut store = Store::open(Some(&link)).unwrap();
        change(&mut store, "show_nsfw", "true");
        assert_eq!(store.persistence, Persistence::Temporary);
        assert_eq!(
            fs::read_to_string(&real).unwrap(),
            "# user\nshow_nsfw = false\n"
        );
        store.reload();
        assert!(store.current.show_nsfw);
        fs::set_permissions(&real, fs::Permissions::from_mode(0o640)).unwrap();
        store.save();
        assert_eq!(store.persistence, Persistence::Saved);
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::metadata(&real).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert!(
            fs::read_to_string(&real)
                .unwrap()
                .contains("show_nsfw = true")
        );
        let other = dir.path().join("other.toml");
        fs::write(&other, "[media]\nimage_preview = false").unwrap();
        fs::remove_file(&link).unwrap();
        symlink(&other, &link).unwrap();
        store.reload();
        assert_eq!(store.persistence, Persistence::Unsaved);
        assert!(store.current.media.image_preview);
        store.revert();
        assert!(!store.current.media.image_preview);
    }
    #[test]
    fn empty_file_and_removing_keys_restore_defaults_on_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "show_nsfw = true\n").unwrap();
        let mut store = Store::open(Some(&path)).unwrap();
        fs::write(&path, "").unwrap();
        store.reload();
        assert!(!store.current.show_nsfw);
        assert_eq!(store.persistence, Persistence::Saved);
        fs::write(&path, "show_nsfw = true\n").unwrap();
        store.reload();
        assert!(store.current.show_nsfw);
    }
    #[test]
    fn settings_review_array_edits_should_keep_internal_and_removed_element_comments() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "# top\n[filters]\nkeywords = [\n    # before first\n    \"old\", # first note\n    \"removed\", # removed note\n    # trailing note\n] # outer\n").unwrap();
        let mut store = Store::open(Some(&path)).unwrap();
        for (input, expected) in [
            ("[\"new\"]", vec!["new"]),
            ("[]", vec![]),
            ("[\"one\", \"two\"]", vec!["one", "two"]),
        ] {
            change(&mut store, "filters.keywords", input);
            let text = fs::read_to_string(&path).unwrap();
            assert_eq!(
                Config::load(Some(&path)).unwrap().filters.keywords,
                expected,
                "{text}"
            );
            for note in [
                "# top",
                "# before first",
                "# first note",
                "# removed note",
                "# trailing note",
                "# outer",
            ] {
                assert_eq!(
                    text.matches(note).count(),
                    1,
                    "lost or duplicated {note}: {text}"
                );
            }
        }
    }
}
