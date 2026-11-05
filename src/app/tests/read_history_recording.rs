use super::*;

#[test]
fn only_a_revealed_drawn_page_records_a_read_and_requests_one_redraw() {
    let mut app = app_with(&[]);
    app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
    app.post_drawn(true);
    assert!(!app.state.is_read("p"));
    let generation = app.post_view.as_ref().unwrap().generation;
    app.on_msg(thread_message(generation, vec![]));
    app.post_drawn(false);
    assert!(!app.state.is_read("p"));
    app.overlay = Some(Overlay::Auth);
    app.post_drawn(true);
    assert!(!app.state.is_read("p"));
    app.overlay = None;
    app.post_view.as_mut().unwrap().post.spoiler = true;
    app.post_drawn(true);
    assert!(!app.state.is_read("p"));
    app.post_view.as_mut().unwrap().reveal_spoilers = true;

    press(&mut app, crossterm::event::KeyCode::Char(' '));
    app.keymap
        .tick(Instant::now() + crate::keys::map::HINT_DELAY);
    app.post_drawn(true);
    assert!(!app.state.is_read("p"));
    press(&mut app, crossterm::event::KeyCode::Esc);
    press(&mut app, crossterm::event::KeyCode::Char(' '));
    app.post_drawn(true);

    assert!(app.state.is_read("p"));
    assert_eq!(app.wake_in(), Duration::ZERO);
    assert!(app.tick());
    assert!(!app.read_redraw);
    app.post_drawn(true);
    assert!(!app.read_redraw);
}

#[test]
fn rereading_a_bookmark_should_renew_its_read_history_without_a_listing() {
    let mut app = app_with(&[]);
    let post = post("p", "s");
    app.state.mark_read(&post, now() - 89 * 86_400);
    app.state.toggle_bookmark(&post, now() - 89 * 86_400);
    app.open_post_permalink(post, "/r/s/comments/p/x/".into());
    let generation = app.post_view.as_ref().unwrap().generation;
    app.on_msg(thread_message(generation, vec![]));
    app.post_drawn(true);
    let dir = tempfile::tempdir().unwrap();
    app.state
        .save(&dir.path().join("state.json"), now() + 2 * 86_400)
        .unwrap();
    assert!(app.state.is_read("p"));
    assert!(
        !app.read_redraw,
        "renewal does not change visible read status"
    );
}
