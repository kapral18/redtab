use super::*;

#[test]
fn a_failed_action_read_should_keep_the_target_and_offer_read_retry_advice() {
    let (mut app, _dir, _fixture) = app();
    let target = Target {
        id: "t1_original".into(),
        author: "author".into(),
        subreddit: "rust".into(),
        permalink: "/comments/post/_/original".into(),
    };
    let query = Query::Inspect(target.clone());
    app.overlay = Some(Overlay::Experience);
    app.experience.screens.push(Screen::Loading(query.clone()));
    app.experience.pending = Some((91, Request::Read { query, after: None }));

    app.on_experience(
        91,
        Err("Reddit denied this request (HTTP 403)".into()),
        crate::cache::Freshness {
            fetched_at: crate::app::now(),
            source: Default::default(),
        },
    );

    let error = app.experience.error.as_ref().unwrap();
    assert!(error.contains("HTTP 403"));
    assert!(error.contains("r retries"));
    assert!(!error.contains("submitting"));
    assert!(
        matches!(app.experience.screens.last(), Some(Screen::Loading(Query::Inspect(actual))) if actual == &target)
    );
    assert!(app.experience.pending.is_none());
    key(&mut app, KeyCode::Char('r'));
    assert!(
        matches!(&app.experience.pending, Some((_, Request::Read { query: Query::Inspect(actual), after: None })) if actual == &target)
    );
}

#[test]
fn when_a_read_is_abandoned_its_late_response_should_not_replace_the_new_screen() {
    let (mut app, _dir, _fixture) = app();
    app.reddit_menu();
    app.experience.pending = Some((
        91,
        Request::Read {
            query: Query::Multis,
            after: None,
        },
    ));
    app.experience.screens.push(Screen::Loading(Query::Multis));
    key(&mut app, KeyCode::Esc);
    app.on_experience(
        91,
        Ok(Response::Page(Page {
            title: "stale".into(),
            text: String::new(),
            entries: Vec::new(),
            after: None,
            url: "/".into(),
        })),
        crate::cache::Freshness {
            fetched_at: crate::app::now(),
            source: Default::default(),
        },
    );
    assert!(
        matches!(app.experience.screens.last(), Some(Screen::Menu { title, .. }) if title == "Reddit")
    );
}

#[test]
fn ui_audit_read_errors_should_offer_the_reload_key() {
    let (mut app, _dir, _fixture) = app();
    app.experience.pending = Some((
        71,
        Request::Read {
            query: Query::Search {
                text: "rust".into(),
                kind: ListSearch::Communities,
            },
            after: Some("next".into()),
        },
    ));

    app.on_experience(
        71,
        Err("Page failed".into()),
        crate::cache::Freshness {
            fetched_at: crate::app::now(),
            source: Default::default(),
        },
    );

    assert!(
        app.experience
            .error
            .as_deref()
            .unwrap()
            .contains("r retries")
    );
}
