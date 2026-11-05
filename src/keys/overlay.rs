use super::*;

pub(super) fn dispatch_overlay(app: &mut App, key: KeyEvent) {
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

pub(super) fn help_key(app: &mut App, key: KeyEvent) {
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

/// Returns the overlay to keep open, or `None` to close it.
pub(super) fn handle_overlay(app: &mut App, overlay: Overlay, key: KeyEvent) -> Option<Overlay> {
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
                (KeyCode::Enter, _)
                    if app
                        .picker_entries(&picker.input)
                        .get(picker.selected)
                        .is_some() =>
                {
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

pub(super) fn back(app: &mut App) {
    if app.overlay.is_some() {
        dispatch_overlay(app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    } else if app.post_focused() {
        app.close_post();
    } else if app.focus == Focus::Sidebar {
        app.focus_main();
    } else if app.active == crate::app::SEARCH_TAB || !app.clear_search() {
        if app.post_view.is_some() {
            app.close_post();
        } else if !app.return_to_search() {
            app.status = None;
        }
    }
}

pub(super) fn leave_collection(app: &mut App) {
    if matches!(app.overlay, Some(Overlay::Experience)) {
        app.clear_experience();
        app.overlay = None;
    }
}
