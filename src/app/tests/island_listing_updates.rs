use super::*;

#[test]
fn when_subscriptions_reorder_it_should_keep_the_selected_island() {
    let mut app = app_with(&["rust", "vim"]);
    app.all_subs.group = 1;
    app.on_msg(Msg::Subscriptions {
        generation: 0,
        result: Ok((vec!["neovim".into(), "rust".into(), "vim".into()])
            .into_iter()
            .map(|name| crate::model::Subscription::Community {
                name,
                icon: crate::model::SubscriptionIcon::Unknown,
            })
            .collect()),
    });
    assert_eq!(app.all_subs.groups[app.all_subs.group].subreddit, "vim");
    assert_eq!(app.active, ALL_SUBS_TAB);
}

#[test]
fn an_older_island_listing_cannot_replace_a_newer_one() {
    let mut app = app_with(&["vim"]);
    let generation = app.all_subs.groups[0].generation;
    let group = |ids: &[&str]| Msg::Group {
        subreddit: "vim".into(),
        generation,
        result: Ok(listing("vim", ids, None)),
    };
    app.on_msg(ordered_snapshot(group(&["newer"]), 2, false, false));
    let accepted = app.all_subs.groups[0].freshness.unwrap();
    app.on_msg(ordered_snapshot(group(&["older"]), 1, true, false));

    assert_eq!(app.all_subs.groups[0].posts[0].id, "newer");
    assert_eq!(
        app.all_subs.groups[0].freshness.unwrap().source,
        accepted.source
    );
}

#[test]
fn failed_group_snapshots_should_keep_the_cached_content_age() {
    for cached in [false, true] {
        for error in [
            FeedError::Other("connection failed".into()),
            FeedError::Access(AccessDenied::PrivateCommunity),
        ] {
            let mut app = app_with(&["vim"]);
            let generation = app.all_subs.groups[0].generation;
            if cached {
                app.on_msg(ordered_snapshot(
                    Msg::Group {
                        subreddit: "vim".into(),
                        generation,
                        result: Ok(listing("vim", &["cached"], None)),
                    },
                    1,
                    true,
                    false,
                ));
            }
            app.on_msg(Msg::Snapshot {
                message: Box::new(Msg::Group {
                    subreddit: "vim".into(),
                    generation,
                    result: Err(error),
                }),
                fetched_at: 2000,
                refreshing: false,
                source: ordered_source(2, false),
            });

            assert_eq!(
                app.all_subs.groups[0].freshness.map(|f| f.fetched_at),
                cached.then_some(1000)
            );
            assert!(!app.all_subs.groups[0].loading);
        }
    }
}
