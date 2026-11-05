use super::*;

#[test]
fn fetched_posts_remain_unread_when_later_listings_omit_them() {
    let mut state = State::default();
    let config = Config::default();
    state.observe(
        Some("Neovim"),
        &[post("a", "Neovim"), post("b", "Neovim")],
        cached(100),
        100,
    );
    assert_eq!(state.unread("neovim", &config), Some(2));

    state.observe(
        Some("neovim"),
        &[post("c", "neovim"), post("a", "neovim")],
        cached(200),
        200,
    );

    assert_eq!(state.unread("NEOVIM", &config), Some(3));
}

#[test]
fn reading_a_post_clears_only_that_post() {
    let mut state = State::default();
    state.observe(
        Some("vim"),
        &[post("x", "vim"), post("y", "vim")],
        cached(1),
        1,
    );
    assert_eq!(state.unread("vim", &Config::default()), Some(2));

    state.mark_read(&post("x", "vim"), 5);

    assert!(state.is_read("x"));
    assert!(!state.is_read("y"));
    assert_eq!(state.unread("vim", &Config::default()), Some(1));
}

#[test]
fn save_round_trips_and_prunes_stale_marks_but_keeps_bookmarks() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let mut state = State::default();
    let old = post("old", "s");
    state.mark_read(&old, 0);
    state.toggle_bookmark(&old, 0);
    state.mark_read(&post("new", "s"), RETENTION_SECS + 10);

    state.save(&path, RETENTION_SECS + 20).unwrap();
    let (loaded, warning) = State::load(&path);

    assert!(warning.is_none());
    assert!(!loaded.is_read("old"));
    assert!(loaded.is_read("new"));
    assert!(loaded.is_bookmarked("old"));
}

#[test]
fn observing_a_read_pinned_post_keeps_its_read_mark_from_expiring() {
    let mut state = State::default();
    state.mark_read(&post("pinned", "s"), 0);
    state.observe(
        Some("s"),
        &[post("pinned", "s")],
        cached(RETENTION_SECS),
        RETENTION_SECS,
    );

    state.prune(RETENTION_SECS + 10);

    assert!(state.is_read("pinned"));
    assert_eq!(state.unread("s", &Config::default()), Some(0));
}

#[test]
fn reading_before_observation_never_resurrects_an_unread_post() {
    let mut state = State::default();
    state.mark_read(&post("x", "pics"), 1);
    state.mark_read(&post("y", "pics"), 1);

    state.observe(
        Some("pics"),
        &[post("a", "pics"), post("b", "pics"), post("x", "pics")],
        cached(2),
        2,
    );
    state.observe(Some("pics"), &[post("y", "pics")], cached(3), 3);

    assert_eq!(state.unread("pics", &Config::default()), Some(2));
    assert!(state.is_read("x"));
    assert!(state.is_read("y"));
}

#[test]
fn unread_distinguishes_unknown_from_a_successful_empty_listing() {
    let mut state = State::default();
    let config = Config::default();
    assert_eq!(state.unread("s", &config), None);
    state.observe(Some("s"), &[], cached(1), 1);
    assert_eq!(state.unread("S", &config), Some(0));
    state.mark_read(&post("read", "other"), 2);
    assert_eq!(state.unread("other", &config), Some(0));
}

#[test]
fn legacy_visit_marks_do_not_hide_unopened_posts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    std::fs::write(
        &path,
        r#"{"seen":{"s":{"opened":100,"unopened":100}},"read":{"opened":100}}"#,
    )
    .unwrap();
    let (mut state, warning) = State::load(&path);
    assert!(warning.is_none());

    state.observe(
        Some("s"),
        &[post("opened", "s"), post("unopened", "s")],
        cached(101),
        101,
    );
    state.save(&path, 101).unwrap();

    assert_eq!(state.unread("s", &Config::default()), Some(1));
    assert!(state.is_read("opened"));
    assert!(!state.is_read("unopened"));
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert!(saved.get("seen").is_none());
    assert!(saved.get("unread_counts").is_none());
    assert!(saved.get("read_revision").is_none());
}

#[test]
fn unread_filter_cache_tracks_both_directions_of_every_filter() {
    let mut state = State::default();
    let mut title = post("title", "s");
    title.title = "Blocked title".into();
    let mut author = post("author", "s");
    author.author = "FilteredAuthor".into();
    let mut flair = post("flair", "s");
    flair.flair = Some("FilteredFlair".into());
    let mut adult = post("adult", "s");
    adult.nsfw = true;
    state.observe(Some("s"), &[title, author, flair, adult], cached(1), 1);
    let mut config = Config::default();
    assert_eq!(state.unread("s", &config), Some(3));
    config.show_nsfw = true;
    assert_eq!(state.unread("s", &config), Some(4));
    config.filters.keywords = vec!["BLOCKED".into()];
    assert_eq!(state.unread("s", &config), Some(3));
    config.filters.keywords.clear();
    config.filters.authors = vec!["filteredauthor".into()];
    assert_eq!(state.unread("s", &config), Some(3));
    config.filters.authors.clear();
    config.filters.flairs = vec!["filteredflair".into()];
    assert_eq!(state.unread("s", &config), Some(3));
    config.filters.flairs.clear();
    config.show_nsfw = false;
    assert_eq!(state.unread("s", &config), Some(3));
}

#[test]
fn merging_an_older_instance_preserves_reads_and_new_observations() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let mut first = State::default();
    first.observe(
        Some("s"),
        &[post("a", "s"), post("b", "s")],
        cached(100),
        100,
    );
    first.save(&path, 100).unwrap();
    let (mut second, _) = State::load(&path);
    assert_eq!(second.unread("s", &Config::default()), Some(2));
    first.mark_read(&post("a", "s"), 101);
    first.save(&path, 101).unwrap();
    second.observe(
        Some("s"),
        &[post("a", "s"), post("c", "s")],
        cached(100),
        102,
    );

    second.save(&path, 103).unwrap();
    first.save(&path, 104).unwrap();

    assert!(second.is_read("a"));
    assert_eq!(second.unread("s", &Config::default()), Some(2));
    assert_eq!(first.unread("s", &Config::default()), Some(2));
    assert_eq!(
        State::load(&path).0.unread("s", &Config::default()),
        Some(2)
    );
}

#[test]
fn older_cached_metadata_cannot_replace_a_newer_filter_observation() {
    let mut state = State::default();
    let safe = post("a", "s");
    let mut adult = safe.clone();
    adult.nsfw = true;
    let config = Config::default();
    state.observe(Some("s"), &[safe], cached(200), 200);
    assert_eq!(state.unread("s", &config), Some(1));

    state.observe(Some("s"), std::slice::from_ref(&adult), cached(100), 300);
    assert_eq!(state.unread("s", &config), Some(1));
    state.observe(Some("s"), &[adult], cached(400), 400);
    assert_eq!(state.unread("s", &config), Some(0));
}

#[test]
fn unread_history_prunes_only_expired_observations() {
    let mut state = State::default();
    state.observe(Some("expired"), &[post("old", "expired")], cached(0), 0);
    state.observe(
        Some("recent"),
        &[post("new", "recent")],
        cached(RETENTION_SECS),
        RETENTION_SECS,
    );

    state.prune(RETENTION_SECS + 1);

    assert_eq!(state.unread("expired", &Config::default()), None);
    assert_eq!(state.unread("recent", &Config::default()), Some(1));
    assert!(!state.is_read("new"));
}

#[test]
fn duplicate_observations_and_global_read_ids_keep_cached_counts_consistent() {
    let mut state = State::default();
    let config = Config::default();
    state.observe(
        None,
        &[post("a", "s"), post("a", "s"), post("a", "other")],
        cached(1),
        1,
    );
    assert_eq!(state.unread("s", &config), Some(1));
    assert_eq!(state.unread("other", &config), Some(1));

    state.mark_read(&post("a", "s"), 2);
    state.observe(None, &[post("a", "s"), post("a", "other")], cached(3), 3);

    assert_eq!(state.unread("s", &config), Some(0));
    assert_eq!(state.unread("other", &config), Some(0));
}

#[test]
fn observation_metadata_keeps_request_order_across_save_merge_and_clock_rollback() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let mut newest = State::default();
    newest.observe(
        Some("s"),
        &[post("a", "s")],
        ordered_freshness(2, 1000, false),
        1000,
    );
    newest.save(&path, 1000).unwrap();
    let mut old = State::default();
    let mut adult = post("a", "s");
    adult.nsfw = true;
    old.observe(
        Some("s"),
        std::slice::from_ref(&adult),
        ordered_freshness(1, 2000, false),
        2000,
    );
    old.save(&path, 2000).unwrap();
    assert_eq!(old.unread("s", &Config::default()), Some(1));
    newest.observe(Some("s"), &[adult], ordered_freshness(3, 900, false), 2001);
    newest.save(&path, 2001).unwrap();
    old.save(&path, 2002).unwrap();
    assert_eq!(old.unread("s", &Config::default()), Some(0));
    let (loaded, warning) = State::load(&path);
    assert!(warning.is_none());
    assert_eq!(loaded.unread("s", &Config::default()), Some(0));
    let record = &loaded.observed["s"].posts["a"];
    assert_eq!(record.source.order.unwrap().sequence, 3);
    assert!(
        record.source.observed.is_none(),
        "process-local events never persist"
    );
}

#[test]
fn unordered_cached_metadata_cannot_replace_a_current_live_observation() {
    let mut state = State::default();
    let mut adult = post("a", "s");
    adult.nsfw = true;
    state.observe(Some("s"), &[adult], ordered_freshness(3, 900, false), 1000);
    let disk = Freshness {
        fetched_at: 2000,
        source: Source {
            cached: true,
            observed: Some((10, 11)),
            ..Source::default()
        },
    };
    state.observe(Some("s"), &[post("a", "s")], disk, 1001);
    assert_eq!(state.unread("s", &Config::default()), Some(0));
}

#[test]
fn merging_an_incomparable_disk_source_cannot_roll_back_a_current_live_observation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let mut state = State::default();
    let mut old = post("a", "s");
    old.nsfw = true;
    state.observe(Some("s"), &[old], ordered_freshness(1, 1000, false), 1000);
    state.save(&path, 1000).unwrap();
    let mut new_source = ordered_freshness(2, 900, false);
    new_source.source.order.as_mut().unwrap().generation = [2; 16];
    state.observe(Some("s"), &[post("a", "s")], new_source, 1001);
    assert_eq!(state.unread("s", &Config::default()), Some(1));
    state.save(&path, 1001).unwrap();
    assert_eq!(state.unread("s", &Config::default()), Some(1));
}

#[test]
fn equivalent_disk_snapshots_cannot_discard_live_metadata_ordering() {
    for save in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let mut state = State::default();
        let mut adult = post("a", "s");
        adult.nsfw = true;
        state.observe(
            Some("s"),
            std::slice::from_ref(&adult),
            ordered_freshness(3, 900, false),
            1000,
        );
        if save {
            state.save(&path, 1000).unwrap();
            state.save(&path, 1001).unwrap();
        } else {
            state.observe(Some("s"), &[adult], ordered_freshness(3, 900, true), 1001);
        }
        let old_disk = Freshness {
            fetched_at: 2000,
            source: Source {
                cached: true,
                observed: Some((10, 11)),
                ..Source::default()
            },
        };
        state.observe(Some("s"), &[post("a", "s")], old_disk, 1002);
        assert_eq!(
            state.unread("s", &Config::default()),
            Some(0),
            "save={save}"
        );
    }
}

#[test]
fn marking_a_community_read_should_clear_filtered_ids_and_keep_later_arrivals_unread() {
    let mut state = State::default();
    let mut adult = post("adult", "s");
    adult.nsfw = true;
    state.observe(Some("s"), &[post("a", "s"), adult], cached(100), 100);
    state.observe(Some("other"), &[post("other", "other")], cached(100), 100);
    assert_eq!(state.unread("s", &Config::default()), Some(1));

    let actual = state.mark_community_read("S", 101);

    assert_eq!(actual, 2);
    assert!(state.is_read("a") && state.is_read("adult"));
    assert_eq!(state.unread("s", &Config::default()), Some(0));
    assert_eq!(state.unread("other", &Config::default()), Some(1));
    state.observe(
        Some("s"),
        &[post("a", "s"), post("new", "s")],
        cached(102),
        102,
    );
    assert_eq!(state.unread("s", &Config::default()), Some(1));
    assert_eq!(state.mark_community_read("s", 103), 1);
    assert_eq!(state.mark_community_read("s", 104), 0);
}

#[test]
fn community_read_marks_should_survive_an_older_instances_save_with_new_arrivals() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let mut first = State::default();
    first.observe(
        Some("s"),
        &[post("a", "s"), post("b", "s")],
        cached(100),
        100,
    );
    first.save(&path, 100).unwrap();
    let (mut second, _) = State::load(&path);
    first.mark_community_read("s", 101);
    first.save(&path, 101).unwrap();
    second.observe(
        Some("s"),
        &[post("a", "s"), post("c", "s")],
        cached(102),
        102,
    );

    second.save(&path, 102).unwrap();

    let (actual, warning) = State::load(&path);
    assert!(warning.is_none());
    assert!(actual.is_read("a") && actual.is_read("b"));
    assert!(!actual.is_read("c"));
    assert_eq!(actual.unread("s", &Config::default()), Some(1));
}

#[test]
fn marking_an_unfetched_community_read_should_preserve_unknown() {
    let mut state = State::default();

    let actual = state.mark_community_read("unknown", 100);

    assert_eq!(actual, 0);
    assert_eq!(state.unread("unknown", &Config::default()), None);
}

#[test]
fn profile_owned_posts_should_record_reads_without_creating_a_fake_community_badge() {
    let mut state = State::default();
    let mut profile = post("p", "u_blackvault");
    profile.permalink = "/user/blackvault/comments/p/title/".into();
    state.observe(None, std::slice::from_ref(&profile), cached(100), 100);
    assert_eq!(state.unread("u_blackvault", &Config::default()), None);
    state.mark_read(&profile, 101);
    assert!(state.is_read("p"));
    assert_eq!(state.unread("u_blackvault", &Config::default()), None);
}
