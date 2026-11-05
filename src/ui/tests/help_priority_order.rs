use super::*;

#[test]
fn post_help_should_put_comment_navigation_before_reddit_tools() {
    let (mut app, _) = open_long_post(|_| {});
    press(&mut app, '?');

    let shown = screen(&mut app, 100, 24);

    assert!(shown.contains("Next / previous comment"));
    let entries = help::entries(&app);
    for motion in ["]] / [[", "]p / [p", "]s / [s", "gt / gT"] {
        assert!(
            entries.iter().any(|(key, _)| key == motion),
            "missing {motion}"
        );
    }
    let position = |wanted: &str| entries.iter().position(|(key, _)| key == wanted).unwrap();
    assert!(position("Here") < position("Move"));
    assert!(position("Move") < position("C-d / C-u"));
    assert!(position("Space leader") < position("SPC b b"));
}

#[test]
fn sidebar_help_should_prioritize_switching_tabs() {
    let mut app = app(&[]);
    app.focus = Focus::Sidebar;
    press(&mut app, '?');

    let shown = screen(&mut app, 100, 12);

    assert!(shown.contains("Sidebar and buffers"));
    assert!(shown.contains("Next / previous sidebar tab"));
    assert!(!shown.contains("Reddit: saved"));
}

#[test]
fn compact_help_should_put_applicable_controls_before_card_line_scrolling() {
    let mut app = feed_app(Picker::halfblocks());
    app.active = 0;
    key(&mut app, KeyCode::F(1), KeyModifiers::NONE);

    let shown = screen(&mut app, 100, 12);

    assert!(shown.contains("Feed"));
    assert!(shown.contains("Card / Compact view"));
    assert!(!shown.contains("cards: scroll down / up one line"));
}

#[test]
fn selected_comment_help_should_prioritize_comment_actions() {
    let (mut app, _) = open_long_post(|_| {});
    app.activate_comment();
    press(&mut app, '?');

    let shown = screen(&mut app, 100, 12);

    assert!(shown.contains("Post and comments"));
    assert!(!help::entries(&app).iter().any(|(key, _)| key == "za"));
    assert!(shown.contains("Reddit content actions"));
}

#[test]
fn video_help_should_prioritize_playback_controls() {
    let mut app = open_video_post();
    press(&mut app, '?');

    let shown = screen(&mut app, 100, 12);

    assert!(shown.contains("Post and comments"));
    assert!(shown.contains("Play / pause / retry video"));
    assert!(!shown.contains("Reddit: saved"));
}

#[test]
fn help_should_prioritize_reading_when_the_video_is_scrolled_away() {
    let mut app = open_video_post();
    app.scroll_post(1000);
    screen(&mut app, 100, 30);
    assert!(!app.post_view.as_ref().unwrap().image_shown);
    press(&mut app, '?');

    let shown = screen(&mut app, 100, 12);

    assert!(shown.contains("Post and comments"));
    assert!(shown.contains("Next / previous comment"));
    assert!(!shown.contains("video: pause"));
}

#[test]
fn gallery_help_should_prioritize_browsing_images() {
    let mut app = image_post("caption");
    app.post_view
        .as_mut()
        .unwrap()
        .page
        .as_mut()
        .unwrap()
        .post
        .media = Media::Gallery {
        images: vec!["https://i.redd.it/first.png".into()],
        previews: Vec::new(),
        thumb: None,
    };
    press(&mut app, '?');

    let shown = screen(&mut app, 100, 12);

    assert!(shown.contains("Post and comments"));
    assert!(shown.contains("Next / previous gallery image"));
    assert!(!shown.contains("Reddit: saved"));
}

#[test]
fn hidden_media_help_should_prioritize_revealing_spoilers() {
    let mut app = open_video_post();
    app.post_view
        .as_mut()
        .unwrap()
        .page
        .as_mut()
        .unwrap()
        .post
        .spoiler = true;
    press(&mut app, '?');

    let shown = screen(&mut app, 100, 10);

    assert!(shown.contains("Post and comments"));
    assert!(shown.contains("Reveal / hide spoilers"));
    assert!(!shown.contains("video: pause"));
    assert!(!help::entries(&app).iter().any(|(key, _)| key == "/"));
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    press_keys(&mut app, "zs");
    press(&mut app, '/');
    assert!(
        matches!(app.experience.screens.last(), Some(crate::app::experience::Screen::Input(form)) if form.title == "Find in loaded comments")
    );
}

#[test]
fn sort_help_should_only_offer_the_current_feeds_supported_sorts() {
    for (context, rising, best) in [(0, false, false), (1, true, true), (2, true, false)] {
        let mut app = app(&["rust"]);
        match context {
            0 => app.open_user("blackvault"),
            1 => app.active = 0,
            _ => app.open_subreddit("rust"),
        }
        app.overlay = Some(Overlay::Sort { selected: 0 });
        key(
            &mut app,
            crossterm::event::KeyCode::F(1),
            crossterm::event::KeyModifiers::NONE,
        );
        let shown = screen(&mut app, 120, 60);
        assert_eq!(shown.contains("rising"), rising, "{shown}");
        assert_eq!(shown.contains("best"), best, "{shown}");
        assert!(shown.contains("controversial"), "{shown}");
    }
}

#[test]
fn ui_audit_auth_confirmation_help_should_not_advertise_account_navigation() {
    let mut app = app(&[]);
    app.overlay = Some(Overlay::Auth);
    app.auth.phase = crate::auth::Phase::Confirm;
    let actual = help::entries(&app);
    let context: Vec<_> = actual.iter().take_while(|(key, _)| key != "Feed").collect();
    assert!(
        !context
            .iter()
            .any(|(_, text)| text.contains("select an account"))
    );
    assert!(
        !context
            .iter()
            .any(|(_, text)| text.contains("reconnect the selected"))
    );
}

#[test]
fn ui_audit_single_field_help_should_not_advertise_field_navigation() {
    let app = form_app("Open a user profile");
    let actual = help::entries(&app);
    assert!(
        !actual
            .iter()
            .any(|(_, text)| text == "next / previous field")
    );
}

#[test]
fn help_should_prioritize_each_list_view() {
    for (tab, compact, title, expected_key) in [
        (0, false, "Feed", "Toggle Card / Compact"),
        (1, true, "Feed", "Remove the local bookmark"),
        (2, false, "Feed", "Next / previous community group"),
        (4, false, "Feed", "Search this community"),
    ] {
        let mut app = card_feed_app(Picker::halfblocks());
        if tab == 1 {
            let post = app.tabs[4].feed.posts[0].clone();
            app.state.toggle_bookmark(&post, 0);
        }
        app.active = tab;
        if compact {
            app.state.toggle_list_view();
        }
        key(&mut app, KeyCode::F(1), KeyModifiers::NONE);

        let shown = screen(&mut app, 120, 20);

        assert!(shown.contains(title), "missing {title}");
        assert!(shown.contains(expected_key), "missing {expected_key}");
    }
}

#[test]
fn help_should_follow_the_focused_pane_when_a_post_remains_beside_the_list() {
    let (mut app, _) = open_long_post(|_| {});
    app.resize(160);
    app.focus_main();
    press(&mut app, '?');

    let shown = screen(&mut app, 160, 20);

    assert!(shown.contains("Feed"));
    assert!(!shown.contains("Post and comments"));
}

#[test]
fn dialog_help_should_show_the_active_action_before_view_shortcuts() {
    for (overlay, title, description) in [
        (
            Overlay::Picker(crate::app::PickerState::default()),
            "Text entry",
            "Accept a single-line",
        ),
        (
            Overlay::Search {
                input: "query".into(),
            },
            "Text entry",
            "Accept a single-line",
        ),
        (Overlay::Sort { selected: 0 }, "Choose sort", "Back / open"),
        (
            Overlay::Membership {
                target: crate::model::MembershipTarget::Community("rust".into()),
                join: true,
            },
            "Confirmation",
            "Confirm the displayed action",
        ),
        (Overlay::Auth, "Accounts", "Back / open"),
    ] {
        let mut app = app(&[]);
        app.auth.phase = crate::auth::Phase::Accounts;
        app.overlay = Some(overlay);
        key(&mut app, KeyCode::F(1), KeyModifiers::NONE);

        let shown = screen(&mut app, 100, 12);

        assert!(shown.contains(title), "missing {title}");
        assert!(shown.contains(description), "missing {description}");
        assert!(!shown.contains("Reddit: saved"));
    }
}
