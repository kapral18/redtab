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

mod controller;
mod helper;
mod poll;

pub use controller::*;
use helper::*;

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
            Self::Window { .. } => "Sign in in a new login window (recommended)",
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
mod tests;
