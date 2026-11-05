use super::*;

#[test]
fn when_opening_the_current_community_feed_from_a_post_it_should_reveal_the_feed() {
    let (mut app, _dir, _fixture) = app();
    app.open_subreddit("s");
    let expected_tab = app.active_tab().id;
    let mut post = post_page().post;
    post.subreddit = "s".into();
    let mut other = post.clone();
    other.id = "other".into();
    let feed = &mut app.tabs[app.active].feed;
    feed.posts = vec![other, post.clone()];
    feed.selected = 1;
    feed.loaded = true;
    app.resize(80);
    app.show_post(post.clone(), &post.permalink);
    app.focus = Focus::Post;

    sequence(&mut app, " c");
    key(&mut app, KeyCode::Enter);

    assert!(app.overlay.is_none());
    assert!(app.post_view.is_none(), "the post still hides the feed");
    assert_eq!(app.focus, Focus::List);
    assert_eq!(app.active_tab().id, expected_tab);
    assert_eq!(app.tabs.len(), 5);
    assert_eq!(app.active_tab().feed.selected, 1);
    assert!(app.experience.screens.is_empty());
}

#[test]
fn when_opening_a_community_feed_from_the_sidebar_it_should_focus_the_feed() {
    let (mut app, _dir, _fixture) = app();
    app.open_subreddit("s");
    app.show_sidebar = false;
    app.focus_sidebar();

    sequence(&mut app, " c");
    key(&mut app, KeyCode::Enter);

    assert_eq!(app.focus, Focus::List);
    assert!(!app.sidebar_peek);
    assert!(!app.show_sidebar);
    assert!(app.overlay.is_none());
}

#[test]
fn when_opening_a_posts_existing_community_feed_it_should_reuse_its_tab() {
    let (mut app, _dir, _fixture) = app();
    app.open_subreddit("s");
    let expected_tab = app.active_tab().id;
    app.switch_tab(0);
    let mut post = post_page().post;
    post.subreddit = "s".into();
    app.show_post(post.clone(), &post.permalink);
    app.focus = Focus::Post;

    sequence(&mut app, " c");
    key(&mut app, KeyCode::Enter);

    assert_eq!(app.active_tab().id, expected_tab);
    assert_eq!(app.tabs.len(), 5);
    assert!(app.post_view.is_none());
    assert_eq!(app.focus, Focus::List);
    assert!(app.overlay.is_none());
}

#[test]
fn when_opening_a_posts_new_community_feed_it_should_open_its_tab() {
    let (mut app, _dir, _fixture) = app();
    let mut post = post_page().post;
    post.subreddit = "new".into();
    app.show_post(post.clone(), &post.permalink);
    app.focus = Focus::Post;

    sequence(&mut app, " c");
    key(&mut app, KeyCode::Enter);

    assert_eq!(app.active_tab().subreddit(), Some("new"));
    assert_eq!(app.tabs.len(), 5);
    assert!(app.post_view.is_none());
    assert_eq!(app.focus, Focus::List);
    assert!(app.overlay.is_none());
}

#[test]
fn when_opening_the_current_user_feed_it_should_reveal_and_focus_the_feed() {
    let (mut app, _dir, _fixture) = app();
    app.open_user("writer");
    let expected_tab = app.active_tab().id;
    let post = post_page().post;
    app.show_post(post.clone(), &post.permalink);
    app.focus = Focus::Sidebar;

    sequence(&mut app, " c");
    key(&mut app, KeyCode::Char('j'));
    key(&mut app, KeyCode::Enter);

    assert_eq!(app.active_tab().id, expected_tab);
    assert_eq!(app.tabs.len(), 5);
    assert!(app.post_view.is_none());
    assert_eq!(app.focus, Focus::List);
    assert!(app.overlay.is_none());
    assert!(app.experience.screens.is_empty());
}
