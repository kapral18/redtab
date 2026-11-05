use super::*;

#[test]
fn restarting_empty_visible_islands_should_hold_prefetch_and_icons_until_ready() {
    for recovery in ["cancelled stream", "failed retry", "stale empty cache"] {
        let mut app = app_with(&["visible", "next", "side"]);
        let draw = |app: &mut App| app.groups_drawn(&[0], &["side".into()], &[1]);
        draw(&mut app);
        let generation = app.all_subs.groups[0].generation;
        match recovery {
            "cancelled stream" => {
                app.on_msg(Msg::GroupProgress {
                    subreddit: "visible".into(),
                    generation,
                    post: Box::new(post("partial", "visible")),
                    source: Source::default(),
                });
                app.on_msg(Msg::Group {
                    subreddit: "visible".into(),
                    generation,
                    result: Err(crate::reddit::REQUEST_CANCELLED.into()),
                });
            }
            "failed retry" => {
                app.on_msg(Msg::Group {
                    subreddit: "visible".into(),
                    generation,
                    result: Err("offline".into()),
                });
                app.reload();
            }
            "stale empty cache" => app.on_group(
                "visible",
                generation,
                Ok(listing("visible", &[], None)),
                Freshness {
                    fetched_at: now() - crate::cache::CONTENT_TTL - 1,
                    source: Source {
                        cached: true,
                        ..Default::default()
                    },
                },
                false,
            ),
            _ => unreachable!(),
        }
        show_thumbnails(&mut app);
        draw(&mut app);
        assert!(app.all_subs.groups[0].loading, "{recovery}");
        assert!(app.group_posts(&app.all_subs.groups[0]).next().is_none());
        assert!(app.icon_lookups.is_empty(), "{recovery}");
        assert!(!app.all_subs.groups[1].loading, "{recovery}");

        app.on_msg(Msg::Group {
            subreddit: "visible".into(),
            generation: app.all_subs.groups[0].generation,
            result: if recovery == "cancelled stream" {
                Err("offline".into())
            } else {
                Ok(listing("visible", &[], None))
            },
        });
        draw(&mut app);
        assert!(app.icon_lookups.contains("visible"), "{recovery}");
        assert!(app.icon_lookups.contains("side"), "{recovery}");
        assert_eq!(app.all_subs.groups[1].loading, recovery != "failed retry");
    }
}

#[test]
fn island_lookahead_should_wait_for_visible_titles_before_prefetch_and_icons() {
    let mut app = app_with(&["a", "b", "c", "d", "side"]);
    show_thumbnails(&mut app);
    let draw = |app: &mut App| app.groups_drawn(&[0, 1, 2], &["side".into()], &[3]);
    draw(&mut app);
    assert!(app.icon_lookups.is_empty());
    assert!(!app.all_subs.groups[3].loading);

    let mut pinned = post("pin", "a");
    pinned.stickied = true;
    app.on_msg(Msg::GroupProgress {
        subreddit: "a".into(),
        generation: app.all_subs.groups[0].generation,
        post: Box::new(pinned),
        source: Source::default(),
    });
    draw(&mut app);
    assert!(app.icon_lookups.is_empty());
    assert!(!app.all_subs.groups[3].loading);

    for (index, name) in ["a", "b", "c"].into_iter().enumerate() {
        app.on_msg(Msg::GroupProgress {
            subreddit: name.into(),
            generation: app.all_subs.groups[index].generation,
            post: Box::new(post(name, name)),
            source: Source::default(),
        });
        draw(&mut app);
        assert_eq!(app.all_subs.groups[3].loading, index == 2);
        assert_eq!(app.icon_lookups.contains("side"), index == 2);
    }
    assert!(app.all_subs.groups[..3].iter().all(|group| group.loading));
    app.groups_drawn(&[], &[], &[]);
    app.active = 0;
    app.pool.set_visible_icons(&["other"]);
    app.demand_icon("other");
    assert!(app.icon_lookups.contains("other"));
}

#[test]
fn island_lookahead_should_keep_refreshes_and_icons_tied_to_visible_demand() {
    let mut app = app_with(&["visible", "cold", "stale", "forced"]);
    app.active = ALL_SUBS_TAB;
    show_thumbnails(&mut app);
    let fresh = Freshness {
        fetched_at: now(),
        source: Source::default(),
    };
    app.all_subs.groups[0].freshness = Some(fresh);
    app.all_subs.groups[2].freshness = Some(Freshness {
        fetched_at: now() - 600,
        ..fresh
    });
    app.all_subs.groups[3].needs_refresh = true;

    app.groups_drawn(&[0], &[], &[1, 2, 3]);
    let generation = app.all_subs.groups[1].generation;
    app.groups_drawn(&[0], &[], &[1, 2, 3]);

    assert!(app.all_subs.groups[1].loading);
    assert_eq!(app.all_subs.groups[1].generation, generation);
    assert!(!app.all_subs.groups[2].loading && !app.all_subs.groups[3].loading);
    assert!(!app.icon_lookups.contains("cold"));
    app.on_msg(Msg::Group {
        subreddit: "cold".into(),
        generation,
        result: Ok(listing("cold", &["prefetched"], None)),
    });
    assert!(!app.icon_lookups.contains("cold"));
    assert!(!app.all_subs.has_new_posts(&app.config));
    assert!(!app.state.is_read("prefetched"));
    app.groups_drawn(&[1], &[], &[]);
    assert!(app.icon_lookups.contains("cold"));

    app.groups_drawn(&[], &[], &[3]);
    assert!(!app.all_subs.groups[3].loading);
}

#[test]
fn when_offscreen_work_is_cancelled_the_group_should_keep_content_and_resume_on_return() {
    for forced in [false, true] {
        let mut app = app_with(&["s"]);
        let group = &mut app.all_subs.groups[0];
        group.posts = vec![post("kept", "s")];
        group.loading = true;
        group.needs_refresh = forced;
        let generation = group.generation;
        app.on_msg(Msg::Group {
            subreddit: "s".into(),
            generation,
            result: Err(crate::reddit::REQUEST_CANCELLED.into()),
        });
        let group = &app.all_subs.groups[0];
        assert!(!group.loading && group.error.is_none());
        assert_eq!(
            group.needs_refresh, forced,
            "cancellation must preserve whether cache may answer"
        );
        assert_eq!(group.posts[0].id, "kept");
        app.ensure_group_loaded(0);
        let generation = app.all_subs.groups[0].generation;
        assert!(app.all_subs.groups[0].loading);
        app.ensure_group_loaded(0);
        assert_eq!(app.all_subs.groups[0].generation, generation);
    }
}

#[test]
fn scrolling_islands_should_fetch_only_the_settled_view_and_accept_incoming_rows() {
    let mut app = app_with(&["a", "b", "c"]);
    app.groups_drawn(&[0], &[], &[]);
    assert!(
        app.all_subs.groups[0].loading,
        "initial entry loads immediately"
    );
    let generation = app.all_subs.groups[0].generation;

    app.move_all_subs_group(1);
    app.groups_drawn(&[1], &[], &[]);
    assert!(!app.all_subs.groups[1].loading, "the first scroll waits");
    app.on_msg(Msg::GroupProgress {
        subreddit: "a".into(),
        generation,
        post: Box::new(post("a1", "a")),
        source: Source::default(),
    });
    assert_eq!(app.all_subs.groups[0].posts[0].id, "a1");
    app.move_all_subs_group(1);
    app.groups_drawn(&[2], &[], &[]);
    assert!(!app.all_subs.groups[2].loading);
    app.tick_at(Instant::now() + NAV_SETTLE);
    app.groups_drawn(&[2], &[], &[]);

    assert!(
        !app.all_subs.groups[1].loading,
        "passed islands stay unfetched"
    );
    assert!(app.all_subs.groups[2].loading);
    assert_eq!(app.all_subs.groups[0].posts[0].id, "a1");
}

#[test]
fn when_subscriptions_arrive_on_home_communities_should_wait_for_visible_demand() {
    let mut app = app_with(&[]);
    app.active = 0;
    app.sync_islands(vec!["rust".into(), "vim".into()]);
    assert_eq!(app.all_subs.groups.len(), 2);
    assert!(
        app.all_subs.groups.iter().all(|g| !g.loading),
        "offscreen groups must not start HTTP loads"
    );
}
