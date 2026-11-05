use super::*;

#[test]
fn when_reconnecting_after_disconnect_it_should_restore_that_accounts_bookmarks() {
    use crate::reddit::tests::{Fixture, Reply};
    let fixture = Fixture::new(|_| {
        Reply::json(serde_json::json!({"data":{"id":"readerid", "name":"reader"}}))
    });
    let (mut app, _messages) = app_at(&fixture.base, &[]);
    let directory = tempfile::tempdir().unwrap();
    app.state_path = directory.path().join("accounts/readerid/state.json");
    std::fs::create_dir_all(app.state_path.parent().unwrap()).unwrap();
    app.reddit.verify_account().unwrap();
    app.state.toggle_bookmark(&post("saved", "rust"), 1000);
    app.save_state();
    app.install_connection(Reddit::disconnected(false).unwrap());
    let connection = fixture.reddit();
    connection.verify_account().unwrap();

    app.install_connection(connection);

    assert!(app.state.is_bookmarked("saved"));
    assert_eq!(
        app.state_path,
        directory.path().join("accounts/readerid/state.json")
    );
    assert_eq!(app.reddit.account().unwrap().id, "readerid");
}

#[test]
fn when_switching_between_saved_accounts_and_back_it_should_restore_each_accounts_own_bookmarks() {
    use crate::reddit::tests::{Fixture, Reply, saved_export};
    let first = Fixture::new(|_| {
        Reply::json(serde_json::json!({"data":{"id":"first","name":"first_reader"}}))
    });
    let second = Fixture::new(|_| {
        Reply::json(serde_json::json!({"data":{"id":"second","name":"second_reader"}}))
    });
    let (mut app, _messages) = app_at(&first.base, &[]);
    let directory = tempfile::tempdir().unwrap();
    app.state_path = directory.path().join("accounts/first/state.json");
    app.reddit.verify_account().unwrap();
    app.state
        .toggle_bookmark(&post("first_saved", "rust"), 1000);
    app.save_state();
    app.install_connection(second.saved_reddit(&saved_export("second", "second_reader")));
    app.state
        .toggle_bookmark(&post("second_saved", "python"), 1000);
    app.save_state();

    app.install_connection(first.saved_reddit(&saved_export("first", "first_reader")));

    assert!(app.state.is_bookmarked("first_saved"));
    assert!(!app.state.is_bookmarked("second_saved"));
    assert_eq!(
        app.state_path,
        directory.path().join("accounts/first/state.json")
    );
    let other = State::load(&directory.path().join("accounts/second/state.json")).0;
    assert!(other.is_bookmarked("second_saved"));
    assert!(!other.is_bookmarked("first_saved"));
}

#[test]
fn when_switching_verified_accounts_it_should_drop_old_account_payloads_and_use_a_separate_state_file()
 {
    use crate::reddit::tests::{Fixture, Reply};
    let old = Fixture::new(|_| {
        Reply::json(serde_json::json!({"data":{"id":"oldaccount", "name":"oldreader"}}))
    });
    let new = Fixture::new(|_| {
        Reply::json(serde_json::json!({"data":{"id":"newaccount", "name":"newreader"}}))
    });
    let (mut app, _messages) = app_at(&old.base, &["privatecommunity"]);
    let directory = tempfile::tempdir().unwrap();
    app.state_path = directory.path().join("state.json");
    app.reddit.verify_account().unwrap();
    app.tabs[0].feed.posts = vec![post("privatepost", "privatecommunity")];
    app.all_subs.groups[0].posts = vec![post("islandpost", "privatecommunity")];
    app.pending_media.insert("/old-account".into());
    app.state
        .toggle_bookmark(&post("privatebookmark", "privatecommunity"), 1000);
    let connection = new.reddit();
    connection.verify_account().unwrap();
    app.save_state();
    app.install_connection(connection);
    assert!(app.tabs.iter().all(|t| t.feed.posts.is_empty()));
    assert!(app.all_subs.groups.is_empty());
    assert!(app.pending_media.is_empty());
    assert!(app.state.bookmarks().is_empty());
    assert_eq!(
        app.state_path,
        directory.path().join("accounts/newaccount/state.json")
    );
    assert!(
        directory.path().join("state.json").exists(),
        "the prior state stays intact"
    );
}

#[test]
fn when_a_loaded_page_marks_the_original_deleted_its_list_and_bookmark_should_learn_the_flag() {
    let (mut app, _) = split_feed(&["cross", "other"]);
    app.state.toggle_bookmark(&post("cross", "s"), 1);
    let generation = app.post_view.as_ref().unwrap().generation;
    let mut page_post = post("cross", "s");
    page_post.original_deleted = true;

    app.on_msg(Msg::Post {
        generation,
        result: Ok(Box::new(PostPage {
            post: page_post,
            body: String::new(),
            comments: Vec::new(),
        })),
    });

    assert!(app.active_tab().feed.posts[0].original_deleted);
    assert!(!app.active_tab().feed.posts[1].original_deleted);
    assert!(app.state.bookmarks()[0].post.original_deleted);
}

#[test]
fn opening_a_bookmarked_post_records_its_spoiler_tag_only() {
    let mut app = app_with(&[]);
    let mut listed = post("p", "s");
    listed.media = Media::Gallery {
        images: vec![],
        previews: Vec::new(),
        thumb: Some("/preview/small.jpg".into()),
    };
    app.state.toggle_bookmark(&listed, 1);
    app.open_post_permalink(listed.clone(), "/r/s/comments/p/x/".into());
    let generation = app.post_view.as_ref().unwrap().generation;
    let mut page_post = post("p", "s");
    page_post.spoiler = true;
    page_post.media = Media::Gallery {
        images: vec!["https://i.redd.it/full.jpg".into()],
        previews: Vec::new(),
        thumb: None,
    };

    app.on_msg(Msg::Post {
        generation,
        result: Ok(Box::new(PostPage {
            post: page_post,
            body: String::new(),
            comments: vec![],
        })),
    });

    let stored = &app.state.bookmarks()[0];
    assert!(stored.post.spoiler);
    assert_eq!(
        stored.post.media, listed.media,
        "the small listing thumbnail stays"
    );
    assert_eq!(stored.saved_at, 1);
}

#[test]
fn removing_a_bookmark_from_another_tab_keeps_the_bookmarks_selection_usable() {
    let mut app = app_with(&["s"]);
    for id in ["a", "b", "c"] {
        app.state.toggle_bookmark(&post(id, "s"), 1);
    }
    // Newest first: c, b, a. Select the last row, "a".
    app.tabs[BOOKMARKS_TAB].feed.selected = 2;
    let i = tab_index(&app, "s");
    app.switch_tab(i);
    let id = app.tabs[i].id;
    let g = app.tabs[i].feed.generation;
    app.on_msg(Msg::Feed {
        tab: id,
        generation: g,
        append: false,
        result: Ok(listing("s", &["b"], None)),
    });

    app.toggle_bookmark(); // removes "b" from the subreddit tab
    app.switch_tab(BOOKMARKS_TAB);
    let current = app.current_post().map(|p| p.id);

    assert_eq!(current.as_deref(), Some("a"));
}

#[test]
fn a_merge_from_another_instance_keeps_the_selected_bookmark() {
    let mut app = app_with(&[]);
    app.state.toggle_bookmark(&post("x", "s"), 50);
    app.state.toggle_bookmark(&post("y", "s"), 100);
    app.switch_tab(BOOKMARKS_TAB);
    app.tabs[BOOKMARKS_TAB].feed.selected = 1; // [y, x] → x
    let mut other = State::default();
    other.toggle_bookmark(&post("z", "s"), 200);
    other.save(&app.state_path, 200).unwrap();

    let redraw = app.persist();

    assert!(redraw);
    assert_eq!(app.current_post().unwrap().id, "x");
}

#[test]
fn a_failed_write_after_a_merge_keeps_the_selection_and_reports_the_error() {
    use std::os::unix::fs::PermissionsExt;
    let mut app = app_with(&[]);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    app.state_path = path.clone();
    app.state.toggle_bookmark(&post("x", "s"), 50);
    app.state.toggle_bookmark(&post("y", "s"), 100);
    app.switch_tab(BOOKMARKS_TAB);
    app.tabs[BOOKMARKS_TAB].feed.selected = 1; // [y, x] → x
    let mut other = State::default();
    other.toggle_bookmark(&post("z", "s"), 200);
    other.save(&path, 200).unwrap();
    // A read-only directory: the merge reads the file, then writing the
    // temp file fails.
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500)).unwrap();

    let redraw = app.persist();
    let selected = app.current_post().map(|p| p.id);
    app.toggle_bookmark();
    let status = app.status.as_ref().map(|s| (s.error, s.text.clone()));
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();

    assert!(redraw);
    assert_eq!(selected.as_deref(), Some("x"));
    assert!(
        app.state.is_bookmarked("z"),
        "the merge ran before the write failed"
    );
    let (error, text) = status.unwrap();
    assert!(
        error,
        "a failed bookmark save must not read as success: {text}"
    );
}

#[test]
fn a_bookmark_merged_above_the_selection_keeps_the_selected_row_on_screen() {
    let mut app = app_with(&[]);
    for (n, id) in ["c", "b", "a"].into_iter().enumerate() {
        app.state
            .toggle_bookmark(&post(id, "rust"), 1000 + n as i64);
    }
    app.switch_tab(BOOKMARKS_TAB);
    app.tabs[BOOKMARKS_TAB].feed.selected = 2;
    app.tabs[BOOKMARKS_TAB].feed.offset = 1;
    let selected = app.selected_bookmark_id();

    app.state.toggle_bookmark(&post("merged", "rust"), 2000);
    app.reselect_bookmark(selected);

    let feed = &app.tabs[BOOKMARKS_TAB].feed;
    assert_eq!(app.current_post().unwrap().id, "c");
    assert_eq!(feed.selected - feed.offset, 1);
}

#[test]
fn a_bookmark_added_above_an_untouched_bookmarks_top_shows_at_the_top() {
    let mut app = app_with(&[]);
    app.state.toggle_bookmark(&post("old", "rust"), 1000);
    app.switch_tab(BOOKMARKS_TAB);
    let selected = app.selected_bookmark_id();

    app.state.toggle_bookmark(&post("fresh", "rust"), 2000);
    app.reselect_bookmark(selected);

    let feed = &app.tabs[BOOKMARKS_TAB].feed;
    assert_eq!(app.current_post().unwrap().id, "fresh");
    assert_eq!((feed.selected, feed.offset), (0, 0));

    // An open top bookmark, or a first card scrolled into, is being read.
    for open in [true, false] {
        let mut app = app_with(&[]);
        app.state.toggle_bookmark(&post("old", "rust"), 1000);
        app.switch_tab(BOOKMARKS_TAB);
        if open {
            app.resize(200);
            app.open_current_post();
            app.focus = Focus::List;
        } else {
            app.tabs[BOOKMARKS_TAB].feed.cards.line = 3;
        }
        let selected = app.selected_bookmark_id();

        app.state.toggle_bookmark(&post("fresh", "rust"), 2000);
        app.reselect_bookmark(selected);

        let feed = &app.tabs[BOOKMARKS_TAB].feed;
        assert_eq!(app.current_post().unwrap().id, "old", "open {open}");
        assert_eq!((feed.selected, feed.offset), (1, 1), "open {open}");
        if open {
            assert_eq!(app.post_view.as_ref().unwrap().shown_post().id, "old");
        }
    }
}

#[test]
fn settings_nsfw_rebind_should_clear_old_content_without_carrying_preferences_to_another_account() {
    use crate::reddit::tests::{Fixture, Reply, saved_export};
    let fixture = Fixture::new(|_| Reply::json(serde_json::json!({"data":{"children":[]}})));
    let mut app = app_with(&["rust"]);
    let directory = tempfile::tempdir().unwrap();
    app.state_path = directory.path().join("state.json");
    app.state.toggle_bookmark(&post("kept", "rust"), 1);
    let mut adult = post("adult", "rust");
    adult.nsfw = true;
    app.state.toggle_bookmark(&adult, 2);
    app.tabs[0].feed.posts = vec![adult];
    let fingerprint = app.reddit.session_fingerprint().to_string();
    confirm_preferences(&mut app, true);
    assert_eq!(app.reddit.session_fingerprint(), fingerprint);
    assert!(app.tabs[0].feed.posts.is_empty());
    assert_eq!(app.visible_bookmarks().count(), 2);
    confirm_preferences(&mut app, false);
    assert_eq!(app.visible_bookmarks().count(), 1);
    assert!(app.state.is_bookmarked("adult"));
    confirm_preferences(&mut app, true);
    app.change_setting("media.image_preview", toml::Value::Boolean(false));
    app.install_connection(fixture.saved_reddit(&saved_export("other", "reader")));
    assert!(!app.config.show_nsfw && !app.settings.store.current.show_nsfw);
    assert!(!app.show_images);
    assert!(app.settings.store.dirty());
    assert_eq!(
        app.settings.store.persistence,
        crate::config::store::Persistence::Temporary
    );
    assert!(app.reddit.cache_namespace().contains("nsfw=false"));
}

#[test]
fn unread_observations_survive_saving_and_another_instances_reads_trigger_redraw() {
    let (mut app, i) = unread_badge_app();
    let dir = tempfile::tempdir().unwrap();
    app.state_path = dir.path().join("state.json");
    app.save_state();
    assert_eq!(
        State::load(&app.state_path).0.unread("s", &app.config),
        Some(2)
    );
    let (mut other, _) = State::load(&app.state_path);
    other.mark_read(&post("new1", "s"), now());
    other.save(&app.state_path, now()).unwrap();

    let redraw = app.persist_checked().unwrap();

    assert!(redraw);
    assert_eq!(app.badge(&app.tabs[i]), UnreadBadge::Count(1));
    assert!(!app.state.is_read("new2"));
}

#[test]
fn merged_reads_redraw_home_posts_even_without_a_subreddit_tab() {
    let mut app = app_with(&[]);
    app.active = 0;
    app.load_feed(0, false);
    let (tab, generation) = (app.tabs[0].id, app.tabs[0].feed.generation);
    app.on_msg(Msg::Feed {
        tab,
        generation,
        append: false,
        result: Ok(listing("unjoined", &["homepost"], None)),
    });
    let dir = tempfile::tempdir().unwrap();
    app.state_path = dir.path().join("state.json");
    app.save_state();
    let (mut other, _) = State::load(&app.state_path);
    other.mark_read(&post("homepost", "unjoined"), now());
    other.save(&app.state_path, now()).unwrap();

    let redraw = app.persist_checked().unwrap();

    assert!(redraw);
    assert!(app.state.is_read("homepost"));
    assert!(
        !app.tabs
            .iter()
            .any(|tab| tab.subreddit() == Some("unjoined"))
    );
}
