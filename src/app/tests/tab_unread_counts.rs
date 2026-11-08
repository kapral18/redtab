use super::*;

#[test]
fn navigating_a_loaded_community_never_clears_its_unread_posts() {
    let (mut app, i) = unread_badge_app();
    app.focus = Focus::Sidebar;
    app.switch_tab(0);
    app.switch_tab(i);
    app.focus = Focus::List;
    app.move_selection(1);
    app.switch_tab(0);

    assert_eq!(app.badge(&app.tabs[i]), UnreadBadge::Count(2));
    assert!(!app.state.is_read("new1"));
    assert!(!app.state.is_read("new2"));
}

#[test]
fn closing_a_tab_preserves_the_next_communitys_unread_count() {
    let (mut app, i) = unread_badge_app();
    app.open_subreddit("other");

    app.close_active_tab();

    assert_eq!(app.active, i);
    assert_eq!(app.badge(&app.tabs[i]), UnreadBadge::Count(2));
}

#[test]
fn an_unfetched_or_failed_listing_does_not_claim_zero_unread() {
    let mut app = app_with(&["s"]);
    let i = tab_index(&app, "s");
    assert_eq!(app.badge(&app.tabs[i]), UnreadBadge::Unknown);
    app.active = i;
    app.load_feed(i, false);
    let (tab, generation) = (app.tabs[i].id, app.tabs[i].feed.generation);

    app.on_msg(Msg::Feed {
        tab,
        generation,
        append: false,
        result: Err("fixture unavailable".into()),
    });

    assert_eq!(app.badge(&app.tabs[i]), UnreadBadge::Unknown);
}

#[test]
fn partial_records_union_with_known_unread_and_survive_terminal_failure() {
    let (mut app, i) = unread_badge_app();
    app.search("query".into());
    let (tab, generation) = (app.tabs[i].id, app.tabs[i].feed.generation);
    app.on_feed_progress(tab, generation, post("streamed", "s"), Source::default());
    assert_eq!(app.badge(&app.tabs[i]), UnreadBadge::Count(3));

    app.on_msg(Msg::Feed {
        tab,
        generation,
        append: false,
        result: Err("fixture unavailable".into()),
    });

    assert_eq!(app.badge(&app.tabs[i]), UnreadBadge::Count(3));
}

#[test]
fn badge_metadata_should_ignore_a_rejected_page() {
    let mut app = app_with(&["s"]);
    app.config.filters.keywords = vec!["blocked".into()];
    let i = tab_index(&app, "s");
    app.active = i;
    let (tab, generation) = (app.tabs[i].id, app.tabs[i].feed.generation);
    let mut fresh = listing("s", &["a"], None);
    fresh.posts[0].title = "blocked".into();
    app.on_feed(
        tab,
        generation,
        false,
        Ok(fresh),
        Freshness {
            fetched_at: 1000,
            source: ordered_source(2, false),
        },
        false,
    );
    assert_eq!(app.badge(&app.tabs[i]), UnreadBadge::Count(0));
    app.on_feed(
        tab,
        generation,
        false,
        Ok(listing("s", &["a"], None)),
        Freshness {
            fetched_at: 2000,
            source: ordered_source(1, false),
        },
        false,
    );
    assert_eq!(app.tabs[i].feed.posts[0].title, "blocked");
    assert_eq!(app.badge(&app.tabs[i]), UnreadBadge::Count(0));
}

#[test]
fn returning_to_hot_keeps_unread_count_while_waiting_or_after_failure() {
    let (mut app, i) = unread_badge_app();
    app.set_sort(Sort::Top);
    let (tab, generation) = (app.tabs[i].id, app.tabs[i].feed.generation);
    app.on_msg(Msg::Feed {
        tab,
        generation,
        append: false,
        result: Ok(listing("s", &["old"], None)),
    });
    app.set_sort(Sort::Hot);
    assert_eq!(app.badge(&app.tabs[i]), UnreadBadge::Count(2));

    let generation = app.tabs[i].feed.generation;
    app.on_msg(Msg::Feed {
        tab,
        generation,
        append: false,
        result: Err("fixture unavailable".into()),
    });

    assert_eq!(app.badge(&app.tabs[i]), UnreadBadge::Count(2));
}

#[test]
fn sorting_cannot_replace_newer_observations_with_older_group_counts() {
    let (mut app, i) = unread_badge_app();
    app.load_feed(i, false);
    let (tab, generation) = (app.tabs[i].id, app.tabs[i].feed.generation);
    app.on_msg(Msg::Feed {
        tab,
        generation,
        append: false,
        result: Ok(listing("s", &["old", "new1", "new2", "new3"], None)),
    });
    assert_eq!(app.badge(&app.tabs[i]), UnreadBadge::Count(3));

    app.search("some query".into());
    app.set_sort(Sort::Top);

    assert_eq!(app.badge(&app.tabs[i]), UnreadBadge::Count(3));
}
