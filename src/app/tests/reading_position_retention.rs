use super::*;

#[test]
fn reloading_a_newer_cached_copy_keeps_it_over_an_older_loads_late_answer() {
    let (mut app, _) = split_feed(&["ord1", "ord2"]);
    let first = app.post_view.as_ref().unwrap().generation;
    app.move_selection(1);
    app.move_selection(-1);
    let back = app.post_view.as_ref().unwrap().generation;
    let answer = |generation, body: &str, order, cached| {
        let page = PostPage {
            post: post("ord1", "s"),
            body: body.into(),
            comments: vec![],
        };
        ordered_snapshot(
            Msg::Post {
                generation,
                result: Ok(Box::new(page)),
            },
            order,
            cached,
            false,
        )
    };
    app.on_msg(answer(back, "newer", 12, true));
    app.open_current_post();
    app.reload();
    let refresh = app.post_view.as_ref().unwrap().generation;

    app.on_msg(answer(first, "older", 10, false));

    let view = app.post_view.as_ref().unwrap();
    assert_eq!(view.generation, refresh, "the refresh still owns the view");
    assert_eq!(view.page.as_ref().unwrap().body, "newer");
}

#[test]
fn a_home_refresh_with_fewer_posts_keeps_the_post_being_read() {
    let mut app = app_with(&["a"]);
    app.switch_tab(ALL_SUBS_TAB);
    app.on_msg(Msg::Group {
        generation: 0,
        subreddit: "a".into(),
        result: Ok(listing("a", &["a1", "a2", "a3"], None)),
    });
    app.all_subs.post = 2;

    app.on_msg(Msg::Group {
        generation: 0,
        subreddit: "a".into(),
        result: Ok(listing("a", &["a1"], None)),
    });

    assert_eq!(app.current_post().map(|p| p.id).as_deref(), Some("a3"));
}

#[test]
fn only_a_background_answer_keeps_the_island_post_being_read() {
    let mut app = app_with(&["vim"]);
    app.switch_tab(ALL_SUBS_TAB);
    app.on_msg(island_answer(&app, &["a", "b", "c", "d"]));

    // An untouched island top stays the top as the island reranks or drops it.
    app.on_msg(island_answer(&app, &["b", "a", "c", "d"]));
    assert_eq!(island_ids(&app), ["b", "a", "c"]);
    assert_eq!(app.current_post().unwrap().id, "b");
    app.on_msg(island_answer(&app, &["a", "c", "d"]));
    assert_eq!(island_ids(&app), ["a", "c", "d"]);
    assert_eq!(app.current_post().unwrap().id, "a");

    // An open top post is being read.
    app.open_current_post();
    app.on_msg(island_answer(&app, &["c", "d"]));
    assert_eq!(island_ids(&app), ["a", "c", "d"]);
    assert_eq!(app.current_post().unwrap().id, "a");

    // `r` shows the island as Reddit ranks it.
    app.close_post();
    app.all_subs.post = 1;
    app.reload();
    app.on_msg(island_answer(&app, &["d", "e"]));
    assert_eq!(island_ids(&app), ["d", "e"]);
    assert_eq!(app.current_post().unwrap().id, "e");
}

#[test]
fn opening_a_post_before_r_answers_keeps_the_list_on_it() {
    // From the list, or from elsewhere such as Reddit tools.
    for from_list in [true, false] {
        let mut app = app_with(&["vim"]);
        let i = tab_index(&app, "vim");
        app.switch_tab(i);
        let tab = app.tabs[i].id;
        app.on_msg(Msg::Feed {
            tab,
            generation: app.tabs[i].feed.generation,
            append: false,
            result: Ok(listing("vim", &["a", "b", "c"], None)),
        });
        app.focus = Focus::List;
        app.move_selection(2);

        app.reload();
        if from_list {
            app.open_current_post();
        } else {
            let post = app.current_post().unwrap();
            app.open_post_permalink(post.clone(), post.permalink);
        }
        app.on_msg(Msg::Feed {
            tab,
            generation: app.tabs[i].feed.generation,
            append: false,
            result: Ok(listing("vim", &["n", "a", "b", "c"], None)),
        });

        assert!(app.tabs[i].feed.has_new_posts(&app.config));
        assert_eq!(app.current_post().unwrap().id, "c");
        assert_eq!(app.post_view.as_ref().unwrap().shown_post().id, "c");
    }

    // A thread read from Reddit tools opens its post over the community already shown.
    use crate::reddit::experience::{Query, Request, Response};
    let mut app = app_with(&["neovim"]);
    let i = tab_index(&app, "neovim");
    app.switch_tab(i);
    let tab = app.tabs[i].id;
    app.on_msg(Msg::Feed {
        tab,
        generation: app.tabs[i].feed.generation,
        append: false,
        result: Ok(listing("neovim", &["a", "b", "c"], None)),
    });
    app.focus = Focus::List;
    app.move_selection(2);
    app.reload();
    let page =
        crate::reddit::parse::parse_post_page(include_str!("../../../tests/fixtures/post.json"))
            .unwrap();
    let query = Query::Thread(page.post.permalink.clone());
    app.experience.pending = Some((7, Request::Read { query, after: None }));
    app.on_msg(Msg::Experience {
        generation: 7,
        result: Ok(Response::Thread(Box::new(page))),
    });
    app.on_msg(Msg::Feed {
        tab,
        generation: app.tabs[i].feed.generation,
        append: false,
        result: Ok(listing("neovim", &["n", "a", "b", "c"], None)),
    });
    assert_eq!(app.active, i);
    assert!(app.tabs[i].feed.has_new_posts(&app.config));
    assert_eq!(app.tabs[i].feed.selected, 2);
}

#[test]
fn following_an_open_post_onto_a_list_top_keeps_new_posts_waiting() {
    // Onto a feed whose top is the open post.
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(ALL_SUBS_TAB);
    app.on_msg(island_answer(&app, &["a", "b"]));
    app.switch_tab(i);
    let tab = app.tabs[i].id;
    app.load_feed(i, false);
    app.on_msg(Msg::Feed {
        tab,
        generation: app.tabs[i].feed.generation,
        append: false,
        result: Ok(listing("vim", &["n", "a", "b"], None)),
    });
    assert!(app.tabs[i].feed.has_new_posts(&app.config));
    app.switch_tab(ALL_SUBS_TAB);
    app.open_current_post();
    app.switch_ancestor_tab(i);
    assert_eq!(app.active, i);
    assert!(app.tabs[i].feed.has_new_posts(&app.config));
    assert_eq!(app.list_post(i).unwrap().id, "a");
    assert_eq!(app.post_view.as_ref().unwrap().shown_post().id, "a");

    // Onto the All subs island whose first post is the open post.
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(ALL_SUBS_TAB);
    app.on_msg(island_answer(&app, &["a", "b"]));
    app.switch_tab(i);
    app.open_current_post();
    app.on_msg(island_answer(&app, &["n", "a", "b"]));
    assert!(app.all_subs.has_new_posts(&app.config));
    assert_eq!(app.list_post(i).unwrap().id, "a");
    app.switch_ancestor_tab(ALL_SUBS_TAB);
    assert_eq!(app.active, ALL_SUBS_TAB);
    assert!(app.all_subs.has_new_posts(&app.config));
    assert_eq!(app.current_post().unwrap().id, "a");
    assert_eq!(app.post_view.as_ref().unwrap().shown_post().id, "a");
}

#[test]
fn a_post_open_on_another_surface_leaves_an_untouched_top_alone() {
    // Open in All subs: the community tab seeded from its island keeps its top.
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(ALL_SUBS_TAB);
    app.on_msg(island_answer(&app, &["a", "b", "c"]));
    app.open_current_post();
    app.on_msg(island_answer(&app, &["b", "a", "c"]));
    assert_eq!(app.tabs[i].feed.selected, 0);
    assert_eq!(app.tabs[i].feed.posts[0].id, "b");

    // Open in the community tab: the hidden island keeps its top.
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.on_msg(island_answer(&app, &["a", "b", "c"]));
    app.switch_tab(i);
    app.open_current_post();
    app.on_msg(island_answer(&app, &["b", "a", "c"]));
    assert_eq!(app.all_subs.post, 0);
}

#[test]
fn settings_filters_should_keep_visible_selection_and_close_a_hidden_detail() {
    let mut app = app_with(&[]);
    app.active = 0;
    app.tabs[0].feed.posts = vec![
        post("first", "rust"),
        post("second", "rust"),
        post("third", "rust"),
    ];
    app.tabs[0].feed.selected = 1;
    app.change_setting(
        "filters.keywords",
        toml::Value::Array(vec![toml::Value::String("first".into())]),
    );
    assert_eq!(app.current_post().unwrap().id, "second");
    app.open_current_post();
    assert!(app.post_view.is_some());
    app.change_setting(
        "filters.keywords",
        toml::Value::Array(vec![toml::Value::String("second".into())]),
    );
    assert!(app.post_view.is_none());
}

#[test]
fn settings_nsfw_changes_should_keep_a_safe_open_detail_and_its_reading_position() {
    let mut app = app_with(&[]);
    app.active = 0;
    app.tabs[0].feed.posts = vec![post("safe", "rust")];
    app.open_current_post();
    app.post_view.as_mut().unwrap().scroll = 23;
    confirm_preferences(&mut app, true);
    let view = app.post_view.as_ref().unwrap();
    assert_eq!(view.post.id, "safe");
    assert_eq!(view.scroll, 23);
    assert!(view.loading);
    app.post_view.as_mut().unwrap().post.nsfw = true;
    confirm_preferences(&mut app, false);
    assert!(app.post_view.is_none());
}

#[test]
fn hiding_the_last_selected_post_keeps_a_visible_target() {
    let mut app = app_with(&["s"]);
    app.all_subs.groups[0].posts = vec![post("a", "s"), post("b", "s"), post("filtered", "s")];
    app.config.filters.keywords = vec!["filtered".into()];
    app.all_subs.post = 1;
    app.experience.pending = Some((
        123,
        crate::reddit::experience::Request::Write(crate::reddit::experience::Action::Hide {
            id: "t3_b".into(),
            hide: true,
        }),
    ));
    app.on_experience(
        123,
        Ok(crate::reddit::experience::Response::Written { url: None }),
        crate::cache::Freshness {
            fetched_at: crate::app::now(),
            source: Default::default(),
        },
    );
    assert_eq!(app.current_post().map(|post| post.id), Some("a".into()));
    let generation = app.all_subs.groups[0].generation;
    app.on_msg(Msg::Group {
        subreddit: "s".into(),
        generation,
        result: Err("offline".into()),
    });
    assert_eq!(app.current_post().map(|post| post.id), Some("a".into()));
}

#[test]
fn a_background_feed_answer_keeps_the_post_being_read_in_its_row() {
    let (mut app, _messages) = split_feed(&["a", "b", "c", "d"]);
    let i = tab_index(&app, "s");
    let tab = app.tabs[i].id;
    let ids = |app: &App| {
        app.tabs[i]
            .feed
            .posts
            .iter()
            .map(|post| post.id.clone())
            .collect::<Vec<_>>()
    };
    let answer = |app: &mut App, posts: &[&str]| {
        app.on_msg(Msg::Feed {
            tab,
            generation: app.tabs[i].feed.generation,
            append: false,
            result: Ok(listing("s", posts, None)),
        });
    };
    // An open top post is being read.
    answer(&mut app, &["b", "c", "d"]);
    assert_eq!(ids(&app), ["a", "b", "c", "d"]);
    assert_eq!(app.post_view.as_ref().unwrap().shown_post().id, "a");

    app.focus = Focus::List;
    app.move_selection(2);
    assert_eq!(app.post_view.as_ref().unwrap().shown_post().id, "c");

    answer(&mut app, &["a", "b", "d", "e"]);
    assert_eq!(ids(&app), ["a", "b", "c", "d", "e"]);
    assert_eq!(app.current_post().unwrap().id, "c");
    assert_eq!(app.post_view.as_ref().unwrap().shown_post().id, "c");

    // `r` shows the feed as Reddit ranks it.
    app.reload();
    answer(&mut app, &["a", "b", "d", "e"]);
    assert_eq!(ids(&app), ["a", "b", "d", "e"]);
    assert_eq!(app.current_post().unwrap().id, "d");

    // An untouched top stays the top as the list reranks or drops it, even when
    // the dropped post stays below the page while new posts wait.
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    let tab = app.tabs[i].id;
    let answer = |app: &mut App, posts: &[&str]| {
        app.on_msg(Msg::Feed {
            tab,
            generation: app.tabs[i].feed.generation,
            append: false,
            result: Ok(listing("vim", posts, None)),
        });
        let feed = &app.tabs[i].feed;
        (
            app.current_post().unwrap().id,
            feed.selected + feed.offset,
            feed.posts.len(),
        )
    };
    answer(&mut app, &["a", "b", "c"]);
    // Card scrolling left over from card view does not count in compact view.
    app.tabs[i].feed.cards.line = 3;
    if app.state.list_view() == crate::state::ListView::Card {
        app.state.toggle_list_view();
    }
    assert_eq!(answer(&mut app, &["b", "a", "c"]), ("b".into(), 0, 3));
    assert_eq!(answer(&mut app, &["a", "c"]), ("a".into(), 0, 2));
    assert_eq!(answer(&mut app, &["n", "c"]), ("c".into(), 0, 2));
    assert!(app.tabs[i].feed.has_new_posts(&app.config));
}
