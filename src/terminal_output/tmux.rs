use super::*;

pub fn inside_tmux() -> bool {
    std::env::var("TERM").is_ok_and(|term| term.starts_with("tmux"))
        || std::env::var("TERM_PROGRAM").is_ok_and(|program| program == "tmux")
}

pub struct TmuxPane {
    pub(super) socket: String,
    pub(super) id: String,
}

impl TmuxPane {
    pub fn for_stdout() -> Option<Self> {
        use std::os::unix::fs::MetadataExt;
        if !inside_tmux() {
            return None;
        }
        let tmux = std::env::var("TMUX").ok()?;
        let socket = tmux.rsplitn(3, ',').nth(2)?.to_owned();
        let pane = Self {
            socket,
            id: std::env::var("TMUX_PANE").ok()?,
        };
        let tty = pane.command_for_pane(&["display-message", "-p", "#{pane_tty}"])?;
        let tty = std::fs::metadata(tty.trim()).ok()?;
        let stdout = std::fs::metadata("/dev/stdout").ok()?;
        (tty.rdev() == stdout.rdev()).then_some(pane)
    }

    pub fn command_for_pane(&self, args: &[&str]) -> Option<String> {
        self.command(&[&args[..1], &["-t", &self.id], &args[1..]].concat())
    }

    pub fn command(&self, args: &[&str]) -> Option<String> {
        use std::process::{Command, Stdio};
        let mut child = Command::new("tmux")
            .args(["-S", &self.socket])
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
            }
        }
        let output = child.wait_with_output().ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }
}

pub struct TmuxPassthrough<C: Fn(&[&str]) -> Option<String>> {
    pub(super) command: C,
    pub(super) previous: String,
    pub(super) expected: &'static str,
}

impl<C: Fn(&[&str]) -> Option<String>> TmuxPassthrough<C> {
    pub fn capture(command: C) -> Option<Self> {
        let previous = command(&["show-options", "-pqv", "allow-passthrough"])?;
        Some(Self {
            command,
            previous,
            expected: "on",
        })
    }

    pub fn enable_native(&mut self) -> bool {
        if (self.command)(&["set-option", "-p", "allow-passthrough", "all"]).is_none() {
            return false;
        }
        self.expected = "all";
        true
    }
}

impl<C: Fn(&[&str]) -> Option<String>> Drop for TmuxPassthrough<C> {
    fn drop(&mut self) {
        let current = (self.command)(&["show-options", "-pqv", "allow-passthrough"]);
        if current.as_deref() != Some(self.expected)
            || current.as_deref() == Some(self.previous.as_str())
        {
            return;
        }
        if self.previous.is_empty() {
            (self.command)(&["set-option", "-pu", "allow-passthrough"]);
        } else {
            (self.command)(&["set-option", "-p", "allow-passthrough", &self.previous]);
        }
    }
}
