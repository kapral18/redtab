use super::*;

#[test]
fn when_bookmarks_reorder_before_the_viewport_its_anchor_should_follow_the_post() {
    let mut app = two_tall_media_cards();
    app.state
        .toggle_bookmark(&app.tabs[4].feed.posts[1].clone(), 1);
    app.state
        .toggle_bookmark(&app.tabs[4].feed.posts[0].clone(), 2);
    app.switch_tab(1);
    screen(&mut app, 100, 20);
    app.scroll_card(8);
    screen(&mut app, 100, 20);
    let selected = app.current_post().unwrap().id;

    app.state
        .toggle_bookmark(&post_with("inserted", Media::None), 3);
    // The state merge re-selects the bookmark by id.
    let index = app
        .visible_bookmarks()
        .position(|bookmark| bookmark.post.id == selected)
        .unwrap();
    app.tabs[1].feed.selected = index;
    screen(&mut app, 100, 20);

    assert_eq!(app.active_tab().feed.cards.post.as_deref(), Some("p1"));
    assert_eq!(app.active_tab().feed.offset, 1);
    assert_eq!(app.active_tab().feed.cards.line, 8);
}

#[test]
fn when_a_refresh_removes_the_viewport_anchor_it_should_reveal_a_surviving_post() {
    let mut app = two_tall_media_cards();
    app.scroll_card(82);
    screen(&mut app, 100, 20);
    let posts = vec![app.tabs[4].feed.posts[0].clone()];
    app.reload();

    app.on_msg(Msg::Feed {
        tab: app.active_tab().id,
        generation: app.active_tab().feed.generation,
        append: false,
        result: Ok(crate::model::Listing { posts, after: None }),
    });
    screen(&mut app, 100, 20);

    assert_eq!(app.active_tab().feed.cards.post.as_deref(), Some("p1"));
    assert_eq!(app.active_tab().feed.cards.line, 0);
    assert_eq!(app.current_post().unwrap().id, "p1");
}

#[test]
fn when_the_scrolled_bookmark_is_removed_it_should_reveal_a_surviving_post() {
    let mut app = two_tall_media_cards();
    let second = app.tabs[4].feed.posts[1].clone();
    app.state.toggle_bookmark(&second, 1);
    app.state
        .toggle_bookmark(&app.tabs[4].feed.posts[0].clone(), 2);
    app.switch_tab(1);
    screen(&mut app, 100, 20);
    app.scroll_card(82);
    screen(&mut app, 100, 20);

    app.state.toggle_bookmark(&second, 3);
    screen(&mut app, 100, 20);

    assert_eq!(app.active_tab().feed.cards.post.as_deref(), Some("p1"));
    assert_eq!(app.active_tab().feed.cards.line, 0);
}
