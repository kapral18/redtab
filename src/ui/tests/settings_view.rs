use super::*;

#[test]
fn settings_f2_should_cover_every_field_and_restore_the_underlying_dialog() {
    let mut app = app(&[]);
    app.overlay = Some(Overlay::Search {
        input: "draft?".into(),
    });
    crate::keys::handle(
        &mut app,
        crossterm::event::KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE),
    );
    assert!(matches!(app.overlay, Some(Overlay::Settings)));
    let actual = screen(&mut app, 100, 30);
    for field in crate::config::schema::SETTINGS {
        assert!(actual.contains(field.label), "{}: {actual}", field.key);
    }
    assert!(actual.contains("Temporary"));
    crate::keys::handle(
        &mut app,
        crossterm::event::KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
    );
    assert!(matches!(app.overlay, Some(Overlay::Search { ref input }) if input == "draft?"));
}

#[test]
fn scoped_settings_narrow_rows_should_keep_boolean_and_command_values_visible() {
    let mut app = app(&[]);
    app.change_setting("media.image_preview", false.into());
    app.change_setting("commands.image", toml::Value::Array(vec!["viewer".into()]));
    app.show_settings();
    for width in [24, 31, 32, 33, 40, 100] {
        for (key, label, value) in [
            ("media.image_preview", "Image previews", "Off"),
            ("commands.image", "Image command", "viewer"),
        ] {
            app.settings.selected = crate::config::schema::SETTINGS
                .iter()
                .position(|f| f.key == key)
                .unwrap();
            let actual = screen(&mut app, width, 12);
            assert!(
                actual.contains(label) && actual.contains(value),
                "{width}: {actual}"
            );
        }
    }
}

#[test]
fn settings_small_dialogs_should_keep_selection_value_and_escape_visible() {
    let mut app = app(&[]);
    app.show_settings();
    app.settings.selected = 10;
    for (width, height) in [(40, 12), (24, 8), (10, 6), (1, 1)] {
        let actual = screen(&mut app, width, height);
        if width >= 24 {
            assert!(actual.contains("Image command"), "{actual}");
            assert!(actual.contains("Esc"), "{actual}");
        }
    }
}

#[test]
fn settings_popup_should_show_temporary_and_retry_revert_actual_file_changes() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[media]\nimage_preview = false\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
    let mut app = app(&[]);
    app.settings = crate::app::settings::Settings::new(
        crate::config::store::Store::open(Some(&path)).unwrap(),
    );
    let fixture = crate::config::managed::tests::Fixture::new();
    app.settings.chezmoi = fixture.tool.clone();
    app.show_settings();
    app.settings.selected = crate::config::schema::SETTINGS
        .iter()
        .position(|f| f.key == "media.image_preview")
        .unwrap();
    crate::keys::handle(
        &mut app,
        crossterm::event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );
    assert!(app.config.media.image_preview && app.show_images);
    app.settings.wait_managed();
    app.tick();
    let actual = screen(&mut app, 100, 30);
    assert!(
        actual.contains("Temporary") && actual.contains("read-only"),
        "{actual}"
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    press(&mut app, 'r');
    assert!(Config::load(Some(&path)).unwrap().media.image_preview);
    std::fs::write(&path, "[media]\nimage_preview = false\n").unwrap();
    press(&mut app, 'u');
    assert!(!app.show_images);
    assert!(screen(&mut app, 100, 30).contains("Saved"));
}

#[test]
fn managed_settings_should_confirm_before_source_writes_and_keep_session_changes_on_cancel() {
    let (mut app, fixture) = managed_settings_app();
    assert!(!app.config.media.image_preview);
    app.settings.wait_managed();
    app.tick();
    let actual = screen(&mut app, 100, 30);
    assert!(actual.contains("Save through chezmoi?"), "{actual}");
    assert!(
        actual.contains("media.image_preview: true → false"),
        "{actual}"
    );
    assert!(
        Config::load(Some(&fixture.source))
            .unwrap()
            .media
            .image_preview
    );
    assert!(
        Config::load(Some(&fixture.target))
            .unwrap()
            .media
            .image_preview
    );

    crate::keys::handle(
        &mut app,
        crossterm::event::KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
    );

    assert!(app.settings.managed.is_none());
    assert!(!app.config.media.image_preview);
    assert_eq!(
        app.settings.store.persistence,
        crate::config::store::Persistence::Temporary
    );
    assert!(!fixture.directory.path().join("applied-args").exists());
    crate::keys::handle(
        &mut app,
        crossterm::event::KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
    );
    assert!(
        matches!(app.overlay, Some(Overlay::Search { ref input }) if input == "retained draft")
    );
}

#[test]
fn managed_settings_confirmation_should_scroll_and_keep_controls_in_narrow_windows() {
    let (mut app, _fixture) = managed_settings_app();
    app.settings.wait_managed();
    app.tick();
    for (width, height) in [(80, 24), (40, 12), (24, 8), (10, 6), (1, 1)] {
        let before = screen(&mut app, width, height);
        crate::keys::handle(
            &mut app,
            crossterm::event::KeyEvent::new(KeyCode::End, KeyModifiers::NONE),
        );
        let after = screen(&mut app, width, height);
        if width >= 24 {
            assert!(
                after.contains("Esc") && (after.contains('⏎') || after.contains("Enter")),
                "{after}"
            );
        }
        if width == 40 {
            assert_ne!(before, after);
        }
        crate::keys::handle(
            &mut app,
            crossterm::event::KeyEvent::new(KeyCode::Home, KeyModifiers::NONE),
        );
    }
}

#[test]
fn managed_settings_should_finish_an_approved_save_when_the_app_quits() {
    let (mut app, fixture) = managed_settings_app();
    app.settings.wait_managed();
    app.tick();
    crate::keys::handle(
        &mut app,
        crossterm::event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );
    crate::keys::handle(
        &mut app,
        crossterm::event::KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
    );
    assert!(app.quit);

    drop(app);

    assert!(
        !Config::load(Some(&fixture.target))
            .unwrap()
            .media
            .image_preview
    );
}

#[test]
fn managed_settings_retry_should_confirm_even_when_values_are_already_applied() {
    let (mut app, fixture) = managed_settings_app();
    app.settings.wait_managed();
    app.tick();
    std::fs::write(fixture.directory.path().join("writable"), "").unwrap();
    crate::keys::handle(
        &mut app,
        crossterm::event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );
    app.settings.wait_managed();
    app.tick();
    assert_eq!(
        app.settings.store.persistence,
        crate::config::store::Persistence::Unsaved
    );
    app.settings.store.reload();
    assert!(!app.settings.store.dirty());

    press(&mut app, 'r');
    app.settings.wait_managed();
    app.tick();

    let actual = screen(&mut app, 100, 30);
    assert!(actual.contains("Save through chezmoi?"), "{actual}");
    assert!(
        actual.contains("previously approved file permissions"),
        "{actual}"
    );
    assert_ne!(
        app.settings.store.persistence,
        crate::config::store::Persistence::Saved
    );
}

#[test]
fn managed_settings_should_apply_after_confirmation_and_preserve_help_and_the_covered_dialog() {
    let (mut app, fixture) = managed_settings_app();
    app.settings.wait_managed();
    app.tick();
    press(&mut app, 'l');
    key(&mut app, KeyCode::Right, KeyModifiers::NONE);
    key(&mut app, KeyCode::Enter, KeyModifiers::CONTROL);
    assert!(matches!(
        app.settings.managed,
        Some(crate::app::settings::ManagedSave::Confirm(_))
    ));
    assert!(!fixture.directory.path().join("applied-args").exists());
    crate::keys::handle(
        &mut app,
        crossterm::event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );
    assert!(app.settings.busy() && app.loading_active());
    press(&mut app, 'u');
    crate::keys::handle(
        &mut app,
        crossterm::event::KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
    );
    assert!(app.settings.busy());
    crate::keys::handle(
        &mut app,
        crossterm::event::KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE),
    );
    assert!(!app.loading_active());
    app.settings.wait_managed();

    app.tick();

    assert_eq!(
        app.settings.store.persistence,
        crate::config::store::Persistence::Saved
    );
    assert!(
        !Config::load(Some(&fixture.target))
            .unwrap()
            .media
            .image_preview
    );
    assert!(app.help_scroll.is_some());
    crate::keys::handle(
        &mut app,
        crossterm::event::KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
    );
    assert!(matches!(app.overlay, Some(Overlay::Settings)));
    crate::keys::handle(
        &mut app,
        crossterm::event::KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
    );
    assert!(
        matches!(app.overlay, Some(Overlay::Search { ref input }) if input == "retained draft")
    );
}

#[test]
fn settings_editor_should_keep_unicode_drafts_across_help_and_reject_bad_arrays() {
    let mut app = app(&[]);
    app.show_settings();
    app.settings.selected = crate::config::schema::SETTINGS
        .iter()
        .position(|f| f.key == "filters.keywords")
        .unwrap();
    let key = |app: &mut App, code, modifiers| {
        crate::keys::handle(app, crossterm::event::KeyEvent::new(code, modifiers))
    };
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    key(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
    crate::keys::paste(&mut app, "[\"café 👩‍💻\"]");
    key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);
    assert!(app.help_scroll.is_none(), "? is literal in settings inputs");
    key(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
    key(&mut app, KeyCode::Left, KeyModifiers::NONE);
    key(&mut app, KeyCode::Left, KeyModifiers::NONE);
    key(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
    assert_eq!(app.settings.editing.as_ref().unwrap().value, "[\"café \"]");
    key(&mut app, KeyCode::F(1), KeyModifiers::NONE);
    assert!(screen(&mut app, 70, 20).contains("Text entry"));
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(app.settings.editing.is_some());
    let actual = screen(&mut app, 70, 16);
    assert!(actual.contains("café"), "{actual}");
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(app.config.filters.keywords, ["café "]);
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    key(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
    crate::keys::paste(&mut app, "[42]");
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.settings.message.is_some());
    assert!(app.settings.editing.is_some());
    assert_eq!(app.config.filters.keywords, ["café "]);
}
