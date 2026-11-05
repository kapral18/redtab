use super::*;

#[test]
fn a_search_feed_should_cycle_search_sort_and_time_and_a_new_query_should_reset_them() {
    let mut app = app_with(&["vim"]);
    app.open_search(None);
    crate::keys::paste(&mut app, "neovim");
    let _ = app.tick_at(Instant::now() + search::DEBOUNCE);
    let i = SEARCH_TAB;
    assert_eq!(app.active, i);
    assert_eq!(app.tabs[i].feed.search.as_deref(), Some("neovim"));
    assert!(app.tabs[i].feed.loading(), "the paused query is requested");
    let generation = app.tabs[i].feed.generation;

    app.on_msg(Msg::Feed {
        tab: app.tabs[i].id,
        generation,
        append: false,
        result: Ok(listing("vim", &["a", "b"], Some("t3_b"))),
    });
    app.resize(240);
    press(&mut app, crossterm::event::KeyCode::Enter);
    assert!(
        app.post_view.is_none() && !app.search_editing(),
        "Enter while typing leaves the query for the results"
    );
    press(&mut app, crossterm::event::KeyCode::Enter);
    assert!(
        app.post_view.is_some(),
        "Enter on the results reads the chosen post"
    );
    app.prefetch_list(usize::MAX);
    assert!(app.tabs[i].feed.loading_more(), "results page on");
    app.focus_main();
    assert!(app.post_view.is_some() && !app.post_focused());
    press(&mut app, crossterm::event::KeyCode::Esc);
    assert!(
        app.post_view.is_none(),
        "Esc from the results closes the post"
    );
    let generation = app.tabs[i].feed.generation;
    sequence(&mut app, " fs");
    sequence(&mut app, " ft");
    let feed = &app.tabs[i].feed;
    assert_eq!(feed.search_sort, crate::reddit::experience::SearchSort::Hot);
    assert_eq!(feed.search_time, TimeRange::All.next());
    assert!(feed.loading() && feed.generation > generation);
    assert_eq!(feed.search.as_deref(), Some("neovim"));

    sequence(&mut app, "/");
    crate::keys::handle(
        &mut app,
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('u'),
            crossterm::event::KeyModifiers::CONTROL,
        ),
    );
    crate::keys::paste(&mut app, "rust");
    let _ = app.tick_at(Instant::now() + search::DEBOUNCE);
    let feed = &app.tabs[i].feed;
    assert_eq!(feed.search.as_deref(), Some("rust"));
    assert_eq!(feed.search_sort, Default::default());
    assert_eq!(feed.search_time, TimeRange::All);
}

#[test]
fn home_reaches_a_hot_tab_that_kept_an_earlier_time_range() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    app.set_sort(Sort::Top);
    app.cycle_time();
    app.set_sort(Sort::Hot);

    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Group {
            subreddit: "vim".into(),
            generation: 0,
            result: Ok(listing("vim", &["home"], None)),
        }),
        fetched_at: now(),
        refreshing: false,
        source: ordered_source(4, false),
    });

    assert_eq!(app.tabs[i].feed.posts[0].id, "home");
}
