use super::*;

#[test]
fn ctrl_letters_do_not_run_plain_letter_commands() {
    use crossterm::event::{KeyCode, KeyModifiers};
    let mut app = app(&[]);
    app.open_subreddit("c");
    let tabs = app.tabs.len();

    key(&mut app, KeyCode::Char('x'), KeyModifiers::CONTROL);
    key(&mut app, KeyCode::Char('q'), KeyModifiers::CONTROL);

    assert_eq!(app.tabs.len(), tabs, "Ctrl+X closed the tab");
    assert!(!app.quit, "Ctrl+Q quit");
}

#[test]
fn prefix_hint_should_wait_for_the_latest_key_and_stay_open_while_narrowing() {
    use std::time::{Duration, Instant};
    let mut app = app(&[]);
    press(&mut app, ' ');
    let before = Instant::now() - Duration::from_millis(400);
    app.keymap.schedule_hint(before);
    press(&mut app, 'p');
    assert!(!app.keymap.tick(before + crate::keys::map::HINT_DELAY));
    assert!(!app.popup_open());
    assert!(
        app.keymap
            .tick(Instant::now() + crate::keys::map::HINT_DELAY)
    );
    assert!(screen(&mut app, 80, 24).contains("Preferences"));
    press(&mut app, 's');
    assert!(matches!(app.overlay, Some(Overlay::Settings)));

    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    press(&mut app, ' ');
    assert!(
        app.keymap
            .tick(Instant::now() + crate::keys::map::HINT_DELAY)
    );
    press(&mut app, 'p');
    assert!(app.popup_open());
    assert!(screen(&mut app, 80, 24).contains("Preferences"));
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(!app.popup_open());
    assert!(
        !app.keymap
            .tick(Instant::now() + crate::keys::map::HINT_DELAY)
    );
}

#[test]
fn vim_prefixes_should_wait_and_stay_literal_in_text_input() {
    use crossterm::event::{KeyCode, KeyModifiers};
    let mut app = app(&["rust", "vim"]);
    app.focus_sidebar();
    app.sidebar_edge(true);
    let active = app.active;

    press(&mut app, 'g');
    assert_eq!(app.active, active, "a pending g must not move yet");
    press(&mut app, 'g');
    assert_eq!(app.active, 0);
    app.overlay = Some(Overlay::Picker(crate::app::PickerState::default()));
    for c in " ps".chars() {
        press(&mut app, c);
    }
    assert!(matches!(&app.overlay, Some(Overlay::Picker(picker)) if picker.input == " ps"));
    key(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL);
    assert!(matches!(&app.overlay, Some(Overlay::Picker(picker)) if picker.input == " "));
}

#[test]
fn leader_hints_should_page_without_skipping_choices_in_a_short_pane() {
    use crossterm::event::{KeyCode, KeyModifiers};
    let mut app = app(&[]);
    press(&mut app, ' ');
    app.keymap
        .tick(std::time::Instant::now() + crate::keys::map::HINT_DELAY);
    let mut saw_preferences = false;
    for _ in 0..20 {
        let actual = screen(&mut app, 30, 8);
        saw_preferences |= actual.contains("Preferences");
        key(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
    }
    assert!(saw_preferences);
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(app.keymap.prefix.is_empty());
    assert!(!app.quit);
}

#[test]
fn modified_keys_should_not_close_dialogs_or_confirm_account_actions() {
    use crossterm::event::{KeyCode, KeyModifiers};
    let mut app = app(&["rust"]);
    app.show_settings();
    key(&mut app, KeyCode::Char('q'), KeyModifiers::CONTROL);
    assert!(matches!(app.overlay, Some(Overlay::Settings)));
    app.overlay = Some(Overlay::Sort { selected: 0 });
    key(&mut app, KeyCode::Char('q'), KeyModifiers::ALT);
    assert!(matches!(app.overlay, Some(Overlay::Sort { .. })));
    assert!(!app.quit);
    app.overlay = Some(Overlay::Membership {
        target: crate::model::MembershipTarget::Community("rust".into()),
        join: true,
    });
    key(&mut app, KeyCode::Enter, KeyModifiers::CONTROL);
    assert!(matches!(app.overlay, Some(Overlay::Membership { .. })));
}

#[test]
fn alt_keys_should_preserve_the_current_view_and_dialog() {
    let (mut app, _) = open_long_post(|_| {});
    key(&mut app, KeyCode::Char('?'), KeyModifiers::ALT);
    assert!(app.post_view.is_some());
    assert!(app.help_scroll.is_none());

    let mut feed = feed_app(Picker::halfblocks());
    let active = feed.active;
    press_keys(&mut feed, " fs");
    key(&mut feed, KeyCode::Tab, KeyModifiers::ALT);
    assert!(matches!(feed.overlay, Some(Overlay::Sort { .. })));
    assert_eq!(feed.active, active);
}

#[test]
fn prefix_hint_should_not_interrupt_a_fast_key_sequence() {
    let mut app = app(&["rust", "vim"]);
    let footer = crate::keys::footer_hints(&app);
    press(&mut app, ']');

    let shown = screen(&mut app, 100, 24);
    assert!(!shown.contains("Next · ] · Esc cancel"), "{shown}");
    assert!(!app.popup_open(), "a pending key is not yet a popup");
    assert_eq!(crate::keys::footer_hints(&app), footer);

    press(&mut app, 's');
    assert_eq!(app.all_subs.group, 1);

    app.reddit_menu();
    let before = popup_snapshot(&mut app, 80, 24);
    press(&mut app, 's');
    let after = popup_snapshot(&mut app, 80, 24);
    assert_eq!(
        after, before,
        "the menu stays visible while the hint is delayed"
    );
}
