use super::*;

pub(super) fn input_key(app: &mut App, mut key: KeyEvent) {
    if (key.modifiers == KeyModifiers::ALT && key.code == KeyCode::Backspace)
        || (key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('w'))
    {
        if app.search_editing() {
            app.search_key(key);
            return;
        }
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
    if app.search_editing() {
        app.search_key(key);
    } else {
        dispatch_overlay(app, key);
    }
}

/// Readline's Alt+Backspace: drop the last word and the spaces after it.
pub(super) fn delete_word(input: &mut String) {
    let kept = input
        .trim_end()
        .trim_end_matches(|c: char| !c.is_whitespace())
        .len();
    input.truncate(kept);
}

pub(super) fn backspace(input: &mut String) {
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
    if app.overlay.is_none() && app.search_editing() {
        app.search_paste(text);
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
