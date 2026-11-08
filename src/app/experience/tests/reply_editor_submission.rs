use super::*;

#[test]
fn when_editing_unicode_in_the_middle_of_a_draft_it_should_preserve_valid_text() {
    let (mut app, _dir, _fixture) = app();
    reply_form(&mut app);
    crate::keys::paste(&mut app, "a界b");
    key(&mut app, KeyCode::Left);
    key(&mut app, KeyCode::Backspace);
    key(&mut app, KeyCode::Char('é'));
    assert_eq!(app.experience.input().unwrap().fields[0].value, "aéb");
}

#[test]
fn when_enter_is_pressed_in_a_reply_it_should_add_a_line_instead_of_publishing() {
    let (mut app, _dir, _fixture) = app();
    reply_form(&mut app);
    key(&mut app, KeyCode::Char('a'));
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.experience.input().unwrap().fields[0].value, "a\n");
    assert!(app.experience.pending.is_none());
}

#[test]
fn when_submitting_a_reply_it_should_capture_the_original_target_and_prevent_duplicate_writes() {
    let (mut app, _dir, _fixture) = app();
    reply_form(&mut app);
    crate::keys::paste(&mut app, "reply");
    crate::keys::handle(
        &mut app,
        KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
    );
    let generation = app.experience.pending.as_ref().unwrap().0;
    crate::keys::handle(
        &mut app,
        KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
    );
    assert_eq!(app.experience.pending.as_ref().unwrap().0, generation);
    assert!(
        matches!(&app.experience.pending, Some((_, Request::Write(Action::Reply { parent, text }))) if parent == "t1_original" && text == "reply")
    );
}

#[test]
fn when_a_mutation_is_pending_account_switching_and_escape_should_not_drop_it() {
    let (mut app, _dir, _fixture) = app();
    reply_form(&mut app);
    app.experience.pending = Some((
        91,
        Request::Write(Action::Save {
            id: "t3_post".into(),
            save: true,
        }),
    ));
    app.show_auth();
    key(&mut app, KeyCode::Esc);
    assert!(app.experience.writing());
    assert!(matches!(app.overlay, Some(Overlay::Experience)));
}

#[test]
fn editor_navigation_and_deletion_should_preserve_grapheme_boundaries() {
    let (mut app, _dir, _fixture) = app();
    app.experience.screens.push(Screen::Input(form(
        "User profile",
        FormKind::Profile,
        vec![field("Username", "a👩‍💻b", false)],
        "/search",
    )));
    app.overlay = Some(Overlay::Experience);

    key(&mut app, KeyCode::Left);
    key(&mut app, KeyCode::Left);
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Delete);
    key(&mut app, KeyCode::Backspace);

    assert_eq!(app.experience.input().unwrap().fields[0].value, "a");
}
