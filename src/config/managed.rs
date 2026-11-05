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
pub(crate) mod tests;
