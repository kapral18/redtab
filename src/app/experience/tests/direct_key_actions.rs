use super::*;

#[test]
fn direct_content_keys_should_run_their_action_once_the_target_loads() {
    for (keys, expected) in [("u", Some(1)), ("d", Some(-1)), ("s", None), ("c", None)] {
        let (mut app, _dir, _fixture) = app();
        let post = post_page().post;
        let target = Target::from_post(&post);
        app.tabs[0]
            .feed
            .replace_page(None, vec![post], None, Source::default());
        sequence(&mut app, keys);
        let Some((
            generation,
            Request::Read {
                query: Query::Inspect(actual),
                ..
            },
        )) = app.experience.pending.clone()
        else {
            panic!("inspect read missing for {keys}")
        };
        assert_eq!(actual, target);
        app.on_experience(
            generation,
            Ok(Response::Details(Details {
                target: target.clone(),
                raw_text: String::new(),
                saved: false,
                hidden: false,
                vote: 0,
                editable: false,
                crosspost: None,
            })),
            crate::cache::Freshness {
                fetched_at: crate::app::now(),
                source: Default::default(),
            },
        );
        match (&app.experience.pending, expected) {
            (Some((_, Request::Write(Action::Vote { id, direction }))), Some(want)) => {
                assert_eq!((id.as_str(), *direction), (target.id.as_str(), want))
            }
            (Some((_, Request::Write(Action::Save { id, save: true }))), None) => {
                assert_eq!(id, &target.id)
            }
            (None, None) if keys == "c" => assert!(
                matches!(app.experience.screens.last(), Some(Screen::Input(Form { kind: FormKind::Reply(actual), .. })) if actual == &target)
            ),
            _ => panic!("{keys} did not write its action"),
        }
    }
}

#[test]
fn a_cancelled_direct_key_should_not_run_on_a_later_actions_menu() {
    let (mut app, _dir, _fixture) = app();
    let post = post_page().post;
    let target = Target::from_post(&post);
    app.tabs[0]
        .feed
        .replace_page(None, vec![post], None, Source::default());
    sequence(&mut app, "u");
    key(&mut app, KeyCode::Esc);
    app.show_actions();
    let generation = app.experience.pending.as_ref().unwrap().0;
    app.on_experience(
        generation,
        Ok(Response::Details(Details {
            target,
            raw_text: String::new(),
            saved: false,
            hidden: false,
            vote: 0,
            editable: false,
            crosspost: None,
        })),
        crate::cache::Freshness {
            fetched_at: crate::app::now(),
            source: Default::default(),
        },
    );
    assert!(app.experience.pending.is_none());
    assert!(matches!(
        app.experience.screens.last(),
        Some(Screen::Menu { .. })
    ));
}

#[test]
fn a_failed_direct_key_read_should_not_run_on_a_later_actions_menu() {
    let (mut app, _dir, _fixture) = app();
    let post = post_page().post;
    let target = Target::from_post(&post);
    app.tabs[0]
        .feed
        .replace_page(None, vec![post], None, Source::default());
    sequence(&mut app, "u");
    let generation = app.experience.pending.as_ref().unwrap().0;
    app.on_experience(
        generation,
        Err("HTTP 500".into()),
        crate::cache::Freshness {
            fetched_at: crate::app::now(),
            source: Default::default(),
        },
    );
    app.experience_back();
    app.show_actions();
    let generation = app.experience.pending.as_ref().unwrap().0;
    app.on_experience(
        generation,
        Ok(Response::Details(Details {
            target,
            raw_text: String::new(),
            saved: false,
            hidden: false,
            vote: 0,
            editable: false,
            crosspost: None,
        })),
        crate::cache::Freshness {
            fetched_at: crate::app::now(),
            source: Default::default(),
        },
    );
    assert!(app.experience.pending.is_none());
    assert!(matches!(
        app.experience.screens.last(),
        Some(Screen::Menu { .. })
    ));
}
