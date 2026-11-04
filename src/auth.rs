//! Sign-in orchestration. Candidate credentials stay separate until confirmation.

use std::collections::HashMap;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::auth_browser::{Reply, Request};
use crate::reddit::Reddit;

pub fn shutdown_flag() -> Arc<AtomicBool> {
    static FLAG: OnceLock<Arc<AtomicBool>> = OnceLock::new();
    Arc::clone(FLAG.get_or_init(|| Arc::new(AtomicBool::new(false))))
}

pub fn load_export() -> Result<String> {
    match run_helper(Request::Load, &Attempt::default(), Duration::from_secs(30))? {
        Reply::Cookies(text) => Ok(text),
        _ => bail!("Could not read the OS credential store"),
    }
}

pub fn save_export(text: &str) -> Result<()> {
    commit_with(Request::Store(text.into()), &Attempt::default())
}

fn commit_with(request: Request, attempt: &Attempt) -> Result<()> {
    match run_helper(request, attempt, Duration::from_secs(45))? {
        Reply::Stored => Ok(()),
        _ => bail!("Could not save to the OS credential store"),
    }
}

fn remove_export_with(id: &str, attempt: &Attempt) -> Result<()> {
    match run_helper(Request::Delete(id.into()), attempt, Duration::from_secs(45))? {
        Reply::Deleted => Ok(()),
        _ => bail!("Could not remove the saved Reddit connection"),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    Browser {
        browser: String,
        profile: String,
        label: String,
    },
    Window {
        executable: PathBuf,
    },
}

impl Source {
    fn same_profile(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Browser {
                    browser: a,
                    profile: p,
                    ..
                },
                Self::Browser {
                    browser: b,
                    profile: q,
                    ..
                },
            ) => a == b && p == q,
            (Self::Window { executable: a }, Self::Window { executable: b }) => a == b,
            _ => false,
        }
    }
    pub fn label(&self) -> &str {
        match self {
            Self::Browser { label, .. } => label,
            Self::Window { .. } => "Open a separate Reddit login window",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    LoadingAccounts,
    Accounts,
    Switching,
    Discovering,
    Choose,
    Importing,
    Confirm,
    Saving,
    DisconnectConfirm,
    Disconnecting,
}

struct Candidate {
    reddit: Reddit,
    source: Option<Source>,
    activation: Option<(String, String)>,
}

enum Event {
    Accounts(Result<Vec<crate::reddit::SavedAccount>, String>),
    Sources(Result<Vec<Source>, String>),
    Candidate(Result<Candidate, String>),
    Saved(Result<Reddit, String>),
    Removed(Result<(), String>),
}

pub struct Controller {
    pub phase: Phase,
    pub sources: Vec<Source>,
    pub accounts: Vec<crate::reddit::SavedAccount>,
    pub active_id: Option<String>,
    pub active_name: Option<String>,
    pub removal_name: Option<String>,
    pub removing_current: bool,
    removal_id: Option<String>,
    pub selected: usize,
    pub message: Option<String>,
    candidate: Option<Candidate>,
    accounts_view: bool,
    pending: Option<Receiver<Event>>,
    cancelled: Arc<Attempt>,
    preferred: Option<Source>,
    pub cli_selection: Option<(String, Option<String>)>,
}

impl Default for Controller {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            sources: Vec::new(),
            accounts: Vec::new(),
            active_id: None,
            active_name: None,
            removal_name: None,
            removing_current: true,
            removal_id: None,
            selected: 0,
            message: None,
            candidate: None,
            accounts_view: false,
            pending: None,
            cancelled: Arc::default(),
            preferred: None,
            cli_selection: None,
        }
    }
}

impl Controller {
    pub fn open(&mut self, preferred: Option<Source>) {
        if matches!(self.phase, Phase::Saving | Phase::Disconnecting) {
            return;
        }
        self.cancel();
        self.preferred = preferred;
        if self.cli_selection.is_some() {
            self.discover();
            return;
        }
        self.accounts_view = true;
        self.phase = Phase::LoadingAccounts;
        self.message = None;
        let cancelled = self.new_attempt();
        let (tx, rx) = channel();
        self.pending = Some(rx);
        std::thread::spawn(move || {
            let result = run_helper(Request::Accounts, &cancelled, Duration::from_secs(30))
                .and_then(|reply| match reply {
                    Reply::Accounts(accounts) => Ok(accounts),
                    _ => bail!("Could not list saved accounts"),
                })
                .map_err(|error| error.to_string());
            let _ = tx.send(Event::Accounts(result));
        });
    }

    pub fn discover(&mut self) {
        if self.busy() {
            return;
        }
        self.cancel();
        self.accounts_view = false;
        self.phase = Phase::Discovering;
        self.message = None;
        let cancelled = self.new_attempt();
        let (tx, rx) = channel();
        self.pending = Some(rx);
        std::thread::spawn(move || {
            let result = run_helper(Request::Discover, &cancelled, Duration::from_secs(40))
                .and_then(|reply| match reply {
                    Reply::Sources(sources) => Ok(sources),
                    _ => bail!("Could not discover browser profiles"),
                })
                .map_err(|e| e.to_string());
            let _ = tx.send(Event::Sources(result));
        });
    }

    pub fn account_name(&self) -> Option<String> {
        self.candidate.as_ref()?.reddit.account().map(|a| a.name)
    }

    pub fn busy(&self) -> bool {
        matches!(
            self.phase,
            Phase::LoadingAccounts
                | Phase::Switching
                | Phase::Discovering
                | Phase::Importing
                | Phase::Saving
                | Phase::Disconnecting
        )
    }

    fn new_attempt(&mut self) -> Arc<Attempt> {
        self.cancelled = Arc::default();
        Arc::clone(&self.cancelled)
    }

    pub fn cancel(&mut self) {
        if matches!(self.phase, Phase::Saving | Phase::Disconnecting) {
            return;
        }
        self.cancelled.store(true, Ordering::Release);
        self.pending = None;
        self.candidate = None;
        self.phase = if self.accounts_view {
            Phase::Accounts
        } else {
            Phase::Choose
        };
    }

    pub fn choices(&self) -> usize {
        if self.phase == Phase::Accounts {
            self.accounts.len() + 1
        } else {
            self.sources.len()
        }
    }

    pub fn reconnect(&mut self) {
        if self.busy() {
            return;
        }
        if self.phase == Phase::Accounts {
            self.preferred = self
                .accounts
                .get(self.selected)
                .and_then(|account| account.source.clone());
        }
        self.discover();
    }

    pub fn select(&mut self, show_nsfw: bool) {
        if self.phase == Phase::Accounts {
            let Some(account) = self.accounts.get(self.selected).cloned() else {
                self.discover();
                return;
            };
            if account.expired {
                self.message = Some(
                    "Saved sign-in expired; press r to reconnect this account from your browser"
                        .into(),
                );
                return;
            }
            self.phase = Phase::Switching;
            self.message = None;
            let cancelled = self.new_attempt();
            let (tx, rx) = channel();
            self.pending = Some(rx);
            std::thread::spawn(move || {
                let result = prepare_saved(&account.id, show_nsfw, &cancelled)
                    .map_err(|error| error.to_string());
                if !cancelled.load(Ordering::Acquire) {
                    let _ = tx.send(Event::Candidate(result));
                }
            });
            return;
        }
        if self.phase != Phase::Choose {
            return;
        }
        let Some(source) = self.sources.get(self.selected).cloned() else {
            return;
        };
        self.phase = Phase::Importing;
        self.message = None;
        let cancelled = self.new_attempt();
        let (tx, rx) = channel();
        self.pending = Some(rx);
        std::thread::spawn(move || {
            let result = prepare(source, show_nsfw, &cancelled).map_err(|e| e.to_string());
            if !cancelled.load(Ordering::Acquire) {
                let _ = tx.send(Event::Candidate(result));
            }
        });
    }

    pub fn request_disconnect(&mut self) {
        if !self.can_remove() {
            return;
        }
        if self.phase == Phase::Accounts {
            let Some(account) = self.accounts.get(self.selected) else {
                return;
            };
            self.removal_id = Some(account.id.clone());
            self.removal_name = Some(account.name.clone());
            self.removing_current = self.active_id.as_ref() == Some(&account.id);
        } else {
            self.removal_id = self.active_id.clone();
            self.removal_name = self.active_name.as_ref().map(|name| format!("u/{name}"));
            self.removing_current = true;
        }
        self.cancel();
        self.message = None;
        self.phase = Phase::DisconnectConfirm;
    }

    pub fn disconnect(&mut self, show_nsfw: bool) {
        let Some(id) = self.removal_id.clone() else {
            return;
        };
        self.disconnect_with(show_nsfw, move |attempt| remove_export_with(&id, attempt));
    }

    pub fn can_remove(&self) -> bool {
        match self.phase {
            Phase::DisconnectConfirm | Phase::Saving | Phase::Disconnecting => false,
            Phase::Accounts => self.selected < self.accounts.len(),
            _ => self.active_id.is_some(),
        }
    }

    fn disconnect_with(
        &mut self,
        show_nsfw: bool,
        remove: impl FnOnce(&Attempt) -> Result<()> + Send + 'static,
    ) {
        if self.phase != Phase::DisconnectConfirm {
            return;
        }
        let cancelled = self.new_attempt();
        self.phase = Phase::Disconnecting;
        let (tx, rx) = channel();
        self.pending = Some(rx);
        let disconnect = self.removing_current;
        std::thread::spawn(move || {
            let event = if disconnect {
                let result = Reddit::disconnected(show_nsfw).and_then(|reddit| {
                    remove(&cancelled)?;
                    Ok(reddit)
                });
                Event::Saved(result.map_err(|error| error.to_string()))
            } else {
                Event::Removed(remove(&cancelled).map_err(|error| error.to_string()))
            };
            let _ = tx.send(event);
        });
    }

    pub fn confirm(&mut self) {
        self.confirm_with(commit_with);
    }

    fn confirm_with(
        &mut self,
        save: impl FnOnce(Request, &Attempt) -> Result<()> + Send + 'static,
    ) {
        if self.phase != Phase::Confirm {
            return;
        }
        let Some(Candidate {
            reddit,
            source,
            activation,
        }) = self.candidate.take()
        else {
            return;
        };
        self.phase = Phase::Saving;
        let (tx, rx) = channel();
        self.pending = Some(rx);
        let cancelled = Arc::clone(&self.cancelled);
        std::thread::spawn(move || {
            let result = reddit
                .save_connection_with(source, |text| {
                    let request = match activation {
                        Some((id, expected)) => Request::Activate {
                            id,
                            expected,
                            text: text.into(),
                        },
                        None => Request::Store(text.into()),
                    };
                    save(request, &cancelled)
                })
                .map(|()| reddit)
                .map_err(|e| e.to_string());
            let _ = tx.send(Event::Saved(result));
        });
    }

    /// Polling never accesses browsers, the network or the credential store.
    pub fn poll(&mut self, show_nsfw: bool) -> (bool, Option<Reddit>) {
        self.poll_with(show_nsfw, commit_with)
    }

    fn poll_with(
        &mut self,
        show_nsfw: bool,
        save: impl FnOnce(Request, &Attempt) -> Result<()> + Send + 'static,
    ) -> (bool, Option<Reddit>) {
        let Some(receiver) = &self.pending else {
            return (false, None);
        };
        let event = match receiver.try_recv() {
            Ok(event) => event,
            Err(std::sync::mpsc::TryRecvError::Empty) => return (false, None),
            Err(_) => {
                self.fail("Sign-in stopped unexpectedly; choose a profile to retry".into());
                return (true, None);
            }
        };
        self.pending = None;
        match event {
            Event::Accounts(Ok(accounts)) => {
                self.accounts = accounts;
                self.selected = self
                    .active_id
                    .as_ref()
                    .and_then(|id| self.accounts.iter().position(|account| &account.id == id))
                    .unwrap_or(0);
                self.phase = Phase::Accounts;
                if self.accounts.is_empty() {
                    self.discover();
                }
            }
            Event::Sources(Ok(sources)) => {
                self.sources = sources;
                self.selected = self
                    .preferred
                    .as_ref()
                    .and_then(|p| self.sources.iter().position(|s| s.same_profile(p)))
                    .unwrap_or(0);
                self.phase = Phase::Choose;
                if self.sources.is_empty() {
                    self.message = Some("No supported browsers found. Install Chrome/Chromium or Firefox, or pipe a cookie export to redtab --signin.".into());
                }
                if let Some((browser, profile)) = self.cli_selection.take() {
                    let matches: Vec<_> = self.sources.iter().enumerate().filter_map(|(i,s)| matches!(s, Source::Browser { browser: b, profile: p, label } if b == &browser && profile.as_ref().is_none_or(|q| p == q || label.rsplit(" · ").next() == Some(q.as_str()))).then_some(i)).collect();
                    if matches.len() == 1 {
                        self.selected = matches[0];
                        self.select(show_nsfw);
                    } else {
                        self.message = Some("Select a detected profile; the requested browser/profile was missing or ambiguous.".into());
                    }
                }
            }
            Event::Candidate(Ok(candidate)) => {
                let switching = self.phase == Phase::Switching;
                self.candidate = Some(candidate);
                self.phase = Phase::Confirm;
                if switching {
                    self.confirm_with(save);
                }
            }
            Event::Saved(Ok(reddit)) => {
                self.phase = Phase::Idle;
                return (true, Some(reddit));
            }
            Event::Removed(Ok(())) => {
                self.accounts
                    .retain(|account| Some(&account.id) != self.removal_id.as_ref());
                self.selected = self.selected.min(self.accounts.len());
                self.phase = Phase::Accounts;
                self.message = None;
            }
            Event::Accounts(Err(error))
            | Event::Sources(Err(error))
            | Event::Candidate(Err(error))
            | Event::Saved(Err(error))
            | Event::Removed(Err(error)) => self.fail(error),
        }
        (true, None)
    }

    fn fail(&mut self, error: String) {
        self.message = Some(error);
        self.phase = if self.accounts_view {
            Phase::Accounts
        } else {
            Phase::Choose
        };
        self.candidate = None;
        self.pending = None;
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

fn prepare(source: Source, show_nsfw: bool, cancelled: &Attempt) -> Result<Candidate> {
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

fn prepare_saved(id: &str, show_nsfw: bool, cancelled: &Attempt) -> Result<Candidate> {
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
struct Attempt {
    cancelled: AtomicBool,
    helpers: Mutex<HashMap<u32, PathBuf>>,
}

impl Attempt {
    fn load(&self, ordering: Ordering) -> bool {
        self.cancelled.load(ordering) || shutdown_flag().load(ordering)
    }

    fn store(&self, value: bool, ordering: Ordering) {
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

struct Helper<'a> {
    child: std::process::Child,
    _directory: tempfile::TempDir,
    owner: &'a Attempt,
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

fn run_helper(request: Request, cancelled: &Attempt, timeout: Duration) -> Result<Reply> {
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

pub fn account_state_path(
    root: &std::path::Path,
    account: Option<&crate::reddit::Account>,
) -> PathBuf {
    match account {
        Some(account) => root.join("accounts").join(&account.id).join("state.json"),
        None => root.join("state.json"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finish(controller: &mut Controller) -> Option<Reddit> {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let (changed, connection) = controller.poll(false);
            if changed {
                return connection;
            }
            assert!(Instant::now() < deadline, "Auth operation did not finish");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn saved_account(id: &str, expired: bool) -> crate::reddit::SavedAccount {
        crate::reddit::SavedAccount {
            id: id.into(),
            name: format!("u/{id}"),
            expired,
            source: None,
        }
    }

    #[test]
    fn when_accounts_are_listed_it_should_select_the_current_account_and_include_add() {
        let mut actual = Controller::default();
        actual.accounts_view = true;
        actual.active_id = Some("second".into());
        let (sender, receiver) = channel();
        actual.pending = Some(receiver);
        sender
            .send(Event::Accounts(Ok(vec![
                saved_account("first", false),
                saved_account("second", false),
            ])))
            .unwrap();

        actual.poll(false);

        assert_eq!(actual.phase, Phase::Accounts);
        assert_eq!(actual.selected, 1);
        assert_eq!(actual.choices(), 3);
    }

    #[test]
    fn when_a_saved_account_is_expired_selection_should_offer_reconnect_without_loading_it() {
        let mut actual = Controller::default();
        actual.phase = Phase::Accounts;
        actual.accounts = vec![saved_account("expired", true)];

        actual.select(false);

        assert_eq!(actual.phase, Phase::Accounts);
        assert!(!actual.busy());
        assert!(
            actual
                .message
                .as_deref()
                .unwrap()
                .contains("press r to reconnect")
        );
        assert!(actual.pending.is_none());
    }

    #[test]
    fn when_a_saved_switch_fails_it_should_return_to_accounts_without_emitting_a_connection() {
        let mut actual = Controller::default();
        actual.accounts_view = true;
        actual.phase = Phase::Switching;
        let (sender, receiver) = channel();
        actual.pending = Some(receiver);
        sender
            .send(Event::Candidate(Err("Saved sign-in expired".into())))
            .unwrap();

        let result = actual.poll(false);

        assert!(result.1.is_none());
        assert_eq!(actual.phase, Phase::Accounts);
        assert_eq!(actual.message.as_deref(), Some("Saved sign-in expired"));
    }

    #[test]
    fn when_a_saved_switch_is_cancelled_its_late_result_should_not_change_accounts() {
        let mut actual = Controller::default();
        actual.accounts_view = true;
        actual.phase = Phase::Switching;
        let (sender, receiver) = channel();
        actual.pending = Some(receiver);

        actual.cancel();

        assert!(
            sender
                .send(Event::Candidate(Err("Late result".into())))
                .is_err()
        );
        assert_eq!(actual.phase, Phase::Accounts);
        assert!(actual.poll(false).1.is_none());
    }

    #[test]
    fn when_a_saved_account_is_verified_it_should_save_and_switch_without_another_confirmation() {
        use crate::reddit::tests::{Fixture, Reply, saved_export};
        let fixture = Fixture::new(|_| {
            Reply::json(serde_json::json!({"data":{"id":"second","name":"reader"}}))
        });
        let candidate = fixture.saved_reddit(&saved_export("second", "reader"));
        candidate.verify_saved_account().unwrap();
        let mut actual = Controller::default();
        actual.accounts_view = true;
        actual.phase = Phase::Switching;
        let (sender, receiver) = channel();
        actual.pending = Some(receiver);
        sender
            .send(Event::Candidate(Ok(Candidate {
                reddit: candidate,
                source: None,
                activation: Some(("second".into(), "snapshot".into())),
            })))
            .unwrap();
        let saved = Arc::new(Mutex::new(String::new()));
        let destination = Arc::clone(&saved);

        actual.poll_with(false, move |request, _| {
            let Request::Activate { id, expected, text } = request else {
                panic!("Saved switching must activate an existing account")
            };
            assert_eq!(id, "second");
            assert_eq!(expected, "snapshot");
            *destination.lock().unwrap() = text;
            Ok(())
        });

        assert_eq!(actual.phase, Phase::Saving);
        let connection = finish(&mut actual).unwrap();
        assert_eq!(connection.account().unwrap().id, "second");
        assert!(saved.lock().unwrap().contains("# redtab-connection"));
    }

    #[test]
    fn when_a_new_browser_account_is_verified_it_should_still_wait_for_confirmation() {
        use crate::reddit::tests::{Fixture, Reply};
        let fixture = Fixture::new(|_| {
            Reply::json(serde_json::json!({"data":{"id":"second","name":"reader"}}))
        });
        let candidate = fixture.reddit();
        candidate.verify_account().unwrap();
        let mut actual = Controller::default();
        actual.phase = Phase::Importing;
        let (sender, receiver) = channel();
        actual.pending = Some(receiver);
        sender
            .send(Event::Candidate(Ok(Candidate {
                reddit: candidate,
                source: None,
                activation: None,
            })))
            .unwrap();

        actual.poll_with(false, |_, _| panic!("New imports need user confirmation"));

        assert_eq!(actual.phase, Phase::Confirm);
        assert!(!actual.busy());
        assert_eq!(actual.account_name().as_deref(), Some("reader"));
    }

    #[test]
    fn when_the_saved_switch_cannot_be_committed_it_should_not_install_the_candidate() {
        use crate::reddit::tests::{Fixture, Reply};
        let fixture = Fixture::new(|_| {
            Reply::json(serde_json::json!({"data":{"id":"second","name":"reader"}}))
        });
        let candidate = fixture.reddit();
        candidate.verify_account().unwrap();
        let mut actual = Controller::default();
        actual.accounts_view = true;
        actual.phase = Phase::Switching;
        let (sender, receiver) = channel();
        actual.pending = Some(receiver);
        sender
            .send(Event::Candidate(Ok(Candidate {
                reddit: candidate,
                source: None,
                activation: Some(("second".into(), "snapshot".into())),
            })))
            .unwrap();

        actual.poll_with(false, |_, _| bail!("Keyring locked"));

        assert!(finish(&mut actual).is_none());
        assert_eq!(actual.phase, Phase::Accounts);
        assert_eq!(actual.message.as_deref(), Some("Keyring locked"));
    }

    #[test]
    fn when_an_expired_inactive_account_is_removed_it_should_keep_the_current_account_and_other_rows()
     {
        let mut actual = Controller::default();
        actual.accounts_view = true;
        actual.phase = Phase::Accounts;
        actual.active_id = Some("second".into());
        actual.accounts = vec![saved_account("first", true), saved_account("second", false)];
        actual.request_disconnect();
        assert!(!actual.removing_current);
        assert!(!actual.can_remove());
        actual.request_disconnect();
        assert_eq!(actual.removal_id.as_deref(), Some("first"));
        assert!(!actual.removing_current);

        actual.disconnect_with(false, |_| Ok(()));

        assert!(finish(&mut actual).is_none());
        assert_eq!(actual.phase, Phase::Accounts);
        assert_eq!(actual.active_id.as_deref(), Some("second"));
        assert_eq!(actual.accounts, vec![saved_account("second", false)]);
    }

    #[test]
    fn when_disconnect_is_not_confirmed_it_should_not_remove_credentials() {
        let mut controller = Controller::default();
        controller.phase = Phase::Choose;
        controller.disconnect_with(false, |_| panic!("Unconfirmed deletion"));
        assert_eq!(controller.phase, Phase::Choose);
        assert!(!controller.poll(false).0);
    }

    #[test]
    fn when_disconnect_confirmation_is_cancelled_it_should_keep_the_connection() {
        let mut controller = Controller::default();
        controller.active_id = Some("current".into());
        controller.request_disconnect();
        assert_eq!(controller.phase, Phase::DisconnectConfirm);
        controller.cancel();
        assert_eq!(controller.phase, Phase::Choose);
        assert!(!controller.poll(false).0);
    }

    #[test]
    fn when_credential_deletion_fails_it_should_report_the_error_without_emitting_a_connection() {
        let mut controller = Controller::default();
        controller.active_id = Some("current".into());
        controller.request_disconnect();
        controller.disconnect_with(false, |_| bail!("Keyring locked"));
        let actual = finish(&mut controller);
        assert!(actual.is_none());
        assert_eq!(controller.phase, Phase::Choose);
        assert_eq!(controller.message.as_deref(), Some("Keyring locked"));
    }

    #[test]
    fn when_confirmed_credentials_are_deleted_it_should_emit_a_disconnected_client() {
        let mut controller = Controller::default();
        controller.active_id = Some("current".into());
        let directory = tempfile::tempdir().unwrap();
        let entry = directory.path().join("synthetic-vault-entry");
        std::fs::write(&entry, b"synthetic session").unwrap();
        let deleted = entry.clone();
        controller.request_disconnect();
        controller.disconnect_with(false, move |_| {
            std::fs::remove_file(deleted)?;
            Ok(())
        });
        assert!(controller.busy());
        controller.cancel();
        controller.open(None);
        assert_eq!(controller.phase, Phase::Disconnecting);
        let actual = finish(&mut controller).unwrap();
        assert!(actual.needs_sign_in());
        assert!(actual.account().is_none());
        assert!(!entry.exists());
        assert_eq!(controller.phase, Phase::Idle);
    }

    #[test]
    fn when_a_profile_is_renamed_reconnect_should_still_select_its_stable_identity() {
        let mut controller = Controller::default();
        controller.preferred = Some(Source::Browser {
            browser: "firefox".into(),
            profile: "stable-id".into(),
            label: "Old name".into(),
        });
        let (sender, receiver) = channel();
        controller.pending = Some(receiver);
        sender
            .send(Event::Sources(Ok(vec![
                Source::Browser {
                    browser: "chrome".into(),
                    profile: "other".into(),
                    label: "Chrome".into(),
                },
                Source::Browser {
                    browser: "firefox".into(),
                    profile: "stable-id".into(),
                    label: "New name".into(),
                },
            ])))
            .unwrap();
        assert!(controller.poll(false).0);
        assert_eq!(controller.selected, 1);
    }

    #[test]
    fn when_an_attempt_is_cancelled_its_late_candidate_should_not_reach_the_controller() {
        let mut actual = Controller::default();
        let (sender, receiver) = channel();
        actual.pending = Some(receiver);
        actual.phase = Phase::Importing;
        actual.cancel();
        assert!(
            sender
                .send(Event::Candidate(Err("late failure".into())))
                .is_err()
        );
        assert!(!actual.poll(false).0);
    }

    #[test]
    fn when_accounts_differ_their_bookmark_paths_should_be_distinct() {
        let actual = account_state_path(
            std::path::Path::new("/state"),
            Some(&crate::reddit::Account {
                id: "fixture".into(),
                name: "reader".into(),
            }),
        );
        assert_eq!(actual, PathBuf::from("/state/accounts/fixture/state.json"));
    }

    #[test]
    fn when_cancellation_happens_it_should_reap_only_its_helper_and_remove_its_private_directory() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("owned");
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("snapshot"), b"synthetic browser data").unwrap();
        let mut child = Command::new("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let attempt = Attempt::default();
        attempt
            .helpers
            .lock()
            .unwrap()
            .insert(child.id(), path.clone());
        attempt.store(true, Ordering::Release);
        assert!(!path.exists());
        assert!(attempt.load(Ordering::Acquire));
        assert!(
            child.wait().is_err(),
            "cancellation already reaped this owned child"
        );
    }

    #[test]
    fn ui_audit_short_verified_confirmation_should_preserve_account_and_cancel() {
        use crate::reddit::tests::{Fixture, Reply, saved_export};
        let fixture = Fixture::new(|_| {
            Reply::json(serde_json::json!({"kind":"Listing","data":{"children":[],"after":null}}))
        });
        for name in ["reader", "abcdefghijklmnopqrst"] {
            let reddit = fixture.saved_reddit(&saved_export(name, name));
            for (width, height) in [(14, 6), (18, 6), (20, 6), (24, 8), (40, 12)] {
                let (pool, _) = crate::worker::Pool::start(reddit.clone(), 1, None);
                let dir = tempfile::tempdir().unwrap();
                let mut app = crate::app::App::new(
                    crate::config::Config {
                        media: crate::config::MediaSettings {
                            image_preview: false,
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    pool,
                    reddit.clone(),
                    dir.path().join("state.json"),
                    ratatui_image::picker::Picker::halfblocks(),
                );
                app.overlay = Some(crate::app::Overlay::Auth);
                app.auth.phase = Phase::Confirm;
                app.auth.candidate = Some(Candidate {
                    reddit: reddit.clone(),
                    source: None,
                    activation: None,
                });
                let actual = crate::ui::tests::auth_content(&mut app, width, height);
                assert!(
                    actual.contains(&format!("u/{name}"))
                        || (actual.contains(&format!("u/{}", &name[..1])) && actual.contains('…')),
                    "{actual}"
                );
                assert!(actual.contains("Esc") || actual.contains('⎋'), "{actual}");
            }
        }
    }
}
