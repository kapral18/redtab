use super::*;

pub(super) fn prepare(source: Source, show_nsfw: bool, cancelled: &Attempt) -> Result<Candidate> {
    let timeout = if matches!(source, Source::Window { .. }) {
        Duration::from_secs(330)
    } else {
        Duration::from_secs(45)
    };
    let Reply::Cookies(text) = run_helper(Request::Import(source.clone()), cancelled, timeout)?
    else {
        bail!("The browser did not supply a Reddit session");
    };
    if cancelled.load(Ordering::Acquire) {
        bail!("Sign-in cancelled");
    }
    let reddit = Reddit::from_export(&text, show_nsfw)?;
    reddit.verify_account()?;
    if cancelled.load(Ordering::Acquire) {
        bail!("Sign-in cancelled");
    }
    Ok(Candidate {
        reddit,
        source: Some(source),
        activation: None,
    })
}

pub(super) fn prepare_saved(id: &str, show_nsfw: bool, cancelled: &Attempt) -> Result<Candidate> {
    let Reply::Cookies(text) = run_helper(
        Request::LoadAccount(id.into()),
        cancelled,
        Duration::from_secs(30),
    )?
    else {
        bail!("Could not read the saved account");
    };
    let reddit = Reddit::from_saved(&text, show_nsfw)?;
    reddit.verify_saved_account()?;
    if cancelled.load(Ordering::Acquire) {
        bail!("Account switch cancelled");
    }
    let source = reddit.source();
    Ok(Candidate {
        reddit,
        source,
        activation: Some((id.into(), crate::reddit::saved_revision(&text))),
    })
}

#[derive(Default)]
pub(super) struct Attempt {
    pub(super) cancelled: AtomicBool,
    pub(super) helpers: Mutex<HashMap<u32, PathBuf>>,
}

impl Attempt {
    pub(super) fn load(&self, ordering: Ordering) -> bool {
        self.cancelled.load(ordering) || shutdown_flag().load(ordering)
    }

    pub(super) fn store(&self, value: bool, ordering: Ordering) {
        self.cancelled.store(value, ordering);
        if !value {
            return;
        }
        let mut helpers = self.helpers.lock().unwrap_or_else(|e| e.into_inner());
        for (pid, directory) in helpers.drain() {
            // Registered children are not reaped until cleanup, so these process
            // group IDs cannot be reused by another process in the meantime.
            // SAFETY: each PID belongs to a child spawned with process_group(0).
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
            loop {
                // SAFETY: this is an unreaped child we registered, with no data pointer.
                if unsafe { libc::waitpid(pid as i32, std::ptr::null_mut(), 0) } >= 0
                    || std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR)
                {
                    break;
                }
            }
            let _ = std::fs::remove_dir_all(directory);
        }
    }
}

pub(super) struct Helper<'a> {
    pub(super) child: std::process::Child,
    pub(super) _directory: tempfile::TempDir,
    pub(super) owner: &'a Attempt,
}

impl Drop for Helper<'_> {
    fn drop(&mut self) {
        let mut helpers = self.owner.helpers.lock().unwrap_or_else(|e| e.into_inner());
        if helpers.remove(&self.child.id()).is_some() {
            // SAFETY: this registered child is still unreaped and owns its group.
            unsafe {
                libc::kill(-(self.child.id() as i32), libc::SIGKILL);
            }
            let _ = self.child.wait();
        }
    }
}

pub(super) fn run_helper(
    request: Request,
    cancelled: &Attempt,
    timeout: Duration,
) -> Result<Reply> {
    let directory = tempfile::Builder::new()
        .prefix("redtab-auth-")
        .tempdir()
        .context("Could not create a private sign-in directory")?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg(crate::auth_browser::HELPER)
        .process_group(0)
        .env("TMPDIR", directory.path())
        .env("TMP", directory.path())
        .env("TEMP", directory.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut helpers = cancelled.helpers.lock().unwrap_or_else(|e| e.into_inner());
    if cancelled.load(Ordering::Acquire) {
        bail!("Sign-in cancelled");
    }
    let child = command.spawn().context("Could not start browser sign-in")?;
    helpers.insert(child.id(), directory.path().to_path_buf());
    drop(helpers);
    let timed_out = match &request {
        Request::Import(Source::Window { .. }) => crate::auth_browser::WINDOW_TIMED_OUT,
        Request::Import(Source::Browser { .. }) => {
            "Browser sign-in timed out. Unlock the keyring or use the separate login window."
        }
        Request::Discover => "Finding browser profiles timed out; r rescans.",
        Request::Load
        | Request::Accounts
        | Request::LoadAccount(_)
        | Request::Store(_)
        | Request::Activate { .. }
        | Request::Delete(_) => {
            "The OS credential store did not answer in time; unlock it and retry."
        }
    };
    let mut helper = Helper {
        child,
        _directory: directory,
        owner: cancelled,
    };
    let input = helper
        .child
        .stdin
        .take()
        .context("Missing sign-in input pipe")?;
    std::thread::spawn(move || {
        let _ = serde_json::to_writer(input, &request);
    });
    let output = helper
        .child
        .stdout
        .take()
        .context("Missing sign-in output pipe")?;
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = output
            .take(crate::auth_browser::PACKET_LIMIT + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = tx.send(result);
    });
    let started = Instant::now();
    loop {
        if cancelled.load(Ordering::Acquire) {
            bail!("Sign-in cancelled");
        }
        if started.elapsed() > timeout {
            bail!("{timed_out}");
        }
        if let Ok(result) = rx.try_recv() {
            let bytes = result?;
            if bytes.len() as u64 > crate::auth_browser::PACKET_LIMIT {
                bail!("Browser session exceeds the import limit");
            }
            return match serde_json::from_slice(&bytes)
                .context("Invalid browser sign-in response")?
            {
                Reply::Error(error) => bail!("{error}"),
                reply => Ok(reply),
            };
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}
