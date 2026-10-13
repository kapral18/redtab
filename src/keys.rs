//! Key bindings. Overlays get keys first, then the post view, then lists.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::app::{App, Overlay, PickerState, TabKind};
use crate::model::Sort;

const PAGE: isize = 10;

pub fn handle(app: &mut App, key: KeyEvent) {
    if key.kind == KeyEventKind::Release {
        return;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if ctrl && key.code == KeyCode::Char('c') {
        app.quit = true;
        return;
    }
    if let Some(overlay) = app.overlay.take() {
        app.overlay = handle_overlay(app, overlay, key);
        return;
    }
    if app.post_view.is_some() && handle_post(app, key) {
        return;
    }
    handle_global(app, key);
}

/// Returns the overlay to keep open, or `None` to close it.
fn handle_overlay(app: &mut App, overlay: Overlay, key: KeyEvent) -> Option<Overlay> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    // Esc followed quickly by a key can arrive as Alt+key; never type it.
    let typing = !ctrl && !key.modifiers.contains(KeyModifiers::ALT);
    match overlay {
        Overlay::Help => None,
        Overlay::Picker(mut picker) => {
            let count = app.picker_entries(&picker.input).len();
            match key.code {
                KeyCode::Esc => return None,
                KeyCode::Enter => {
                    app.picker_accept(&picker);
                    return None;
                }
                KeyCode::Up => picker.selected = picker.selected.saturating_sub(1),
                KeyCode::Char('p' | 'k') if ctrl => {
                    picker.selected = picker.selected.saturating_sub(1)
                }
                KeyCode::Down | KeyCode::Tab => {
                    picker.selected = (picker.selected + 1).min(count.saturating_sub(1))
                }
                KeyCode::Char('n' | 'j') if ctrl => {
                    picker.selected = (picker.selected + 1).min(count.saturating_sub(1))
                }
                KeyCode::Backspace => {
                    picker.input.pop();
                    picker.selected = 0;
                }
                KeyCode::Char('u') if ctrl => {
                    picker.input.clear();
                    picker.selected = 0;
                }
                KeyCode::Char(c) if typing => {
                    picker.input.push(c);
                    picker.selected = 0;
                }
                _ => {}
            }
            Some(Overlay::Picker(picker))
        }
        Overlay::Sort { mut selected } => {
            let pick = match key.code {
                KeyCode::Esc | KeyCode::Char('q') => return None,
                KeyCode::Enter => Some(Sort::ALL[selected]),
                KeyCode::Char('j') | KeyCode::Down => {
                    selected = (selected + 1).min(Sort::ALL.len() - 1);
                    None
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    selected = selected.saturating_sub(1);
                    None
                }
                KeyCode::Char('h') => Some(Sort::Hot),
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
                    input.pop();
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
        KeyCode::Esc | KeyCode::Backspace | KeyCode::Left | KeyCode::Char('h' | 'q') => {
            app.close_post()
        }
        KeyCode::Char('j') | KeyCode::Down => app.move_comment_cursor(1),
        KeyCode::Char('k') | KeyCode::Up => app.move_comment_cursor(-1),
        KeyCode::Char('d') if ctrl => app.scroll_post(PAGE * 2),
        KeyCode::Char('u') if ctrl => app.scroll_post(-PAGE * 2),
        KeyCode::Char('e') if ctrl => app.scroll_post(3),
        KeyCode::Char('y') if ctrl => app.scroll_post(-3),
        KeyCode::PageDown | KeyCode::Char(' ') => app.scroll_post(PAGE * 3),
        KeyCode::PageUp => app.scroll_post(-PAGE * 3),
        KeyCode::Char('g') | KeyCode::Home => app.post_top(),
        KeyCode::Char('G') | KeyCode::End => app.post_bottom(),
        KeyCode::Char('n') => app.jump_top_level(true),
        KeyCode::Char('N') => app.jump_top_level(false),
        KeyCode::Char('c') | KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
            app.activate_comment()
        }
        _ => return false,
    }
    true
}

fn handle_global(app: &mut App, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let in_post = app.post_view.is_some();
    let kind = app.active_tab().kind.clone();
    let on_sub = matches!(kind, TabKind::Subreddit { .. });
    match key.code {
        KeyCode::Char('q') if !in_post => app.quit = true,
        KeyCode::Esc if !(on_sub && app.clear_search()) => app.status = None,
        KeyCode::Char('?') => app.overlay = Some(Overlay::Help),
        KeyCode::Char('p') => app.overlay = Some(Overlay::Picker(PickerState::default())),
        KeyCode::Tab | KeyCode::Char(']') => app.cycle_tab(true),
        KeyCode::BackTab | KeyCode::Char('[') => app.cycle_tab(false),
        KeyCode::Char(c @ '1'..='9') => app.switch_tab(c as usize - '1' as usize),
        KeyCode::Char('0') => app.switch_tab(app.tabs.len() - 1),
        KeyCode::Char('r') => app.reload(),
        KeyCode::Char('S') => app.start_redlib(),
        KeyCode::Char('i') => app.toggle_images(),
        KeyCode::Char('o') => app.open_in_browser(),
        KeyCode::Char('y') => app.copy_link(),
        KeyCode::Char('m') => app.open_media(),
        KeyCode::Char('b') => app.toggle_bookmark(),
        KeyCode::Char('\\') => app.toggle_sidebar(),
        _ if in_post => {}
        KeyCode::Char('j') | KeyCode::Down => app.move_selection(1),
        KeyCode::Char('k') | KeyCode::Up => app.move_selection(-1),
        KeyCode::Char('d') if ctrl => app.move_selection(PAGE),
        KeyCode::Char('u') if ctrl => app.move_selection(-PAGE),
        KeyCode::PageDown => app.move_selection(PAGE),
        KeyCode::PageUp => app.move_selection(-PAGE),
        KeyCode::Char('g') | KeyCode::Home => app.select_edge(false),
        KeyCode::Char('G') | KeyCode::End => app.select_edge(true),
        KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => app.open_current_post(),
        KeyCode::Char('J') if kind == TabKind::Home => app.move_home_group(1),
        KeyCode::Char('K') if kind == TabKind::Home => app.move_home_group(-1),
        KeyCode::Char(' ') if kind == TabKind::Home => app.open_home_group_tab(),
        KeyCode::Char('s') if on_sub => {
            app.overlay = Some(Overlay::Sort {
                selected: sort_index(app),
            })
        }
        KeyCode::Char('t') if on_sub => app.cycle_time(),
        KeyCode::Char('/') if on_sub => {
            let input = app.active_tab().feed.search.clone().unwrap_or_default();
            app.overlay = Some(Overlay::Search { input });
        }
        KeyCode::Char('L') if on_sub => {
            let active = app.active;
            app.load_feed(active, true);
        }
        KeyCode::Char('x') => app.close_active_tab(),
        _ => {}
    }
}

fn sort_index(app: &App) -> usize {
    let sort = app.active_tab().feed.sort;
    Sort::ALL.iter().position(|s| *s == sort).unwrap_or(0)
}
