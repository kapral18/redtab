use super::*;
use crate::reddit::tests::{Fixture, Reply, saved_export};
use ratatui::{Terminal, backend::TestBackend};
use serde_json::json;

#[test]
fn a_profile_with_three_links_should_render_as_a_compact_dialog() {
    use crate::reddit::experience::{Entry, Page, Query};
    let fixture = Fixture::new(|_| Reply::json(json!({})));
    let reddit = fixture.saved_reddit(&saved_export("reader", "reader"));
    let (pool, _) = crate::worker::Pool::start(reddit.clone(), 1, None);
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::new(
        crate::config::Config {
            media: crate::config::MediaSettings {
                image_preview: false,
                ..Default::default()
            },
            ..Default::default()
        },
        pool,
        reddit,
        directory.path().join("state.json"),
        ratatui_image::picker::Picker::halfblocks(),
    );
    app.experience.screens.push(Screen::Browse {
        pagination: Default::default(),
        query: Query::Profile("reader".into()),
        page: Page {
            title: "u/reader".into(),
            text: format!(
                "Post karma: {}23499{}    Comment karma: 43077",
                crate::model::TEXT_BOLD,
                crate::model::TEXT_END
            ),
            entries: ["Overview", "Posts", "Comments"]
                .map(|title| Entry {
                    icon: Default::default(),
                    title: title.into(),
                    body: String::new(),
                    target: None,
                    query: Some(Query::Profile("reader".into())),
                    post: None,
                })
                .into(),
            after: None,
            url: "/user/reader".into(),
        },
        selected: 2,
        scroll: 0,
    });
    let mut terminal = Terminal::new(TestBackend::new(120, 60)).unwrap();
    terminal
        .draw(|f| {
            super::draw(f, &mut app);
        })
        .unwrap();
    let karma = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .find(|cell| cell.symbol() == "2")
        .unwrap();
    assert!(karma.modifier.contains(Modifier::BOLD), "{karma:?}");
    let rows: Vec<String> = terminal
        .backend()
        .buffer()
        .content()
        .chunks(120)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect())
        .collect();
    let occupied: Vec<_> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| !row.trim().is_empty())
        .collect();
    assert!(
        occupied.last().unwrap().0 - occupied.first().unwrap().0 < 12,
        "{rows:?}"
    );
    assert!(rows.iter().any(|row| row.contains("Post karma: 23499")));
    assert!(rows.iter().any(|row| row.contains("Comments")));
    assert!(!rows.iter().any(|row| row.contains("Selected content")));
}

#[test]
fn failed_action_loading_should_show_retry_controls_without_submission_advice() {
    use crate::reddit::experience::{Query, Request, Target};
    let fixture = Fixture::new(|_| Reply::json(json!({})));
    let reddit = fixture.saved_reddit(&saved_export("reader", "reader"));
    let (pool, _) = crate::worker::Pool::start(reddit.clone(), 1, None);
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::new(
        crate::config::Config {
            media: crate::config::MediaSettings {
                image_preview: false,
                ..Default::default()
            },
            ..Default::default()
        },
        pool,
        reddit,
        directory.path().join("state.json"),
        ratatui_image::picker::Picker::halfblocks(),
    );
    let query = Query::Inspect(Target {
        id: "t1_comment".into(),
        author: "reader".into(),
        subreddit: "rust".into(),
        permalink: "/comments/post/_/comment".into(),
    });
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
    let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();

    terminal
        .draw(|frame| {
            super::draw(frame, &mut app);
        })
        .unwrap();

    let actual: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(actual.contains("Could not load actions"));
    assert!(actual.contains("HTTP 403"));
    assert!(actual.contains("F1 help"));
    assert!(actual.contains("r retry"));
    assert!(actual.contains("Esc back"));
    assert!(!actual.contains("submitting again"));
    assert!(!actual.contains("SPC f s sort"));
}

#[test]
fn when_a_reddit_menu_is_rendered_in_a_narrow_terminal_it_should_keep_navigation_visible() {
    let fixture = Fixture::new(|_| Reply::json(json!({})));
    let reddit = fixture.saved_reddit(&saved_export("reader", "reader"));
    let (pool, _) = crate::worker::Pool::start(reddit.clone(), 1, None);
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::new(
        crate::config::Config {
            media: crate::config::MediaSettings {
                image_preview: false,
                ..Default::default()
            },
            ..Default::default()
        },
        pool,
        reddit,
        directory.path().join("state.json"),
        ratatui_image::picker::Picker::halfblocks(),
    );
    app.reddit_menu();
    let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
    terminal
        .draw(|frame| {
            super::draw(frame, &mut app);
        })
        .unwrap();
    let actual: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(actual.contains("Unread notifications"));
    assert!(actual.contains("Esc back"));
}

#[test]
fn when_editing_raw_markdown_spoiler_delimiters_it_should_keep_the_literal_source() {
    assert_eq!(clean(">!secret!<"), ">!secret!<");
}

#[test]
fn when_preview_text_has_spoilers_or_terminal_controls_it_should_mask_or_remove_them() {
    assert_eq!(
        clean(&format!("a\u{1b}{}秘密{}b", SPOILER_START, SPOILER_END)),
        "a▒▒▒▒b"
    );
}
