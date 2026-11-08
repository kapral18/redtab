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
    let fixture =
        Fixture::new(|_| Reply::json(serde_json::json!({"data":{"id":"second","name":"reader"}})));
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
    let fixture =
        Fixture::new(|_| Reply::json(serde_json::json!({"data":{"id":"second","name":"reader"}})));
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
    let fixture =
        Fixture::new(|_| Reply::json(serde_json::json!({"data":{"id":"second","name":"reader"}})));
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
fn when_an_expired_inactive_account_is_removed_it_should_keep_the_current_account_and_other_rows() {
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
fn short_verified_confirmation_should_preserve_account_and_cancel() {
    use crate::reddit::tests::{Fixture, Reply, saved_export};
    let fixture = Fixture::new(|_| {
        Reply::json(serde_json::json!({"kind":"Listing","data":{"children":[],"after":null}}))
    });
    for name in ["reader", "abcdefghijklmnopqrst"] {
        let reddit = fixture.saved_reddit(&saved_export(name, name));
        for (width, height) in [(14, 6), (24, 8), (40, 12)] {
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
