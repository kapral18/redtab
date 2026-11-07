use super::*;

#[test]
fn when_congestion_subscription_icon_is_removed_and_readded_metadata_should_follow_order() {
    let mut app = app_with(&[]);
    show_thumbnails(&mut app);
    let events = [
        (
            1,
            crate::model::SubscriptionIcon::Url("https://i.redd.it/old.png".into()),
            Some("https://i.redd.it/old.png"),
        ),
        (2, crate::model::SubscriptionIcon::Missing, None),
        (
            3,
            crate::model::SubscriptionIcon::Url("https://i.redd.it/new.png".into()),
            Some("https://i.redd.it/new.png"),
        ),
    ];
    for (sequence, icon, expected) in events {
        app.on_subscriptions(
            app.all_subs.generation,
            Ok(vec![Subscription::Community {
                name: "Rust".into(),
                icon,
            }]),
            Freshness {
                fetched_at: now(),
                source: ordered_source(sequence, false),
            },
            false,
        );
        app.on_result(
            Msg::Icon {
                subreddit: "rust".into(),
                result: Ok(None),
            },
            ordered_source(0, true),
            false,
        );
        assert_eq!(app.shown_icon("rust"), expected);
        app.demand_icon("Rust");
        assert!(!app.icon_lookups.contains("rust"));
    }
}

#[test]
fn icon_lookup_should_reuse_known_or_pending_metadata_and_allow_retry_after_failure() {
    let (mut app, messages) = app_with_messages(&[]);
    show_thumbnails(&mut app);
    // A new lookup joins `icon_lookups` and its answer arrives; a pending one must not
    // be asked again, which only an answer would show.
    let load = |app: &mut App| {
        let pending = app.icon_lookups.contains("s");
        app.request_icon("S");
        if pending {
            return usize::from(
                messages
                    .recv_timeout(std::time::Duration::from_millis(200))
                    .is_ok(),
            );
        }
        if !app.icon_lookups.contains("s") {
            return 0;
        }
        loop {
            let msg = messages
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            let answered = matches!(&msg, Msg::Icon { subreddit, .. } if subreddit == "s");
            app.on_msg(msg);
            if answered {
                return 1;
            }
        }
    };

    let first = load(&mut app);
    let after_failure = load(&mut app);
    app.observe_icon("s", Some("/icon/s.png".into()), Source::default());
    let known = load(&mut app);
    app.icons.remove("s");
    app.icon_lookups.insert("s".into());
    let pending = load(&mut app);
    // A stale disk answer, then its refresh fails: the answer stands.
    app.icon_lookups.clear();
    app.on_msg(Msg::Icon {
        subreddit: "s".into(),
        result: Ok(None),
    });
    app.on_msg(Msg::Icon {
        subreddit: "s".into(),
        result: Err("offline".into()),
    });
    let stale = load(&mut app);

    assert_eq!(
        (first, after_failure, known, pending, stale),
        (1, 1, 0, 0, 0)
    );
}

#[test]
fn a_multireddit_asks_for_no_icon() {
    let (mut app, _messages) = app_with_messages(&[]);
    show_thumbnails(&mut app);

    app.request_icon("a+B");

    assert!(app.icon_lookups.is_empty());
}

#[test]
fn sidebar_should_keep_an_icon_lookup_alive_after_leaving_its_feed() {
    use crate::reddit::tests::{Fixture, Reply};
    let (started, began) = std::sync::mpsc::channel();
    let (finish, wait) = std::sync::mpsc::channel();
    let fixture = Fixture::new(move |request| {
        if request.url.starts_with("/r/blocker/about.json") {
            started.send(()).unwrap();
            wait.recv_timeout(Duration::from_secs(3)).unwrap();
            Reply::json(serde_json::json!({"data":{"icon_img":"/icon/s.png"}}))
        } else if request.url.starts_with("/r/s/about.json") {
            Reply::json(serde_json::json!({"data":{"icon_img":"/icon/s.png"}}))
        } else {
            Reply::json(serde_json::json!({"kind":"Listing","data":{"children":[],"after":null}}))
        }
    });
    let (mut app, messages) = app_at(&fixture.base, &["blocker", "s"]);
    show_thumbnails(&mut app);
    app.active = 4;
    app.groups_drawn(&[], &[], &[]);
    app.demand_icon("blocker");
    began.recv_timeout(Duration::from_secs(2)).unwrap();
    app.active = 5;
    app.groups_drawn(&[], &[], &[]);
    app.demand_icon("s");
    app.active = 0;
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 20)).unwrap();
    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    finish.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while app.icon_lookups.contains("s") {
        let message = messages
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap();
        app.on_msg(message);
    }
    assert_eq!(app.shown_icon("s"), Some("/icon/s.png"));
}

#[test]
fn when_a_cached_community_is_shown_its_missing_icon_should_load_once_on_demand() {
    for group in [false, true] {
        let (mut app, messages) = app_with_messages(&["s"]);
        show_thumbnails(&mut app);
        app.active = 0;
        let freshness = Freshness {
            fetched_at: now(),
            source: Source {
                cached: true,
                ..Default::default()
            },
        };
        let feed = &mut app.tabs[4].feed;
        feed.posts = vec![post("cached", "s")];
        feed.loaded = true;
        feed.freshness = Some(freshness);
        feed.shown_query = Some(feed.query());
        app.all_subs.groups[0].freshness = Some(freshness);
        let demand = |app: &mut App| {
            if group {
                app.ensure_group_loaded(0);
            } else {
                app.ensure_feed_loaded(4);
            }
        };
        if group {
            app.active = ALL_SUBS_TAB;
        } else {
            app.active = 4;
        }
        demand(&mut app);
        let icon = messages
            .recv_timeout(Duration::from_secs(1))
            .expect("visible missing icon should start");
        assert!(matches!(&icon, Msg::Icon { subreddit, .. } if subreddit == "s"));
        app.on_msg(icon);
        demand(&mut app);
        assert!(
            messages.recv_timeout(Duration::from_millis(100)).is_err(),
            "failed icons must not retry every draw"
        );
        let live = Freshness {
            fetched_at: now(),
            source: Source::default(),
        };
        if group {
            app.on_group(
                "s",
                app.all_subs.groups[0].generation,
                Ok(listing("s", &["live"], None)),
                live,
                false,
            );
        } else {
            app.on_feed(
                app.tabs[4].id,
                app.tabs[4].feed.generation,
                false,
                Ok(listing("s", &["live"], None)),
                live,
                false,
            );
        }
        assert!(
            messages.recv_timeout(Duration::from_millis(100)).is_err(),
            "a successful listing must not retry its failed icon in the same demand"
        );
        app.active = 0;
        app.groups_drawn(&[], &[], &[]);
        app.active = if group { ALL_SUBS_TAB } else { 4 };
        if group {
            app.groups_drawn(&[0], &[], &[]);
        } else {
            app.groups_drawn(&[], &[], &[]);
            demand(&mut app);
        }
        assert!(
            matches!(
                messages.recv_timeout(Duration::from_secs(1)).unwrap(),
                Msg::Icon { .. }
            ),
            "returning to a visible community should permit another lookup"
        );
    }
}

#[test]
fn when_congestion_membership_icons_arrive_visible_pixels_should_load_without_metadata() {
    let mut app = app_with(&[]);
    show_thumbnails(&mut app);
    let entries: Vec<Subscription> = serde_json::from_value(serde_json::json!([
        {"community":{"name":"rust","icon":{"url":"https://i.redd.it/rust.png"}}},
        {"community":{"name":"vim","icon":"missing"}}
    ]))
    .unwrap();
    app.on_subscriptions(
        app.all_subs.generation,
        Ok(entries),
        Freshness {
            fetched_at: now(),
            source: ordered_source(2, false),
        },
        false,
    );
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(110, 20)).unwrap();

    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();

    assert!(app.thumbs.contains_key("https://i.redd.it/rust.png"));
    assert!(!app.icon_lookups.contains("rust"));
    assert!(!app.icon_lookups.contains("vim"));
    assert_eq!(app.shown_icon("rust"), Some("https://i.redd.it/rust.png"));
}

#[test]
fn when_congestion_icon_snapshots_arrive_late_the_newer_metadata_should_remain() {
    let mut app = app_with(&[]);
    show_thumbnails(&mut app);
    app.on_result(
        Msg::Icon {
            subreddit: "Rust".into(),
            result: Ok(Some("https://i.redd.it/new.png".into())),
        },
        ordered_source(2, false),
        false,
    );

    app.on_result(
        Msg::Icon {
            subreddit: "rust".into(),
            result: Ok(None),
        },
        ordered_source(1, true),
        false,
    );

    assert_eq!(app.shown_icon("RUST"), Some("https://i.redd.it/new.png"));
}

#[test]
fn a_feed_looks_its_icon_up_on_its_first_page_only() {
    let (mut app, _messages) = app_with_messages(&["s"]);
    show_thumbnails(&mut app);
    let (tab, generation) = (app.tabs[4].id, app.tabs[4].feed.generation);
    let load = |app: &mut App, append: bool| {
        app.icons.remove("s");
        app.icon_lookups.clear();
        app.on_msg(Msg::Feed {
            tab,
            generation,
            append,
            result: Ok(listing("s", &["a"], Some("t3_a"))),
        });
        usize::from(app.icon_lookups.contains("s"))
    };

    let next_page = load(&mut app, true);
    let first_page = load(&mut app, false);

    assert_eq!((next_page, first_page), (0, 1));
}

#[test]
fn icons_are_looked_up_only_while_thumbnails_show() {
    let (mut app, _messages) = app_with_messages(&[]);
    app.all_subs.groups = vec![Group {
        subreddit: "s".into(),
        ..Group::default()
    }];
    let asked = |app: &App| usize::from(!app.icon_lookups.is_empty());
    show_thumbnails(&mut app);
    app.show_images = false;

    app.on_msg(Msg::Group {
        generation: 0,
        subreddit: "s".into(),
        result: Ok(listing("s", &["a"], None)),
    });
    let while_off = asked(&app);
    app.toggle_images();
    let turned_on = asked(&app);

    assert_eq!((while_off, turned_on), (0, 1));
}
