use super::*;

#[test]
fn staying_on_a_stale_feed_or_all_subs_refetches_and_holds_new_posts_behind_the_notice() {
    let mut app = app_with(&["vim"]);
    let i = tab_index(&app, "vim");
    app.switch_tab(i);
    let tab = app.tabs[i].id;
    app.on_msg(Msg::Feed {
        tab,
        generation: app.tabs[i].feed.generation,
        append: false,
        result: Ok(listing("vim", &["a", "b"], None)),
    });
    let start = Instant::now();
    let _ = app.tick_at(start + FEED_POLL);
    assert!(!app.tabs[i].feed.loading(), "fresh posts are not refetched");

    app.tabs[i].feed.freshness.as_mut().unwrap().fetched_at = 0;
    let _ = app.tick_at(start + FEED_POLL);
    assert!(!app.tabs[i].feed.loading(), "the poll waits its interval");
    let _ = app.tick_at(start + FEED_POLL * 2);
    assert!(app.tabs[i].feed.loading());
    app.on_msg(Msg::Feed {
        tab,
        generation: app.tabs[i].feed.generation,
        append: false,
        result: Ok(listing("vim", &["n", "a", "b"], None)),
    });
    assert!(app.tabs[i].feed.has_new_posts(&app.config));
    assert_eq!(app.current_post().unwrap().id, "a");

    app.tabs[i]
        .feed
        .update
        .as_mut()
        .unwrap()
        .freshness
        .fetched_at = 0;
    let _ = app.tick_at(start + FEED_POLL * 3);
    assert!(app.tabs[i].feed.loading());
    app.on_msg(Msg::Feed {
        tab,
        generation: app.tabs[i].feed.generation,
        append: false,
        result: Err("offline".into()),
    });
    let _ = app.tick_at(start + FEED_POLL * 4);
    assert!(!app.tabs[i].feed.loading(), "a failed refetch waits for r");

    let mut app = app_with(&["vim"]);
    app.switch_tab(ALL_SUBS_TAB);
    let answer = |app: &mut App, ids: &[&str]| {
        app.on_msg(Msg::Group {
            generation: app.all_subs.groups[0].generation,
            subreddit: "vim".into(),
            result: Ok(listing("vim", ids, None)),
        });
    };
    app.all_subs.groups[0].loading = true;
    answer(&mut app, &["a", "b"]);
    app.groups_drawn(&[0], &[], &[]);
    assert!(!app.all_subs.groups[0].loading);
    app.all_subs.groups[0]
        .freshness
        .as_mut()
        .unwrap()
        .fetched_at = 0;
    let _ = app.tick_at(Instant::now() + FEED_POLL);
    assert!(app.all_subs.groups[0].loading);
    answer(&mut app, &["n", "a", "b"]);
    assert!(app.all_subs.has_new_posts(&app.config));
}

#[test]
fn r_in_all_subs_applies_its_refetch_and_shows_the_top_until_you_move() {
    for moved in [false, true] {
        let mut app = app_with(&["vim", "zsh"]);
        app.switch_tab(ALL_SUBS_TAB);
        app.on_msg(island_answer(&app, &["a", "b"]));
        app.all_subs.group = 1;
        app.reload();
        app.ensure_group_loaded(0);
        if moved {
            app.move_selection(-1);
        }
        app.on_msg(island_answer(&app, &["fresh", "a", "b"]));

        assert_eq!(
            app.all_subs.has_new_posts(&app.config),
            moved,
            "moved={moved}"
        );
        if !moved {
            assert_eq!(island_ids(&app)[0], "fresh");
            assert_eq!((app.all_subs.group, app.all_subs.post), (0, 0));
        }
    }
}

#[test]
fn banner_refresh_should_apply_island_snapshots_without_refetching_or_invalidating_reads() {
    use crate::reddit::tests::{Fixture, Reply};
    use std::sync::{Arc, Mutex};
    let requests = Arc::new(Mutex::new(Vec::new()));
    let calls = requests.clone();
    let fixture = Fixture::new(move |request| {
        calls.lock().unwrap().push(request.url.clone());
        Reply::json(serde_json::from_str(include_str!("../../../tests/fixtures/sub.json")).unwrap())
    });
    let (mut app, messages) = app_at(&fixture.base, &["vim", "zsh", "zz", "zzz"]);
    app.reddit
        .listing("seed", Sort::Hot, TimeRange::Day, None)
        .unwrap();
    requests.lock().unwrap().clear();
    for name in ["vim", "zsh", "zz", "zzz"] {
        app.on_msg(Msg::Group {
            subreddit: name.into(),
            generation: 0,
            result: Ok(listing(name, &["old"], None)),
        });
    }
    for name in ["vim", "zsh"] {
        app.on_msg(Msg::Group {
            subreddit: name.into(),
            generation: 0,
            result: Ok(listing(name, &["new", "old"], None)),
        });
    }
    let inflight_generation = app.all_subs.next_group_generation;
    app.all_subs.next_group_generation += 1;
    app.all_subs.groups[2].loading = true;
    app.all_subs.groups[2].generation = inflight_generation;
    app.all_subs.groups[3].error = Some("offline".into());
    app.all_subs.error = Some("subscriptions offline".into());
    app.all_subs.group = 1;
    app.all_subs.post = 0;
    app.all_subs.offset = 1;
    app.all_subs.drawn = Some(("zsh".into(), 0));
    app.icon_demands.insert("vim".into());
    let generations: Vec<_> = app.all_subs.groups.iter().map(|g| g.generation).collect();
    let subscription_generation = app.all_subs.generation;

    app.reload();
    app.groups_drawn(&[0, 1, 2, 3], &[], &[]);

    assert!(messages.recv_timeout(Duration::from_millis(150)).is_err());
    assert!(requests.lock().unwrap().is_empty());
    assert_eq!(island_ids(&app), ["new", "old"]);
    assert_eq!(app.all_subs.groups[1].posts[0].id, "new");
    assert!(!app.all_subs.has_new_posts(&app.config));
    assert_eq!(
        (app.all_subs.group, app.all_subs.post, app.all_subs.offset),
        (0, 0, 0)
    );
    assert!(app.all_subs.drawn.is_none());
    assert_eq!(app.all_subs.generation, subscription_generation);
    assert_eq!(
        app.all_subs
            .groups
            .iter()
            .map(|g| g.generation)
            .collect::<Vec<_>>(),
        generations
    );
    assert!(
        app.all_subs
            .groups
            .iter()
            .all(|g| !g.needs_refresh && !g.manual)
    );
    assert!(app.all_subs.groups[2].loading);
    assert!(app.all_subs.groups[3].error.is_some());
    assert_eq!(app.all_subs.error.as_deref(), Some("subscriptions offline"));
    assert!(app.icon_demands.contains("vim"));

    app.on_msg(Msg::Group {
        subreddit: "zz".into(),
        generation: inflight_generation,
        result: Ok(listing("zz", &["later", "old"], None)),
    });
    assert!(app.all_subs.groups[2].arrivals.is_some());
    assert_eq!(app.all_subs.groups[2].posts[0].id, "old");
    app.reload();
    assert_eq!(app.all_subs.groups[2].posts[0].id, "later");
    assert!(!app.all_subs.has_new_posts(&app.config));
    assert!(requests.lock().unwrap().is_empty());

    app.reload();
    app.groups_drawn(&[0, 1, 2, 3], &[], &[]);
    assert!(
        messages.recv_timeout(Duration::from_secs(1)).is_ok(),
        "manual refresh: signed_out={}, loading={}, generation={}, calls={:?}",
        app.reddit.needs_sign_in(),
        app.all_subs.loading,
        app.all_subs.generation,
        requests.lock().unwrap()
    );
    assert!(!requests.lock().unwrap().is_empty());
}
