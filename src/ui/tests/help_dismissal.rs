use super::*;

#[test]
fn help_should_preserve_text_dialogs_and_literal_question_marks() {
    for overlay in [
        Overlay::Picker(crate::app::PickerState {
            input: "draft".into(),
            selected: 2,
        }),
        Overlay::Search {
            input: "draft".into(),
        },
    ] {
        let mut app = app(&[]);
        app.overlay = Some(overlay);
        press(&mut app, '?');
        key(&mut app, KeyCode::F(1), KeyModifiers::NONE);
        crate::keys::paste(&mut app, "ignored paste");
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);

        assert!(app.help_scroll.is_none());
        match app.overlay.as_ref().unwrap() {
            Overlay::Picker(p) => assert_eq!(p.input, "draft?"),
            Overlay::Search { input } => assert_eq!(input, "draft?"),
            _ => panic!("help replaced the text dialog"),
        }
    }
}

#[test]
fn composer_help_should_preserve_the_draft_and_consume_submit_keys() {
    use crate::app::experience::Screen;
    let mut app = form_app("Create text post");
    crate::keys::paste(&mut app, "Հայերեն");
    press(&mut app, '?');
    key(&mut app, KeyCode::F(1), KeyModifiers::NONE);
    let shown = screen(&mut app, 100, 20);
    key(&mut app, KeyCode::Char('s'), KeyModifiers::CONTROL);

    assert!(shown.contains("Text entry"));
    assert!(shown.contains("post"));
    assert!(app.help_scroll.is_some());
    assert!(app.experience.pending.is_none());
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    let Some(Screen::Input(form)) = app.experience.screens.last() else {
        panic!("help lost the composer")
    };
    assert_eq!(form.fields[0].value, "Հայերեն?");
    assert_eq!(form.fields[0].cursor, "Հայերեն?".len());
}

#[test]
fn menu_help_should_return_to_the_same_selected_action() {
    use crate::app::experience::Screen;
    let mut app = app(&[]);
    app.reddit_menu();
    press(&mut app, 'j');
    press(&mut app, '?');
    let shown = screen(&mut app, 100, 14);
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);

    assert!(shown.contains("Action menu"));
    assert!(matches!(
        app.experience.screens.last(),
        Some(Screen::Menu { selected: 1, .. })
    ));
    assert!(app.experience.pending.is_none());
    assert!(matches!(app.overlay, Some(Overlay::Experience)));
}

#[test]
fn help_should_follow_a_read_result_without_dismissing_the_new_screen() {
    use crate::app::experience::Screen;
    use crate::reddit::experience::{Page, Query, Request, Response};
    let mut app = app(&[]);
    app.overlay = Some(Overlay::Experience);
    app.experience.screens.push(Screen::Loading(Query::Multis));
    app.experience.pending = Some((
        91,
        Request::Read {
            query: Query::Multis,
            after: None,
        },
    ));
    key(&mut app, KeyCode::F(1), KeyModifiers::NONE);
    app.on_experience(
        91,
        Ok(Response::Page(Page {
            title: "Loaded feeds".into(),
            text: "No feeds".into(),
            entries: vec![],
            after: None,
            url: "/api/multi/mine".into(),
        })),
        crate::cache::Freshness {
            fetched_at: crate::app::now(),
            source: Default::default(),
        },
    );
    let shown = screen(&mut app, 100, 14);
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);

    assert!(shown.contains("Reading dialog"));
    assert!(app.experience.pending.is_none());
    assert!(
        matches!(app.experience.screens.last(), Some(Screen::Browse { page, .. }) if page.title == "Loaded feeds")
    );
    assert!(matches!(app.overlay, Some(Overlay::Experience)));
}

#[test]
fn help_should_not_cancel_or_resubmit_a_pending_write() {
    use crate::app::experience::Screen;
    use crate::reddit::experience::{Action, Request, Response};
    let mut app = app(&[]);
    app.overlay = Some(Overlay::Experience);
    app.experience.screens.push(Screen::Confirm {
        label: "Save post".into(),
        action: Action::Save {
            id: "t3_p".into(),
            save: true,
        },
        native: "/".into(),
    });
    app.experience.pending = Some((
        91,
        Request::Write(Action::Save {
            id: "t3_p".into(),
            save: true,
        }),
    ));
    key(&mut app, KeyCode::F(1), KeyModifiers::NONE);
    let shown = screen(&mut app, 100, 14);
    app.on_experience(
        91,
        Ok(Response::Written { url: None }),
        crate::cache::Freshness {
            fetched_at: crate::app::now(),
            source: Default::default(),
        },
    );
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);

    assert!(shown.contains("Sending to Reddit"));
    assert!(app.help_scroll.is_none());
    assert!(app.experience.pending.is_none());
    assert!(app.overlay.is_none());
    assert!(!app.quit);
}
