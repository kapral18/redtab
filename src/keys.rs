//! Key bindings: overlays first, then the focused pane.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use unicode_segmentation::UnicodeSegmentation;

use crate::app::{App, Focus, Overlay, PickerState, TabKind};
use crate::model::Sort;

pub fn handle(app: &mut App, mut key: KeyEvent) {
    if key.kind == KeyEventKind::Release {
        return;
    }
    // ^H (terminals' Backspace, readline's Ctrl+H) is Backspace; with Alt it stays Alt+Backspace.
    if key.code == KeyCode::Char('h')
        && key.modifiers.difference(KeyModifiers::ALT) == KeyModifiers::CONTROL
    {
        key = KeyEvent::new(
            KeyCode::Backspace,
            key.modifiers.intersection(KeyModifiers::ALT),
        );
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if ctrl && key.code == KeyCode::Char('c') {
        app.quit = true;
        return;
    }
    if let Some(scroll) = &mut app.help_scroll {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => *scroll = scroll.saturating_add(1),
            KeyCode::Char('k') | KeyCode::Up => *scroll = scroll.saturating_sub(1),
            _ => app.help_scroll = None,
        }
        return;
    }
    let typing = matches!(
        app.overlay,
        Some(Overlay::Picker(_) | Overlay::Search { .. })
    ) || (matches!(app.overlay, Some(Overlay::Settings))
        && app.settings.editing.is_some())
        || (matches!(app.overlay, Some(Overlay::Experience))
            && matches!(
                app.experience.screens.last(),
                Some(crate::app::experience::Screen::Input(_))
            ));
    if key.code == KeyCode::F(1)
        || (key.code == KeyCode::Char('?')
            && !key
                .modifiers
                .intersects(KeyModifiers::ALT | KeyModifiers::CONTROL)
            && !typing)
    {
        app.help_scroll = Some(0);
        return;
    }
    if key.code == KeyCode::F(2) && !matches!(app.overlay, Some(Overlay::Settings)) {
        app.show_settings();
        return;
    }
    if app.overlay.is_none() {
        if ctrl && key.code == KeyCode::Char('l') {
            app.show_open_link();
            return;
        }
        if key.code == KeyCode::F(3) {
            app.search_explore();
            return;
        }
    }
    if matches!(app.overlay, Some(Overlay::Settings)) {
        app.settings_key(key);
        return;
    }
    if matches!(app.overlay, Some(Overlay::Experience)) {
        app.experience_key(key);
        return;
    }
    // Esc followed quickly by a key arrives as Alt+key: handle both.
    if key.modifiers.contains(KeyModifiers::ALT)
        && matches!(
            key.code,
            KeyCode::Char(_) | KeyCode::Enter | KeyCode::Tab | KeyCode::Backspace
        )
    {
        // In a text box Alt+letter is a readline motion: ignore it rather than close
        // the box. Alt+Enter submits; Alt+Backspace deletes a word.
        if let Some(Overlay::Picker(PickerState { input, .. }) | Overlay::Search { input }) =
            &mut app.overlay
        {
            match key.code {
                KeyCode::Enter => {
                    handle(app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
                }
                KeyCode::Backspace => {
                    delete_word(input);
                    if let Some(Overlay::Picker(picker)) = &mut app.overlay {
                        picker.selected = 0;
                    }
                }
                _ => {}
            }
            return;
        }
        handle(app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        let mut plain = key;
        plain.modifiers.remove(KeyModifiers::ALT);
        handle(app, plain);
        return;
    }
    if let Some(overlay) = app.overlay.take() {
        app.overlay = handle_overlay(app, overlay, key);
        return;
    }
    // Ctrl+letter runs only Ctrl bindings (Ctrl+X must not close a tab).
    if ctrl && let KeyCode::Char(c) = key.code {
        match (app.post_focused(), c) {
            (true, 'd' | 'u' | 'e' | 'y') => {
                handle_post(app, key);
            }
            (false, 'd' | 'u' | 'e' | 'y') => handle_global(app, key),
            _ => {}
        }
        return;
    }
    if app.focus == Focus::Sidebar && handle_sidebar(app, key) {
        return;
    }
    if app.post_focused() && handle_post(app, key) {
        return;
    }
    handle_global(app, key);
}

/// Keys while the sidebar has focus. Returns true when handled; others act globally.
fn handle_sidebar(app: &mut App, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => app.move_sidebar(1),
        KeyCode::Char('k') | KeyCode::Up => app.move_sidebar(-1),
        KeyCode::Char('g') | KeyCode::Home => app.sidebar_edge(false),
        KeyCode::Char('G') | KeyCode::End => app.sidebar_edge(true),
        KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter | KeyCode::Esc => app.focus_main(),
        // `h` moves focus outward (post, list, sidebar); the sidebar is the outermost.
        KeyCode::Char('h') | KeyCode::Left => {}
        _ => return false,
    }
    true
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
            KeyCode::Esc | KeyCode::Char('q') => None,
            KeyCode::Enter => {
                app.set_membership(target, join);
                None
            }
            KeyCode::Char(' ') => {
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
                KeyCode::Enter => Some(sorts[selected]),
                KeyCode::Char('j') | KeyCode::Down => {
                    selected = (selected + 1).min(sorts.len() - 1);
                    None
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    selected = selected.saturating_sub(1);
                    None
                }
                KeyCode::Char('h') => Some(Sort::Hot),
                KeyCode::Char('b') if sorts.contains(&Sort::Best) => Some(Sort::Best),
                KeyCode::Char('n') => Some(Sort::New),
                KeyCode::Char('t') => Some(Sort::Top),
                KeyCode::Char('r') => Some(Sort::Rising),
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

/// Keys that only mean something in the post view. Returns true when handled.
fn handle_post(app: &mut App, key: KeyEvent) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('h') | KeyCode::Left => {
            if app.detail_fits() {
                app.focus_main();
            } else if !app.leave_comment() {
                app.close_post()
            }
        }
        KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('q') => app.close_post(),
        KeyCode::Char('j') | KeyCode::Down => app.move_comment_cursor(1),
        KeyCode::Char('k') | KeyCode::Up => app.move_comment_cursor(-1),
        KeyCode::Char('d') if ctrl => app.page_post(1, 2),
        KeyCode::Char('u') if ctrl => app.page_post(-1, 2),
        KeyCode::Char('e') if ctrl => app.scroll_post(1),
        KeyCode::Char('y') if ctrl => app.scroll_post(-1),
        KeyCode::PageDown | KeyCode::Char(' ') => app.page_post(1, 1),
        KeyCode::PageUp => app.page_post(-1, 1),
        KeyCode::Char('g') | KeyCode::Home => app.post_top(),
        KeyCode::Char('G') | KeyCode::End => app.post_bottom(),
        KeyCode::Char('n') => app.jump_top_level(true),
        KeyCode::Char('v') => app.toggle_spoilers(),
        KeyCode::Char('N') => app.jump_top_level(false),
        KeyCode::Char('f') => app.comment_tools(),
        KeyCode::Char('{') => app.gallery_step(false),
        KeyCode::Char('}') => app.gallery_step(true),
        KeyCode::Char('P') => app.toggle_video_pause(),
        KeyCode::Char('a') => app.toggle_video_sound(),
        KeyCode::Char(',') => app.seek_video(false),
        KeyCode::Char('.') => app.seek_video(true),
        KeyCode::Char('c') | KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
            app.activate_comment()
        }
        _ => return false,
    }
    true
}

fn handle_global(app: &mut App, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let in_post = app.post_focused();
    let all_subs = matches!(app.active_tab().kind, TabKind::AllSubs);
    let on_sub = matches!(app.active_tab().kind, TabKind::Subreddit { .. });
    let on_feed = on_sub || matches!(app.active_tab().kind, TabKind::Home | TabKind::User { .. });
    match key.code {
        KeyCode::Char('q') if app.post_view.is_some() => app.close_post(),
        KeyCode::Char('q') => app.quit = true,
        KeyCode::Esc if on_sub && app.clear_search() => {}
        KeyCode::Esc if app.post_view.is_some() => app.close_post(),
        KeyCode::Esc => app.status = None,
        KeyCode::Char('A') => app.show_auth(),
        KeyCode::Char(':') => app.reddit_menu(),
        KeyCode::Char(';') => app.show_actions(),
        KeyCode::Char('B') => app.community_menu(),
        KeyCode::Char('p') => app.overlay = Some(Overlay::Picker(PickerState::default())),
        KeyCode::Tab | KeyCode::Char(']') => app.cycle_tab(true),
        KeyCode::BackTab | KeyCode::Char('[') => app.cycle_tab(false),
        KeyCode::Char(c @ '1'..='9') => app.switch_tab(c as usize - '1' as usize),
        KeyCode::Char('0') => app.switch_tab(app.tabs.len() - 1),
        KeyCode::Char('r') => app.reload(),
        KeyCode::Char('i') => app.toggle_images(),
        KeyCode::Char('V') => app.toggle_list_view(),
        KeyCode::Char('o') => app.open_in_browser(),
        KeyCode::Char('O') => app.open_source_in_browser(),
        KeyCode::Char('y') if !ctrl => app.copy_link(),
        KeyCode::Char('m') => app.open_media(),
        KeyCode::Char('P') if app.video_controls_available() => app.toggle_video_pause(),
        KeyCode::Char('a') if app.video_controls_available() => app.toggle_video_sound(),
        KeyCode::Char(',') if app.video_controls_available() => app.seek_video(false),
        KeyCode::Char('.') if app.video_controls_available() => app.seek_video(true),
        KeyCode::Char('b') => app.toggle_bookmark(),
        KeyCode::Char('\\') => app.toggle_sidebar(),
        KeyCode::Char('S') => app.show_membership(),
        KeyCode::Char('/') if !on_sub => app.search_explore(),
        _ if in_post => {}
        KeyCode::Char('j') | KeyCode::Down => app.move_selection(1),
        KeyCode::Char('k') | KeyCode::Up => app.move_selection(-1),
        KeyCode::Char('d') if ctrl => app.page_list(1, 2),
        KeyCode::Char('u') if ctrl => app.page_list(-1, 2),
        KeyCode::Char('e') if ctrl => app.scroll_card(1),
        KeyCode::Char('y') if ctrl => app.scroll_card(-1),
        KeyCode::PageDown => app.page_list(1, 1),
        KeyCode::PageUp => app.page_list(-1, 1),
        KeyCode::Char('g') | KeyCode::Home => app.select_edge(false),
        KeyCode::Char('G') | KeyCode::End => app.select_edge(true),
        KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => app.open_current_post(),
        KeyCode::Char('h') | KeyCode::Left => app.focus_sidebar(),
        KeyCode::Char('J') if all_subs => app.move_all_subs_group(1),
        KeyCode::Char('K') if all_subs => app.move_all_subs_group(-1),
        KeyCode::Char(' ') => app.open_current_community(),
        KeyCode::Char('s') if on_feed => {
            app.overlay = Some(Overlay::Sort {
                selected: sort_index(app),
            })
        }
        KeyCode::Char('t') if on_feed => app.cycle_time(),
        KeyCode::Char('/') if on_sub => {
            let input = app.active_tab().feed.search.clone().unwrap_or_default();
            app.overlay = Some(Overlay::Search { input });
        }
        KeyCode::Char('L') if on_feed => {
            let active = app.active;
            app.load_feed(active, true);
        }
        KeyCode::Char('x') => app.close_active_tab(),
        _ => {}
    }
}

fn sort_index(app: &App) -> usize {
    let sort = app.active_tab().feed.sort;
    app.active_tab()
        .sorts()
        .iter()
        .position(|s| *s == sort)
        .unwrap_or(0)
}
