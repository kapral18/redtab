use super::*;
use crate::reddit::tests::{Fixture, Reply, saved_export};
use ratatui_image::picker::Picker;
use serde_json::json;

mod action_menu_contents;
mod collections;
mod comment_sort_requests;
mod custom_feeds;
mod direct_key_actions;
mod feed_opening;
mod form_draft_validation;
mod hide_write_invalidation;
mod links;
mod menu_targets;
mod pagination_scroll;
mod preview_navigation;
mod read_action_recovery;
mod reply_editor_submission;
mod search_tab_rows;
mod settings_find;

fn app() -> (App, tempfile::TempDir, Fixture) {
    let fixture = Fixture::new(|_| {
        Reply::json(json!({"kind":"Listing","data":{"children":[],"after":null}}))
    });
    let reddit = fixture.saved_reddit(&saved_export("reader", "reader"));
    let (pool, _) = Pool::start(reddit.clone(), 1, None);
    let dir = tempfile::tempdir().unwrap();
    let app = App::new(
        Config {
            media: crate::config::MediaSettings {
                image_preview: false,
                ..Default::default()
            },
            ..Config::default()
        },
        pool,
        reddit,
        dir.path().join("state.json"),
        Picker::halfblocks(),
    );
    (app, dir, fixture)
}

fn key(app: &mut App, code: KeyCode) {
    crate::keys::handle(app, KeyEvent::new(code, KeyModifiers::NONE));
}

fn sequence(app: &mut App, keys: &str) {
    for c in keys.chars() {
        key(app, KeyCode::Char(c));
    }
}

fn reply_form(app: &mut App) {
    app.overlay = Some(Overlay::Experience);
    app.experience.screens.push(Screen::Input(form(
        "Reply",
        FormKind::Reply(Target {
            id: "t1_original".into(),
            author: "author".into(),
            subreddit: "rust".into(),
            permalink: "/r/rust/comments/post/title/original/".into(),
        }),
        vec![field("Reply", "", true)],
        "/r/rust/comments/post/title/original/",
    )));
}

fn post_page() -> PostPage {
    crate::reddit::parse::parse_post_page(include_str!("../../../tests/fixtures/post.json"))
        .unwrap()
}
