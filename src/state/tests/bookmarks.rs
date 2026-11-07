use super::*;

#[test]
fn when_a_bookmark_has_preview_geometry_it_should_survive_state_serialization() {
    let mut saved = post("p", "s");
    saved.preview_size = Some((320, 640));
    let mut state = State::default();
    state.toggle_bookmark(&saved, 1);

    let actual: State = serde_json::from_value(serde_json::to_value(state).unwrap()).unwrap();

    assert_eq!(actual.bookmarks()[0].post.preview_size, Some((320, 640)));
}

#[test]
fn bookmarks_toggle_newest_first() {
    let mut state = State::default();

    assert!(state.toggle_bookmark(&post("a", "s"), 1));
    assert!(state.toggle_bookmark(&post("b", "s"), 2));
    assert_eq!(state.bookmarks()[0].post.id, "b");
    assert!(!state.toggle_bookmark(&post("a", "s"), 3));

    assert!(!state.is_bookmarked("a"));
    assert!(state.is_bookmarked("b"));
}

#[test]
fn two_instances_keep_each_others_bookmarks_and_removals() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let mut first = State::default();
    first.toggle_bookmark(&post("keep", "s"), 1);
    first.toggle_bookmark(&post("drop", "s"), 2);
    first.save(&path, 10).unwrap();

    let (mut second, _) = State::load(&path);
    first.toggle_bookmark(&post("mine", "s"), 3);
    second.toggle_bookmark(&post("theirs", "s"), 4);
    second.toggle_bookmark(&post("drop", "s"), 5);
    second.save(&path, 10).unwrap();
    first.save(&path, 10).unwrap();
    let (merged, _) = State::load(&path);

    let ids: Vec<&str> = merged
        .bookmarks()
        .iter()
        .map(|b| b.post.id.as_str())
        .collect();
    assert!(
        ids.contains(&"keep") && ids.contains(&"mine") && ids.contains(&"theirs"),
        "{ids:?}"
    );
    // `second` removed "drop" after `first` saved it; `first` still had it in
    // memory, but the newer removal wins no matter which instance saves last.
    assert!(!ids.contains(&"drop"), "{ids:?}");
}

#[test]
fn re_adding_a_removed_bookmark_in_the_same_second_keeps_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let mut state = State::default();
    let p = post("x", "s");
    state.toggle_bookmark(&p, 7);
    state.toggle_bookmark(&p, 7);
    state.save(&path, 7).unwrap();
    state.toggle_bookmark(&p, 7);
    state.save(&path, 7).unwrap();
    let (loaded, _) = State::load(&path);

    assert!(loaded.is_bookmarked("x"));
}

#[test]
fn a_spoiler_tag_learned_by_one_instance_survives_the_others_save() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let mut a = State::default();
    a.toggle_bookmark(&post("p", "s"), 5);
    a.save(&path, 5).unwrap();
    let (mut b, _) = State::load(&path);
    a.mark_bookmark_spoiler("p");

    a.save(&path, 6).unwrap();
    b.save(&path, 7).unwrap();
    let (loaded, _) = State::load(&path);

    assert!(loaded.bookmarks()[0].post.spoiler);
}

#[test]
fn when_an_original_deletion_is_learned_it_should_survive_another_instances_save() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let mut a = State::default();
    a.toggle_bookmark(&post("p", "s"), 5);
    a.save(&path, 5).unwrap();
    let (mut b, _) = State::load(&path);
    a.mark_original_deleted("p", 6);
    a.save(&path, 6).unwrap();

    b.save(&path, 7).unwrap();
    let (loaded, _) = State::load(&path);

    assert!(loaded.bookmarks()[0].post.original_deleted);
    assert!(loaded.original_deleted("p"));
}

#[test]
fn when_a_known_deleted_original_is_bookmarked_again_its_saved_flag_should_persist() {
    let mut state = State::default();
    state.mark_original_deleted("p", 1);
    state.toggle_bookmark(&post("p", "s"), 2);
    state.toggle_bookmark(&post("p", "s"), 3);

    state.toggle_bookmark(&post("p", "s"), 4);

    assert!(state.bookmarks()[0].post.original_deleted);
}

#[test]
fn when_original_deletion_observations_age_out_only_older_than_ninety_days_should_expire() {
    let mut state = State::default();
    state.mark_original_deleted("old", 1);
    state.mark_original_deleted("recent", 2);

    state.prune(7_776_002);

    assert!(!state.original_deleted("old"));
    assert!(state.original_deleted("recent"));
}

#[test]
fn bookmark_deletion_should_survive_a_stale_readd_saved_by_another_instance() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let mut a = State::default();
    a.toggle_bookmark(&post("p", "s"), 5);
    a.save(&path, 5).unwrap();
    let (mut b, _) = State::load(&path);
    a.mark_original_deleted("p", 6);
    a.save(&path, 6).unwrap();
    b.toggle_bookmark(&post("p", "s"), 7);
    b.toggle_bookmark(&post("p", "s"), 8);

    b.save(&path, 7_776_009).unwrap();
    let (loaded, _) = State::load(&path);

    assert!(loaded.original_deleted("p"));
    assert!(loaded.bookmarks()[0].post.original_deleted);
}

#[test]
fn bookmark_deletion_should_survive_remove_save_readd_after_observation_expiry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let mut state = State::default();
    let mut deleted = post("p", "s");
    deleted.original_deleted = true;
    state.toggle_bookmark(&deleted, 5);
    state.save(&path, 7_776_010).unwrap();
    assert!(!state.deleted_originals.contains_key("p"));
    state.toggle_bookmark(&post("p", "s"), 7_776_011);
    state.save(&path, 7_776_011).unwrap();

    state.toggle_bookmark(&post("p", "s"), 7_776_012);
    state.save(&path, 7_776_012).unwrap();
    let (loaded, _) = State::load(&path);

    assert!(loaded.bookmarks()[0].post.original_deleted);
    assert!(loaded.original_deleted("p"));
}

#[test]
fn bookmark_deletion_should_survive_stale_instance_removal_of_disk_only_knowledge() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let mut stale = State::default();
    stale.toggle_bookmark(&post("p", "s"), 1);
    let mut disk = State::default();
    let mut deleted = post("p", "s");
    deleted.original_deleted = true;
    disk.toggle_bookmark(&deleted, 5);
    disk.save(&path, 7_776_010).unwrap();
    assert!(!disk.deleted_originals.contains_key("p"));
    stale.toggle_bookmark(&post("p", "s"), 7_776_011);
    stale.save(&path, 7_776_011).unwrap();

    stale.toggle_bookmark(&post("p", "s"), 7_776_012);
    stale.save(&path, 7_776_012).unwrap();
    let (loaded, _) = State::load(&path);

    assert!(loaded.bookmarks()[0].post.original_deleted);
    assert!(loaded.original_deleted("p"));
}

#[test]
fn simultaneous_saves_from_two_instances_lose_no_bookmark() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let savers: Vec<_> = ["a", "b"]
        .into_iter()
        .map(|who| {
            let path = path.clone();
            std::thread::spawn(move || {
                let mut state = State::default();
                for i in 0..20 {
                    state.toggle_bookmark(&post(&format!("{who}{i}"), "s"), 1);
                    state.save(&path, 1).unwrap();
                }
            })
        })
        .collect();
    for s in savers {
        s.join().unwrap();
    }

    let (loaded, _) = State::load(&path);
    let lost: Vec<String> = ["a", "b"]
        .iter()
        .flat_map(|who| (0..20).map(move |i| format!("{who}{i}")))
        .filter(|id| !loaded.is_bookmarked(id))
        .collect();

    assert!(lost.is_empty(), "lost {} bookmarks: {lost:?}", lost.len());
}

#[test]
fn bookmark_deletion_should_survive_both_timestamp_orders_without_an_observation_map() {
    for (mine_saved, mine_deleted, other_saved, other_deleted) in
        [(5, true, 8, false), (8, false, 5, true)]
    {
        let mut mine = State::default();
        let mut deleted = post("p", "s");
        deleted.original_deleted = mine_deleted;
        mine.toggle_bookmark(&deleted, mine_saved);
        let mut other = State::default();
        let mut other_post = post("p", "s");
        other_post.original_deleted = other_deleted;
        other.toggle_bookmark(&other_post, other_saved);
        mine.prune(7_776_010);
        other.prune(7_776_010);

        mine.merge(other, 7_776_010);

        assert!(mine.bookmarks()[0].post.original_deleted);
        assert_eq!(mine.bookmarks()[0].saved_at, 8);
    }
}
