//! Key bindings: overlays first, then the focused pane.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use unicode_segmentation::UnicodeSegmentation;

use crate::app::{App, Focus, Overlay, PickerState, TabKind};
use crate::model::Sort;

pub mod map;
pub use map::State;

pub fn handle(app: &mut App, mut key: KeyEvent) {
    if key.kind == KeyEventKind::Release {
        return;
    }
    map::sync(app);
    if app.ancestor_following() {
        if app.keymap.prefix.is_empty()
            && key.modifiers.is_empty()
            && key.code == KeyCode::Char('q')
        {
            app.cancel_ancestor_follow();
            return;
        }
        let plain = key.modifiers.difference(KeyModifiers::SHIFT).is_empty();
        let prefix = app.keymap.prefix.as_str();
        let navigating = plain
            && match key.code {
                KeyCode::Char(c) => {
                    let sequence = format!("{prefix}{c}");
                    map::candidates(app).iter().any(|candidate| {
                        candidate.keys.starts_with(&sequence)
                            && matches!(
                                candidate.target,
                                map::Target::Action(
                                    map::Action::NextCommunity
                                        | map::Action::PreviousCommunity
                                        | map::Action::NextTab
                                        | map::Action::PreviousTab
                                )
                            )
                    })
                }
                KeyCode::Tab | KeyCode::BackTab => prefix.is_empty(),
                _ => false,
            };
        let paging_hint = !prefix.is_empty()
            && (key.modifiers.is_empty()
                && matches!(key.code, KeyCode::PageUp | KeyCode::PageDown)
                || key.modifiers == KeyModifiers::CONTROL
                    && matches!(key.code, KeyCode::Char('f' | 'b' | 'd' | 'u' | 'e' | 'y')));
        let cancelling_prefix =
            !prefix.is_empty() && key.modifiers.is_empty() && key.code == KeyCode::Esc;
        if !navigating && !paging_hint && !cancelling_prefix {
            app.cancel_ancestor_follow();
        }
    }
    if key.code == KeyCode::Char('h')
        && key.modifiers.difference(KeyModifiers::ALT) == KeyModifiers::CONTROL
    {
        key = KeyEvent::new(
            KeyCode::Backspace,
            key.modifiers.intersection(KeyModifiers::ALT),
        );
    }
    let ctrl = key.modifiers == KeyModifiers::CONTROL;
    let alt = key.modifiers == KeyModifiers::ALT;
    if ctrl && key.code == KeyCode::Char('c') {
        app.quit = true;
        return;
    }
    if app.help_scroll.is_some() {
        help_key(app, key);
        return;
    }
    if key.modifiers.is_empty() && key.code == KeyCode::F(1) {
        app.help_scroll = Some(0);
        return;
    }
    let context = map::context(app);
    if key.code == KeyCode::Char('?') && plain(key) && context != map::Context::Input {
        app.help_scroll = Some(0);
        return;
    }
    if key.modifiers.is_empty() && key.code == KeyCode::F(2) {
        app.keymap.clear();
        if matches!(app.overlay, Some(Overlay::Settings)) {
            app.settings_key(key);
        } else {
            app.show_settings();
        }
        return;
    }
    if context == map::Context::Input {
        app.keymap.clear();
        input_key(app, key);
        return;
    }
    if alt
        || key.modifiers.intersects(
            KeyModifiers::ALT | KeyModifiers::SUPER | KeyModifiers::HYPER | KeyModifiers::META,
        )
    {
        app.keymap.clear();
        return;
    }
    if key.code == KeyCode::Esc && key.modifiers.is_empty() && !app.keymap.prefix.is_empty() {
        app.keymap.clear();
        return;
    }
    if !app.keymap.prefix.is_empty() {
        let full = app.keymap.hint_rows.saturating_sub(1).max(1);
        let half = (app.keymap.hint_rows / 2).max(1);
        let movement = match (key.code, key.modifiers) {
            (KeyCode::PageDown, KeyModifiers::NONE)
            | (KeyCode::Char('f'), KeyModifiers::CONTROL) => Some((true, full)),
            (KeyCode::PageUp, KeyModifiers::NONE) | (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
                Some((false, full))
            }
            (KeyCode::Char('d'), KeyModifiers::CONTROL) => Some((true, half)),
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => Some((false, half)),
            (KeyCode::Char('e'), KeyModifiers::CONTROL) => Some((true, 1)),
            (KeyCode::Char('y'), KeyModifiers::CONTROL) => Some((false, 1)),
            _ => None,
        };
        if let Some((down, rows)) = movement {
            app.keymap.scroll = if down {
                app.keymap.scroll.saturating_add(rows)
            } else {
                app.keymap.scroll.saturating_sub(rows)
            };
            return;
        }
    }
    if ctrl && key.code == KeyCode::Char('o') && matches!(app.overlay, Some(Overlay::Experience)) {
        app.keymap.clear();
        app.experience_key(key);
        return;
    }
    if matches!(context, map::Context::Pending | map::Context::Confirm) {
        if key.modifiers.is_empty() && key.code == KeyCode::Char('h') {
            dispatch_overlay(app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
            return;
        }
        if key.modifiers.is_empty()
            && key.code == KeyCode::Char('d')
            && matches!(app.overlay, Some(Overlay::Auth))
            && app.auth.can_remove()
        {
            dispatch_overlay(app, key);
            return;
        }
        if key.modifiers.is_empty()
            && matches!(
                key.code,
                KeyCode::Esc
                    | KeyCode::Enter
                    | KeyCode::Char('q' | 'r' | 'y' | 'n' | 'o')
                    | KeyCode::F(2)
                    | KeyCode::Up
                    | KeyCode::Down
                    | KeyCode::PageUp
                    | KeyCode::PageDown
                    | KeyCode::Home
                    | KeyCode::End
                    | KeyCode::Char('j' | 'k')
            )
        {
            dispatch_overlay(app, key);
        }
        return;
    }
    let had_prefix = !app.keymap.prefix.is_empty();
    if let Some(target) = map::resolve(app, key) {
        match target {
            map::Target::Action(action) => execute(app, action),
            map::Target::Menu(index) => {
                if let Some(crate::app::experience::Screen::Menu { selected, .. }) =
                    app.experience.screens.last_mut()
                {
                    *selected = index;
                }
                app.experience_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
            }
        }
        return;
    }
    if had_prefix || !app.keymap.prefix.is_empty() {
        return;
    }
    if ctrl {
        return;
    }
    if !plain(key) {
        return;
    }
    match key.code {
        KeyCode::F(3) if app.overlay.is_none() => app.search_explore(),
        KeyCode::Down => execute(app, map::Action::Down),
        KeyCode::Up => execute(app, map::Action::Up),
        KeyCode::Left => execute(app, map::Action::Left),
        KeyCode::Right if map::available(app, map::Action::Right) => {
            execute(app, map::Action::Right)
        }
        KeyCode::Enter if app.overlay.is_some() => dispatch_overlay(app, key),
        KeyCode::Enter => execute(app, map::Action::Right),
        KeyCode::Home => execute(app, map::Action::First),
        KeyCode::End => execute(app, map::Action::Last),
        KeyCode::PageDown => execute(app, map::Action::PageDown),
        KeyCode::PageUp => execute(app, map::Action::PageUp),
        KeyCode::Tab if app.overlay.is_none() => app.cycle_tab(true, true),
        KeyCode::BackTab if app.overlay.is_none() => app.cycle_tab(false, true),
        KeyCode::Esc | KeyCode::Backspace => back(app),
        _ => dispatch_overlay(app, key),
    }
}

fn plain(key: KeyEvent) -> bool {
    if matches!(key.code, KeyCode::Char(_) | KeyCode::BackTab) {
        key.modifiers.difference(KeyModifiers::SHIFT).is_empty()
    } else {
        key.modifiers.is_empty()
    }
}

fn input_key(app: &mut App, mut key: KeyEvent) {
    if (key.modifiers == KeyModifiers::ALT && key.code == KeyCode::Backspace)
        || (key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('w'))
    {
        match &mut app.overlay {
            Some(Overlay::Picker(picker)) => {
                delete_word(&mut picker.input);
                picker.selected = 0;
            }
            Some(Overlay::Search { input }) => delete_word(input),
            Some(Overlay::Settings) => {
                if let Some(field) = &mut app.settings.editing {
                    field.delete_word();
                }
            }
            Some(Overlay::Experience) => {
                if let Some(form) = app.experience.input() {
                    form.fields[form.active].delete_word();
                }
            }
            _ => {}
        }
        return;
    }
    if key.modifiers == KeyModifiers::ALT {
        if !matches!(key.code, KeyCode::Backspace | KeyCode::Enter) {
            return;
        }
        if key.code == KeyCode::Enter && !matches!(app.overlay, Some(Overlay::Experience)) {
            key.modifiers = KeyModifiers::NONE;
        }
    } else if key.modifiers == KeyModifiers::CONTROL {
        if matches!(app.overlay, Some(Overlay::Settings))
            && matches!(key.code, KeyCode::Char('a' | 'e'))
        {
            key = KeyEvent::new(
                if key.code == KeyCode::Char('a') {
                    KeyCode::Home
                } else {
                    KeyCode::End
                },
                KeyModifiers::NONE,
            );
            dispatch_overlay(app, key);
            return;
        }
        if !matches!(
            key.code,
            KeyCode::Char('a' | 'e' | 'u' | 's' | 'o' | 'j' | 'k' | 'n' | 'p')
        ) {
            return;
        }
    } else if !plain(key) {
        return;
    }
    dispatch_overlay(app, key);
}

fn dispatch_overlay(app: &mut App, key: KeyEvent) {
    match app.overlay {
        Some(Overlay::Settings) => app.settings_key(key),
        Some(Overlay::Experience) => {
            let allowed = !matches!(map::context(app), map::Context::Browse | map::Context::Menu)
                || matches!(key.code, KeyCode::Enter | KeyCode::Esc)
                || (matches!(map::context(app), map::Context::Browse)
                    && matches!(key.code, KeyCode::Char('a' | 'd' | 'D')));
            if allowed {
                app.experience_key(key);
            }
        }
        Some(_) => {
            if let Some(overlay) = app.overlay.take() {
                app.overlay = handle_overlay(app, overlay, key);
            }
        }
        None => {}
    }
}

fn help_key(app: &mut App, key: KeyEvent) {
    if plain(key) && key.code == KeyCode::Char('g') {
        if app.keymap.help_g {
            app.help_scroll = Some(0);
        }
        app.keymap.help_g = !app.keymap.help_g;
        return;
    }
    app.keymap.help_g = false;
    let page = app.keymap.help_rows.saturating_sub(1).max(1);
    let half = (app.keymap.help_rows / 2).max(1);
    let scroll = app.help_scroll.as_mut().unwrap();
    match (key.code, key.modifiers) {
        (KeyCode::Down | KeyCode::Char('j'), KeyModifiers::NONE) => {
            *scroll = scroll.saturating_add(1)
        }
        (KeyCode::Up | KeyCode::Char('k'), KeyModifiers::NONE) => {
            *scroll = scroll.saturating_sub(1)
        }
        (KeyCode::PageDown, KeyModifiers::NONE) | (KeyCode::Char('f'), KeyModifiers::CONTROL) => {
            *scroll = scroll.saturating_add(page)
        }
        (KeyCode::PageUp, KeyModifiers::NONE) | (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
            *scroll = scroll.saturating_sub(page)
        }
        (KeyCode::Char('d'), KeyModifiers::CONTROL) => *scroll = scroll.saturating_add(half),
        (KeyCode::Char('u'), KeyModifiers::CONTROL) => *scroll = scroll.saturating_sub(half),
        (KeyCode::Char('e'), KeyModifiers::CONTROL) => *scroll = scroll.saturating_add(1),
        (KeyCode::Char('y'), KeyModifiers::CONTROL) => *scroll = scroll.saturating_sub(1),
        (KeyCode::Home, KeyModifiers::NONE) => *scroll = 0,
        (KeyCode::End | KeyCode::Char('G'), _) if plain(key) => *scroll = usize::MAX,
        _ if plain(key) => app.help_scroll = None,
        _ => {}
    }
}

/// Readline's Alt+Backspace: drop the last word and the spaces after it.
fn delete_word(input: &mut String) {
    let kept = input
        .trim_end()
        .trim_end_matches(|c: char| !c.is_whitespace())
        .len();
    input.truncate(kept);
}

fn backspace(input: &mut String) {
    if let Some((start, _)) = input.grapheme_indices(true).next_back() {
        input.truncate(start);
    }
}

/// Paste goes only into the picker or search input, so pasted text never runs
/// commands. In a search, line breaks and tabs become spaces.
pub fn paste(app: &mut App, text: &str) {
    app.cancel_ancestor_follow();
    app.keymap.clear();
    if app.help_scroll.is_some() {
        return;
    }
    if matches!(app.overlay, Some(Overlay::Settings)) {
        app.settings_paste(text);
        return;
    }
    if matches!(app.overlay, Some(Overlay::Experience)) {
        app.experience_paste(text);
        return;
    }
    match &mut app.overlay {
        Some(Overlay::Picker(picker)) => {
            picker
                .input
                .extend(text.chars().filter(|c| !c.is_control()));
            picker.selected = 0;
        }
        Some(Overlay::Search { input }) => {
            for c in text.chars() {
                let c = if c.is_control() { ' ' } else { c };
                if c != ' ' || !input.ends_with(' ') {
                    input.push(c);
                }
            }
        }
        _ => {}
    }
}

/// Returns the overlay to keep open, or `None` to close it.
fn handle_overlay(app: &mut App, overlay: Overlay, key: KeyEvent) -> Option<Overlay> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let typing = !ctrl;
    match overlay {
        Overlay::Settings => Some(Overlay::Settings),
        Overlay::Experience => Some(Overlay::Experience),
        Overlay::Auth => {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q')
                    if !matches!(
                        app.auth.phase,
                        crate::auth::Phase::Saving | crate::auth::Phase::Disconnecting
                    ) =>
                {
                    app.auth.cancel();
                    return None;
                }
                KeyCode::Down | KeyCode::Char('j') if !app.auth.busy() => {
                    app.auth.selected =
                        (app.auth.selected + 1).min(app.auth.choices().saturating_sub(1))
                }
                KeyCode::Up | KeyCode::Char('k') if !app.auth.busy() => {
                    app.auth.selected = app.auth.selected.saturating_sub(1)
                }
                KeyCode::Home if !app.auth.busy() => app.auth.selected = 0,
                KeyCode::End if !app.auth.busy() => {
                    app.auth.selected = app.auth.choices().saturating_sub(1)
                }
                KeyCode::Enter
                    if matches!(
                        app.auth.phase,
                        crate::auth::Phase::Confirm | crate::auth::Phase::DisconnectConfirm
                    ) =>
                {
                    app.confirm_auth()
                }
                KeyCode::Char('d') if app.auth.can_remove() => app.auth.request_disconnect(),
                KeyCode::Enter if app.select_auth() => return None,
                KeyCode::Char('a') if app.auth.phase == crate::auth::Phase::Accounts => {
                    app.auth.discover()
                }
                KeyCode::Char('r') if !app.auth.busy() => app.auth.reconnect(),
                _ => {}
            }
            Some(Overlay::Auth)
        }
        Overlay::Membership { target, join } => match key.code {
            KeyCode::Esc | KeyCode::Char('q' | 'h') => None,
            KeyCode::Enter => {
                app.set_membership(target, join);
                None
            }
            KeyCode::Char('o') => {
                match target {
                    crate::model::MembershipTarget::Community(name) => app.open_subreddit(&name),
                    crate::model::MembershipTarget::User(name) => app.open_user(&name),
                }
                None
            }
            _ => Some(Overlay::Membership { target, join }),
        },
        Overlay::Picker(mut picker) => {
            match (key.code, ctrl) {
                (KeyCode::Esc, _) => return None,
                (KeyCode::Enter, _) => {
                    app.picker_accept(&picker);
                    return None;
                }
                (KeyCode::Up, _) | (KeyCode::Char('p' | 'k'), true) => {
                    picker.selected = picker.selected.saturating_sub(1)
                }
                (KeyCode::Down | KeyCode::Tab, _) | (KeyCode::Char('n' | 'j'), true) => {
                    let count = app.picker_entries(&picker.input).len();
                    picker.selected = (picker.selected + 1).min(count.saturating_sub(1));
                }
                (KeyCode::Backspace, _) => {
                    backspace(&mut picker.input);
                    picker.selected = 0;
                }
                (KeyCode::Char('u'), true) => {
                    picker.input.clear();
                    picker.selected = 0;
                }
                (KeyCode::Char(c), false) => {
                    picker.input.push(c);
                    picker.selected = 0;
                }
                _ => {}
            }
            Some(Overlay::Picker(picker))
        }
        Overlay::Sort { mut selected } => {
            let sorts = app.active_tab().sorts();
            let pick = match key.code {
                KeyCode::Esc | KeyCode::Char('q') => return None,
                KeyCode::Home => {
                    selected = 0;
                    None
                }
                KeyCode::End => {
                    selected = sorts.len().saturating_sub(1);
                    None
                }
                KeyCode::Enter => Some(sorts[selected]),
                KeyCode::Char('j') | KeyCode::Down => {
                    selected = (selected + 1).min(sorts.len() - 1);
                    None
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    selected = selected.saturating_sub(1);
                    None
                }
                KeyCode::Char('H') => Some(Sort::Hot),
                KeyCode::Char('b') if sorts.contains(&Sort::Best) => Some(Sort::Best),
                KeyCode::Char('n') => Some(Sort::New),
                KeyCode::Char('t') => Some(Sort::Top),
                KeyCode::Char('r') if sorts.contains(&Sort::Rising) => Some(Sort::Rising),
                KeyCode::Char('c') => Some(Sort::Controversial),
                _ => None,
            };
            match pick {
                Some(sort) => {
                    app.set_sort(sort);
                    None
                }
                None => Some(Overlay::Sort { selected }),
            }
        }
        Overlay::Search { mut input } => {
            match key.code {
                KeyCode::Esc => return None,
                KeyCode::Enter => {
                    app.search(input);
                    return None;
                }
                KeyCode::Backspace => {
                    backspace(&mut input);
                }
                KeyCode::Char('u') if ctrl => input.clear(),
                KeyCode::Char(c) if typing => input.push(c),
                _ => {}
            }
            Some(Overlay::Search { input })
        }
    }
}

fn back(app: &mut App) {
    if app.overlay.is_some() {
        dispatch_overlay(app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    } else if app.post_focused() {
        app.close_post();
    } else if app.focus == Focus::Sidebar {
        app.focus_main();
    } else if !app.clear_search() {
        if app.post_view.is_some() {
            app.close_post();
        } else {
            app.status = None;
        }
    }
}

fn leave_collection(app: &mut App) {
    if matches!(app.overlay, Some(Overlay::Experience)) {
        app.experience = crate::app::experience::Experience::default();
        app.overlay = None;
    }
}

fn execute(app: &mut App, action: map::Action) {
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
            } else {
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
        Home | Bookmarks | AllSubs => {
            leave_collection(app);
            app.focus_main();
            app.switch_tab(match action {
                Home => 0,
                Bookmarks => 1,
                _ => 2,
            });
        }
        Popular | All => {
            leave_collection(app);
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
            } else {
                app.focus_sidebar();
            }
        }
        FocusRight => {
            if app.focus == Focus::Sidebar {
                app.focus_main();
            } else if app.post_view.is_some() {
                app.focus = Focus::Post;
            }
        }
        Actions => app.show_actions(),
        Community => app.community_menu(),
        Reddit => app.reddit_menu(),
        Membership => app.show_membership(),
        Bookmark => app.toggle_bookmark(),
        Search => {
            if context == Context::Post {
                app.find_in_comments();
            } else if app.overlay.is_none()
                && matches!(app.active_tab().kind, TabKind::Subreddit { .. })
            {
                let input = app.active_tab().feed.search.clone().unwrap_or_default();
                app.overlay = Some(Overlay::Search { input });
            } else {
                app.show_search(crate::reddit::experience::SearchKind::Posts);
            }
        }
        SearchPosts | SearchCommunities | SearchPeople => app.show_search(match action {
            SearchPosts => crate::reddit::experience::SearchKind::Posts,
            SearchCommunities => crate::reddit::experience::SearchKind::Communities,
            _ => crate::reddit::experience::SearchKind::People,
        }),
        Explore => app.search_explore(),
        Sort if context == Context::Post => app.comment_tools(),
        Sort | Time | Reload if context == Context::Browse => app.collection_action(action),
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
        OpenLinks => app.show_body_links(),
        Browser if context == Context::Browse => app.experience_native(),
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

fn navigate(app: &mut App, action: map::Action) {
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
            } else if context != Context::Sidebar {
                app.focus_sidebar();
            }
        }
        Right if context == Context::Sidebar => app.focus_main(),
        Right if context == Context::Post => app.activate_comment(),
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

pub fn footer_hints(app: &App) -> Vec<(String, String)> {
    use map::{Action::*, Target};
    if app.help_scroll.is_some() {
        return vec![
            ("j/k".into(), "scroll help".into()),
            ("C-f/b".into(), "page".into()),
            ("Esc".into(), "back".into()),
        ];
    }
    if app.keymap.hint_visible() {
        return vec![
            ("Esc".into(), "cancel prefix".into()),
            ("PgUp/Dn".into(), "scroll keys".into()),
        ];
    }
    let candidates = map::candidates(app);
    let mut result = Vec::new();
    let context = map::context(app);
    let priorities = [
        (
            Pause,
            if app.selected_video().is_some_and(|player| !player.paused()) {
                "pause"
            } else {
                "play"
            },
        ),
        (Mute, "sound"),
        (Fold, map::fold_label(app).unwrap_or("fold")),
        (
            Down,
            if context == map::Context::Sidebar {
                "tabs"
            } else if context == map::Context::Post {
                "comments"
            } else {
                "move"
            },
        ),
        (
            Right,
            if context == map::Context::Sidebar {
                "list"
            } else {
                "open"
            },
        ),
        (NextCommunity, "next community"),
        (NextThread, "next root"),
        (NextPost, "next post"),
        (
            Membership,
            match app
                .membership_target()
                .as_ref()
                .map(|target| (target, app.subscribed(target)))
            {
                Some((crate::model::MembershipTarget::User(_), Some(true))) => "unfollow",
                Some((crate::model::MembershipTarget::User(_), Some(false))) => "follow",
                Some((_, Some(true))) => "leave",
                Some((_, Some(false))) => "join",
                _ => "check membership",
            },
        ),
        (
            Bookmark,
            if app
                .current_post()
                .is_some_and(|post| app.state.is_bookmarked(&post.id))
            {
                "unbookmark"
            } else {
                "bookmark"
            },
        ),
        (Time, "time"),
        (CloseTab, "close tab"),
        (OpenLinks, "links"),
        (Community, "source"),
        (Spoilers, "spoilers"),
        (Actions, "actions"),
        (Sort, "sort"),
        (Media, "media"),
        (CopyLink, "yank"),
        (Jump, "buffers"),
        (ListView, "view"),
        (Left, "back"),
    ];
    for (action, description) in priorities {
        if action == CloseTab
            && !matches!(
                app.active_tab().kind,
                TabKind::Subreddit { pinned: false, .. }
                    | TabKind::User {
                        followed: false,
                        ..
                    }
            )
        {
            continue;
        }
        if candidates
            .iter()
            .any(|candidate| matches!(candidate.target, Target::Action(actual) if actual == action))
        {
            let key = if action == Down {
                format!("{}/{}", map::action_label(Down), map::action_label(Up))
            } else {
                map::action_label(action)
            };
            result.push((key, description.into()));
        }
    }
    if candidates
        .iter()
        .any(|candidate| candidate.keys.starts_with(' '))
    {
        result.push(("SPC".into(), "actions".into()));
    }
    result
}
