use super::*;

#[test]
fn when_the_list_has_retained_detail_q_should_close_it_before_quitting() {
    let (mut app, _) = open_long_post(|_| {});
    screen(&mut app, 180, 30);
    press(&mut app, 'h');

    press(&mut app, 'q');

    assert!(!app.quit, "the first q only closes the retained post");
    assert!(app.post_view.is_none());
    press(&mut app, 'q');
    assert!(app.quit, "q quits once no post remains open");
}

#[test]
fn when_the_list_becomes_empty_l_should_resume_the_retained_detail() {
    let (mut app, _) = open_long_post(|_| {});
    screen(&mut app, 180, 30);
    press(&mut app, 'h');
    app.all_subs.groups[0].posts.clear();
    screen(&mut app, 180, 30);

    press(&mut app, 'l');
    key(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
    screen(&mut app, 180, 30);

    let view = app
        .post_view
        .as_ref()
        .expect("still reading the opened post");
    assert!(
        view.scroll > 0,
        "Space pages detail instead of opening a tab"
    );
}

#[test]
fn when_a_list_search_is_cleared_escape_should_keep_the_retained_detail() {
    use crossterm::event::{KeyCode, KeyModifiers};
    let mut app = feed_app(Picker::halfblocks());
    app.open_current_post();
    screen(&mut app, 180, 30);
    press(&mut app, 'h');
    app.search("query".into());
    let expected_generation = app.post_view.as_ref().unwrap().generation;

    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);

    assert!(app.active_tab().feed.search.is_none());
    assert_eq!(
        app.post_view.as_ref().unwrap().generation,
        expected_generation
    );
}
