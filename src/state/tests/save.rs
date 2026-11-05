use super::*;

#[test]
fn when_both_instances_change_view_the_latest_choice_should_survive_a_symlink_save() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let link = dir.path().join("linked.json");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    let mut first = State::default();
    first.toggle_list_view();
    first.save(&link, 1).unwrap();
    let (mut second, _) = State::load(&link);
    first.toggle_list_view();
    first.toggle_list_view();
    second.toggle_list_view();
    second.list_view_changed_at = first.list_view_changed_at + 1;
    second.save(&link, 2).unwrap();

    first.save(&link, 3).unwrap();
    let (saved, warning) = State::load(&path);
    assert!(link.is_symlink());
    assert!(warning.is_none());
    assert_eq!(saved.list_view(), ListView::Card);
}

#[test]
fn when_an_unchanged_instance_saves_it_should_keep_the_latest_list_view() {
    let dir = tempfile::Builder::new()
        .prefix("list-view-東京")
        .tempdir()
        .unwrap();
    let path = dir.path().join("state.json");
    let mut first = State::default();
    first.save(&path, 1).unwrap();
    let (mut second, _) = State::load(&path);
    second.toggle_list_view();
    second.save(&path, 2).unwrap();

    first.save(&path, 3).unwrap();
    let (saved, warning) = State::load(&path);

    assert!(warning.is_none());
    assert_eq!(saved.list_view(), ListView::Compact);
    assert_eq!(first.list_view(), ListView::Compact);
}

#[test]
fn saving_keeps_a_symlink_and_the_file_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real.json");
    let link = dir.path().join("state.json");
    std::fs::write(&real, b"{}").unwrap();
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let mut state = State::default();
    state.toggle_bookmark(&post("a", "s"), 1);

    state.save(&link, 1).unwrap();
    let still_link = std::fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink();
    let mode = std::fs::metadata(&real).unwrap().permissions().mode() & 0o777;
    let (loaded, _) = State::load(&real);

    assert!(still_link);
    assert_eq!(mode, 0o600);
    assert!(loaded.is_bookmarked("a"));
}

#[test]
fn saving_through_a_chain_of_dangling_links_keeps_every_link() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("real")).unwrap();
    std::os::unix::fs::symlink("hop.json", dir.path().join("state.json")).unwrap();
    std::os::unix::fs::symlink("real/state.json", dir.path().join("hop.json")).unwrap();
    let mut state = State::default();
    state.toggle_bookmark(&post("a", "s"), 1);

    state.save(&dir.path().join("state.json"), 1).unwrap();
    let is_link = |p: &str| {
        std::fs::symlink_metadata(dir.path().join(p))
            .unwrap()
            .file_type()
            .is_symlink()
    };
    let links = (is_link("state.json"), is_link("hop.json"));
    let written = dir.path().join("real/state.json").is_file();

    assert_eq!(links, (true, true));
    assert!(written);
}
