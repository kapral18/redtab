use super::*;
use crate::reddit::{
    Preferences,
    tests::{Fixture, Reply, saved_export},
};
use crate::{config::Config, worker::Pool};
use ratatui_image::picker::Picker;
use serde_json::json;

fn fixture() -> Fixture {
    Fixture::new(|request| {
        match request.url.as_str() {
            "/settings/preferences" => Reply { status:200, body:"<settings-preferences initialstate='{\"identity\":{\"preferences\":{\"isNsfwContentShown\":false,\"isVideoAutoplayDisabled\":true}}}'></settings-preferences>".into(), headers:vec![] },
            _ => Reply::json(json!({"kind":"Listing","data":{"children":[]}})),
        }
    })
}

fn app(fixture: &Fixture, config: Config, directory: &std::path::Path) -> App {
    let reddit = fixture.saved_reddit(&saved_export("first", "reader"));
    let (pool, _) = Pool::start(reddit.clone(), 1, None);
    App::new(
        config,
        pool,
        reddit,
        directory.join("accounts/first/state.json"),
        Picker::halfblocks(),
    )
}

#[test]
fn a_content_policy_restart_should_ask_the_search_tab_again() {
    let fixture = fixture();
    let directory = tempfile::tempdir().unwrap();
    let mut app = app(&fixture, Config::default(), directory.path());
    app.open_search(None);
    let asked = app.search.discovered.loading;
    app.search_preview_drawn(true);
    let _ = app.tick_at(std::time::Instant::now() + crate::app::search::PREVIEW_SETTLE);
    assert!(asked && app.search.preview.loading);

    app.reload_content_policy();

    assert!(
        app.search.discovered.loading,
        "discovery is asked of the new pool"
    );
    assert!(!app.search.preview.loading && app.search.preview.target.is_none());
    let _ = app.tick_at(std::time::Instant::now() + crate::app::search::PREVIEW_SETTLE);
    assert!(app.search.preview.loading, "the preview is asked again");
}

fn finish(app: &mut App) {
    if let Some(thread) = app
        .settings
        .account
        .task
        .as_mut()
        .and_then(|t| t.thread.take())
    {
        thread.join().unwrap();
    }
    app.poll_account_preferences();
}

fn completed_preference_read(app: &mut App) {
    let mut task = Task::start(|| {
        Ok(Preferences {
            show_nsfw: true,
            video_autoplay: true,
            mature_content_editable: true,
        })
    });
    task.thread.take().unwrap().join().unwrap();
    app.settings.account.task = Some(task);
    app.settings.account.owner = Some("first".into());
}

#[test]
fn scoped_settings_completed_preference_read_should_wait_for_membership_write() {
    let directory = tempfile::tempdir().unwrap();
    let fixture = fixture();
    let mut app = app(&fixture, Config::default(), directory.path());
    completed_preference_read(&mut app);
    app.membership = Some(crate::app::MembershipChange {
        generation: 9,
        target: crate::model::MembershipTarget::Community("rust".into()),
        join: true,
        checking: false,
        error: None,
    });
    assert!(!app.poll_account_preferences());
    assert!(app.settings.account.pending());
    assert!(app.settings.account.values.is_none());
    assert!(app.membership.is_some());
    assert!(app.take_replacement_messages().is_none());
    app.membership = None;
    assert!(app.poll_account_preferences());
    assert!(app.config.show_nsfw);
    assert!(!app.settings.account.pending());
}

#[test]
fn scoped_settings_completed_preference_read_should_preserve_a_pending_experience_write() {
    use crate::reddit::experience::{Action, Request};
    let directory = tempfile::tempdir().unwrap();
    let fixture = fixture();
    let mut app = app(&fixture, Config::default(), directory.path());
    completed_preference_read(&mut app);
    app.experience.pending = Some((
        91,
        Request::Write(Action::Save {
            id: "t3_post".into(),
            save: true,
        }),
    ));
    assert!(!app.poll_account_preferences());
    assert!(app.settings.account.pending());
    assert!(app.experience.writing());
    assert!(!app.config.show_nsfw);
    assert!(app.take_replacement_messages().is_none());
}

fn managed_with_pending_account_read() -> (
    App,
    crate::config::managed::tests::Fixture,
    std::sync::mpsc::Sender<()>,
) {
    let directory = tempfile::tempdir().unwrap();
    let reddit_fixture = fixture();
    let mut app = app(&reddit_fixture, Config::default(), directory.path());
    let managed = crate::config::managed::tests::Fixture::new();
    app.install_settings_store(managed.store());
    app.settings.chezmoi = managed.tool.clone();
    app.overlay = Some(Overlay::Settings);
    let (release, gate) = mpsc::channel();
    let mut task = Task::start(move || {
        let _ = gate.recv_timeout(Duration::from_secs(5));
        Ok(Preferences {
            show_nsfw: false,
            video_autoplay: false,
            mature_content_editable: true,
        })
    });
    task.join_on_drop = false;
    app.settings.account.task = Some(task);
    app.settings.account.owner = Some("first".into());
    app.change_setting("media.image_preview", false.into());
    app.settings.wait_managed();
    app.poll_managed_save();
    assert!(matches!(
        app.settings.managed,
        Some(ManagedSave::Confirm(_))
    ));
    (app, managed, release)
}

#[test]
fn scoped_settings_pending_reddit_read_should_not_block_managed_save_confirmation() {
    let (mut app, fixture, release) = managed_with_pending_account_read();
    app.settings_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(matches!(
        app.settings.managed,
        Some(ManagedSave::Applying(..))
    ));
    app.settings.wait_managed();
    app.poll_managed_save();
    assert!(app.settings.managed.is_none());
    assert!(
        !Config::load(Some(&fixture.target))
            .unwrap()
            .media
            .image_preview
    );
    let _ = release.send(());
}

#[test]
fn scoped_settings_pending_reddit_read_should_not_block_managed_save_cancellation() {
    let (mut app, _fixture, release) = managed_with_pending_account_read();
    app.settings_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.settings.managed.is_none());
    assert!(app.settings.account.pending());
    let _ = release.send(());
}

#[test]
fn scoped_settings_pending_write_should_guard_an_already_open_account_selection() {
    let directory = tempfile::tempdir().unwrap();
    let fixture = fixture();
    let mut app = app(&fixture, Config::default(), directory.path());
    app.auth.phase = crate::auth::Phase::Accounts;
    app.auth.active_id = Some("first".into());
    app.auth.accounts = vec![crate::reddit::SavedAccount {
        id: "first".into(),
        name: "reader".into(),
        expired: false,
        source: None,
    }];
    app.settings.account.writing = true;
    assert!(!app.select_auth());
    assert_eq!(app.auth.phase, crate::auth::Phase::Accounts);
    assert!(
        app.auth
            .message
            .as_ref()
            .unwrap()
            .contains("preference save")
    );
    app.settings.account.writing = false;
}

#[test]
fn scoped_settings_pending_write_should_guard_an_already_open_account_confirmation() {
    let directory = tempfile::tempdir().unwrap();
    let fixture = fixture();
    let mut app = app(&fixture, Config::default(), directory.path());
    app.auth.phase = crate::auth::Phase::Confirm;
    app.settings.account.writing = true;
    app.confirm_auth();
    assert_eq!(app.auth.phase, crate::auth::Phase::Confirm);
    assert!(
        app.auth
            .message
            .as_ref()
            .unwrap()
            .contains("preference save")
    );
    app.settings.account.writing = false;
}

#[test]
fn scoped_settings_unidentified_session_should_not_edit_reader_rules() {
    let directory = tempfile::tempdir().unwrap();
    let reddit = crate::reddit::Reddit::test_at("http://127.0.0.1:9", false);
    let (pool, _) = Pool::start(reddit.clone(), 1, None);
    let config = Config::default();
    let mut app = App::new(
        config.clone(),
        pool,
        reddit,
        directory.path().join("state.json"),
        Picker::halfblocks(),
    );
    app.install_settings_store(Store::memory(config));
    assert!(app.config.filters.keywords.is_empty());
    app.change_setting("filters.keywords", toml::Value::Array(vec!["wrong".into()]));
    assert!(app.settings.message.as_ref().unwrap().contains("identify"));
    assert!(!app.settings.store.dirty());
}

#[test]
fn settings_without_an_identified_account_should_skip_reader_rows() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let directory = tempfile::tempdir().unwrap();
    let reddit = crate::reddit::Reddit::disconnected(false).unwrap();
    let (pool, _) = Pool::start(reddit.clone(), 1, None);
    let mut app = App::new(
        Config::default(),
        pool,
        reddit,
        directory.path().join("state.json"),
        Picker::halfblocks(),
    );
    app.install_settings_store(Store::memory(Config::default()));
    app.settings.selected = SETTINGS
        .iter()
        .rposition(|f| f.scope == Scope::Reddit)
        .unwrap();
    app.settings_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    assert_eq!(SETTINGS[app.settings.selected].scope, Scope::Device);
}

#[test]
fn scoped_settings_switch_should_replace_reader_rules_and_keep_device_preferences() {
    let directory = tempfile::tempdir().unwrap();
    let fixture = fixture();
    let config:Config=toml::from_str("[accounts.ReAdEr.filters]\nkeywords=[\"first\"]\n[accounts.Other.highlight]\nkeywords=[\"second\"]\n[media]\nimage_preview=false\n").unwrap();
    let mut app = app(&fixture, config, directory.path());
    assert_eq!(app.config.filters.keywords, ["first"]);
    app.install_connection(fixture.saved_reddit(&saved_export("second", "other")));
    assert!(app.config.filters.keywords.is_empty());
    assert_eq!(app.config.highlight.keywords, ["second"]);
    assert!(!app.show_images);
    app.install_connection(fixture.saved_reddit(&saved_export("first", "READER")));
    assert_eq!(app.config.filters.keywords, ["first"]);
    assert!(app.config.highlight.keywords.is_empty());
    assert!(!app.show_images);
}

#[test]
fn scoped_settings_switch_should_not_wait_for_an_obsolete_preference_read() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let directory = tempfile::tempdir().unwrap();
    let fixture = fixture();
    let mut app = app(&fixture, Config::default(), directory.path());
    let (release, gate) = mpsc::channel();
    let completed = Arc::new(AtomicBool::new(false));
    let done = completed.clone();
    let mut task = Task::start(move || {
        let _ = gate.recv_timeout(Duration::from_secs(2));
        done.store(true, Ordering::SeqCst);
        Ok(Preferences {
            show_nsfw: true,
            video_autoplay: true,
            mature_content_editable: true,
        })
    });
    task.join_on_drop = false;
    app.settings.account.task = Some(task);
    app.settings.account.owner = Some("first".into());
    app.install_connection(fixture.saved_reddit(&saved_export("second", "other")));
    assert!(!completed.load(Ordering::SeqCst));
    assert!(!app.config.show_nsfw && !app.config.media.video_autoplay);
    let _ = release.send(());
}

#[test]
fn scoped_settings_stale_account_completion_should_not_change_the_active_policy() {
    let directory = tempfile::tempdir().unwrap();
    let fixture = fixture();
    let mut app = app(&fixture, Config::default(), directory.path());
    app.settings.account.owner = Some("other".into());
    app.settings.account.task = Some(Task::start(|| {
        Ok(Preferences {
            show_nsfw: true,
            video_autoplay: true,
            mature_content_editable: true,
        })
    }));
    finish(&mut app);
    assert!(app.settings.account.values.is_none());
    assert!(!app.config.show_nsfw);
}

#[test]
fn scoped_settings_failed_write_should_require_readback_before_another_change() {
    let directory = tempfile::tempdir().unwrap();
    let fixture = fixture();
    let mut app = app(&fixture, Config::default(), directory.path());
    app.settings.account.values = Some(Preferences {
        show_nsfw: false,
        video_autoplay: true,
        mature_content_editable: true,
    });
    app.settings.account.owner = Some("first".into());
    app.settings.account.writing = true;
    app.settings.account.task = Some(Task::start(|| anyhow::bail!("Save may have succeeded")));
    finish(&mut app);
    assert!(app.settings.account.values.is_none());
    assert!(
        app.settings
            .account
            .error
            .as_ref()
            .unwrap()
            .contains("may have succeeded")
    );
    app.change_setting("media.video_autoplay", false.into());
    assert!(app.settings.account.task.is_none());
    assert!(!app.settings.writing());
    assert!(!app.settings.store.dirty());
}

#[test]
fn scoped_settings_loading_reddit_should_leave_file_edits_usable() {
    let directory = tempfile::tempdir().unwrap();
    let fixture = fixture();
    let mut app = app(&fixture, Config::default(), directory.path());
    app.load_account_preferences(false);
    app.change_setting("filters.keywords", toml::Value::Array(vec!["local".into()]));
    assert_eq!(app.config.filters.keywords, ["local"]);
    finish(&mut app);
    assert_eq!(app.config.filters.keywords, ["local"]);
    assert!(!app.config.media.video_autoplay);
}
