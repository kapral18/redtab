use super::*;

#[test]
fn when_sidebar_navigation_starts_its_first_feed_and_images_should_wait_for_settling() {
    let mut app = app_with(&["a", "b"]);
    app.active = SEARCH_TAB;
    app.focus_sidebar();

    app.move_sidebar(1);

    let a = tab_index(&app, "a");
    assert_eq!(app.active, a);
    assert!(!app.tabs[a].feed.loading());
    app.card_pictures_drawn(&["/image.png"]);
    assert!(app.card_images.is_empty());
    assert!(!app.load_settled(Instant::now()));
    assert!(app.load_settled(Instant::now() + NAV_SETTLE));
    assert!(app.tabs[a].feed.loading());
}

#[test]
fn when_sidebar_navigation_crosses_several_tabs_only_the_settled_tab_should_load() {
    let mut app = app_with(&["a", "b", "c"]);
    app.focus_sidebar();

    for _ in 0..4 {
        app.move_sidebar(1);
    }

    let [a, b, c] = ["a", "b", "c"].map(|name| tab_index(&app, name));
    assert_eq!(app.active, c);
    assert!(
        !app.search.discovered.loading,
        "Search passed over asks for nothing"
    );
    assert!(!app.tabs[a].feed.loading());
    assert!(!app.tabs[b].feed.loading());
    assert!(!app.tabs[c].feed.loading());
    app.load_settled(Instant::now() + NAV_SETTLE);
    assert!(!app.tabs[a].feed.loading());
    assert!(!app.tabs[b].feed.loading());
    assert!(app.tabs[c].feed.loading());
}

#[test]
fn when_sidebar_navigation_is_waiting_an_explicit_reload_should_load_immediately_once() {
    let mut app = app_with(&["a"]);
    app.active = SEARCH_TAB;
    app.focus_sidebar();
    app.move_sidebar(1);

    app.reload();

    assert!(app.active_tab().feed.loading());
    assert!(!app.load_settled(Instant::now() + NAV_SETTLE));
}

#[test]
fn beside_an_open_post_a_card_that_loads_under_the_cursor_shows_its_post() {
    let mut app = app_with(&["ca", "cb"]);
    let group = |subreddit: &str, id: &str| Msg::Group {
        subreddit: subreddit.into(),
        generation: 0,
        result: Ok(listing(subreddit, &[id], None)),
    };
    app.on_msg(group("ca", "card1"));
    app.resize(200);
    app.open_current_post();
    app.focus_main();
    app.move_all_subs_group(1);
    assert_eq!(
        app.post_view.as_ref().unwrap().post.id,
        "card1",
        "the next card has nothing yet"
    );

    app.on_msg(group("cb", "card2"));

    assert_eq!(app.post_view.as_ref().unwrap().post.id, "card2");
}

#[test]
fn reloading_a_held_preview_requests_it_without_marking_unshown_content_read() {
    let (mut app, _) = split_feed(&["rel1", "rel2", "rel3"]);
    app.move_selection(1);
    app.move_selection(1);
    app.resize(100);

    app.reload();

    assert!(app.pending_posts.contains_key("/r/s/comments/rel3/x/"));
    assert!(!app.state.is_read("rel3"));
}

#[test]
fn when_a_detail_page_is_loading_without_media_results_should_be_polled_promptly() {
    let (app, _) = split_feed(&["pending"]);
    assert!(app.post_view.as_ref().unwrap().loading);
    assert!(app.queued_images.is_empty());
    assert!(app.wake_in() <= Duration::from_millis(16));
}

#[test]
fn beside_an_open_post_a_quick_run_of_list_moves_previews_each_and_loads_only_its_end() {
    let (mut app, _) = split_feed(&["run1", "run2", "run3", "run4"]);

    app.move_selection(1);
    assert!(
        !app.pending_posts.contains_key("/r/s/comments/run2/x/"),
        "the first scroll preview must wait too"
    );
    app.move_selection(1);
    assert_eq!(app.post_view.as_ref().unwrap().post.id, "run3");
    app.move_selection(1);

    assert_eq!(app.post_view.as_ref().unwrap().post.id, "run4");
    assert_eq!(app.focus, Focus::List, "the list keeps focus");
    assert!(
        !app.state.is_read("run2"),
        "a request is not a displayed page"
    );
    assert!(!app.state.is_read("run4"), "mid-run nothing loads");
    assert!(!app.load_settled(Instant::now()));
    assert!(app.load_settled(Instant::now() + NAV_SETTLE));
    assert!(
        !app.state.is_read("run3"),
        "a post passed over stays unread"
    );
    assert!(app.pending_posts.contains_key("/r/s/comments/run4/x/"));
    assert!(!app.state.is_read("run4"));
}

#[test]
fn opening_a_held_back_preview_loads_it_at_once_and_only_once() {
    let (mut app, _) = split_feed(&["hold1", "hold2", "hold3"]);
    app.move_selection(1);
    app.move_selection(1);
    assert!(!app.state.is_read("hold3"), "held back");

    app.open_current_post();

    assert_eq!(app.focus, Focus::Post);
    assert!(app.pending_posts.contains_key("/r/s/comments/hold3/x/"));
    assert!(!app.state.is_read("hold3"));
    assert!(
        !app.load_settled(Instant::now() + NAV_SETTLE),
        "opening ended the wait"
    );
}

#[test]
fn a_quick_run_of_tab_switches_loads_only_the_tab_it_stops_on() {
    let mut app = app_with(&["a", "b", "c"]);
    let [a, b, c] = ["a", "b", "c"].map(|name| tab_index(&app, name));

    app.switch_tab(a);
    app.switch_tab(b);
    app.switch_tab(c);

    assert!(
        app.tabs[a].feed.loading(),
        "a switch after a pause loads at once"
    );
    assert!(!app.tabs[c].feed.loading(), "mid-run nothing loads");
    assert!(app.load_settled(Instant::now() + NAV_SETTLE));
    assert!(
        !app.tabs[b].feed.loading(),
        "a tab passed over is not loaded"
    );
    assert!(app.tabs[c].feed.loading());
}

#[test]
fn a_reload_during_a_tab_switch_wait_is_not_asked_again_once_it_fails() {
    let mut app = app_with(&["a", "b"]);
    let [a, b] = ["a", "b"].map(|name| tab_index(&app, name));
    app.switch_tab(a);
    app.switch_tab(b);

    app.reload();
    let (tab, generation) = (app.tabs[b].id, app.tabs[b].feed.generation);
    app.on_msg(Msg::Feed {
        tab,
        generation,
        append: false,
        result: Err("connection refused".into()),
    });

    assert!(!app.load_settled(Instant::now() + NAV_SETTLE));
    assert!(
        !app.tabs[b].feed.loading(),
        "the failed reload is not repeated"
    );
}
