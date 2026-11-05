use super::*;

#[test]
fn when_a_visible_append_loader_is_due_tick_should_advance_its_rendered_frame() {
    let mut app = app_with_pending_pagination();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 20)).unwrap();
    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    assert!(
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .any(|cell| cell.symbol() == "⠋")
    );
    app.loading_since = Some(Instant::now() - Duration::from_millis(100));

    let changed = app.tick();

    assert!(changed);
    assert_eq!(app.loading_frame, 1);
    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    assert!(
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .any(|cell| cell.symbol() == "⠙")
    );
}

#[test]
fn when_the_page_finishes_tick_should_stop_the_append_loader_before_the_next_draw() {
    let mut app = app_with_pending_pagination();
    app.loading_since = Some(Instant::now() - Duration::from_millis(100));
    app.on_msg(Msg::Feed {
        tab: app.tabs[4].id,
        generation: app.tabs[4].feed.generation,
        append: true,
        result: Ok(listing("s", &["next"], None)),
    });

    app.tick();

    assert_eq!(app.loading_frame, 0);
}

#[test]
fn when_the_page_fails_tick_should_stop_the_append_loader_before_the_next_draw() {
    let mut app = app_with_pending_pagination();
    app.loading_since = Some(Instant::now() - Duration::from_millis(100));
    app.on_msg(Msg::Feed {
        tab: app.tabs[4].id,
        generation: app.tabs[4].feed.generation,
        append: true,
        result: Err("offline".into()),
    });

    app.tick();

    assert_eq!(app.loading_frame, 0);
}

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

#[test]
fn when_a_popup_is_open_tick_should_pause_the_append_loader() {
    let mut app = app_with_pending_pagination();
    app.loading_since = Some(Instant::now() - Duration::from_millis(100));
    app.overlay = Some(Overlay::Picker(PickerState::default()));

    app.tick();

    assert_eq!(app.loading_frame, 0);
}

#[test]
fn when_the_append_loader_is_not_due_tick_should_not_advance_it_or_spin() {
    let mut app = app_with_pending_pagination();
    app.loading_drawn(true);

    app.tick();

    assert_eq!(app.loading_frame, 0);
    assert!(!app.wake_in().is_zero());
}

#[test]
fn when_the_append_loader_leaves_the_viewport_tick_should_stop_advancing_it() {
    let mut app = app_with_pending_pagination();
    app.loading_since = Some(Instant::now() - Duration::from_millis(100));
    app.loading_drawn(false);

    app.tick();

    assert_eq!(app.loading_frame, 0);
    assert!(app.loading_since.is_none());
}
