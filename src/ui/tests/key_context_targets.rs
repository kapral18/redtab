use super::*;

#[test]
fn when_a_resize_hides_the_list_the_next_key_should_target_detail() {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    let (mut app, _) = open_long_post(|_| {});
    let mut other = app.all_subs.groups[0].posts[0].clone();
    other.id = "other".into();
    other.permalink = "/r/s/comments/other/x/".into();
    app.all_subs.groups[0].posts.push(other);
    screen(&mut app, 180, 30);
    press(&mut app, 'h');
    press(&mut app, 'j');

    crate::handle_event(&mut app, Event::Resize(157, 30));
    crate::handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE)),
    );

    assert_eq!(app.post_view.as_ref().unwrap().post.id, "other");
    assert_eq!(app.all_subs.post, 1, "the hidden list selection stays put");
}

#[test]
fn when_a_resize_shows_the_list_h_should_focus_it_before_the_next_key() {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    let (mut app, _) = open_long_post(|_| {});
    let mut other = app.all_subs.groups[0].posts[0].clone();
    other.id = "other".into();
    other.permalink = "/r/s/comments/other/x/".into();
    app.all_subs.groups[0].posts.push(other);
    press(&mut app, 'j');
    screen(&mut app, 100, 30);

    crate::handle_event(&mut app, Event::Resize(180, 30));
    crate::handle_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE)),
    );
    assert!(
        !app.post_view.as_ref().unwrap().free_scroll,
        "h kept the comment selected"
    );
    press(&mut app, 'j');

    assert_eq!(app.all_subs.post, 1);
}

#[test]
fn key_context_audit_open_and_media_keys_should_require_a_usable_target() {
    use crate::keys::map::{self, Action};
    use crate::reddit::experience::Query;
    let mut empty = app(&[]);
    empty.active = 0;
    empty.focus_main();
    assert!(!map::available(&empty, Action::Right));
    assert!(!map::available(&empty, Action::Browser));
    empty.active = crate::app::ALL_SUBS_TAB;
    assert!(!map::available(&empty, Action::NextCommunity));
    let mut rules = description_popup(Query::Rules("rust".into()), "Rules", &[]);
    assert!(!map::available(&rules, Action::Right));
    let before = rules.experience.screens.len();
    press(&mut rules, 'l');
    assert_eq!(rules.experience.screens.len(), before);

    let (mut post, _) = open_long_post(|_| {});
    post.post_view
        .as_mut()
        .unwrap()
        .page
        .as_mut()
        .unwrap()
        .comments = vec![reply("leaf".into())];
    screen(&mut post, 100, 24);
    press(&mut post, 'G');
    assert!(!map::available(&post, Action::Right));
    let gallery = Media::Gallery {
        images: vec![],
        previews: vec![],
        thumb: None,
    };
    post.post_view
        .as_mut()
        .unwrap()
        .page
        .as_mut()
        .unwrap()
        .post
        .media = gallery;
    assert!(!map::available(&post, Action::NextImage));
    let video = Media::Video {
        gif: false,
        url: Some(VIDEO.into()),
        poster: None,
    };
    post.post_view
        .as_mut()
        .unwrap()
        .page
        .as_mut()
        .unwrap()
        .post
        .media = video;
    post.show_images = true;
    post.config.media.image_preview = true;
    assert!(map::available(&post, Action::Pause));
    for action in [Action::Mute, Action::SeekBack, Action::SeekForward] {
        assert!(!map::available(&post, action));
    }
    post.focus_sidebar();
    assert!(!map::available(&post, Action::Media));
    post.close_post();
    post.active = crate::app::SEARCH_TAB;
    post.search.query.insert("video");
    post.search.editing = true;
    post.focus_main();
    let mut video_post = post_with("typing", Media::None);
    video_post.media = Media::Video {
        gif: false,
        url: Some(VIDEO.into()),
        poster: None,
    };
    post.tabs[crate::app::SEARCH_TAB].feed.posts = vec![video_post];
    assert!(!map::available(&post, Action::Pause));
}

#[test]
fn key_context_audit_end_should_reach_the_last_visible_content() {
    let (mut post, _) = open_long_post(|_| {});
    post.post_view
        .as_mut()
        .unwrap()
        .page
        .as_mut()
        .unwrap()
        .comments
        .clear();
    screen(&mut post, 100, 24);
    press(&mut post, 'G');
    let bottom = screen(&mut post, 100, 24);
    assert!(bottom.contains("paragraph 60"), "{bottom}");
    assert!(post.post_view.as_ref().unwrap().scroll > 0);
    press_keys(&mut post, "gg");
    assert_eq!(post.post_view.as_ref().unwrap().scroll, 0);

    let text = (1..=60)
        .map(|index| format!("Rule {index}\n"))
        .collect::<String>();
    let mut rules = description_popup(
        crate::reddit::experience::Query::Rules("rust".into()),
        &text,
        &[],
    );
    popup_snapshot(&mut rules, 80, 24);
    press(&mut rules, 'G');
    let (_, bottom) = popup_snapshot(&mut rules, 80, 24);
    assert!(bottom.contains("Rule 60"), "{bottom}");
    let scroll = match rules.experience.screens.last().unwrap() {
        crate::app::experience::Screen::Browse { scroll, .. } => *scroll,
        _ => unreachable!(),
    };
    key(&mut rules, KeyCode::Char('e'), KeyModifiers::CONTROL);
    popup_snapshot(&mut rules, 80, 24);
    key(&mut rules, KeyCode::Char('y'), KeyModifiers::CONTROL);
    popup_snapshot(&mut rules, 80, 24);
    assert!(matches!(rules.experience.screens.last(),
            Some(crate::app::experience::Screen::Browse { scroll: actual, .. }) if *actual == scroll - 1));

    let mut islands = app(&["first", "last"]);
    islands.all_subs.groups[1].posts = ["one", "two", "three"]
        .into_iter()
        .map(|id| post_with(id, Media::None))
        .collect();
    press(&mut islands, 'G');
    assert_eq!(islands.current_post().unwrap().id, "three");
    press_keys(&mut islands, "gg");
    assert_eq!((islands.all_subs.group, islands.all_subs.post), (0, 0));
}

#[test]
fn key_context_audit_narrow_confirmation_should_keep_its_explicit_enter_control() {
    use crate::app::experience::Screen;
    use crate::reddit::experience::{Action, Target};
    let mut app = app(&[]);
    app.overlay = Some(Overlay::Experience);
    app.experience.screens = vec![Screen::Confirm {
        label: "Delete this post?".into(),
        action: Action::Delete {
            target: Target::from_post(&post_with("delete", Media::None)),
        },
        native: "/r/s/comments/delete/x/".into(),
    }];
    let (_, shown) = popup_snapshot(&mut app, 24, 12);
    assert!(shown.contains("Enter") || shown.contains('⏎'), "{shown}");
    press(&mut app, 'l');
    assert!(matches!(
        app.experience.screens.last(),
        Some(Screen::Confirm { .. })
    ));
    assert!(app.experience.pending.is_none());
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(app.overlay.is_none());
}

#[test]
fn key_context_audit_empty_picker_should_keep_the_query_until_a_target_exists() {
    let mut app = app(&[]);
    press_keys(&mut app, " bb");
    crate::keys::paste(&mut app, "???");
    assert!(app.picker_entries("???").is_empty());
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(matches!(&app.overlay, Some(Overlay::Picker(picker)) if picker.input == "???"));
    key(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
    crate::keys::paste(&mut app, "Home");
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.overlay.is_none());
    assert_eq!(app.active, 0);
}

#[test]
fn key_context_audit_go_commands_should_focus_the_destination_feed() {
    for width in [100, 200] {
        for command in ["gh", "ga", "gp", "gc"] {
            let (mut app, _) = open_long_post(|_| {});
            let view = app.post_view.take();
            match command {
                "gh" => app.active = 0,
                "ga" => app.active = crate::app::ALL_SUBS_TAB,
                "gp" => app.open_subreddit("popular"),
                _ => app.open_subreddit("s"),
            }
            let destination = app.active_tab().id;
            app.post_view = view;
            app.focus = Focus::Post;
            screen(&mut app, width, 24);

            press_keys(&mut app, command);

            assert_eq!(app.active_tab().id, destination, "{command}");
            assert!(app.post_view.is_none(), "{width}: {command}");
            assert_eq!(app.focus, Focus::List);
        }
    }
}

#[test]
fn key_context_audit_invalid_terminal_keys_should_cancel_only_the_prefix() {
    for invalid in [
        KeyCode::Tab,
        KeyCode::Left,
        KeyCode::Home,
        KeyCode::Backspace,
        KeyCode::F(3),
    ] {
        let mut app = app(&["first", "second"]);
        app.active = 4;
        app.focus_main();
        press(&mut app, ']');
        assert_eq!(app.keymap.prefix, "]");
        key(&mut app, invalid, KeyModifiers::NONE);
        assert!(app.keymap.prefix.is_empty(), "{invalid:?}");
        assert_eq!(app.active, 4, "consume the invalid key without navigating");
        assert_eq!(app.focus, Focus::List);
    }
}

#[test]
fn key_context_audit_auth_confirmation_should_only_accept_its_own_verbs() {
    for phase in [
        crate::auth::Phase::Confirm,
        crate::auth::Phase::DisconnectConfirm,
    ] {
        let mut app = app(&[]);
        app.auth.phase = phase;
        app.auth.active_id = Some("reader".into());
        app.auth.selected = 1;
        app.overlay = Some(Overlay::Auth);
        for code in [
            KeyCode::Char('j'),
            KeyCode::Char('d'),
            KeyCode::Char('r'),
            KeyCode::Home,
        ] {
            key(&mut app, code, KeyModifiers::NONE);
            assert_eq!(app.auth.phase, phase, "{code:?}");
            assert_eq!(app.auth.selected, 1, "{code:?}");
        }
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.overlay.is_none());
    }
}

#[test]
fn key_context_audit_local_find_reload_should_keep_the_local_results() {
    use crate::app::experience::Screen;
    let (mut app, _) = open_long_post(|_| {});
    let generation = app.post_view.as_ref().unwrap().generation;
    press(&mut app, '/');
    crate::keys::paste(&mut app, "reply");
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    app.post_view
        .as_mut()
        .unwrap()
        .page
        .as_mut()
        .unwrap()
        .comments
        .insert(0, reply("new".into()));
    press(&mut app, 'r');
    assert!(
        app.experience.pending.is_none(),
        "Find must not read a remote thread"
    );
    assert!(
        matches!(app.experience.screens.last(), Some(Screen::Browse { page, .. }) if page.entries.len() == 2)
    );
    assert!(
        matches!(app.experience.screens.last(), Some(Screen::Browse { page, selected, .. })
            if page.entries[*selected].target.as_ref().unwrap().id == "t1_c")
    );
    assert_eq!(app.post_view.as_ref().unwrap().generation, generation);
    assert!(matches!(app.overlay, Some(Overlay::Experience)));
}

#[test]
fn key_context_audit_opened_multi_post_should_regain_content_actions() {
    use crate::app::experience::Screen;
    use crate::keys::map::{self, Action};
    use crate::reddit::experience::{Entry, Page, Query, Request, Target};
    let mut app = app(&[]);
    let post = post_with("selected", Media::None);
    let target = Target::from_post(&post);
    app.overlay = Some(Overlay::Experience);
    app.experience.screens = vec![Screen::Browse {
        query: Query::Multi {
            owner: "reader".into(),
            name: "feed".into(),
        },
        page: Page {
            title: "Feed".into(),
            text: String::new(),
            entries: vec![Entry {
                icon: Default::default(),
                title: "Selected".into(),
                body: String::new(),
                query: None,
                target: Some(target.clone()),
                post: Some(post),
            }],
            after: None,
            url: "/user/reader/m/feed".into(),
        },
        selected: 0,
        scroll: 0,
        pagination: Default::default(),
    }];
    for action in [Action::Upvote, Action::Save, Action::Reply] {
        assert!(
            map::available(&app, action),
            "nonconflicting verb {action:?}"
        );
    }
    assert!(!map::available(&app, Action::Downvote));
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.post_focused());
    for action in [
        Action::Upvote,
        Action::Downvote,
        Action::Save,
        Action::Reply,
    ] {
        assert!(map::available(&app, action), "{action:?}");
    }
    press(&mut app, 'd');
    assert!(
        matches!(&app.experience.pending, Some((_, Request::Read { query: Query::Inspect(actual), .. })) if actual == &target)
    );
}

#[test]
fn key_context_audit_messages_should_offer_message_actions_instead_of_post_verbs() {
    use crate::app::experience::Screen;
    use crate::keys::map::{self, Action};
    use crate::reddit::experience::{Inbox, Query, Target};
    let mut app = description_popup(Query::Inbox(Inbox::All), "", &["Message"]);
    if let Some(Screen::Browse { page, .. }) = app.experience.screens.last_mut() {
        page.entries[0].query = None;
        page.entries[0].target = Some(Target {
            id: "t4_message".into(),
            author: "sender".into(),
            subreddit: String::new(),
            permalink: String::new(),
        });
    }
    for action in [
        Action::Upvote,
        Action::Downvote,
        Action::Save,
        Action::Reply,
    ] {
        assert!(!map::available(&app, action), "{action:?}");
    }
    press_keys(&mut app, " a");
    assert!(
        matches!(app.experience.screens.last(), Some(Screen::Menu { title, .. }) if title == "Message actions")
    );
}
