//! Confirmed edits to plain TOML sources managed by chezmoi.
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions, Permissions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

#[cfg(test)]
use super::schema::SETTINGS;
use super::{Config, store};

struct Child {
    process: std::process::Child,
    running: bool,
}

impl Drop for Child {
    fn drop(&mut self) {
        if !self.running {
            return;
        }
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.process.id() as i32), libc::SIGKILL);
        }
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

#[derive(Clone)]
pub struct Chezmoi {
    program: PathBuf,
    args: Vec<OsString>,
    timeout: Duration,
}

impl Default for Chezmoi {
    fn default() -> Self {
        Self {
            program: "chezmoi".into(),
            args: Vec::new(),
            timeout: Duration::from_secs(15),
        }
    }
}

impl Chezmoi {
    fn run(&self, args: &[OsString]) -> Result<Option<String>> {
        let mut output = tempfile::tempfile()?;
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .args(["--no-tty", "--no-pager", "--refresh-externals=never"])
            .args(args)
            .stdin(Stdio::null())
            .stdout(output.try_clone()?)
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = match command.spawn() {
            Ok(process) => Child {
                process,
                running: true,
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).context("Could not start chezmoi"),
        };
        let started = Instant::now();
        let status = loop {
            if let Some(status) = child.process.try_wait()? {
                child.running = false;
                break status;
            }
            if started.elapsed() >= self.timeout {
                bail!("chezmoi timed out; changes remain available for retry");
            }
            if output.metadata()?.len() > 65536 {
                bail!("chezmoi returned an oversized response");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        if !status.success() {
            return Ok(None);
        }
        use std::io::{Seek, SeekFrom};
        output.seek(SeekFrom::Start(0))?;
        let mut text = String::new();
        output.take(65537).read_to_string(&mut text)?;
        if text.len() > 65536 {
            bail!("chezmoi returned an oversized response");
        }
        Ok(Some(text))
    }

    fn source(&self, target: &Path) -> Result<Option<PathBuf>> {
        Ok(self
            .run(&["source-path".into(), "--".into(), target.into()])?
            .map(|text| PathBuf::from(text.trim_end_matches(['\r', '\n']))))
    }
}

struct Snapshot {
    path: PathBuf,
    resolved: PathBuf,
    text: String,
    permissions: Permissions,
}

impl Snapshot {
    fn read(path: &Path) -> Result<Self> {
        let resolved = fs::canonicalize(path).context("Config source or target is missing")?;
        let metadata = fs::metadata(&resolved)?;
        if !metadata.is_file() {
            bail!("Config source and target must be regular files");
        }
        Ok(Self {
            path: path.into(),
            resolved,
            text: fs::read_to_string(path)?,
            permissions: metadata.permissions(),
        })
    }

    fn unchanged(&self) -> Result<()> {
        let now = Self::read(&self.path)?;
        if now.resolved != self.resolved
            || now.text != self.text
            || now.permissions != self.permissions
        {
            bail!(
                "Config source or target changed while confirming; retry to review the latest changes"
            );
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct Verification {
    target: PathBuf,
    permissions: Permissions,
}

pub struct Request {
    before: Config,
    after: Config,
    verification: Option<Verification>,
}

impl Request {
    pub(super) fn new(before: Config, after: Config, verification: Option<Verification>) -> Self {
        Self {
            before,
            after,
            verification,
        }
    }

    pub fn discover(mut self, tool: Chezmoi) -> Result<Option<Plan>> {
        let target = Snapshot::read(&self.after.path)?;
        let verification = self.verification.unwrap_or_else(|| Verification {
            target: target.resolved.clone(),
            permissions: target.permissions.clone(),
        });
        if verification.target != target.resolved {
            bail!("The previously approved target changed; restore it or revert before saving");
        }
        let Some(source) = tool.source(&target.path)? else {
            return Ok(None);
        };
        if fs::symlink_metadata(&target.path)?.file_type().is_symlink() {
            bail!(
                "This config is a symlink. Changes remain session-only; edit its managed source directly."
            );
        }
        let name = source
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if name.ends_with(".tmpl")
            || [
                "encrypted_",
                "create_",
                "modify_",
                "remove_",
                "run_",
                "symlink_",
            ]
            .iter()
            .any(|prefix| name.starts_with(prefix))
        {
            bail!(
                "This chezmoi source is templated or generated. Changes are session-only; edit its source to keep them."
            );
        }
        let source = Snapshot::read(&source)?;
        if store::readonly(&source.resolved, &fs::metadata(&source.resolved)?) {
            bail!("The chezmoi source is read-only too. Changes remain session-only.");
        }
        let mut disk = store::parse(&target.path, &target.text)?;
        let mut original = store::parse(&source.path, &source.text)?;
        Config::align_account_names(&mut [
            &mut original,
            &mut disk,
            &mut self.after,
            &mut self.before,
        ]);
        let before = self.before.entries();
        let after = self.after.entries();
        let disk_values = disk.entries();
        let source_values = original.entries();
        let keys: std::collections::BTreeSet<_> = before
            .keys()
            .chain(after.keys())
            .chain(disk_values.keys())
            .collect();
        let mut updates = Vec::new();
        for key in keys {
            if !Config::same_entry(key, before.get(key), disk_values.get(key)) {
                bail!("Config changed; retry to review the latest values");
            }
            let before = before.get(key);
            let after = after.get(key);
            if Config::same_entry(key, before, after) {
                continue;
            }
            if !Config::same_entry(key, source_values.get(key), before)
                && !Config::same_entry(key, source_values.get(key), after)
            {
                bail!(
                    "{} changed in the chezmoi source; revert or resolve it before retrying",
                    key
                );
            }
            updates.push((key.clone(), after.cloned()));
        }
        let desired = original.with_entries(updates)?;
        let desired_values = desired.entries();
        let keys: std::collections::BTreeSet<_> =
            disk_values.keys().chain(desired_values.keys()).collect();
        let mut changes: Vec<_> = keys
            .into_iter()
            .filter_map(|key| {
                let before = disk_values.get(key);
                let after = desired_values.get(key);
                let shown = |value: Option<&toml::Value>| {
                    value.map_or_else(|| "(absent)".into(), ToString::to_string)
                };
                (before != after).then(|| format!("{}: {} → {}", key, shown(before), shown(after)))
            })
            .collect();
        if target.permissions != verification.permissions {
            changes.push("Restore the previously approved file permissions".into());
        }
        if changes.is_empty() {
            changes.push("Verify the saved settings and file permissions".into());
        }
        let updated = store::patch(&source.text, &original, &desired)?;
        Ok(Some(Plan {
            source,
            target,
            updated,
            expected: desired,
            changes,
            tool,
            verification,
        }))
    }
}

pub struct Plan {
    source: Snapshot,
    target: Snapshot,
    updated: String,
    expected: Config,
    pub changes: Vec<String>,
    tool: Chezmoi,
    verification: Verification,
}

impl Plan {
    pub fn verification(&self) -> Verification {
        self.verification.clone()
    }
    pub fn source(&self) -> &Path {
        &self.source.path
    }
    pub fn target(&self) -> &Path {
        &self.target.path
    }

    pub fn apply(self) -> Result<()> {
        let lock_path = self.target.resolved.with_file_name(format!(
            "{}.lock",
            self.target.resolved.file_name().unwrap().to_string_lossy()
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
        self.source.unchanged()?;
        self.target.unchanged()?;
        if self.tool.source(&self.target.path)?.as_deref() != Some(&self.source.path) {
            bail!("The chezmoi source changed; retry to review it");
        }
        let mut temp = tempfile::NamedTempFile::new_in(self.source.resolved.parent().unwrap())?;
        temp.as_file()
            .set_permissions(self.source.permissions.clone())?;
        temp.write_all(self.updated.as_bytes())?;
        temp.as_file().sync_all()?;
        self.source.unchanged()?;
        self.target.unchanged()?;
        temp.persist(&self.source.resolved)
            .context("Could not update the chezmoi source")?;
        File::open(self.source.resolved.parent().unwrap())?.sync_all()?;
        self.apply_target().context(
            "Source updated; target save not verified. Changes remain available; retry saving",
        )
    }

    fn apply_target(&self) -> Result<()> {
        if self
            .tool
            .run(&[
                "apply".into(),
                "--include=files".into(),
                "--recursive=false".into(),
                "--".into(),
                self.target.path.clone().into(),
            ])?
            .is_none()
        {
            bail!("chezmoi apply failed");
        }
        let actual = Snapshot::read(&self.target.path)?;
        let config = store::parse(&actual.path, &actual.text)?;
        if actual.resolved != self.target.resolved
            || actual.permissions != self.verification.permissions
            || config.entries() != self.expected.entries()
        {
            bail!("Applied config differs from the confirmed settings or permissions");
        }
        Ok(())
    }
}

#[cfg(all(test, unix))]
pub(crate) mod tests {
    use super::*;
    use crate::config::store::{Persistence, Store};
    use std::os::unix::fs::{PermissionsExt, symlink};

    pub struct Fixture {
        pub directory: tempfile::TempDir,
        pub source: PathBuf,
        pub target: PathBuf,
        pub tool: Chezmoi,
    }

    fn quote(path: &Path) -> String {
        format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
    }

    impl Fixture {
        pub fn new() -> Self {
            Self::named("readonly_réglages file.toml", "réglages file.toml")
        }

        fn named(source_name: &str, target_name: &str) -> Self {
            let directory = tempfile::tempdir().unwrap();
            let source = directory.path().join(source_name);
            let target = directory.path().join(target_name);
            let text = "# mine\n[media]\nimage_preview = true # keep\n[accounts.other.filters]\nkeywords = [\"old\"] # keep array\n";
            fs::write(&source, text).unwrap();
            fs::write(&target, text).unwrap();
            fs::set_permissions(&source, Permissions::from_mode(0o640)).unwrap();
            fs::set_permissions(&target, Permissions::from_mode(0o444)).unwrap();
            let program = directory.path().join("managed-config-probe");
            // This fake implements the documented source-path and target-only apply subset.
            let script = format!(
                r#"#!/bin/sh
source={source}
target={target}
root={root}
action=
for arg in "$@"; do
    case "$arg" in source-path|apply) action="$arg";; esac
    last="$arg"
done
test "$last" = "$target" || exit 2
test ! -f "$root/slow" || sleep 10
case "$action" in
source-path) test ! -f "$root/unmanaged" || exit 1
    if test -f "$root/source-override"; then cat "$root/source-override"; else printf '%s\n' "$source"; fi;;
apply) test ! -f "$root/fail" || exit 1
    printf '%s\n' "$@" > "$root/applied-args"
    test ! -f "$root/noop" || exit 0
    cp "$source" "$target.next" || exit 1
    if test -f "$root/writable"; then chmod 600 "$target.next"; else chmod 444 "$target.next"; fi
    mv "$target.next" "$target";;
*) exit 2;;
esac
"#,
                source = quote(&source),
                target = quote(&target),
                root = quote(directory.path())
            );
            fs::write(&program, script).unwrap();
            fs::set_permissions(&program, Permissions::from_mode(0o700)).unwrap();
            let tool = Chezmoi {
                program,
                args: Vec::new(),
                timeout: Duration::from_secs(2),
            };
            Self {
                directory,
                source,
                target,
                tool,
            }
        }

        pub fn store(&self) -> Store {
            Store::open(Some(&self.target)).unwrap()
        }

        pub fn change(&self, store: &mut Store) {
            store
                .change(
                    SETTINGS
                        .iter()
                        .find(|f| f.key == "media.image_preview")
                        .unwrap(),
                    toml::Value::Boolean(false),
                )
                .unwrap();
        }

        pub fn plan(&self, store: &Store) -> Plan {
            store
                .managed_request()
                .unwrap()
                .discover(self.tool.clone())
                .unwrap()
                .unwrap()
        }

        fn save(&self, store: &mut Store) {
            let plan = self.plan(store);
            store.managed_finished(plan.verification(), plan.apply());
        }
    }

    #[test]
    fn when_username_rules_are_saved_through_chezmoi_the_account_scope_should_survive() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        store.select_account(Some("Reader_One-Two"));
        store
            .change(
                SETTINGS
                    .iter()
                    .find(|f| f.key == "filters.keywords")
                    .unwrap(),
                toml::Value::Array(vec!["mine".into()]),
            )
            .unwrap();

        fixture.save(&mut store);

        assert_eq!(store.persistence, Persistence::Saved);
        assert_eq!(
            store.current.accounts["Reader_One-Two"].filters.keywords,
            ["mine"]
        );
        assert_eq!(
            fs::read(&fixture.source).unwrap(),
            fs::read(&fixture.target).unwrap()
        );
        assert_eq!(
            fs::metadata(&fixture.target).unwrap().permissions().mode() & 0o777,
            0o444
        );
    }

    #[test]
    fn when_username_source_case_differs_managed_saves_should_merge_under_source_spelling() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        store.select_account(Some("Reader"));
        store
            .change(
                SETTINGS
                    .iter()
                    .find(|f| f.key == "filters.keywords")
                    .unwrap(),
                toml::Value::Array(vec!["mine".into()]),
            )
            .unwrap();
        let mut source = fs::read_to_string(&fixture.source).unwrap();
        source.push_str("\n[accounts.reader.filters]\nauthors=[\"bot\"]\n");
        fs::write(&fixture.source, source).unwrap();

        fixture.save(&mut store);

        assert_eq!(store.persistence, Persistence::Saved);
        assert_eq!(store.current.accounts.len(), 2);
        assert_eq!(store.current.accounts["reader"].filters.authors, ["bot"]);
        assert_eq!(store.current.accounts["reader"].filters.keywords, ["mine"]);
    }

    #[test]
    fn when_discovered_managed_changes_should_wait_for_confirmation() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        fixture.change(&mut store);
        let before = fs::read_to_string(&fixture.source).unwrap();

        let plan = fixture.plan(&store);

        assert_eq!(plan.changes, ["media.image_preview: true → false"]);
        assert_eq!(plan.source(), fixture.source);
        assert_eq!(fs::read_to_string(&fixture.source).unwrap(), before);
        assert_eq!(fs::read_to_string(&fixture.target).unwrap(), before);
        assert!(!fixture.directory.path().join("applied-args").exists());
        assert_eq!(store.persistence, Persistence::Temporary);
        assert!(!store.current.media.image_preview);
    }

    #[test]
    fn when_plain_source_names_contain_attribute_words_it_should_offer_confirmation() {
        for (source, target) in [
            ("readonly_config_run_mode.toml", "config_run_mode.toml"),
            ("readonly_literal_run_config.toml", "run_config.toml"),
            ("readonly_run_config.toml", "run_config.toml"),
            ("readonly_script_config.toml", "script_config.toml"),
        ] {
            let fixture = Fixture::named(source, target);
            let mut store = fixture.store();
            fixture.change(&mut store);

            let result = store
                .managed_request()
                .unwrap()
                .discover(fixture.tool.clone());

            assert!(result.is_ok(), "{source}: {:?}", result.err());
            assert!(result.unwrap().is_some(), "{source}");
            assert!(
                Config::load(Some(&fixture.source))
                    .unwrap()
                    .media
                    .image_preview
            );
        }
    }

    #[test]
    fn when_confirmed_managed_save_should_keep_comments_permissions_and_other_source_edits() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        fixture.change(&mut store);
        fs::write(&fixture.source, "# mine\nall_subs_posts_per_group = 4\n[media]\nimage_preview = true # keep\n[accounts.other.filters]\nkeywords = [\"old\"] # keep array\n").unwrap();

        let plan = fixture.plan(&store);
        assert_eq!(
            plan.changes,
            [
                "all_subs_posts_per_group: 3 → 4",
                "media.image_preview: true → false"
            ]
        );
        store.managed_finished(plan.verification(), plan.apply());

        assert_eq!(store.persistence, Persistence::Saved);
        assert!(!store.current.media.image_preview && store.current.all_subs_posts_per_group == 4);
        assert_eq!(
            fs::read_to_string(&fixture.source).unwrap(),
            "# mine\nall_subs_posts_per_group = 4\n[media]\nimage_preview = false # keep\n[accounts.other.filters]\nkeywords = [\"old\"] # keep array\n"
        );
        assert_eq!(
            fs::read(&fixture.target).unwrap(),
            fs::read(&fixture.source).unwrap()
        );
        assert_eq!(
            fs::metadata(&fixture.source).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert_eq!(
            fs::metadata(&fixture.target).unwrap().permissions().mode() & 0o777,
            0o444
        );
        let args = fs::read_to_string(fixture.directory.path().join("applied-args")).unwrap();
        assert!(args.contains("--include=files\n--recursive=false\n--\n"));
    }

    #[test]
    fn when_source_changes_after_confirmation_it_should_remain_untouched() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        fixture.change(&mut store);
        let plan = fixture.plan(&store);
        fs::write(&fixture.source, "# external edit\n").unwrap();

        store.managed_finished(plan.verification(), plan.apply());

        assert_eq!(
            fs::read_to_string(&fixture.source).unwrap(),
            "# external edit\n"
        );
        assert!(
            store
                .error
                .as_ref()
                .unwrap()
                .contains("changed while confirming")
        );
        assert_eq!(store.persistence, Persistence::Unsaved);
        assert!(!store.current.media.image_preview);
        assert!(!fixture.directory.path().join("applied-args").exists());
    }

    #[test]
    fn when_target_is_retargeted_after_confirmation_it_should_preserve_the_new_file() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        fixture.change(&mut store);
        let plan = fixture.plan(&store);
        let other = fixture.directory.path().join("other.toml");
        fs::write(&other, "# different target\n").unwrap();
        fs::remove_file(&fixture.target).unwrap();
        symlink(&other, &fixture.target).unwrap();

        let result = plan.apply();

        assert!(result.is_err());
        assert_eq!(fs::read_to_string(&other).unwrap(), "# different target\n");
        assert!(
            fs::symlink_metadata(&fixture.target)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(!fixture.directory.path().join("applied-args").exists());
    }

    #[test]
    fn when_apply_fails_it_should_keep_source_intent_and_allow_a_verified_retry() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        fixture.change(&mut store);
        fs::write(fixture.directory.path().join("fail"), "").unwrap();
        fixture.save(&mut store);
        assert_eq!(store.persistence, Persistence::Unsaved);
        assert!(store.error.as_ref().unwrap().contains("Source updated"));
        assert!(
            !Config::load(Some(&fixture.source))
                .unwrap()
                .media
                .image_preview
        );
        assert!(
            Config::load(Some(&fixture.target))
                .unwrap()
                .media
                .image_preview
        );
        fs::remove_file(fixture.directory.path().join("fail")).unwrap();
        store.save();

        fixture.save(&mut store);

        assert_eq!(store.persistence, Persistence::Saved);
        assert!(
            !Config::load(Some(&fixture.target))
                .unwrap()
                .media
                .image_preview
        );
    }

    #[test]
    fn when_apply_returns_success_without_the_values_it_should_not_report_saved() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        fixture.change(&mut store);
        fs::write(fixture.directory.path().join("noop"), "").unwrap();

        fixture.save(&mut store);

        assert_eq!(store.persistence, Persistence::Unsaved);
        assert!(store.error.as_ref().unwrap().contains("differs"));
        assert!(
            Config::load(Some(&fixture.target))
                .unwrap()
                .media
                .image_preview
        );
    }

    #[test]
    fn when_the_manager_cannot_find_a_source_it_should_keep_session_changes() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        fixture.change(&mut store);
        fs::write(fixture.directory.path().join("unmanaged"), "").unwrap();

        let actual = store
            .managed_request()
            .unwrap()
            .discover(fixture.tool.clone())
            .unwrap();

        assert!(actual.is_none());
        assert_eq!(store.persistence, Persistence::Temporary);
        assert!(!store.current.media.image_preview);
    }

    #[test]
    fn when_chezmoi_is_missing_it_should_keep_session_changes() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        fixture.change(&mut store);
        let tool = Chezmoi {
            program: fixture.directory.path().join("absent"),
            ..fixture.tool.clone()
        };

        let actual = store.managed_request().unwrap().discover(tool).unwrap();

        assert!(actual.is_none());
        assert_eq!(store.persistence, Persistence::Temporary);
    }

    #[test]
    fn when_a_manager_stalls_it_should_time_out_without_writing() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        fixture.change(&mut store);
        fs::write(fixture.directory.path().join("slow"), "").unwrap();
        let tool = Chezmoi {
            timeout: Duration::from_millis(30),
            ..fixture.tool.clone()
        };

        let actual = store.managed_request().unwrap().discover(tool);

        assert!(actual.err().unwrap().to_string().contains("timed out"));
        assert!(
            Config::load(Some(&fixture.source))
                .unwrap()
                .media
                .image_preview
        );
        assert!(!fixture.directory.path().join("applied-args").exists());
    }

    #[test]
    fn when_source_is_templated_it_should_explain_session_only_without_rewriting_it() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        fixture.change(&mut store);
        let source = fixture.directory.path().join("readonly_config.toml.tmpl");
        fs::write(&source, "[media]\nimage_preview = {{ .autoplay }}\n").unwrap();
        fs::write(
            fixture.directory.path().join("source-override"),
            source.to_str().unwrap(),
        )
        .unwrap();

        let result = store
            .managed_request()
            .unwrap()
            .discover(fixture.tool.clone());

        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("templated or generated")
        );
        assert_eq!(
            fs::read_to_string(&source).unwrap(),
            "[media]\nimage_preview = {{ .autoplay }}\n"
        );
        assert_eq!(store.persistence, Persistence::Temporary);
    }

    #[test]
    fn when_source_is_read_only_it_should_leave_it_and_the_target_unchanged() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        fixture.change(&mut store);
        fs::set_permissions(&fixture.source, Permissions::from_mode(0o444)).unwrap();

        let result = store
            .managed_request()
            .unwrap()
            .discover(fixture.tool.clone());

        assert!(result.err().unwrap().to_string().contains("read-only too"));
        assert!(
            Config::load(Some(&fixture.source))
                .unwrap()
                .media
                .image_preview
        );
        assert!(
            Config::load(Some(&fixture.target))
                .unwrap()
                .media
                .image_preview
        );
    }

    #[test]
    fn when_a_generated_source_parses_as_toml_it_should_still_be_rejected() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        fixture.change(&mut store);
        let source = fixture.directory.path().join("run_config.toml");
        fs::write(&source, "# script with no commands\n").unwrap();
        fs::write(
            fixture.directory.path().join("source-override"),
            source.to_str().unwrap(),
        )
        .unwrap();

        let result = store
            .managed_request()
            .unwrap()
            .discover(fixture.tool.clone());

        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("templated or generated")
        );
        assert_eq!(
            fs::read_to_string(&source).unwrap(),
            "# script with no commands\n"
        );
        assert_eq!(store.persistence, Persistence::Temporary);
    }

    #[test]
    fn when_source_has_a_conflicting_value_it_should_require_resolution() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        let field = SETTINGS
            .iter()
            .find(|f| f.key == "all_subs_posts_per_group")
            .unwrap();
        store.change(field, toml::Value::Integer(4)).unwrap();
        fs::write(&fixture.source, "all_subs_posts_per_group = 5\n").unwrap();

        let result = store
            .managed_request()
            .unwrap()
            .discover(fixture.tool.clone());

        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("all_subs_posts_per_group changed")
        );
        assert_eq!(
            fs::read_to_string(&fixture.source).unwrap(),
            "all_subs_posts_per_group = 5\n"
        );
        assert_eq!(store.current.all_subs_posts_per_group, 4);
    }

    #[test]
    fn when_target_is_writable_it_should_save_directly_without_a_managed_plan() {
        let fixture = Fixture::new();
        fs::set_permissions(&fixture.target, Permissions::from_mode(0o600)).unwrap();
        let mut store = fixture.store();

        fixture.change(&mut store);

        assert_eq!(store.persistence, Persistence::Saved);
        assert!(store.managed_request().is_none());
        assert!(
            Config::load(Some(&fixture.source))
                .unwrap()
                .media
                .image_preview
        );
        assert!(
            !Config::load(Some(&fixture.target))
                .unwrap()
                .media
                .image_preview
        );
    }

    #[test]
    fn when_verification_fails_a_reload_should_not_hide_the_save_failure() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        fixture.change(&mut store);
        fs::write(fixture.directory.path().join("writable"), "").unwrap();
        fixture.save(&mut store);
        assert_eq!(store.persistence, Persistence::Unsaved);

        store.reload();

        assert_eq!(store.persistence, Persistence::Unsaved);
        assert!(store.error.as_ref().unwrap().contains("permissions"));
        store.save();
        assert_ne!(
            store.persistence,
            Persistence::Saved,
            "retry still needs verification"
        );
        fixture.save(&mut store);
        assert_eq!(
            store.persistence,
            Persistence::Unsaved,
            "retry must retain the original permissions"
        );
        fs::remove_file(fixture.directory.path().join("writable")).unwrap();
        store.save();

        fixture.save(&mut store);

        assert_eq!(store.persistence, Persistence::Saved);
        assert_eq!(
            fs::metadata(&fixture.target).unwrap().permissions().mode() & 0o777,
            0o444
        );
    }

    #[test]
    fn when_reverting_an_unverified_save_it_should_explicitly_accept_the_current_file() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        fixture.change(&mut store);
        fs::write(fixture.directory.path().join("writable"), "").unwrap();
        fixture.save(&mut store);

        store.revert();

        assert_eq!(store.persistence, Persistence::Saved);
        assert!(store.error.is_none());
        assert!(store.managed_request().is_none());
        assert!(!store.current.media.image_preview);
    }

    #[test]
    fn when_source_is_deleted_after_confirmation_it_should_not_be_recreated() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        fixture.change(&mut store);
        let plan = fixture.plan(&store);
        fs::remove_file(&fixture.source).unwrap();

        let result = plan.apply();

        assert!(result.is_err());
        assert!(!fixture.source.exists());
        assert!(
            Config::load(Some(&fixture.target))
                .unwrap()
                .media
                .image_preview
        );
    }

    #[test]
    fn when_a_confirmed_source_becomes_read_only_it_should_not_bypass_its_permissions() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        fixture.change(&mut store);
        let plan = fixture.plan(&store);
        fs::set_permissions(&fixture.source, Permissions::from_mode(0o444)).unwrap();

        let result = plan.apply();

        assert!(result.is_err());
        assert!(
            Config::load(Some(&fixture.source))
                .unwrap()
                .media
                .image_preview
        );
        assert_eq!(
            fs::metadata(&fixture.source).unwrap().permissions().mode() & 0o777,
            0o444
        );
    }

    #[test]
    fn when_an_empty_config_is_saved_twice_it_should_keep_the_latest_value_once() {
        let fixture = Fixture::new();
        fs::write(&fixture.source, "").unwrap();
        fs::set_permissions(&fixture.target, Permissions::from_mode(0o600)).unwrap();
        fs::write(&fixture.target, "").unwrap();
        fs::set_permissions(&fixture.target, Permissions::from_mode(0o444)).unwrap();
        let mut store = fixture.store();
        fixture.change(&mut store);
        fixture.save(&mut store);
        assert_eq!(store.persistence, Persistence::Saved);
        store
            .change(
                SETTINGS
                    .iter()
                    .find(|f| f.key == "media.image_preview")
                    .unwrap(),
                toml::Value::Boolean(true),
            )
            .unwrap();

        fixture.save(&mut store);

        assert_eq!(store.persistence, Persistence::Saved);
        assert_eq!(
            fs::read_to_string(&fixture.source).unwrap(),
            "[media]\nimage_preview = true\n"
        );
        assert!(store.current.media.image_preview);
    }

    #[test]
    fn when_installed_chezmoi_applies_an_isolated_source_it_should_change_only_that_target() {
        if Command::new("chezmoi").arg("--version").output().is_err() {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let source_dir = directory.path().join("source");
        let destination = directory.path().join("destination");
        fs::create_dir(&source_dir).unwrap();
        fs::create_dir(&destination).unwrap();
        let source = source_dir.join("readonly_config_run_mode.toml");
        let target = destination.join("config_run_mode.toml");
        fs::write(
            &source,
            "# actual chezmoi fixture\n[media]\nimage_preview = true # keep\n",
        )
        .unwrap();
        fs::write(&target, fs::read(&source).unwrap()).unwrap();
        fs::set_permissions(&target, Permissions::from_mode(0o444)).unwrap();
        fs::write(source_dir.join("other.toml"), "must not be applied\n").unwrap();
        let config = directory.path().join("chezmoi.toml");
        fs::write(&config, "").unwrap();
        let tool = Chezmoi {
            program: "chezmoi".into(),
            timeout: Duration::from_secs(5),
            args: vec![
                "--source".into(),
                source_dir.into(),
                "--destination".into(),
                destination.clone().into(),
                "--config".into(),
                config.into(),
                "--cache".into(),
                directory.path().join("cache").into(),
                "--persistent-state".into(),
                directory.path().join("state").into(),
            ],
        };
        let mut store = Store::open(Some(&target)).unwrap();
        store
            .change(
                SETTINGS
                    .iter()
                    .find(|f| f.key == "media.image_preview")
                    .unwrap(),
                toml::Value::Boolean(false),
            )
            .unwrap();
        let plan = store
            .managed_request()
            .unwrap()
            .discover(tool)
            .unwrap()
            .unwrap();

        store.managed_finished(plan.verification(), plan.apply());

        assert_eq!(store.persistence, Persistence::Saved, "{:?}", store.error);
        assert_eq!(
            fs::read_to_string(&source).unwrap(),
            "# actual chezmoi fixture\n[media]\nimage_preview = false # keep\n"
        );
        assert!(!Config::load(Some(&target)).unwrap().media.image_preview);
        assert!(!destination.join("other.toml").exists());
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o444
        );
    }
}
