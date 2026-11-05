use super::*;

#[test]
fn when_disconnect_is_chosen_it_should_require_confirmation_and_allow_escape() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = app(&[]);
    app.auth.phase = crate::auth::Phase::Choose;
    app.auth.active_id = Some("reader".into());
    app.auth.active_name = Some("reader".into());
    app.overlay = Some(Overlay::Auth);
    crate::keys::handle(
        &mut app,
        KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
    );
    assert_eq!(app.auth.phase, crate::auth::Phase::DisconnectConfirm);
    let actual = screen(&mut app, 60, 12);
    assert!(actual.contains("Remove u/reader from redtab?"), "{actual}");
    assert!(
        actual.contains("Other accounts, bookmarks and history are kept."),
        "{actual}"
    );
    assert!(
        actual.contains("Your browser session is unchanged."),
        "{actual}"
    );
    assert!(actual.contains("Enter removes"), "{actual}");
    crate::keys::handle(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.overlay.is_none());
    assert!(!app.reddit.needs_sign_in());
}

#[test]
fn when_profile_discovery_is_pending_disconnect_should_still_be_available() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = app(&[]);
    app.auth.phase = crate::auth::Phase::Discovering;
    app.auth.active_id = Some("reader".into());
    app.overlay = Some(Overlay::Auth);
    let actual = screen(&mut app, 60, 12);
    assert!(actual.contains("d Remove from redtab"), "{actual}");
    crate::keys::handle(
        &mut app,
        KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
    );
    assert_eq!(app.auth.phase, crate::auth::Phase::DisconnectConfirm);
}

#[test]
fn when_disconnect_is_pending_it_should_show_progress_and_prevent_accidental_escape() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = app(&[]);
    app.auth.phase = crate::auth::Phase::Disconnecting;
    app.overlay = Some(Overlay::Auth);
    crate::keys::handle(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    let actual = screen(&mut app, 60, 12);
    assert!(actual.contains("Removing saved session"), "{actual}");
    assert_eq!(app.auth.phase, crate::auth::Phase::Disconnecting);
    assert!(matches!(app.overlay, Some(Overlay::Auth)));
}

#[test]
fn when_disconnected_all_subs_should_invite_connecting_instead_of_claiming_no_memberships() {
    let mut app = app(&[]);
    app.reddit = Reddit::disconnected(false).unwrap();
    app.health = Health::SignedOut;
    let actual = screen(&mut app, 90, 24);
    assert!(actual.contains("Connect Reddit"), "{actual}");
    assert!(!actual.contains("No joined communities"), "{actual}");
}

#[test]
fn when_disconnect_cannot_save_state_its_controls_should_stay_visible_with_a_long_error() {
    let mut app = app(&[]);
    app.auth.phase = crate::auth::Phase::DisconnectConfirm;
    app.auth.message =
        Some("Could not save the current reading state; connection unchanged. ".repeat(8));
    app.overlay = Some(Overlay::Auth);
    let actual = screen(&mut app, 60, 12);
    assert!(
        actual.contains("Could not save the current reading state"),
        "{actual}"
    );
    assert!(actual.contains("Enter removes · Esc cancels"), "{actual}");
}

#[test]
fn when_the_connect_dialog_is_open_it_should_show_profile_selection_without_export_instructions() {
    let mut app = app(&[]);
    app.auth.phase = crate::auth::Phase::Choose;
    app.auth.sources = vec![crate::auth::Source::Browser {
        browser: "firefox".into(),
        profile: "fixture-profile".into(),
        label: "Firefox · Work".into(),
    }];
    app.overlay = Some(Overlay::Auth);
    let actual = screen(&mut app, 110, 30);
    assert!(actual.contains("Connect Reddit"));
    assert!(actual.contains("Firefox · Work"));
    assert!(actual.contains("Enter connects"));
}

#[test]
fn when_saved_accounts_are_shown_it_should_mark_active_expired_and_add_choices() {
    let mut app = app(&[]);
    app.auth.phase = crate::auth::Phase::Accounts;
    app.auth.active_id = Some("first".into());
    app.auth.accounts = vec![saved_account("first", false), saved_account("second", true)];
    app.overlay = Some(Overlay::Auth);

    let actual = screen(&mut app, 90, 24);

    assert!(actual.contains("Accounts"), "{actual}");
    assert!(
        actual
            .lines()
            .any(|line| line.contains("u/first") && line.contains("[active]")),
        "{actual}"
    );
    assert!(
        actual
            .lines()
            .any(|line| line.contains("u/second") && line.contains("[expired]")),
        "{actual}"
    );
    assert!(actual.contains("+ Add account"), "{actual}");
    assert!(actual.contains("Enter selects"), "{actual}");
    assert!(actual.contains("d Remove from redtab"), "{actual}");
}

#[test]
fn when_saved_account_switching_is_pending_it_should_show_progress_and_allow_escape() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = app(&[]);
    app.auth.phase = crate::auth::Phase::Switching;
    app.overlay = Some(Overlay::Auth);
    let actual = screen(&mut app, 60, 12);
    assert!(actual.contains("Verifying account"), "{actual}");
    assert!(actual.contains("Esc cancels"), "{actual}");

    crate::keys::handle(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    assert!(app.overlay.is_none());
    assert!(!app.reddit.needs_sign_in());
}

#[test]
fn when_saved_account_errors_are_long_the_selected_row_and_controls_should_stay_visible() {
    let mut app = app(&[]);
    app.auth.phase = crate::auth::Phase::Accounts;
    app.auth.accounts = (0..10)
        .map(|n| saved_account(&format!("reader{n}"), false))
        .collect();
    app.auth.selected = 9;
    app.auth.message = Some("Could not switch accounts. Reconnect from your browser. ".repeat(8));
    app.overlay = Some(Overlay::Auth);
    let actual = screen(&mut app, 60, 12);
    assert!(actual.contains("Could not switch accounts"), "{actual}");
    assert!(actual.contains("▌ u/reader9"), "{actual}");
    assert!(actual.contains("Enter selects"), "{actual}");
    assert!(actual.contains("r Reconnect selected"), "{actual}");
    assert!(actual.contains("d Remove from redtab"), "{actual}");
}

#[test]
fn short_picker_and_account_dialogs_should_fit_their_content() {
    let app = app(&[]);
    for accounts in [false, true] {
        let mut terminal = Terminal::new(TestBackend::new(120, 60)).unwrap();
        terminal
            .draw(|f| {
                if accounts {
                    draw_auth(f, &app);
                } else {
                    draw_picker(f, &app, "", 0);
                }
            })
            .unwrap();
        let rows: Vec<_> = terminal
            .backend()
            .buffer()
            .content()
            .chunks(120)
            .enumerate()
            .filter(|(_, row)| row.iter().any(|cell| cell.symbol() != " "))
            .map(|(index, _)| index)
            .collect();
        assert!(rows.last().unwrap() - rows.first().unwrap() < 14);
    }
}

#[test]
fn when_membership_dialog_opens_it_should_name_the_account_action_in_a_short_terminal() {
    let name = "a_very_long_community_name_with_fifty_characters_123";
    for join in [true, false] {
        for width in [56, 80] {
            let mut app = app(&[]);
            app.overlay = Some(Overlay::Membership {
                target: crate::model::MembershipTarget::Community(name.into()),
                join,
            });
            let shown = screen(&mut app, width, 10);
            assert!(
                shown.contains(if join {
                    "Join on Reddit"
                } else {
                    "Leave on Reddit"
                }),
                "{shown}"
            );
            // Wrapping must retain the entire target, including its suffix.
            let unwrapped: String = shown
                .lines()
                .filter_map(|line| line.split('│').nth(1))
                .flat_map(str::chars)
                .filter(|c| !c.is_whitespace())
                .collect();
            assert!(unwrapped.contains(&format!("r/{name}")), "{shown}");
            assert!(
                shown.contains("Enter")
                    && shown.contains("Esc cancel")
                    && shown.contains("o open subreddit"),
                "{shown}"
            );
        }
    }
}

#[test]
fn ui_audit_short_accounts_should_preserve_identity_and_back_controls() {
    use crate::auth::Phase;
    for name in ["reader", "abcdefghijklmnopqrst"] {
        for (width, height) in [(14, 6), (18, 6), (20, 6), (20, 8), (24, 8), (40, 12)] {
            for phase in [Phase::Accounts, Phase::DisconnectConfirm] {
                let mut app = app(&[]);
                app.overlay = Some(Overlay::Auth);
                app.auth.phase = phase;
                app.auth.accounts = vec![saved_account(name, true)];
                app.auth.active_id = Some(name.into());
                app.auth.removal_name = Some(format!("u/{name}"));
                let actual = auth_content(&mut app, width, height);
                if phase == Phase::Accounts && height >= 8 {
                    assert!(actual.contains("expired"), "{actual}");
                }
                if phase == Phase::Accounts && width == 40 {
                    assert!(actual.contains(&format!("u/{name}")), "{actual}");
                }
                assert!(
                    actual.contains(&format!("u/{name}"))
                        || (actual.contains(&format!("u/{}", &name[..1])) && actual.contains('…')),
                    "{phase:?}: {actual}"
                );
                assert!(
                    actual.contains("Esc") || actual.contains('⎋'),
                    "{phase:?}: {actual}"
                );
                assert!(
                    actual.contains("Enter") || actual.contains('⏎'),
                    "{phase:?}: {actual}"
                );
            }
        }
    }
}

#[test]
fn when_navigating_saved_accounts_the_arrow_keys_should_reach_accounts_and_add() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = app(&[]);
    app.auth.phase = crate::auth::Phase::Accounts;
    app.auth.accounts = vec![saved_account("first", false)];
    app.overlay = Some(Overlay::Auth);

    crate::keys::handle(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));

    assert_eq!(
        app.auth.selected, 1,
        "Add account must be reachable without browser sources"
    );
    let actual = screen(&mut app, 90, 24);
    assert!(actual.contains("▌ + Add account"), "{actual}");
}

#[test]
fn when_removing_an_inactive_saved_account_repeated_delete_keys_should_keep_the_same_confirmation()
{
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = app(&[]);
    app.auth.phase = crate::auth::Phase::Accounts;
    app.auth.active_id = Some("current".into());
    app.auth.accounts = vec![saved_account("expired", true)];
    app.overlay = Some(Overlay::Auth);
    crate::keys::handle(
        &mut app,
        KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
    );
    crate::keys::handle(
        &mut app,
        KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
    );
    let actual = screen(&mut app, 76, 22);
    assert!(actual.contains("Remove u/expired from redtab?"), "{actual}");
    assert!(
        actual.contains("Remove only this saved session."),
        "{actual}"
    );
    assert!(actual.contains("Enter removes"), "{actual}");
}

#[test]
fn when_import_fails_in_a_small_terminal_it_should_show_the_error_selection_and_controls() {
    let mut app = app(&[]);
    app.auth.phase = crate::auth::Phase::Choose;
    app.auth.sources = (0..10)
        .map(|index| crate::auth::Source::Browser {
            browser: "firefox".into(),
            profile: format!("profile-{index}"),
            label: format!("Firefox {index} · {}", "Wørk ".repeat(30)),
        })
        .collect();
    app.auth.selected = 9;
    app.auth.message =
        Some("Could not read Reddit cookies. Unlock your browser or choose a login window.".into());
    app.overlay = Some(Overlay::Auth);
    let actual = screen(&mut app, 60, 12);
    assert!(actual.contains("Could not read Reddit cookies"), "{actual}");
    assert!(actual.contains("login window"), "{actual}");
    assert!(actual.contains("▌ Firefox 9"), "{actual}");
    assert!(actual.contains("Enter connects"), "{actual}");
    assert!(actual.contains("Esc cancel"), "{actual}");
}

#[test]
fn when_import_is_pending_it_should_remain_visible_in_a_small_terminal() {
    let mut app = app(&[]);
    app.auth.phase = crate::auth::Phase::Importing;
    app.overlay = Some(Overlay::Auth);
    let actual = screen(&mut app, 60, 12);
    assert!(actual.contains("verifying account"));
    assert!(actual.contains("Esc cancels"));
}
