use super::*;

#[test]
fn settings_external_reload_should_apply_all_values_and_keep_valid_runtime_after_bad_edits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut app = app_with(&["rust"]);
    let mut store = crate::config::store::Store::open(Some(&path)).unwrap();
    store.select_account(Some("reader"));
    app.settings = settings::Settings::new(store);
    std::fs::write(&path, "all_subs_posts_per_group=1\n[media]\nimage_preview=false\n[accounts.reader.filters]\nkeywords=[\"hidden\"]\n[accounts.reader.highlight]\nkeywords=[\"rust\"]\n[commands]\nvideo=[\"video-fixture\",\"--flag\"]\n").unwrap();
    assert!(app.poll_settings(Instant::now() + Duration::from_secs(1)));
    assert_eq!(app.config.all_subs_posts_per_group, 1);
    assert!(!app.show_images);
    assert_eq!(app.config.filters.keywords, ["hidden"]);
    assert_eq!(app.config.highlight.keywords, ["rust"]);
    assert_eq!(app.config.commands.video, ["video-fixture", "--flag"]);
    let working = app.config.clone();
    std::fs::write(&path, "[media]\nimage_preview=42").unwrap();
    assert!(app.poll_settings(Instant::now() + Duration::from_secs(2)));
    assert_eq!(app.config, working);
    assert!(app.settings.store.error.is_some());
}

#[test]
fn settings_should_defer_reload_and_ui_changes_while_a_reddit_write_is_pending() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut app = app_with(&[]);
    app.settings = settings::Settings::new(crate::config::store::Store::open(Some(&path)).unwrap());
    std::fs::write(&path, "[media]\nimage_preview=false").unwrap();
    app.membership = Some(MembershipChange {
        generation: 9,
        target: crate::model::MembershipTarget::Community("rust".into()),
        join: true,
        checking: false,
        error: None,
    });
    assert!(!app.poll_settings(Instant::now() + Duration::from_secs(1)));
    app.change_setting("media.image_preview", toml::Value::Boolean(false));
    assert!(app.config.media.image_preview);
    app.membership = None;
    assert!(app.poll_settings(Instant::now() + Duration::from_secs(2)));
    assert!(!app.config.media.image_preview);
    assert!(!app.show_images);
}

#[test]
fn settings_review_thread_completion_should_keep_editor_but_account_switch_should_cancel_scoped_draft()
 {
    use crate::app::experience::Screen;
    use crate::reddit::experience::{Query, Request, Response};
    let mut app = app_with(&[]);
    let directory = tempfile::tempdir().unwrap();
    app.state_path = directory.path().join("state.json");
    let page =
        crate::reddit::parse::parse_post_page(include_str!("../../../tests/fixtures/post.json"))
            .unwrap();
    let query = Query::Thread(page.post.permalink.clone());
    app.overlay = Some(Overlay::Experience);
    app.experience.screens.push(Screen::Loading(query.clone()));
    app.experience.pending = Some((92, Request::Read { query, after: None }));
    app.show_settings();
    app.settings.selected = 4;
    app.settings_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::NONE,
    ));
    app.settings_paste("draft");
    app.on_msg(Msg::Experience {
        generation: 92,
        result: Ok(Response::Thread(Box::new(page))),
    });
    assert!(matches!(app.overlay, Some(Overlay::Settings)));
    assert!(app.post_view.is_some());
    assert_eq!(app.settings.editing.as_ref().unwrap().value, "[]draft");
    let fixture = crate::reddit::tests::Fixture::new(|_| {
        crate::reddit::tests::Reply::json(serde_json::json!({"data":{"children":[]}}))
    });
    let connection = fixture.saved_reddit(&crate::reddit::tests::saved_export("switch", "reader"));
    app.behind_settings(|app| app.install_connection(connection));
    assert!(matches!(app.overlay, Some(Overlay::Settings)));
    assert!(app.settings.editing.is_none());
    assert!(
        app.settings
            .message
            .as_ref()
            .unwrap()
            .contains("previous account")
    );
}
