use super::*;

#[test]
fn a_waiting_prefix_should_wake_and_redraw_once_then_cancel_on_context_change() {
    let mut app = app_with(&["rust", "vim"]);
    let before = Instant::now();
    press(&mut app, crossterm::event::KeyCode::Char(']'));
    let _ = app.tick_at(before + crate::keys::map::HINT_DELAY - Duration::from_millis(1));
    assert!(!app.popup_open());
    let due = Instant::now() + crate::keys::map::HINT_DELAY;
    assert!(app.keymap.wake_in(due).is_some_and(|wait| wait.is_zero()));
    assert!(app.tick_at(due));
    assert!(app.popup_open());
    assert!(app.keymap.wake_in(due).is_none());

    app.switch_tab(0);
    let _ = app.tick_at(due + Duration::from_secs(1));
    assert!(!app.popup_open());
    assert!(app.keymap.prefix.is_empty());
}
