use super::*;

mod support;

use support::*;

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
fn an_account_is_expired_only_when_its_newest_login_cookie_is() {
    for (current_expiry, expired) in [(0, false), (2, true)] {
        let text = format!(
            "{}www.reddit.com\tFALSE\t/\tTRUE\t1\treddit_session\tolder-token\n",
            export("first", "reader", "current-token", current_expiry)
        );

        let actual = Vault::decode(Some(text)).unwrap().list().unwrap();

        assert_eq!(actual[0].name, "u/reader");
        assert_eq!(
            actual[0].expired, expired,
            "current cookie expiry {current_expiry}"
        );
    }
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
fn an_inconsistent_or_unknown_vault_is_rejected() {
    let first = export("first", "reader", "private-token", 0);
    let mismatched = store_with(&[("different", &first)], Some("different")).payload;
    for payload in [
        mismatched,
        Some(r#"{"version":2,"active":null,"sessions":{}}"#.into()),
        Some(r#"{"version":1,"active":"deleted","sessions":{}}"#.into()),
        // Empty is not a missing item.
        Some(String::new()),
    ] {
        assert!(Vault::decode(payload.clone()).is_err(), "{payload:?}");
    }
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
fn when_an_account_was_renewed_during_verification_switching_should_not_restore_its_old_cookie() {
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
