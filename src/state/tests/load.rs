use super::*;

#[test]
fn when_older_state_has_no_view_it_should_default_to_cards() {
    let state: State = serde_json::from_str("{\"bookmarks\":[]}").unwrap();

    assert_eq!(state.list_view(), ListView::Card);
}

#[test]
fn when_a_saved_post_predates_preview_geometry_it_should_still_load() {
    let saved = post("p", "s");
    let mut value = serde_json::to_value(&saved).unwrap();
    value.as_object_mut().unwrap().remove("preview_size");

    let actual: Post = serde_json::from_value(value).unwrap();

    assert_eq!(actual, saved);
    assert_eq!(actual.preview_size, None);
}

#[test]
fn a_second_corrupt_file_keeps_the_first_backup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    std::fs::write(&path, b"first").unwrap();
    State::load(&path);
    std::fs::write(&path, b"second").unwrap();
    State::load(&path);
    let first = std::fs::read_to_string(dir.path().join("state.json.bad")).unwrap();
    let backups = std::fs::read_dir(dir.path())
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("state.json.bad")
        })
        .count();

    assert_eq!(first, "first");
    assert_eq!(backups, 2);
}

#[test]
fn repeated_corrupt_state_recovery_should_keep_every_backup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    for text in ["first", "second", "third", "fourth"] {
        std::fs::write(&path, text).unwrap();
        let (_, warning) = State::load(&path);
        assert!(warning.unwrap().contains("moved to"));
    }
    let mut backups: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("state.json.bad")
        })
        .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect();
    backups.sort();

    assert_eq!(backups, ["first", "fourth", "second", "third"]);
}

#[test]
fn a_file_corrupted_during_the_session_is_not_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let mut state = State::default();
    state.save(&path, 1).unwrap();
    std::fs::write(&path, b"{\"bookmarks\":[{\"post\":1}]}").unwrap();

    let saved = state.save(&path, 2);
    let on_disk = std::fs::read_to_string(&path).unwrap();

    assert!(saved.is_err());
    assert_eq!(on_disk, "{\"bookmarks\":[{\"post\":1}]}");
}

#[test]
fn corrupt_state_is_moved_aside() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    std::fs::write(&path, b"\xff\xfe not json").unwrap();

    let (state, warning) = State::load(&path);
    let moved = dir.path().join("state.json.bad").exists();

    assert!(state.bookmarks().is_empty());
    assert!(warning.unwrap().contains("moved to"));
    assert!(moved);
}

#[test]
fn unreadable_state_is_never_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    std::fs::create_dir(&path).unwrap(); // reading a directory fails

    let (mut state, warning) = State::load(&path);
    state.toggle_bookmark(&post("a", "s"), 1);
    let saved = state.save(&path, 1);
    let still_dir = path.is_dir();

    assert!(warning.unwrap().contains("will not save"));
    assert!(saved.is_err(), "a skipped save must not report success");
    assert!(still_dir);
}

#[test]
fn a_dangling_or_corrupt_symlinked_state_file_keeps_the_link() {
    let dir = tempfile::tempdir().unwrap();
    let link = dir.path().join("state.json");
    std::os::unix::fs::symlink("synced.json", &link).unwrap();
    let mut state = State::default();
    state.toggle_bookmark(&post("a", "s"), 1);
    state.save(&link, 1).unwrap();
    let dangling_kept = std::fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink();
    let target_written = dir.path().join("synced.json").exists();

    std::fs::write(dir.path().join("synced.json"), b"garbage").unwrap();
    State::load(&link);
    let corrupt_kept = std::fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink();
    let backup = std::fs::read_to_string(dir.path().join("synced.json.bad")).ok();

    assert!(dangling_kept && target_written);
    assert!(corrupt_kept, "the link itself was moved aside");
    assert_eq!(backup.as_deref(), Some("garbage"));
}
