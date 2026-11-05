use super::*;

#[test]
fn a_filter_that_hides_waiting_posts_applies_the_held_answers_and_clears_the_notice() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    let tab = app.tabs[i].id;
    let feed = |app: &App, ids: &[&str]| Msg::Feed {
        tab,
        generation: app.tabs[i].feed.generation,
        append: false,
        result: Ok(listing("vim", ids, None)),
    };
    app.on_msg(feed(&app, &["a", "b"]));
    app.load_feed(i, false);
    app.on_msg(feed(&app, &["n", "b", "a"]));
    app.on_msg(island_answer(&app, &["a", "b"]));
    app.on_msg(island_answer(&app, &["n", "a", "b"]));
    assert!(app.tabs[i].feed.has_new_posts(&app.config));
    assert!(app.all_subs.has_new_posts(&app.config));

    app.change_setting(
        "filters.keywords",
        toml::Value::Array(vec!["title n".into()]),
    );

    assert!(!app.tabs[i].feed.has_new_posts(&app.config));
    assert!(app.tabs[i].feed.update.is_none());
    assert_eq!(
        app.config
            .visible(&app.tabs[i].feed.posts)
            .map(|post| post.id.as_str())
            .collect::<Vec<_>>(),
        // The island answers seed this tab's first page too; the last one ranks a, b.
        ["a", "b"]
    );
    assert!(!app.all_subs.has_new_posts(&app.config));
}

#[test]
fn a_filter_that_empties_an_island_shows_its_waiting_posts() {
    let mut app = app_with(&["vim"]);
    app.on_msg(island_answer(&app, &["a", "b"]));
    app.on_msg(island_answer(&app, &["n", "a", "b"]));
    assert!(app.all_subs.has_new_posts(&app.config));

    app.change_setting(
        "filters.keywords",
        toml::Value::Array(vec!["title a".into(), "title b".into()]),
    );

    assert!(!app.all_subs.has_new_posts(&app.config));
    assert_eq!(island_ids(&app), ["n"]);
}

#[test]
fn a_filter_hiding_a_post_above_the_read_one_keeps_it_when_an_island_settles() {
    let mut app = app_with(&["vim"]);
    app.switch_tab(ALL_SUBS_TAB);
    app.on_msg(island_answer(&app, &["a", "b", "c", "d"]));
    app.all_subs.post = 2;
    app.on_msg(island_answer(&app, &["n", "a", "b", "x", "c", "d"]));
    assert!(app.all_subs.has_new_posts(&app.config));

    app.change_setting(
        "filters.keywords",
        toml::Value::Array(vec!["title n".into(), "title a".into()]),
    );

    assert!(!app.all_subs.has_new_posts(&app.config));
    assert_eq!(island_ids(&app), ["b", "c", "x"]);
    assert_eq!(app.current_post().unwrap().id, "c");
}
