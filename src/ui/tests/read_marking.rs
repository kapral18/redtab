use super::*;

#[test]
fn an_empty_detail_area_does_not_mark_its_loaded_page_read() {
    let (mut app, _) = open_long_post(|app| app.overlay = Some(Overlay::Auth));
    app.overlay = None;
    screen(&mut app, 1, 1);
    assert!(!app.state.is_read("p"));

    screen(&mut app, 100, 30);

    assert!(app.state.is_read("p"));
}

#[test]
fn loading_obscured_and_spoiler_hidden_documents_do_not_mark_posts_read() {
    let (mut app, _) = open_long_post(|app| {
        assert!(!app.state.is_read("p"));
        app.overlay = Some(Overlay::Auth);
    });
    assert!(!app.state.is_read("p"));
    app.overlay = None;
    app.post_view
        .as_mut()
        .unwrap()
        .page
        .as_mut()
        .unwrap()
        .post
        .spoiler = true;
    screen(&mut app, 100, 30);
    assert!(!app.state.is_read("p"));

    app.post_view.as_mut().unwrap().reveal_spoilers = true;
    screen(&mut app, 100, 30);

    assert!(app.state.is_read("p"));
}
