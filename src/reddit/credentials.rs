//! Session payloads live in the OS credential store, never in a cookie file.

#[cfg(not(target_os = "macos"))]
use std::io::Write;
#[cfg(any(not(target_os = "macos"), test))]
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;

const PREFIX: &str = "redtab-session-v1:";
const SERVICE: &str = "redtab";
const ACCOUNT: &str = "reddit-session";
const MISSING: &str = "Cannot access saved Reddit accounts; unlock the OS credential store and allow access, then retry";

pub(super) fn load() -> Result<Option<String>> {
    read_store()?.map(|payload| decode(&payload)).transpose()
}

pub(super) fn save(text: &str) -> Result<()> {
    let payload = format!("{PREFIX}{}", STANDARD.encode(text));
    write_store(payload.as_bytes())
}

#[cfg(target_os = "macos")]
pub(super) fn remove() -> Result<()> {
    remove_result(security_framework::passwords::delete_generic_password(
        SERVICE, ACCOUNT,
    ))
}

#[cfg(target_os = "macos")]
fn remove_result(result: security_framework::base::Result<()>) -> Result<()> {
    const ITEM_NOT_FOUND: i32 = -25300;
    match result {
        Ok(()) => Ok(()),
        Err(error) if error.code() == ITEM_NOT_FOUND => Ok(()),
        Err(_) => anyhow::bail!(
            "Cannot remove Reddit sign-in; unlock macOS Keychain and allow access, then retry"
        ),
    }
}

#[cfg(not(target_os = "macos"))]
pub(super) fn remove() -> Result<()> {
    remove_with(Command::new("secret-tool"))
}

#[cfg(any(not(target_os = "macos"), test))]
fn remove_with(mut command: Command) -> Result<()> {
    let status = command
        .args(["clear", "application", SERVICE, "account", ACCOUNT])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("Cannot open Linux Secret Service; install secret-tool and unlock your keyring")?;
    if !status.success() {
        anyhow::bail!("Cannot remove Reddit sign-in; unlock your desktop keyring and retry");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn read_store() -> Result<Option<Vec<u8>>> {
    match security_framework::passwords::get_generic_password(SERVICE, ACCOUNT) {
        Ok(payload) => Ok(Some(payload)),
        Err(error) if error.code() == -25300 => Ok(None),
        Err(_) => anyhow::bail!(MISSING),
    }
}

#[cfg(target_os = "macos")]
fn write_store(payload: &[u8]) -> Result<()> {
    security_framework::passwords::set_generic_password(SERVICE, ACCOUNT, payload).map_err(|_| {
        anyhow::anyhow!("cannot save Reddit sign-in; unlock macOS Keychain and try again")
    })
}

fn decode(bytes: &[u8]) -> Result<String> {
    if bytes.len() > crate::auth_browser::PACKET_LIMIT as usize {
        anyhow::bail!("Saved accounts exceed the credential-store size limit");
    }
    let payload = std::str::from_utf8(bytes)
        .ok()
        .and_then(|s| s.trim().strip_prefix(PREFIX))
        .context("invalid stored Reddit session; import it again with redtab --signin")?;
    let decoded = STANDARD
        .decode(payload)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .context("invalid stored Reddit session; import it again with redtab --signin")?;
    Ok(decoded)
}

#[cfg(not(target_os = "macos"))]
fn read_store() -> Result<Option<Vec<u8>>> {
    read_with(Command::new("secret-tool"), Command::new("secret-tool"))
}

#[cfg(any(not(target_os = "macos"), test))]
fn read_with(mut lookup: Command, mut search: Command) -> Result<Option<Vec<u8>>> {
    let output = lookup
        .args(["lookup", "application", SERVICE, "account", ACCOUNT])
        .stdin(Stdio::null())
        .output()
        .map_err(|_| anyhow::anyhow!(MISSING))?;
    if output.status.success() {
        return Ok(Some(output.stdout));
    }
    if output.status.code() != Some(1) || !output.stderr.is_empty() {
        anyhow::bail!(MISSING);
    }
    // lookup also returns 1 for a locked item. Only an empty successful search proves absence.
    let output = search
        .args([
            "search",
            "--all",
            "application",
            SERVICE,
            "account",
            ACCOUNT,
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|_| anyhow::anyhow!(MISSING))?;
    if output.status.success() && output.stdout.is_empty() && output.stderr.is_empty() {
        return Ok(None);
    }
    anyhow::bail!(MISSING)
}

#[cfg(not(target_os = "macos"))]
fn write_store(payload: &[u8]) -> Result<()> {
    check_secret_tool_size(payload)?;
    let mut child = Command::new("secret-tool")
        .args([
            "store",
            "--label=Redtab Reddit session",
            "application",
            SERVICE,
            "account",
            ACCOUNT,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("cannot open Linux Secret Service; install secret-tool and unlock your keyring")?;
    let written = child.stdin.take().expect("piped stdin").write_all(payload);
    let status = child
        .wait()
        .context("cannot wait for Linux Secret Service")?;
    if written.is_err() || !status.success() {
        anyhow::bail!("cannot save Reddit sign-in; unlock your keyring and try again");
    }
    Ok(())
}

#[cfg(any(not(target_os = "macos"), test))]
fn check_secret_tool_size(payload: &[u8]) -> Result<()> {
    // secret-tool reads at most 8192 bytes from stdin; a larger write can truncate silently.
    if payload.len() > 8192 {
        anyhow::bail!(
            "Saved accounts exceed secret-tool's 8 KiB input limit; remove an unused saved account first"
        );
    }
    Ok(())
}

pub(super) fn lock() -> Result<std::fs::File> {
    let home = std::env::var_os("HOME").context("Cannot locate the account-store lock")?;
    let home = std::path::PathBuf::from(home);
    if !home.is_absolute() {
        anyhow::bail!("Cannot locate the account-store lock");
    }
    #[cfg(target_os = "macos")]
    let relative = "Library/Caches/redtab/accounts.lock";
    #[cfg(not(target_os = "macos"))]
    let relative = ".cache/redtab/accounts.lock";
    // The OS entry is shared across XDG profiles, so its lock must be shared too.
    lock_at(&home.join(relative))
}

fn lock_at(path: &std::path::Path) -> Result<std::fs::File> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
    let error =
        || anyhow::anyhow!("Cannot lock saved accounts; check access to the user cache directory");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path.parent().context("Missing account-lock directory")?)
        .map_err(|_| error())?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| error())?;
    let metadata = file.metadata().map_err(|_| error())?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::getuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(error());
    }
    let started = std::time::Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock)
                if started.elapsed() < std::time::Duration::from_secs(2) =>
            {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(std::fs::TryLockError::WouldBlock) => anyhow::bail!(
                "Saved accounts are busy; wait for the other redtab connection change and retry"
            ),
            Err(_) => return Err(error()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_probe(output: &str, diagnostic: &str, exit: u8) -> (tempfile::TempDir, Command) {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("probe_read");
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\nprintf '%s' '{output}'\nprintf '%s' '{diagnostic}' >&2\nexit {exit}\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let command = Command::new(&path);
        (directory, command)
    }

    #[test]
    fn when_linux_confirms_no_matching_items_it_should_return_a_missing_store() {
        let (_lookup, lookup) = read_probe("", "", 1);
        let (_search, search) = read_probe("", "", 0);
        assert_eq!(read_with(lookup, search).unwrap(), None);
    }

    #[test]
    fn when_linux_has_a_locked_matching_item_it_should_not_treat_it_as_a_new_store() {
        let (_lookup, lookup) = read_probe("", "", 1);
        let (_search, search) =
            read_probe("[item/1]\nlabel = Redtab", "locked-private-diagnostic", 0);
        let actual = read_with(lookup, search).unwrap_err().to_string();
        assert_eq!(actual, MISSING);
        assert!(!actual.contains("private-diagnostic"));
    }

    #[test]
    fn when_linux_lookup_succeeds_it_should_read_the_payload_without_searching_again() {
        let (_lookup, lookup) = read_probe("private-payload", "", 0);
        let search = Command::new("/does/not/exist");
        assert_eq!(
            read_with(lookup, search).unwrap(),
            Some(b"private-payload".to_vec())
        );
    }

    #[test]
    fn when_linux_reports_a_lookup_error_it_should_not_echo_diagnostics_or_assume_absence() {
        let (_lookup, lookup) = read_probe("", "private-provider-diagnostic", 1);
        let search = Command::new("/does/not/exist");
        assert_eq!(read_with(lookup, search).unwrap_err().to_string(), MISSING);
    }

    #[test]
    fn when_linux_search_fails_it_should_not_assume_absence() {
        let (_lookup, lookup) = read_probe("", "", 1);
        let (_search, search) = read_probe("", "", 1);
        assert!(read_with(lookup, search).is_err());
    }

    #[test]
    fn when_a_payload_fits_secret_tools_input_buffer_it_should_be_accepted() {
        assert!(check_secret_tool_size(&vec![b'x'; 8192]).is_ok());
    }

    #[test]
    fn when_a_payload_would_be_truncated_by_secret_tool_it_should_be_rejected_before_writing() {
        assert!(check_secret_tool_size(&vec![b'x'; 8193]).is_err());
    }

    #[test]
    fn when_two_writers_use_the_same_lock_it_should_serialize_them_and_release_on_drop() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("帳戶/accounts.lock");
        let holder = lock_at(&path).unwrap();
        let contender = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        assert!(matches!(
            contender.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        drop(holder);
        // A process another test is spawning can briefly hold a copy of the descriptor,
        // so the release is awaited as `lock_at` itself waits.
        let started = std::time::Instant::now();
        while contender.try_lock().is_err() {
            assert!(started.elapsed() < std::time::Duration::from_secs(2));
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn when_the_lock_path_is_an_escaping_symlink_it_should_not_follow_it() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("other");
        std::fs::write(&target, b"keep").unwrap();
        let path = directory.path().join("accounts.lock");
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(lock_at(&path).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"keep");
    }

    #[test]
    fn when_a_previous_lock_file_has_been_deleted_it_should_create_a_new_private_lock() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("deleted/accounts.lock");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"old lock").unwrap();
        std::fs::remove_file(&path).unwrap();
        let actual = lock_at(&path).unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            actual.metadata().unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    fn delete_probe(exit: u8) -> (tempfile::TempDir, Command, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("probe_delete");
        let record = directory.path().join("arguments");
        std::fs::write(&executable, format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$PROBE_RECORD\"\nprintf 'provider diagnostics' >&2\nexit {exit}\n"
        )).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut command = Command::new(executable);
        command.env("PROBE_RECORD", &record);
        (directory, command, record)
    }

    #[test]
    fn when_linux_disconnects_it_should_clear_only_the_redtab_service_account() {
        let (_directory, command, record) = delete_probe(0);
        remove_with(command).unwrap();
        assert_eq!(
            std::fs::read_to_string(record).unwrap(),
            "clear\napplication\nredtab\naccount\nreddit-session\n"
        );
    }

    #[test]
    fn when_linux_cannot_confirm_deletion_it_should_fail_without_provider_diagnostics() {
        let (_directory, command, _record) = delete_probe(1);
        let actual = remove_with(command).unwrap_err().to_string();
        assert!(actual.contains("unlock your desktop keyring"));
        assert!(!actual.contains("provider diagnostics"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn when_the_macos_entry_is_already_missing_disconnect_should_succeed() {
        let actual = remove_result(Err(security_framework::base::Error::from_code(-25300)));
        assert!(actual.is_ok());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn when_macos_denies_deletion_disconnect_should_fail() {
        let actual = remove_result(Err(security_framework::base::Error::from_code(-25293)));
        assert!(
            actual
                .unwrap_err()
                .to_string()
                .contains("unlock macOS Keychain")
        );
    }

    #[test]
    fn when_the_store_returns_a_versioned_payload_it_should_decode_in_memory() {
        assert_eq!(
            decode(b"redtab-session-v1:Zml4dHVyZS1zZXNzaW9u\n").unwrap(),
            "fixture-session"
        );
    }

    #[test]
    fn when_the_stored_payload_is_invalid_errors_should_not_include_its_contents() {
        for payload in [
            b"unknown-secret".as_slice(),
            b"redtab-session-v1:invalid-secret",
        ] {
            let actual = decode(payload).unwrap_err().to_string();
            assert!(!actual.contains("secret"));
            assert!(actual.contains("redtab --signin"));
        }
    }
}
