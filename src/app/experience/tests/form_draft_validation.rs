use super::*;

#[test]
fn when_a_write_fails_it_should_keep_the_draft_and_display_the_error() {
    let (mut app, _dir, _fixture) = app();
    reply_form(&mut app);
    crate::keys::paste(&mut app, "unsent draft");
    app.experience.pending = Some((
        91,
        Request::Write(Action::Reply {
            parent: "t1_original".into(),
            text: "unsent draft".into(),
        }),
    ));
    app.on_experience(
        91,
        Err("Reddit denied this request (HTTP 403)".into()),
        crate::cache::Freshness {
            fetched_at: crate::app::now(),
            source: Default::default(),
        },
    );
    assert_eq!(
        app.experience.input().unwrap().fields[0].value,
        "unsent draft"
    );
    assert!(app.experience.error.as_ref().unwrap().contains("HTTP 403"));
    assert!(!app.experience.writing());
}

#[test]
fn when_a_composer_receives_paste_and_alt_text_it_should_preserve_the_draft_without_running_keys() {
    let (mut app, _dir, _fixture) = app();
    reply_form(&mut app);
    crate::keys::paste(&mut app, "Հայերեն\nq;: $()\u{1b}");
    crate::keys::handle(
        &mut app,
        KeyEvent::new(KeyCode::Char('q'), KeyModifiers::ALT),
    );
    assert!(!app.quit);
    assert!(app.experience.pending.is_none());
    assert!(matches!(app.overlay, Some(Overlay::Experience)));
    assert_eq!(
        app.experience.input().unwrap().fields[0].value,
        "Հայերեն\nq;: $()"
    );
}

#[test]
fn required_field_validation_should_name_and_focus_the_field() {
    let (mut app, _dir, _fixture) = app();
    let mut input = app.post_form(true);
    input.fields[0].value = "rust".into();
    input.fields[2].value = "https://example.com".into();
    app.experience.screens.push(Screen::Input(input));
    app.overlay = Some(Overlay::Experience);

    app.experience_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));

    assert_eq!(app.experience.input().unwrap().active, 1);
    assert_eq!(app.experience.error.as_deref(), Some("Title is required"));
    assert!(app.experience.pending.is_none());
}

#[test]
fn settings_draft_reload_should_keep_a_reply_draft_for_both_change_paths() {
    for external in [false, true] {
        let (mut app, directory, _fixture) = app();
        let path = directory.path().join("config.toml");
        std::fs::write(&path, "[media]\nimage_preview=false\n").unwrap();
        app.settings = super::super::super::settings::Settings::new(
            crate::config::store::Store::open(Some(&path)).unwrap(),
        );
        let target = Target {
            id: "t1_original".into(),
            author: "reader".into(),
            subreddit: "rust".into(),
            permalink: "/comments/post/_/original".into(),
        };
        let mut draft = form(
            "Reply",
            FormKind::Reply(target),
            vec![field("Reply", "", true)],
            "/comments/post",
        );
        draft.active = draft.fields.len() - 1;
        draft.fields[draft.active].value = "unsent café\nsecond line".into();
        draft.fields[draft.active].cursor = 5;
        app.experience.screens = vec![
            Screen::Browse {
                pagination: Default::default(),
                query: Query::Communities,
                page: Page {
                    title: "old collection".into(),
                    text: String::new(),
                    entries: vec![],
                    after: None,
                    url: "/".into(),
                },
                selected: 0,
                scroll: 0,
            },
            Screen::Input(draft.clone()),
        ];
        app.overlay = Some(Overlay::Experience);
        if !external {
            app.show_settings();
        }
        crate::app::tests::confirm_preferences(&mut app, true);
        if !external {
            crate::keys::handle(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        }
        assert!(app.config.show_nsfw);
        assert!(
            matches!(app.overlay, Some(Overlay::Experience)),
            "composer lost: external={external}"
        );
        let current = app
            .experience
            .input()
            .expect("draft must survive policy reload");
        assert_eq!(current.active, draft.active);
        for (actual, expected) in current.fields.iter().zip(&draft.fields) {
            assert_eq!(actual.value, expected.value);
            assert_eq!(actual.cursor, expected.cursor);
        }
        assert_eq!(
            app.experience.screens.len(),
            1,
            "policy-dependent browse data must be invalidated"
        );
    }
}

#[test]
fn settings_draft_errors_should_follow_retained_inputs_not_removed_read_screens() {
    for write_error in [false, true] {
        let (mut app, _directory, _fixture) = app();
        reply_form(&mut app);
        app.experience_paste("retained draft");
        let generation = if write_error {
            app.experience.pending = Some((
                95,
                Request::Write(Action::Save {
                    id: "t3_post".into(),
                    save: true,
                }),
            ));
            95
        } else {
            app.read_experience(Query::Communities, None);
            app.experience.pending.as_ref().unwrap().0
        };
        app.on_experience(
            generation,
            Err("fixture error".into()),
            crate::cache::Freshness {
                fetched_at: crate::app::now(),
                source: Default::default(),
            },
        );
        assert!(app.experience.error.is_some());
        crate::app::tests::confirm_preferences(&mut app, true);
        assert!(app.experience.input().is_some());
        assert_eq!(
            app.experience.error.is_some(),
            write_error,
            "read-screen feedback must not migrate onto a text field"
        );
    }
}
