use super::*;

#[test]
fn text_boxes_show_the_last_typed_letter_and_the_caret() {
    use crate::app::PickerState;
    let mut app = feed_app(Picker::halfblocks());
    let mut cut = Vec::new();
    for n in [30, 40, 45, 46, 47, 51, 52, 53, 54, 70] {
        let input = format!("{}Z", "q".repeat(n));
        let boxes = [
            (
                "search",
                Overlay::Search {
                    input: input.clone(),
                },
            ),
            (
                "picker",
                Overlay::Picker(PickerState { input, selected: 0 }),
            ),
        ];
        for (name, overlay) in boxes {
            app.overlay = Some(overlay);
            if !screen(&mut app, 120, 30).contains("Z▏") {
                cut.push((name, n));
            }
        }
    }

    assert!(cut.is_empty(), "the end was cut: {cut:?}");
}

#[test]
fn search_input_takes_ctrl_h_as_backspace_and_pastes_as_text() {
    use crossterm::event::{KeyCode, KeyModifiers};
    let mut app = app(&["s"]);
    app.switch_tab(4);
    let tabs = app.tabs.len();
    app.overlay = Some(Overlay::Search { input: "ab".into() });
    let input = |app: &App| match &app.overlay {
        Some(Overlay::Search { input }) => input.clone(),
        _ => "<closed>".into(),
    };

    key(&mut app, KeyCode::Char('h'), KeyModifiers::CONTROL);
    // A pasted line break keeps the words apart.
    crate::keys::paste(&mut app, "x\nyz");
    let pasted = input(&app);
    // Alt+Backspace deletes the last word, as in readline.
    key(&mut app, KeyCode::Backspace, KeyModifiers::ALT);
    let word_deleted = input(&app);

    assert_eq!(pasted, "ax yz");
    assert_eq!(word_deleted, "ax ");
    assert_eq!(app.tabs.len(), tabs);
}

#[test]
fn alt_letters_in_a_text_box_neither_close_it_nor_run_list_keys() {
    use crossterm::event::{KeyCode, KeyModifiers};
    let mut app = feed_app(Picker::halfblocks());
    let tabs = app.tabs.len();
    press(&mut app, '/');
    for c in "rust".chars() {
        press(&mut app, c);
    }

    // Readline motions: Alt+b, Alt+f; and Alt+x, Alt+q.
    for c in ['b', 'f', 'x', 'q'] {
        key(&mut app, KeyCode::Char(c), KeyModifiers::ALT);
    }

    let search_kept = matches!(&app.overlay, Some(Overlay::Search { input }) if input == "rust");
    // Alt+Enter submits the search, as Enter does.
    key(&mut app, KeyCode::Enter, KeyModifiers::ALT);
    let searched = app.overlay.is_none() && app.active_tab().feed.search.as_deref() == Some("rust");
    // The picker the same way.
    press_keys(&mut app, " bb");
    for c in "fo".chars() {
        press(&mut app, c);
    }
    for c in ['b', 'f', 'x', 'q'] {
        key(&mut app, KeyCode::Char(c), KeyModifiers::ALT);
    }
    let picker_kept = matches!(&app.overlay, Some(Overlay::Picker(p)) if p.input == "fo");

    assert!(search_kept, "the search box stays open with its query");
    assert!(searched, "Alt+Enter searched");
    assert!(picker_kept, "the picker stays open with its query");
    assert!(app.state.bookmarks().is_empty(), "nothing bookmarked");
    assert_eq!(app.tabs.len(), tabs, "no tab closed");
    assert!(!app.quit);
}

#[test]
fn optional_description_should_be_identified_before_submission() {
    let mut app = form_app("Create a custom feed");
    let actual = screen(&mut app, 80, 24);
    assert!(actual.contains("Description (optional)"), "{actual}");
}

#[test]
fn scrolled_multiline_input_should_keep_its_field_label_visible() {
    let mut app = form_app("Create text post");
    let Some(crate::app::experience::Screen::Input(form)) = app.experience.screens.last_mut()
    else {
        panic!("form missing")
    };
    form.active = 2;
    form.fields[2].value = "A paragraph of draft text.\n".repeat(30);
    form.fields[2].cursor = form.fields[2].value.len();

    let actual = screen(&mut app, 64, 12);

    assert!(actual.contains("Body (Markdown)"), "{actual}");
    assert!(actual.contains('▏'), "{actual}");
}

#[test]
fn direct_inputs_should_backspace_complete_graphemes() {
    for (input, expected) in [("cafe\u{301}", "caf"), ("x👩‍💻", "x"), ("x🇮🇪", "x"), ("", "")]
    {
        for picker in [false, true] {
            let mut app = app(&[]);
            app.overlay = Some(if picker {
                Overlay::Picker(crate::app::PickerState::default())
            } else {
                Overlay::Search {
                    input: String::new(),
                }
            });
            crate::keys::paste(&mut app, input);
            key(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
            let actual = match app.overlay.as_ref().unwrap() {
                Overlay::Picker(picker) => &picker.input,
                Overlay::Search { input } => input,
                _ => unreachable!(),
            };
            assert_eq!(actual, expected);
        }
    }
}

#[test]
fn failed_forms_should_keep_correction_visible() {
    for label in [
        "Open a user profile",
        "Create text post",
        "Create link post",
        "Create a custom feed",
    ] {
        for (width, height) in [(20, 6), (24, 8), (40, 12), (64, 18)] {
            let mut app = form_app(label);
            let form = app.experience.input().expect("form missing");
            let missing = form
                .fields
                .iter()
                .rposition(|field| field.required)
                .unwrap();
            for (i, field) in form.fields.iter_mut().enumerate() {
                field.value = if i == missing {
                    String::new()
                } else {
                    "fixture".into()
                };
                field.cursor = field.value.len();
            }
            key(&mut app, KeyCode::Char('s'), KeyModifiers::CONTROL);
            assert!(app.experience.error.is_some());
            assert!(app.experience.pending.is_none());
            let buf = screen_buffer(&mut app, width, height);
            assert!(
                buf.content()
                    .iter()
                    .any(|cell| cell.fg == RED && cell.symbol().chars().any(char::is_alphanumeric)),
                "error feedback missing at {width}x{height}"
            );
            let before = buffer_text(&buf);
            assert!(before.contains('▏'), "{label}: {before}");
            crate::keys::paste(&mut app, "fixedbody");
            let actual = screen(&mut app, width, height);
            assert!(
                actual.contains("fixedbody") && actual.contains('▏'),
                "{label}: {actual}"
            );
            assert!(actual.contains("Esc"), "{actual}");
        }
    }
}

#[test]
fn compact_dialogs_should_keep_input_and_cancel_visible() {
    for (width, height) in [(14, 6), (20, 6), (20, 8), (24, 8), (40, 12)] {
        for kind in ["jump", "search", "sort", "help", "form", "composer"] {
            let mut app = app(&["rust"]);
            match kind {
                "jump" => {
                    press_keys(&mut app, " bb");
                    crate::keys::paste(&mut app, "rust");
                }
                "search" => {
                    app.overlay = Some(Overlay::Search {
                        input: "rust".into(),
                    })
                }
                "sort" => app.overlay = Some(Overlay::Sort { selected: 0 }),
                "help" => app.help_scroll = Some(0),
                "form" => app = form_app("Open a user profile"),
                "composer" => app = form_app("Create text post"),
                _ => unreachable!(),
            }
            let actual = screen(&mut app, width, height);
            if matches!(kind, "jump" | "search") {
                assert!(
                    actual.contains('▏') && (width < 20 || actual.contains("rust")),
                    "{kind}: {actual}"
                );
            }
            if matches!(kind, "form" | "composer") {
                assert!(actual.contains('▏'), "{actual}");
            }
            assert!(
                actual.contains("Esc") || actual.contains('⎋'),
                "{kind}: {actual}"
            );
        }
    }
}
