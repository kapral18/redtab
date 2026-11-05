use super::*;

pub(super) fn dispatch(app: &mut App, key: KeyEvent) {
    if key.kind == KeyEventKind::Release {
        return;
    }
    let mut key = legacy(key);
    map::sync(app);
    if key.code == KeyCode::Char('i')
        && key.modifiers == KeyModifiers::CONTROL
        && !map::available(app, map::Action::JumpForward)
    {
        key = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
    }
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
        if context == map::Context::Confirm && matches!(app.overlay, Some(Overlay::Auth)) {
            if key.modifiers.is_empty()
                && matches!(key.code, KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q'))
            {
                dispatch_overlay(app, key);
            }
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
        KeyCode::F(3) if app.overlay.is_none() => app.open_search(None),
        KeyCode::Down => execute(app, map::Action::Down),
        KeyCode::Up => execute(app, map::Action::Up),
        KeyCode::Left => execute(app, map::Action::Left),
        KeyCode::Right if map::available(app, map::Action::Right) => {
            execute(app, map::Action::Right)
        }
        KeyCode::Enter if app.overlay.is_some() => dispatch_overlay(app, key),
        KeyCode::Enter if map::available(app, map::Action::Right) => {
            execute(app, map::Action::Right)
        }
        KeyCode::Home => execute(app, map::Action::First),
        KeyCode::End => execute(app, map::Action::Last),
        KeyCode::PageDown => execute(app, map::Action::PageDown),
        KeyCode::PageUp => execute(app, map::Action::PageUp),
        KeyCode::Tab | KeyCode::BackTab if map::search_kind_keys(app) => {
            app.cycle_search_kind(key.code == KeyCode::Tab)
        }
        KeyCode::Tab if app.overlay.is_none() => app.cycle_tab(true, true),
        KeyCode::BackTab if app.overlay.is_none() => app.cycle_tab(false, true),
        KeyCode::Esc | KeyCode::Backspace => back(app),
        _ => dispatch_overlay(app, key),
    }
}

/// The kitty keyboard protocol reports these chords apart; they keep their terminal meaning.
pub(super) fn legacy(key: KeyEvent) -> KeyEvent {
    let alt = key.modifiers.intersection(KeyModifiers::ALT);
    let ctrl = key.modifiers.difference(KeyModifiers::ALT) == KeyModifiers::CONTROL;
    let code = match key.code {
        KeyCode::Char('h') | KeyCode::Backspace if ctrl => KeyCode::Backspace,
        KeyCode::Char('[') if ctrl => KeyCode::Esc,
        KeyCode::Char('m') if ctrl => KeyCode::Enter,
        _ => return key,
    };
    KeyEvent::new(code, alt)
}

pub(super) fn plain(key: KeyEvent) -> bool {
    if matches!(key.code, KeyCode::Char(_) | KeyCode::BackTab) {
        key.modifiers.difference(KeyModifiers::SHIFT).is_empty()
    } else {
        key.modifiers.is_empty()
    }
}
