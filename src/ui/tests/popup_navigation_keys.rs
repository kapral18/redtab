use super::*;

#[test]
fn popup_navigation_should_keep_collection_bounds_and_preview_divider_stable() {
    use crate::app::experience::Screen;
    use crate::reddit::experience::{Entry, Page, Query, UserFeed};
    let mut app = app(&[]);
    app.overlay = Some(Overlay::Experience);
    for query in [
        Query::User {
            name: "reader".into(),
            feed: UserFeed::Overview,
        },
        Query::Thread("post".into()),
    ] {
        app.experience.screens = vec![Screen::Browse {
            pagination: Default::default(),
            query,
            page: Page {
                title: "u/reader · Overview".into(),
                text: String::new(),
                entries: (0..20)
                    .map(|index| Entry {
                        icon: Default::default(),
                        title: format!("Entry {index}"),
                        body: match index {
                            0 => "Short preview".into(),
                            1 => "Long preview with enough text to wrap.\n".repeat(12),
                            _ => String::new(),
                        },
                        target: None,
                        query: None,
                        post: None,
                    })
                    .collect(),
                after: None,
                url: "/user/reader".into(),
            },
            selected: 0,
            scroll: 0,
        }];
        for (width, height) in [(120, 60), (40, 20), (24, 10)] {
            let mut layouts = Vec::new();
            for selected in [0, 1, 2, 19, 0] {
                let Screen::Browse {
                    selected: current, ..
                } = app.experience.screens.last_mut().unwrap()
                else {
                    unreachable!()
                };
                *current = selected;
                let (area, shown) = popup_snapshot(&mut app, width, height);
                let divider = shown
                    .lines()
                    .enumerate()
                    .filter(|(y, line)| {
                        *y > usize::from(area.y)
                            && *y + 1 < usize::from(area.bottom())
                            && line.contains("──")
                    })
                    .map(|(y, _)| y)
                    .collect::<Vec<_>>();
                layouts.push((area, divider));
                assert!(shown.contains(&format!("Entry {selected}")), "{shown}");
                assert!(shown.contains("Esc"), "{shown}");
            }
            assert!(
                layouts.windows(2).all(|pair| pair[0] == pair[1]),
                "{width}x{height}: {layouts:?}"
            );
        }
    }
}

#[test]
fn popup_navigation_should_keep_settings_list_and_description_regions_stable() {
    use crate::config::schema::SETTINGS;
    let mut app = app(&[]);
    app.show_settings();
    for (width, height) in [(100, 40), (40, 40), (40, 16)] {
        let mut layouts = Vec::new();
        for (selected, field) in SETTINGS.iter().enumerate() {
            app.settings.selected = selected;
            let (area, shown) = popup_snapshot(&mut app, width, height);
            let description = shown
                .lines()
                .position(|line| line.contains(&field.file_key(&app.config)))
                .unwrap();
            layouts.push((area, description));
            assert!(shown.contains(field.label), "{shown}");
            assert!(shown.contains("Esc"), "{shown}");
        }
        assert!(
            layouts.windows(2).all(|pair| pair[0] == pair[1]),
            "{width}x{height}: {layouts:?}"
        );
    }
}

#[test]
fn popup_navigation_should_keep_account_list_and_controls_stable_including_add() {
    let mut app = app(&[]);
    app.overlay = Some(Overlay::Auth);
    app.auth.phase = crate::auth::Phase::Accounts;
    app.auth.accounts = vec![
        saved_account("reader", false),
        saved_account("abcdefghijklmnopqrst", true),
    ];
    app.auth.active_id = Some("abcdefghijklmnopqrst".into());
    for (width, height) in [(90, 30), (46, 24), (24, 12), (20, 8)] {
        let mut layouts = Vec::new();
        for selected in [0, 1, 2, 0] {
            app.auth.selected = selected;
            let (area, shown) = popup_snapshot(&mut app, width, height);
            let controls = shown
                .lines()
                .position(|line| line.contains("Enter") || line.contains('⏎'))
                .unwrap();
            layouts.push((area, controls));
            assert!(shown.contains("▌"), "{shown}");
            assert!(shown.contains("Esc"), "{shown}");
        }
        assert!(
            layouts.windows(2).all(|pair| pair[0] == pair[1]),
            "{width}x{height}: {layouts:?}"
        );
    }
}

#[test]
fn popup_navigation_should_keep_form_fields_stable_at_caret_wrap_boundaries() {
    use crate::app::experience::Screen;
    for (width, height, value_width) in [(120, 50, 66), (40, 30, 30)] {
        let mut app = form_app("Create text post");
        let Screen::Input(form) = app.experience.screens.last_mut().unwrap() else {
            unreachable!()
        };
        for field in form.fields.iter_mut().take(2) {
            field.value = "é".repeat(value_width);
            field.cursor = field.value.len();
        }
        let mut layouts = Vec::new();
        for active in [0, 1, 2, 0] {
            let Screen::Input(form) = app.experience.screens.last_mut().unwrap() else {
                unreachable!()
            };
            form.active = active;
            let (area, shown) = popup_snapshot(&mut app, width, height);
            let labels = shown
                .lines()
                .enumerate()
                .filter(|(_, line)| {
                    line.contains("Subreddit")
                        || line.contains("Title")
                        || line.contains("Body (Markdown)")
                })
                .map(|(y, _)| y)
                .collect::<Vec<_>>();
            layouts.push((area, labels));
            assert_eq!(shown.matches('▏').count(), 1, "{shown}");
        }
        assert!(
            layouts.windows(2).all(|pair| pair[0] == pair[1]),
            "{width}x{height}: {layouts:?}"
        );
    }
}

#[test]
fn popup_description_footer_should_prioritize_vim_scroll_keys_at_each_width() {
    use crate::reddit::experience::Query;
    for (query, summary, bodies, scrolls) in [
        (
            Query::Profile("reader".into()),
            "",
            vec!["Description\n"; 2],
            true,
        ),
        (
            Query::Profile("reader".into()),
            "Summary\n",
            vec![""; 2],
            true,
        ),
        (Query::Rules("rust".into()), "Rules\n", vec![], true),
        (
            Query::Thread("post".into()),
            "Ignored thread summary",
            vec![""; 2],
            false,
        ),
        (Query::Profile("reader".into()), "", vec![""; 2], false),
    ] {
        let mut app = description_popup(query, summary, &bodies);
        for (width, height) in [(120, 40), (40, 16), (24, 12)] {
            let (_, shown) = popup_snapshot(&mut app, width, height);
            assert_eq!(
                shown.contains("Ctrl+d/u") || shown.contains("C-d/u") || shown.contains("C-e/y"),
                scrolls,
                "{width}: {shown}"
            );
            assert!(shown.contains("Esc"), "{shown}");
            assert_eq!(
                shown.contains("Enter") || shown.contains('⏎'),
                !bodies.is_empty(),
                "{shown}"
            );
        }
    }
}

#[test]
fn popup_description_vim_keys_should_scroll_without_selecting_or_opening_feed_actions() {
    use crate::app::experience::Screen;
    use crate::reddit::experience::Query;
    let body = (0..40)
        .map(|row| format!("Line {row:02}\n"))
        .collect::<String>();
    let mut app = description_popup(
        Query::Multi {
            owner: "reader".into(),
            name: "feed".into(),
        },
        "",
        &[&body, &body],
    );
    let (area, before) = popup_snapshot(&mut app, 100, 40);
    assert!(before.contains("Line 00"));

    key(&mut app, KeyCode::Char('d'), KeyModifiers::CONTROL);
    assert!(matches!(
        app.experience.screens.last(),
        Some(Screen::Browse {
            selected: 0,
            scroll: 5,
            ..
        })
    ));
    let (after_area, after) = popup_snapshot(&mut app, 100, 40);
    assert_eq!(after_area, area);
    assert!(after.contains("Line 05"), "{after}");
    assert!(!after.contains("Line 00"), "{after}");
    assert!(app.experience.pending.is_none());

    key(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
    key(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
    assert!(matches!(
        app.experience.screens.last(),
        Some(Screen::Browse {
            selected: 0,
            scroll: 0,
            ..
        })
    ));
    key(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
    assert!(matches!(
        app.experience.screens.last(),
        Some(Screen::Browse { scroll: 10, .. })
    ));
    key(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
    assert!(matches!(
        app.experience.screens.last(),
        Some(Screen::Browse {
            selected: 1,
            scroll: 0,
            ..
        })
    ));
}

#[test]
fn popup_description_help_should_name_vim_scrolling_and_pending_reads_should_block_it() {
    use crate::app::experience::Screen;
    use crate::reddit::experience::{Query, Request};
    let query = Query::Profile("reader".into());
    let mut app = description_popup(query.clone(), "", &["Description"]);
    key(&mut app, KeyCode::F(1), KeyModifiers::NONE);
    let sections = help::entries(&app);
    assert!(
        sections
            .iter()
            .any(|(keys, action)| keys == "C-d / C-u" && action == "Half page down / up")
    );
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    app.experience.pending = Some((1, Request::Read { query, after: None }));

    key(&mut app, KeyCode::Char('d'), KeyModifiers::CONTROL);
    assert!(matches!(
        app.experience.screens.last(),
        Some(Screen::Browse { scroll: 0, .. })
    ));
    let (_, shown) = popup_snapshot(&mut app, 40, 16);
    assert!(
        !shown.contains("C-d/u") && !shown.contains("C-e/y"),
        "{shown}"
    );
    assert!(shown.contains("Esc"), "{shown}");
}

#[test]
fn popup_description_line_keys_should_reach_text_skipped_by_large_steps_in_short_panes() {
    use crate::reddit::experience::Query;
    let body = (0..40)
        .map(|row| format!("Line {row:02}\n"))
        .collect::<String>();
    let mut app = description_popup(Query::Profile("reader".into()), "", &[&body, &body]);
    let (area, before) = popup_snapshot(&mut app, 24, 12);
    assert!(before.contains("Line 00"));
    assert!(!before.contains("Line 04"));

    key(&mut app, KeyCode::Char('e'), KeyModifiers::CONTROL);
    let (after_area, shown) = popup_snapshot(&mut app, 24, 12);
    assert_eq!(area, after_area);
    assert!(shown.contains("Line 04"), "{shown}");
    assert!(!shown.contains("Line 00"), "{shown}");
    assert!(shown.contains("C-e/y"), "{shown}");
    key(&mut app, KeyCode::Char('y'), KeyModifiers::CONTROL);
    let (_, restored) = popup_snapshot(&mut app, 24, 12);
    assert!(restored.contains("Line 00"), "{restored}");
}

#[test]
fn when_a_pending_collection_is_extremely_narrow_its_cancel_hint_should_survive() {
    use crate::app::experience::Screen;
    use crate::reddit::experience::{Page, Query, Request};
    for width in [5, 6] {
        let mut app = app(&[]);
        app.overlay = Some(Overlay::Experience);
        app.experience.screens.push(Screen::Browse {
            pagination: Default::default(),
            query: Query::Multis,
            page: Page {
                title: "Custom feeds".into(),
                text: String::new(),
                entries: vec![],
                after: Some("next".into()),
                url: String::new(),
            },
            selected: 0,
            scroll: 0,
        });
        app.experience.pending = Some((
            1,
            Request::Read {
                query: Query::Multis,
                after: Some("next".into()),
            },
        ));
        let actual = screen(&mut app, width, 10);
        assert!(actual.contains('⎋'), "{actual}");
    }
}
