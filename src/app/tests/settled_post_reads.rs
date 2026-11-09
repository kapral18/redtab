use super::*;

#[test]
fn scrolling_inside_a_video_card_should_defer_startup_and_keep_existing_playback() {
    let mut app = card_playback_app();
    app.card_video.as_mut().unwrap().since = Instant::now() - NAV_SETTLE;
    app.tabs[0].feed.cards.post = Some("first".into());

    app.scroll_card(1);
    app.sync_video_with(Instant::now(), |_, _, _| {
        panic!("scrolling must hold a new player")
    });
    assert!(app.video().is_none());
    app.sync_video_with(Instant::now() + NAV_SETTLE, |url, area, _| {
        Player::silent(&url, area)
    });
    assert!(app.card_player().is_some());
    app.scroll_card(1);
    app.sync_video_with(Instant::now(), |_, _, _| {
        panic!("scrolling must keep the current player")
    });
    assert!(app.card_player().is_some());
    assert!(!app.card_player().unwrap().hidden());
}

#[test]
fn scrolling_away_from_the_feed_end_should_cancel_deferred_pagination() {
    let mut app = app_with(&["s"]);
    let index = tab_index(&app, "s");
    app.switch_tab(index);
    let posts = (0..20).map(|i| post(&format!("p{i}"), "s")).collect();
    app.on_msg(Msg::Feed {
        tab: app.tabs[index].id,
        generation: app.tabs[index].feed.generation,
        append: false,
        result: Ok(Listing {
            posts,
            after: Some("next".into()),
        }),
    });

    app.select_edge(true);
    assert!(!app.active_tab().feed.loading_more());
    app.select_edge(false);
    app.tick_at(Instant::now() + NAV_SETTLE);
    assert!(
        !app.active_tab().feed.loading_more(),
        "the final viewport is far from the end"
    );
    app.select_edge(true);
    assert!(!app.active_tab().feed.loading_more());
    app.tick_at(Instant::now() + NAV_SETTLE);

    assert!(app.active_tab().feed.loading_more());
    assert_eq!(app.active_tab().feed.selected, 19);
}

#[test]
fn coming_back_over_a_cached_copy_takes_only_the_earlier_loads_network_answer() {
    let (mut app, _) = split_feed(&["back1", "back2"]);
    let first = app.post_view.as_ref().unwrap().generation;
    app.move_selection(1);
    app.move_selection(-1);
    let back = app.post_view.as_ref().unwrap().generation;
    let answer = |generation, order, cached, refreshing| {
        let page = PostPage {
            post: post("back1", "s"),
            body: String::new(),
            comments: vec![],
        };
        ordered_snapshot(
            Msg::Post {
                generation,
                result: Ok(Box::new(page)),
            },
            order,
            cached,
            refreshing,
        )
    };

    app.on_msg(answer(first, 1, true, true));
    assert_eq!(
        app.post_view.as_ref().unwrap().generation,
        back,
        "a cached answer still refreshing is not taken"
    );
    app.on_msg(answer(back, 1, true, true));
    // Replies asked for under the newer load, whose answer will be dropped.
    let view = app.post_view.as_mut().unwrap();
    view.loading_more.insert("parent".into());
    app.on_msg(answer(first, 2, false, false));

    let view = app.post_view.as_ref().unwrap();
    assert_eq!(
        view.generation, first,
        "the earlier load's network answer replaces the cached copy"
    );
    assert!(!view.loading);
    assert!(
        view.loading_more.is_empty(),
        "no reply row stays loading for a dropped answer"
    );
}

#[test]
fn home_navigation_flows_across_groups_and_skips_pinned_posts() {
    let mut app = app_with(&["a", "b"]);
    let mut pinned = listing("a", &["pin", "a1", "a2"], None);
    pinned.posts[0].stickied = true;
    app.on_msg(Msg::Group {
        generation: 0,
        subreddit: "a".into(),
        result: Ok(pinned),
    });
    app.on_msg(Msg::Group {
        generation: 0,
        subreddit: "b".into(),
        result: Ok(listing("b", &["b1"], None)),
    });

    assert_eq!(app.current_post().unwrap().id, "a1");
    app.move_selection(2);
    assert_eq!(app.current_post().unwrap().id, "b1");
    app.move_selection(-1);
    assert_eq!(app.current_post().unwrap().id, "a2");
}

#[test]
fn community_scrolling_from_all_subs_details_should_load_only_the_settled_post() {
    let mut app = app_with(&["a", "b", "c"]);
    for group in &mut app.all_subs.groups {
        group.posts = vec![post(&format!("{}1", group.subreddit), &group.subreddit)];
    }
    app.open_current_post();

    app.move_community(true);
    assert_eq!(app.current_post().unwrap().id, "b1");
    assert!(!app.pending_posts.contains_key("/r/b/comments/b1/x/"));
    app.move_community(true);
    assert_eq!(app.current_post().unwrap().id, "c1");
    assert!(!app.pending_posts.contains_key("/r/c/comments/c1/x/"));
    app.tick_at(Instant::now() + NAV_SETTLE);

    assert!(!app.pending_posts.contains_key("/r/b/comments/b1/x/"));
    assert!(app.pending_posts.contains_key("/r/c/comments/c1/x/"));
    assert_eq!(app.focus, Focus::Post);
}

#[test]
fn moving_after_r_keeps_the_new_position_when_arrivals_land() {
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
    app.reload();
    app.move_selection(2);

    app.on_msg(Msg::Feed {
        tab,
        generation: app.tabs[i].feed.generation,
        append: false,
        result: Ok(listing("vim", &["new", "a", "b", "c"], None)),
    });

    assert_eq!(app.current_post().unwrap().id, "c");
    assert!(app.tabs[i].feed.has_new_posts(&app.config));
}

#[test]
fn when_refresh_replaces_an_unfinished_post_an_older_answer_should_not_take_over() {
    let (mut app, _) = split_feed(&["one"]);
    let previous = app.post_view.as_ref().unwrap().generation;
    app.refresh_post();
    let current = app.post_view.as_ref().unwrap().generation;
    let page = |body: &str| {
        Box::new(PostPage {
            post: post("one", "s"),
            body: body.into(),
            comments: vec![],
        })
    };

    app.on_msg(Msg::Post {
        generation: previous,
        result: Ok(page("old response")),
    });

    assert_eq!(app.post_view.as_ref().unwrap().generation, current);
    assert!(app.post_view.as_ref().unwrap().page.is_none());
    app.on_msg(Msg::Post {
        generation: current,
        result: Ok(page("refreshed response")),
    });
    assert_eq!(
        app.post_view.as_ref().unwrap().page.as_ref().unwrap().body,
        "refreshed response"
    );
}

#[test]
fn when_the_joined_post_read_was_cancelled_it_should_retry_without_showing_an_error() {
    let (mut app, _) = split_feed(&["one"]);
    let view = app.post_view.as_ref().unwrap();
    let generation = view.generation;
    let permalink = view.post.permalink.clone();

    app.on_msg(Msg::Post {
        generation,
        result: Err(crate::reddit::REQUEST_CANCELLED.into()),
    });

    assert!(app.post_view.as_ref().unwrap().error.is_none());
    assert_eq!(app.pending_posts.get(&permalink), Some(&generation));
    assert!(app.post_view.as_ref().unwrap().loading);
}

#[test]
fn when_two_obsolete_feeds_are_in_flight_the_current_feed_should_not_wait_for_their_responses() {
    let server = crate::reddit::tests::local_http_server();
    let base = format!("http://{}", server.server_addr());
    let (mut app, messages) = app_at(&base, &["first", "second", "current"]);
    let receive = |timeout: Duration| server.recv_timeout(timeout).unwrap();
    app.switch_tab(tab_index(&app, "first"));
    let first = receive(Duration::from_secs(2)).unwrap();
    app.feed_settle.last_move = None;
    app.switch_tab(tab_index(&app, "second"));
    let second = receive(Duration::from_secs(2)).unwrap();
    app.feed_settle.last_move = None;
    app.switch_tab(tab_index(&app, "current"));

    let current = receive(Duration::from_millis(1200));
    for request in [first, second] {
        let _ = request.respond(crate::reddit::tests::served::Response::from_string(
            include_str!("../../../tests/fixtures/sub.json"),
        ));
    }
    let bypassed = current.is_some();
    let current = current.unwrap_or_else(|| receive(Duration::from_secs(2)).unwrap());
    assert!(current.url().starts_with("/r/current/"));
    current
        .respond(crate::reddit::tests::served::Response::from_string(
            include_str!("../../../tests/fixtures/sub.json"),
        ))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !app.active_tab().feed.loaded {
        assert!(Instant::now() < deadline);
        app.on_msg(messages.recv_timeout(Duration::from_secs(2)).unwrap());
    }

    assert!(
        bypassed,
        "the current feed waited behind obsolete network reads"
    );
    assert!(app.active_tab().feed.loaded);
    assert!(app.status.as_ref().is_none_or(|status| !status.error));
}
