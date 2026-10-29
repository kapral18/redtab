//! Handing URLs to other programs: browser, video player, image viewer, clipboard.

use std::io::Write;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use base64::Engine;

/// Spawns `argv + extra` with no shared stdio and reaps it in the background.
pub fn spawn_detached(argv: &[String], extra: &[String]) -> Result<()> {
    spawn(argv, extra, |program| Command::new(program))
}

/// `spawn_detached` for video players, through [`local_command`].
pub fn spawn_media(argv: &[String], extra: &[String]) -> Result<()> {
    spawn(argv, extra, local_command)
}

/// Media subprocesses receive loopback URLs. Ignore inherited system proxies for
/// that local hop; the relay alone owns upstream routing.
pub fn local_command(program: &str) -> Command {
    let mut command = Command::new(program);
    for name in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        command.env_remove(name);
    }
    command.env("NO_PROXY", "*").env("no_proxy", "*");
    command
}

fn spawn(argv: &[String], extra: &[String], command: impl FnOnce(&str) -> Command) -> Result<()> {
    let (program, args) = argv.split_first().context("empty command in config")?;
    let mut child = command(program)
        .args(args)
        .args(extra)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("cannot start {program}"))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

pub enum CopyStatus {
    Copied(&'static str),
    Queued,
}

pub fn copy(text: &str) -> Result<CopyStatus> {
    let candidates: &[(&str, &[&str])] = &[
        ("pbcopy", &[]),
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
    ];
    for (program, args) in candidates {
        if copy_with(program, args, text) {
            return Ok(CopyStatus::Copied(program));
        }
    }
    osc52(text)
}

/// Whether `program` took all of `text` and succeeded; always waited for.
fn copy_with(program: &str, args: &[&str], text: &str) -> bool {
    let Ok(mut child) = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let wrote = child
        .stdin
        .take()
        .is_some_and(|mut stdin| stdin.write_all(text.as_bytes()).is_ok());
    child.wait().is_ok_and(|status| status.success()) && wrote
}

fn osc52(text: &str) -> Result<CopyStatus> {
    if text.len() > 74_994 {
        bail!("text too long for OSC 52");
    }
    let command = format!(
        "\x1b]52;c;{}\x07",
        base64::engine::general_purpose::STANDARD.encode(text)
    );
    crate::terminal_output::queue_command(command.as_bytes())?;
    Ok(CopyStatus::Queued)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_children_ignore_system_proxies() {
        use std::ffi::OsStr;

        let command = local_command("player");

        // `get_envs` lists a variable the child will not inherit with no value.
        let envs: std::collections::HashMap<_, _> = command.get_envs().collect();
        for name in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            assert_eq!(envs.get(OsStr::new(name)), Some(&None), "{name}");
        }
        assert_eq!(envs[OsStr::new("no_proxy")], Some(OsStr::new("*")));
    }

    #[test]
    fn when_the_tool_reads_all_input_and_succeeds_the_copy_should_count() {
        assert!(copy_with(
            "sh",
            &["-c", "cat > /dev/null"],
            &"x".repeat(1 << 22)
        ));
    }

    #[test]
    fn when_the_tool_stops_reading_the_copy_should_fail_so_the_next_way_is_tried() {
        // More than any default pipe holds, so the write fails.
        assert!(!copy_with(
            "sh",
            &["-c", "exec 0<&-; exit 0"],
            &"x".repeat(1 << 22)
        ));
    }
}
