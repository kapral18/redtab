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
mod tests {
    use super::*;

    #[derive(Default)]
    struct MemoryStore {
        payload: Option<String>,
        read_fails: bool,
        write_fails: bool,
    }

    impl Store for MemoryStore {
        fn read(&self) -> Result<Option<String>> {
            if self.read_fails {
                bail!("Keyring locked")
            }
            Ok(self.payload.clone())
        }
        fn write(&mut self, text: Option<&str>) -> Result<()> {
            if self.write_fails {
                bail!("Keyring locked")
            }
            self.payload = text.map(str::to_owned);
            Ok(())
        }
    }

    fn export(id: &str, name: &str, token: &str, expires: u64) -> String {
        format!(
            "# redtab-connection {}\n.reddit.com\tTRUE\t/\tTRUE\t{expires}\treddit_session\t{token}\n",
            serde_json::json!({"account":{"id":id,"name":name},"source":null})
        )
    }

    fn store_with(entries: &[(&str, &str)], active: Option<&str>) -> MemoryStore {
        MemoryStore {
            payload: Some(serde_json::json!({"version":1,"active":active,"sessions":entries.iter().map(|(id,text)| (id.to_string(),text.to_string())).collect::<BTreeMap<_,_>>()} ).to_string()),
            ..MemoryStore::default()
        }
    }

    #[test]
    fn when_another_account_is_added_it_should_preserve_the_existing_single_login() {
        let first = export("first", "first_reader", "first-token", 0);
        let mut store = MemoryStore {
            payload: Some(first.clone()),
            ..MemoryStore::default()
        };

        edit(&mut store, |vault| {
            vault.insert(&export("second", "second_reader", "second-token", 0))
        })
        .unwrap();

        let actual = Vault::decode(store.payload).unwrap();
        assert_eq!(actual.sessions.len(), 2);
        assert_eq!(actual.load(Some("first")).unwrap(), first);
        assert_eq!(actual.active.as_deref(), Some("second"));
    }

    #[test]
    fn when_an_account_is_reimported_it_should_replace_its_session_without_duplicates() {
        let first = export("first", "reader", "old-token", 0);
        let second = export("second", "other_reader", "other-token", 0);
        let mut store = store_with(&[("first", &first), ("second", &second)], Some("second"));
        let renewed = export("first", "renamed_reader", "renewed-token", 0);

        edit(&mut store, |vault| vault.insert(&renewed)).unwrap();

        let actual = Vault::decode(store.payload).unwrap();
        assert_eq!(actual.sessions.len(), 2);
        assert_eq!(actual.load(None).unwrap(), renewed);
        assert_eq!(actual.load(Some("second")).unwrap(), second);
    }

    #[test]
    fn when_the_active_account_is_removed_it_should_sign_out_and_keep_other_accounts() {
        let first = export("first", "reader", "first-token", 0);
        let second = export("second", "other_reader", "other-token", 0);
        let mut store = store_with(&[("first", &first), ("second", &second)], Some("first"));

        edit(&mut store, |vault| {
            vault.remove("first");
            Ok(())
        })
        .unwrap();

        let actual = Vault::decode(store.payload).unwrap();
        assert!(actual.load(None).is_err());
        assert_eq!(actual.sessions.len(), 1);
        assert_eq!(actual.load(Some("second")).unwrap(), second);
    }

    #[test]
    fn when_a_different_running_account_is_removed_it_should_keep_the_stored_active_choice() {
        let first = export("first", "reader", "first-token", 0);
        let second = export("second", "other_reader", "other-token", 0);
        let mut store = store_with(&[("first", &first), ("second", &second)], Some("second"));

        edit(&mut store, |vault| {
            vault.remove("first");
            Ok(())
        })
        .unwrap();

        assert_eq!(
            Vault::decode(store.payload).unwrap().load(None).unwrap(),
            second
        );
    }

    #[test]
    fn when_the_last_account_is_removed_it_should_delete_the_native_payload() {
        let first = export("first", "reader", "first-token", 0);
        let mut store = store_with(&[("first", &first)], Some("first"));

        edit(&mut store, |vault| {
            vault.remove("first");
            Ok(())
        })
        .unwrap();

        assert!(store.payload.is_none());
    }

    #[test]
    fn when_a_removed_account_is_added_again_it_should_become_active_without_losing_others() {
        let first = export("first", "reader", "old-token", 0);
        let second = export("second", "other_reader", "other-token", 0);
        let mut store = store_with(&[("first", &first), ("second", &second)], Some("first"));
        edit(&mut store, |vault| {
            vault.remove("first");
            Ok(())
        })
        .unwrap();

        edit(&mut store, |vault| {
            vault.insert(&export("first", "reader", "new-token", 0))
        })
        .unwrap();

        let actual = Vault::decode(store.payload).unwrap();
        assert_eq!(actual.active.as_deref(), Some("first"));
        assert_eq!(actual.sessions.len(), 2);
        assert_eq!(actual.load(Some("second")).unwrap(), second);
    }

    #[test]
    fn when_an_existing_login_has_no_identity_it_should_stay_saved_when_another_is_added() {
        let old = ".reddit.com\tTRUE\t/\tTRUE\t0\treddit_session\told-token\n";
        let mut store = MemoryStore {
            payload: Some(old.into()),
            ..MemoryStore::default()
        };

        edit(&mut store, |vault| {
            vault.insert(&export("second", "other_reader", "other-token", 0))
        })
        .unwrap();

        let actual = Vault::decode(store.payload).unwrap().list().unwrap();
        assert_eq!(actual.len(), 2);
        assert_eq!(actual[0].name, "Existing connection");
        assert_eq!(actual[1].name, "u/other_reader");
    }

    #[test]
    fn when_the_existing_unnamed_session_is_identified_it_should_keep_only_its_named_row() {
        let mut store = MemoryStore {
            payload: Some(".reddit.com\tTRUE\t/\tTRUE\t0\treddit_session\tsame-token\n".into()),
            ..MemoryStore::default()
        };

        edit(&mut store, |vault| {
            vault.insert(&export("first", "reader", "same-token", 0))
        })
        .unwrap();

        let actual = Vault::decode(store.payload).unwrap().list().unwrap();
        assert_eq!(actual.len(), 1);
        assert_eq!(actual[0].id, "first");
        assert_eq!(actual[0].name, "u/reader");
    }

    #[test]
    fn when_a_saved_session_expires_it_should_remain_listed_without_exposing_cookies() {
        let expired = export("first", "reader", "private-token", 1);
        let store = store_with(&[("first", &expired)], Some("first"));

        let actual = Vault::decode(store.payload).unwrap().list().unwrap();

        assert_eq!(actual[0].name, "u/reader");
        assert!(actual[0].expired);
        assert!(
            !serde_json::to_string(&actual)
                .unwrap()
                .contains("private-token")
        );
    }

    #[test]
    fn when_an_existing_login_contains_an_older_expired_cookie_it_should_still_upgrade() {
        let text = format!(
            "{}www.reddit.com\tFALSE\t/\tTRUE\t1\treddit_session\tolder-token\n",
            export("first", "reader", "current-token", 0)
        );

        let actual = Vault::decode(Some(text)).unwrap().list().unwrap();

        assert_eq!(actual[0].name, "u/reader");
        assert!(!actual[0].expired);
    }

    #[test]
    fn when_all_saved_login_cookies_have_expired_it_should_keep_the_account_available_for_reconnect()
     {
        let text = format!(
            "{}www.reddit.com\tFALSE\t/\tTRUE\t1\treddit_session\tolder-token\n",
            export("first", "reader", "last-token", 2)
        );

        let actual = Vault::decode(Some(text)).unwrap().list().unwrap();

        assert_eq!(actual[0].name, "u/reader");
        assert!(actual[0].expired);
    }

    #[test]
    fn when_an_expired_account_is_saved_beside_a_new_account_it_should_not_block_the_update() {
        let expired = export("first", "reader", "old-token", 1);
        let mut store = store_with(&[("first", &expired)], Some("first"));

        edit(&mut store, |vault| {
            vault.insert(&export("second", "other_reader", "other-token", 0))
        })
        .unwrap();

        assert_eq!(Vault::decode(store.payload).unwrap().sessions.len(), 2);
    }

    #[test]
    fn when_the_saved_vault_is_malformed_a_new_import_should_not_overwrite_it_or_echo_it() {
        let mut store = MemoryStore {
            payload: Some("{\"version\":\"private-token\"}".into()),
            ..MemoryStore::default()
        };

        let actual = edit(&mut store, |vault| {
            vault.insert(&export("first", "reader", "new-token", 0))
        })
        .unwrap_err();

        assert_eq!(actual.to_string(), INVALID);
        assert_eq!(
            store.payload.as_deref(),
            Some("{\"version\":\"private-token\"}")
        );
    }

    #[test]
    fn when_the_keyring_is_unreadable_an_import_should_not_replace_its_existing_accounts() {
        let first = export("first", "reader", "old-token", 0);
        let mut store = MemoryStore {
            payload: Some(first.clone()),
            read_fails: true,
            ..MemoryStore::default()
        };

        let actual = edit(&mut store, |vault| {
            vault.insert(&export("second", "other_reader", "new-token", 0))
        });

        assert!(actual.is_err());
        assert_eq!(store.payload, Some(first));
    }

    #[test]
    fn when_saving_fails_the_old_active_account_should_stay_persisted() {
        let first = export("first", "reader", "old-token", 0);
        let mut store = store_with(&[("first", &first)], Some("first"));
        store.write_fails = true;

        let actual = edit(&mut store, |vault| {
            vault.insert(&export("second", "other_reader", "new-token", 0))
        });

        assert!(actual.is_err());
        assert_eq!(
            Vault::decode(store.payload).unwrap().active.as_deref(),
            Some("first")
        );
    }

    #[test]
    fn when_the_native_item_is_already_absent_removal_should_be_idempotent() {
        let mut store = MemoryStore {
            write_fails: true,
            ..MemoryStore::default()
        };

        let actual = edit(&mut store, |vault| {
            vault.remove("first");
            Ok(())
        });

        assert!(actual.is_ok());
        assert!(store.payload.is_none());
    }

    #[test]
    fn when_a_saved_key_disagrees_with_its_account_it_should_reject_the_vault() {
        let first = export("first", "reader", "private-token", 0);
        let store = store_with(&[("different", &first)], Some("different"));

        assert!(Vault::decode(store.payload).is_err());
    }

    #[test]
    fn when_the_saved_version_is_unknown_it_should_reject_it_without_echoing_values() {
        let actual = Vault::decode(Some(r#"{"version":2,"active":null,"sessions":{}}"#.into()));
        assert!(actual.is_err());
    }

    #[test]
    fn when_the_active_key_was_deleted_it_should_reject_the_inconsistent_vault() {
        let actual = Vault::decode(Some(
            r#"{"version":1,"active":"deleted","sessions":{}}"#.into(),
        ));
        assert!(actual.is_err());
    }

    #[test]
    fn when_the_native_payload_is_empty_it_should_not_treat_it_as_a_missing_item() {
        assert!(Vault::decode(Some(String::new())).is_err());
    }

    #[test]
    fn when_a_new_session_has_not_been_verified_it_should_not_save_it() {
        let mut store = MemoryStore::default();
        let actual = edit(&mut store, |vault| {
            vault.insert(".reddit.com\tTRUE\t/\tTRUE\t0\treddit_session\tprivate-token\n")
        });
        assert!(
            actual
                .unwrap_err()
                .to_string()
                .contains("Verify the Reddit account")
        );
        assert!(store.payload.is_none());
    }

    fn full_store() -> MemoryStore {
        let entries: BTreeMap<_, _> = (0..32)
            .map(|n| {
                let id = format!("account{n}");
                (
                    id.clone(),
                    export(&id, &format!("reader{n}"), &format!("token{n}"), 0),
                )
            })
            .collect();
        MemoryStore {
            payload: Some(
                serde_json::json!({"version":1,"active":"account0","sessions":entries}).to_string(),
            ),
            ..MemoryStore::default()
        }
    }

    #[test]
    fn when_the_account_limit_is_reached_it_should_still_allow_refreshing_an_existing_account() {
        let mut store = full_store();
        edit(&mut store, |vault| {
            vault.insert(&export("account0", "reader0", "renewed-token", 0))
        })
        .unwrap();
        assert_eq!(Vault::decode(store.payload).unwrap().sessions.len(), 32);
    }

    #[test]
    fn when_a_saved_account_is_activated_it_should_persist_that_choice_and_keep_other_accounts() {
        let first = export("first", "reader", "first-token", 0);
        let second = export("second", "other_reader", "second-token", 0);
        let mut store = store_with(&[("first", &first), ("second", &second)], Some("second"));

        edit(&mut store, |vault| {
            vault.activate("first", &saved_revision(&first), &first)
        })
        .unwrap();

        let actual = Vault::decode(store.payload).unwrap();
        assert_eq!(actual.load(None).unwrap(), first);
        assert_eq!(actual.load(Some("second")).unwrap(), second);
    }

    #[test]
    fn when_an_account_was_removed_during_verification_switching_should_not_recreate_it() {
        let old_first = export("first", "reader", "old-token", 0);
        let second = export("second", "other_reader", "second-token", 0);
        let mut store = store_with(&[("second", &second)], Some("second"));
        let before = store.payload.clone();

        let actual = edit(&mut store, |vault| {
            vault.activate("first", &saved_revision(&old_first), &old_first)
        });

        assert!(
            actual
                .unwrap_err()
                .to_string()
                .contains("changed or was removed")
        );
        assert_eq!(store.payload, before);
    }

    #[test]
    fn when_an_account_was_renewed_during_verification_switching_should_not_restore_its_old_cookie()
    {
        let old_first = export("first", "reader", "old-token", 0);
        let renewed = export("first", "reader", "renewed-token", 0);
        let mut store = store_with(&[("first", &renewed)], Some("first"));

        let actual = edit(&mut store, |vault| {
            vault.activate("first", &saved_revision(&old_first), &old_first)
        });

        assert!(actual.is_err());
        assert_eq!(
            Vault::decode(store.payload).unwrap().load(None).unwrap(),
            renewed
        );
    }

    #[test]
    fn when_identifying_an_old_session_it_should_not_overwrite_an_already_saved_account() {
        let unidentified = ".reddit.com\tTRUE\t/\tTRUE\t0\treddit_session\told-token\n";
        let id = stored(unidentified).unwrap().saved_id();
        let identified = export("first", "reader", "old-token", 0);
        let renewed = export("first", "reader", "renewed-token", 0);
        let mut store = store_with(&[(&id, unidentified), ("first", &renewed)], Some("first"));
        let before = store.payload.clone();

        let actual = edit(&mut store, |vault| {
            vault.activate(&id, &saved_revision(unidentified), &identified)
        });

        assert!(actual.is_err());
        assert_eq!(store.payload, before);

        edit(&mut store, |vault| {
            vault.remove("first");
            vault.activate(&id, &saved_revision(unidentified), &identified)
        })
        .unwrap();
        let actual = Vault::decode(store.payload).unwrap();
        assert_eq!(actual.sessions.len(), 1);
        assert_eq!(actual.load(None).unwrap(), identified);
    }

    #[test]
    fn when_an_unrelated_account_changes_during_verification_switching_should_keep_that_change() {
        let first = export("first", "reader", "first-token", 0);
        let renewed_second = export("second", "other_reader", "renewed-token", 0);
        let mut store = store_with(
            &[("first", &first), ("second", &renewed_second)],
            Some("second"),
        );

        edit(&mut store, |vault| {
            vault.activate("first", &saved_revision(&first), &first)
        })
        .unwrap();

        assert_eq!(
            Vault::decode(store.payload)
                .unwrap()
                .load(Some("second"))
                .unwrap(),
            renewed_second
        );
    }

    #[test]
    fn when_activation_changes_a_verified_account_id_it_should_not_overwrite_the_entry() {
        let first = export("first", "reader", "first-token", 0);
        let other = export("other", "other_reader", "other-token", 0);
        let mut store = store_with(&[("first", &first)], Some("first"));

        let actual = edit(&mut store, |vault| {
            vault.activate("first", &saved_revision(&first), &other)
        });

        assert!(actual.is_err());
        assert_eq!(
            Vault::decode(store.payload).unwrap().load(None).unwrap(),
            first
        );
    }

    #[test]
    fn when_another_account_would_exceed_the_limit_it_should_leave_the_vault_unchanged() {
        let mut store = full_store();
        let before = store.payload.clone();
        let actual = edit(&mut store, |vault| {
            vault.insert(&export("extra", "extra_reader", "extra-token", 0))
        });
        assert!(actual.is_err());
        assert_eq!(store.payload, before);
    }
}
