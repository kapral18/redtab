//! Handing URLs to other programs: browser, video player, image viewer, clipboard.

use std::io::Write;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use base64::Engine;

/// Spawns `argv + extra` with no shared stdio and reaps it in the background.
pub fn spawn_detached(argv: &[String], extra: &[String]) -> Result<()> {
    let (program, args) = argv.split_first().context("empty command in config")?;
    let mut child = Command::new(program)
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

/// Copies with the platform clipboard tool, else OSC 52.
pub fn copy(text: &str) -> Result<&'static str> {
    let candidates: &[(&str, &[&str])] = &[
        ("pbcopy", &[]),
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
    ];
    for (program, args) in candidates {
        if copy_with(program, args, text) {
            return Ok(program);
        }
    }
    osc52(text)?;
    Ok("OSC 52")
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

fn osc52(text: &str) -> Result<()> {
    if text.len() > 74_994 {
        bail!("text too long for OSC 52");
    }
    let mut out = std::io::stdout();
    write!(
        out,
        "\x1b]52;c;{}\x07",
        base64::engine::general_purpose::STANDARD.encode(text)
    )?;
    out.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
