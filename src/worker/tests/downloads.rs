use super::*;

#[test]
fn when_redtab_starts_it_should_remove_only_old_media_folders_of_exited_processes() {
    let tmp = tempfile::tempdir().unwrap();
    let folder = |name: &str| {
        let path = tmp.path().join(name);
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("a.jpg"), b"x").unwrap();
        path
    };
    let exited_old = folder("redtab-media-100-1a2b0");
    let exited_recent = folder("redtab-media-101-1a2b0");
    let running_old = folder("redtab-media-200-1a2b0");
    let other_name = folder("redtab-media-102-notes");
    let target = folder("unrelated");
    let link = tmp.path().join("redtab-media-103-1a2b0");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let file = tmp.path().join("redtab-media-104-1a2b0");
    std::fs::write(&file, b"x").unwrap();
    // Everything above was last changed a day and an hour before `now`; one folder
    // a minute short of a day.
    let now = std::time::SystemTime::now() + MEDIA_GRACE + std::time::Duration::from_secs(3600);
    std::fs::File::open(&exited_recent)
        .unwrap()
        .set_modified(now - MEDIA_GRACE + std::time::Duration::from_secs(60))
        .unwrap();

    remove_stale_media_dirs_in(tmp.path(), now, |pid| pid == 200);

    assert!(
        !exited_old.exists(),
        "an old folder of an exited redtab goes"
    );
    assert!(exited_recent.exists(), "within the grace period it stays");
    assert!(running_old.exists(), "a running redtab keeps its folder");
    assert!(other_name.exists(), "not a name redtab makes");
    assert!(link.symlink_metadata().is_ok() && target.join("a.jpg").exists());
    assert!(file.exists(), "not a folder");
}

#[test]
fn a_running_process_counts_as_running_and_an_exited_one_does_not() {
    let mut exited = std::process::Command::new("true").spawn().unwrap();
    exited.wait().unwrap();

    assert!(process_running(std::process::id()));
    assert!(!process_running(exited.id()));
}

#[test]
fn when_a_redtab_downloads_again_it_should_drop_its_own_downloads_older_than_a_day() {
    let dir = tempfile::tempdir().unwrap();
    let old = dir.path().join("old.jpg");
    let recent = dir.path().join("recent.jpg");
    std::fs::write(&old, b"x").unwrap();
    std::fs::write(&recent, b"x").unwrap();
    let now = std::time::SystemTime::now() + MEDIA_GRACE;
    std::fs::File::options()
        .write(true)
        .open(&recent)
        .unwrap()
        .set_modified(now - MEDIA_GRACE + std::time::Duration::from_secs(60))
        .unwrap();

    remove_old_downloads(dir.path(), now);

    assert!(!old.exists(), "a day old: gone");
    assert!(recent.exists(), "a minute short of a day: kept");
}
