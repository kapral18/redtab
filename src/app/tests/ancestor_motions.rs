use super::*;

#[test]
fn ancestor_motions_should_jump_root_threads_without_changing_local_navigation() {
    let mut app = app_with(&[]);
    app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
    let generation = app.post_view.as_ref().unwrap().generation;
    app.on_msg(thread_message(
        generation,
        vec![
            comment(
                "root",
                vec![comment("child", vec![comment("deep", vec![], None)], None)],
                None,
            ),
            comment("next", vec![], None),
        ],
    ));
    let view = app.post_view.as_mut().unwrap();
    view.free_scroll = false;
    view.cursor = 2;

    for key in "]]".chars() {
        press(&mut app, crossterm::event::KeyCode::Char(key));
    }
    assert_eq!(app.post_view.as_ref().unwrap().cursor, 3);
    for key in "[[".chars() {
        press(&mut app, crossterm::event::KeyCode::Char(key));
    }
    assert_eq!(app.post_view.as_ref().unwrap().cursor, 0);
    press(&mut app, crossterm::event::KeyCode::Char('j'));
    assert_eq!(app.post_view.as_ref().unwrap().cursor, 1);
    for key in "[c".chars() {
        press(&mut app, crossterm::event::KeyCode::Char(key));
    }
    assert_eq!(app.post_view.as_ref().unwrap().cursor, 1);
    press(&mut app, crossterm::event::KeyCode::Char('k'));
    assert_eq!(app.post_view.as_ref().unwrap().cursor, 0);
}

#[test]
fn ancestor_post_motions_should_follow_the_shown_post_and_keep_details_at_boundaries() {
    for width in [80, 200] {
        for source in 0..3 {
            let mut app = app_with(&["a", "b", "c"]);
            app.config.show_nsfw = false;
            let first = post("first", "a");
            let mut hidden = post("hidden", "a");
            hidden.nsfw = true;
            let second = post("second", "c");
            let third = post("third", "c");
            let posts = vec![first.clone(), hidden, second, third];
            match source {
                0 => {
                    app.active = tab_index(&app, "a");
                    app.tabs[app.active].feed.posts = posts;
                    app.tabs[app.active].feed.selected = 2;
                }
                1 => {
                    for post in posts.iter().rev() {
                        app.state.toggle_bookmark(post, 0);
                    }
                    app.active = BOOKMARKS_TAB;
                    app.tabs[app.active].feed.selected = 2;
                }
                _ => {
                    app.active = ALL_SUBS_TAB;
                    app.config.all_subs_posts_per_group = 3;
                    app.all_subs.groups[0].posts = posts[..2].to_vec();
                    app.all_subs.groups[2].posts = posts[2..].to_vec();
                    app.all_subs.group = 2;
                    app.all_subs.post = 1;
                }
            }
            app.resize(width);
            app.open_post_permalink(first.clone(), first.permalink.clone());
            let old = app.post_view.as_ref().unwrap().generation;

            for key in "]p".chars() {
                press(&mut app, crossterm::event::KeyCode::Char(key));
            }

            assert_eq!(app.focus, Focus::Post, "width={width}, source={source}");
            assert_eq!(app.post_view.as_ref().unwrap().post.id, "second");
            app.on_msg(Msg::Post {
                generation: old,
                result: Ok(Box::new(PostPage {
                    post: first,
                    body: "obsolete".into(),
                    comments: vec![],
                })),
            });
            assert_eq!(app.current_post().unwrap().id, "second");
            for key in "]p".chars() {
                press(&mut app, crossterm::event::KeyCode::Char(key));
            }
            assert_eq!(app.post_view.as_ref().unwrap().post.id, "third");
            let view = app.post_view.as_mut().unwrap();
            view.scroll = 31;
            let generation = view.generation;
            for key in "]p".chars() {
                press(&mut app, crossterm::event::KeyCode::Char(key));
            }
            let view = app.post_view.as_ref().unwrap();
            assert_eq!((view.generation, view.scroll), (generation, 31));
            for key in "[p".chars() {
                press(&mut app, crossterm::event::KeyCode::Char(key));
            }
            assert_eq!(app.post_view.as_ref().unwrap().post.id, "second");
            assert_eq!(app.focus, Focus::Post);
            press(&mut app, crossterm::event::KeyCode::Char('q'));
            assert_eq!(app.current_post().unwrap().id, "second");
            assert_eq!(app.focus, Focus::List);
            let detached = post("detached", "a");
            app.open_post_permalink(detached.clone(), detached.permalink);
            let generation = app.post_view.as_ref().unwrap().generation;
            for key in "]p".chars() {
                press(&mut app, crossterm::event::KeyCode::Char(key));
            }
            assert_eq!(app.post_view.as_ref().unwrap().generation, generation);
        }
    }
}

#[test]
fn when_source_feed_keys_are_pressed_on_home_it_should_open_the_selected_subreddit() {
    let mut app = app_with(&[]);
    app.switch_tab(0);
    app.on_msg(Msg::Feed {
        tab: app.active_tab().id,
        generation: app.active_tab().feed.generation,
        append: false,
        result: Ok(listing("armenia", &["a"], None)),
    });

    sequence(&mut app, "gc");

    assert_eq!(app.active_tab().subreddit(), Some("armenia"));
    assert!(matches!(
        &app.active_tab().kind,
        TabKind::Subreddit { pinned: false, .. }
    ));
}

#[test]
fn ancestor_community_motions_should_skip_other_tabs_and_preserve_list_focus() {
    let mut app = app_with(&["a", "c"]);
    app.push_tab(TabKind::User {
        name: "following".into(),
        followed: true,
        icon: None,
    });
    app.push_tab(TabKind::Subreddit {
        name: "b".into(),
        pinned: false,
    });
    app.push_tab(TabKind::User {
        name: "opened".into(),
        followed: false,
        icon: None,
    });
    app.push_tab(TabKind::Subreddit {
        name: "d".into(),
        pinned: false,
    });
    app.active = 0;
    app.focus_main();
    for expected in ["a", "c", "b", "d", "d"] {
        for key in "]s".chars() {
            press(&mut app, crossterm::event::KeyCode::Char(key));
        }
        assert_eq!(app.active_tab().subreddit(), Some(expected));
        assert_eq!(app.focus, Focus::List);
    }
    for expected in ["b", "c", "a", "a"] {
        for key in "[s".chars() {
            press(&mut app, crossterm::event::KeyCode::Char(key));
        }
        assert_eq!(app.active_tab().subreddit(), Some(expected));
    }
    app.switch_tab(ALL_SUBS_TAB);
    app.all_subs.group = 1;
    app.all_subs.groups[1].posts = vec![post("one", "c"), post("two", "c")];
    app.all_subs.post = 1;
    for key in "]s".chars() {
        press(&mut app, crossterm::event::KeyCode::Char(key));
    }
    assert_eq!((app.all_subs.group, app.all_subs.post), (1, 1));
    for key in "[s".chars() {
        press(&mut app, crossterm::event::KeyCode::Char(key));
    }
    assert_eq!((app.all_subs.group, app.all_subs.post), (0, 0));
}
