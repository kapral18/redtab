//! A bounded collection inside the existing OS credential-store item.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{credentials, session};

const VERSION: u8 = 1;
const LIMIT: usize = 4 * 1024 * 1024;
const MAX_ACCOUNTS: usize = 32;
const INVALID: &str = "Cannot read saved accounts; the OS credential-store entry is invalid";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedAccount {
    pub id: String,
    pub name: String,
    pub expired: bool,
    pub source: Option<crate::auth::Source>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Vault {
    version: u8,
    active: Option<String>,
    sessions: BTreeMap<String, String>,
}

impl Default for Vault {
    fn default() -> Self {
        Self {
            version: VERSION,
            active: None,
            sessions: BTreeMap::new(),
        }
    }
}

impl Vault {
    fn decode(text: Option<String>) -> Result<Self> {
        let Some(text) = text else {
            return Ok(Self::default());
        };
        if text.len() > LIMIT {
            bail!(INVALID);
        }
        let vault = if text.trim_start().starts_with('{') {
            serde_json::from_str::<Self>(&text).map_err(|_| anyhow::anyhow!(INVALID))?
        } else {
            // The previous item contained one Netscape export, sometimes without identity.
            let session = stored(&text)?;
            Self {
                active: Some(session.saved_id()),
                sessions: BTreeMap::from([(session.saved_id(), text)]),
                ..Self::default()
            }
        };
        if vault.version != VERSION
            || vault.sessions.len() > MAX_ACCOUNTS
            || vault
                .active
                .as_ref()
                .is_some_and(|id| !vault.sessions.contains_key(id))
        {
            bail!(INVALID);
        }
        for (id, text) in &vault.sessions {
            let session = stored(text)?;
            if id != &session.saved_id() {
                bail!(INVALID);
            }
        }
        Ok(vault)
    }

    fn list(&self) -> Result<Vec<SavedAccount>> {
        let mut accounts = Vec::new();
        for (id, text) in &self.sessions {
            let session = stored(text)?;
            let name = session.connection.as_ref().map_or_else(
                || "Existing connection".into(),
                |connection| format!("u/{}", connection.account.name),
            );
            accounts.push(SavedAccount {
                id: id.clone(),
                name,
                expired: !session.valid(),
                source: session.connection.and_then(|connection| connection.source),
            });
        }
        accounts.sort_by(|a, b| {
            a.name
                .to_ascii_lowercase()
                .cmp(&b.name.to_ascii_lowercase())
                .then(a.id.cmp(&b.id))
        });
        Ok(accounts)
    }

    fn insert(&mut self, text: &str) -> Result<()> {
        stored(text)?;
        let session = session::Session::stored(text, session::now())?;
        let id = session
            .connection
            .as_ref()
            .context("Verify the Reddit account before saving")?
            .account
            .id
            .clone();
        // Identifying the previous unnamed session must not leave a second saved row.
        self.sessions.retain(|key, saved| {
            key == &id || stored(saved).is_ok_and(|old| old.fingerprint != session.fingerprint)
        });
        self.sessions.insert(id.clone(), text.into());
        self.active = Some(id);
        Ok(())
    }

    fn remove(&mut self, id: &str) {
        self.sessions.remove(id);
        if self.active.as_deref() == Some(id) {
            self.active = None;
        }
    }

    fn activate(&mut self, id: &str, expected: &str, text: &str) -> Result<()> {
        let changed =
            || anyhow::anyhow!("Saved account changed or was removed; reopen Accounts and retry");
        let saved = self.sessions.get(id).ok_or_else(changed)?;
        if saved_revision(saved) != expected {
            return Err(changed());
        }
        let before = stored(saved)?;
        let after = stored(text)?;
        let target = after.saved_id();
        if before.connection.as_ref().is_some_and(|old| {
            after
                .connection
                .as_ref()
                .is_none_or(|new| old.account.id != new.account.id)
        }) || (id != target && self.sessions.contains_key(&target))
        {
            return Err(changed());
        }
        self.sessions.remove(id);
        self.insert(text)
    }

    fn load(&self, id: Option<&str>) -> Result<String> {
        let id = id.or(self.active.as_deref()).context(session::SIGN_IN)?;
        self.sessions
            .get(id)
            .cloned()
            .context("Saved account is no longer available; reopen Accounts")
    }
}

fn stored(text: &str) -> Result<session::Session> {
    if text.len() as u64 > crate::auth_browser::LIMIT {
        bail!(INVALID);
    }
    session::Session::archived(text).map_err(|_| anyhow::anyhow!(INVALID))
}

trait Store {
    fn read(&self) -> Result<Option<String>>;
    fn write(&mut self, text: Option<&str>) -> Result<()>;
}

struct NativeStore;

impl Store for NativeStore {
    fn read(&self) -> Result<Option<String>> {
        credentials::load()
    }

    fn write(&mut self, text: Option<&str>) -> Result<()> {
        match text {
            Some(text) => credentials::save(text),
            None => credentials::remove(),
        }
    }
}

fn edit(store: &mut impl Store, change: impl FnOnce(&mut Vault) -> Result<()>) -> Result<()> {
    let original = store.read()?;
    let missing = original.is_none();
    let mut vault = Vault::decode(original)?;
    change(&mut vault)?;
    if vault.sessions.is_empty() {
        if missing {
            return Ok(());
        }
        return store.write(None);
    }
    let text = serde_json::to_string(&vault).map_err(|_| anyhow::anyhow!(INVALID))?;
    if vault.sessions.len() > MAX_ACCOUNTS || text.len() > LIMIT {
        bail!("Saved accounts exceed the storage limit; remove an unused saved account first");
    }
    store.write(Some(&text))
}

pub(super) fn list() -> Result<Vec<SavedAccount>> {
    Vault::decode(NativeStore.read()?)?.list()
}

pub(super) fn load(id: Option<&str>) -> Result<String> {
    Vault::decode(NativeStore.read()?)?.load(id)
}

pub(super) fn save(text: &str) -> Result<()> {
    let _lock = credentials::lock()?;
    edit(&mut NativeStore, |vault| vault.insert(text))
}

pub(super) fn remove(id: &str) -> Result<()> {
    let _lock = credentials::lock()?;
    edit(&mut NativeStore, |vault| {
        vault.remove(id);
        Ok(())
    })
}

pub(crate) fn saved_revision(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

pub(super) fn activate(id: &str, expected: &str, text: &str) -> Result<()> {
    let _lock = credentials::lock()?;
    edit(&mut NativeStore, |vault| vault.activate(id, expected, text))
}

#[cfg(test)]
mod tests;
