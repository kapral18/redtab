use super::*;

pub(super) fn execute(app: &mut App, action: map::Action) {
    use map::{Action::*, Context};
    let context = map::context(app);
    match action {
        Down | Up | Left | Right | First | Last | PageDown | PageUp | HalfDown | HalfUp
        | LineDown | LineUp => navigate(app, action),
        Close => {
            if app.overlay.is_some() {
                dispatch_overlay(app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
            } else if app.post_view.is_some() {
                app.close_post();
            } else if !app.return_to_search() {
                app.quit = true;
            }
        }
        NextTab | PreviousTab => {
            let preserve_depth = matches!(context, Context::List | Context::Post);
            leave_collection(app);
            app.cycle_tab(action == NextTab, preserve_depth);
        }
        FirstTab | LastTab => {
            leave_collection(app);
            app.switch_tab(if action == FirstTab {
                0
            } else {
                app.tabs.len() - 1
            });
        }
        CloseTab => {
            leave_collection(app);
            app.close_active_tab();
        }
        JumpBack | JumpForward => app.jump(action == JumpForward),
        Home | Bookmarks | AllSubs => {
            leave_collection(app);
            app.close_post();
            app.focus_main();
            app.switch_tab(match action {
                Home => 0,
                Bookmarks => 1,
                _ => 2,
            });
        }
        Popular | All => {
            leave_collection(app);
            app.close_post();
            app.focus_main();
            app.open_subreddit(if action == Popular { "popular" } else { "all" });
        }
        Jump => {
            leave_collection(app);
            app.overlay = Some(Overlay::Picker(PickerState::default()));
        }
        CommunityFeed => app.open_current_community(),
        Profile => app.show_profile_input(),
        NextCommunity | PreviousCommunity => app.move_community(action == NextCommunity),
        NextPost | PreviousPost => app.move_post(action == NextPost),
        NextThread | PreviousThread => app.jump_top_level(action == NextThread),
        NextImage | PreviousImage => app.gallery_step(action == NextImage),
        Fold => app.activate_comment(),
        FocusLeft => {
            if app.post_focused() {
                app.focus_main();
            } else if app.focus == Focus::List
                && app.search_feed_open()
                && app.post_view.is_none()
                && app.detail_fits()
            {
                app.return_to_search();
            } else {
                app.focus_sidebar();
            }
        }
        FocusRight => focus_right(app),
        Actions => app.show_actions(),
        Upvote => app.quick_action("u"),
        Downvote => app.quick_action("d"),
        Save => app.quick_action("s"),
        Reply => app.quick_action("c"),
        Community => app.community_menu(),
        Reddit => app.reddit_menu(),
        Membership => app.show_membership(),
        Bookmark => app.toggle_bookmark(),
        Search => {
            if context == Context::Post {
                app.find_in_comments();
            } else if app.overlay.is_none() && app.active == crate::app::SEARCH_TAB {
                app.edit_search();
            } else if app.overlay.is_none()
                && matches!(app.active_tab().kind, TabKind::Subreddit { .. })
            {
                let input = app.active_tab().feed.search.clone().unwrap_or_default();
                app.overlay = Some(Overlay::Search { input });
            } else {
                app.open_search(None);
            }
        }
        SearchPosts | SearchCommunities | SearchPeople => app.open_search(Some(match action {
            SearchPosts => crate::reddit::experience::SearchKind::Posts,
            SearchCommunities => crate::reddit::experience::SearchKind::Communities,
            _ => crate::reddit::experience::SearchKind::People,
        })),
        Explore => app.open_search(None),
        Sort if context == Context::Post => app.comment_tools(),
        Sort if app.overlay.is_none() && app.active_tab().feed.search.is_some() => {
            app.cycle_search_sort()
        }
        Reload if context == Context::Browse => app.collection_action(action),
        Sort => {
            let selected = app
                .active_tab()
                .sorts()
                .iter()
                .position(|sort| *sort == app.active_tab().feed.sort)
                .unwrap_or(0);
            app.overlay = Some(Overlay::Sort { selected });
        }
        Time => app.cycle_time(),
        Reload => app.reload(),
        OpenUrl => app.show_open_link(),
        OpenLinks | CopyLinks => app.show_body_links(action == CopyLinks),
        Browser if context == Context::Browse => app.open_experience_item(),
        Browser => app.open_in_browser(),
        SourceBrowser => app.open_source_in_browser(),
        CopyLink if context == Context::Browse => app.copy_experience_link(),
        CopyLink => app.copy_link(),
        Media => app.open_media(),
        Pause => app.toggle_video_pause(),
        Mute => app.toggle_video_sound(),
        SeekBack | SeekForward => app.seek_video(action == SeekForward),
        Images => app.toggle_images(),
        ListView => app.toggle_list_view(),
        Sidebar => app.toggle_sidebar(),
        Spoilers => app.toggle_spoilers(),
        Settings => app.show_settings(),
        Accounts => app.show_auth(),
    }
}

pub(super) fn focus_right(app: &mut App) {
    if app.focus == Focus::Sidebar {
        if app.search_feed_open() && app.post_view.is_none() && app.detail_fits() {
            app.return_to_search();
        } else {
            app.focus_main();
        }
    } else if app.post_view.is_some() {
        app.focus = Focus::Post;
    } else if app.search_shows_rows() && app.detail_fits() {
        app.open_search_row();
    }
}

pub(super) fn navigate(app: &mut App, action: map::Action) {
    use map::{Action::*, Context};
    let context = map::context(app);
    let down = matches!(action, Down | PageDown | HalfDown | LineDown);
    if context == Context::Browse {
        let code = match action {
            Down => KeyCode::Down,
            Up => KeyCode::Up,
            First => KeyCode::Home,
            Last => KeyCode::End,
            Right => KeyCode::Enter,
            Left => KeyCode::Esc,
            PageDown => KeyCode::PageDown,
            PageUp => KeyCode::PageUp,
            HalfDown | HalfUp | LineDown | LineUp => {
                let c = match action {
                    HalfDown => 'd',
                    HalfUp => 'u',
                    LineDown => 'e',
                    _ => 'y',
                };
                app.experience_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
                return;
            }
            _ => return,
        };
        app.experience_key(KeyEvent::new(code, KeyModifiers::NONE));
        return;
    }
    if app.overlay.is_some() {
        let count = match action {
            PageDown | PageUp => 10,
            HalfDown | HalfUp => 5,
            _ => 1,
        };
        let code = match action {
            Down | Up | PageDown | PageUp | HalfDown | HalfUp | LineDown | LineUp => {
                if down {
                    KeyCode::Down
                } else {
                    KeyCode::Up
                }
            }
            First => KeyCode::Home,
            Last => KeyCode::End,
            Right => KeyCode::Enter,
            Left => KeyCode::Esc,
            _ => return,
        };
        for _ in 0..count {
            let key = KeyEvent::new(code, KeyModifiers::NONE);
            if matches!(app.overlay, Some(Overlay::Experience)) {
                app.experience_key(key);
            } else {
                dispatch_overlay(app, key);
            }
        }
        return;
    }
    match action {
        Down | Up if context == Context::Sidebar => app.move_sidebar(if down { 1 } else { -1 }),
        Down | Up if context == Context::Post => app.move_comment_cursor(if down { 1 } else { -1 }),
        Down | Up => app.move_selection(if down { 1 } else { -1 }),
        Left => {
            if context == Context::Post {
                if app.detail_fits() {
                    app.focus_main();
                } else if !app.leave_comment() {
                    app.close_post();
                }
            } else if context == Context::List && app.search_feed_open() && app.post_view.is_some()
            {
                app.close_post();
                app.focus_main();
            } else if context != Context::Sidebar && !app.return_to_search() {
                app.focus_sidebar();
            }
        }
        Right if context == Context::Sidebar => focus_right(app),
        Right if context == Context::Post => app.activate_comment(),
        Right if app.search_shows_rows() => app.open_search_row(),
        Right => app.open_current_post(),
        First | Last if context == Context::Sidebar => app.sidebar_edge(action == Last),
        First | Last if context == Context::Post => {
            if action == First {
                app.post_top();
            } else {
                app.post_bottom();
            }
        }
        First | Last => app.select_edge(action == Last),
        PageDown | PageUp | HalfDown | HalfUp if context == Context::Sidebar => {
            let rows = if matches!(action, HalfDown | HalfUp) {
                5
            } else {
                10
            };
            app.move_sidebar(if down {
                rows as isize
            } else {
                -(rows as isize)
            });
        }
        PageDown | PageUp | HalfDown | HalfUp => {
            let divisor = if matches!(action, HalfDown | HalfUp) {
                2
            } else {
                1
            };
            if context == Context::Post {
                app.page_post(if down { 1 } else { -1 }, divisor);
            } else {
                app.page_list(if down { 1 } else { -1 }, divisor as usize);
            }
        }
        LineDown | LineUp if context == Context::Sidebar => {
            app.move_sidebar(if down { 1 } else { -1 })
        }
        LineDown | LineUp if context == Context::Post => app.scroll_post(if down { 1 } else { -1 }),
        LineDown | LineUp => app.scroll_card(if down { 1 } else { -1 }),
        _ => {}
    }
}
