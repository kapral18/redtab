use super::*;

#[test]
fn when_discovered_managed_changes_should_wait_for_confirmation() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    fixture.change(&mut store);
    let before = fs::read_to_string(&fixture.source).unwrap();

    let plan = fixture.plan(&store);

    assert_eq!(plan.changes, ["media.image_preview: true → false"]);
    assert_eq!(plan.source(), fixture.source);
    assert_eq!(fs::read_to_string(&fixture.source).unwrap(), before);
    assert_eq!(fs::read_to_string(&fixture.target).unwrap(), before);
    assert!(!fixture.directory.path().join("applied-args").exists());
    assert_eq!(store.persistence, Persistence::Temporary);
    assert!(!store.current.media.image_preview);
}

#[test]
fn when_plain_source_names_contain_attribute_words_it_should_offer_confirmation() {
    for (source, target) in [
        ("readonly_config_run_mode.toml", "config_run_mode.toml"),
        ("readonly_literal_run_config.toml", "run_config.toml"),
        ("readonly_run_config.toml", "run_config.toml"),
        ("readonly_script_config.toml", "script_config.toml"),
    ] {
        let fixture = Fixture::named(source, target);
        let mut store = fixture.store();
        fixture.change(&mut store);

        let result = store
            .managed_request()
            .unwrap()
            .discover(fixture.tool.clone());

        assert!(result.is_ok(), "{source}: {:?}", result.err());
        assert!(result.unwrap().is_some(), "{source}");
        assert!(
            Config::load(Some(&fixture.source))
                .unwrap()
                .media
                .image_preview
        );
    }
}

#[test]
fn when_confirmed_managed_save_should_keep_comments_permissions_and_other_source_edits() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    fixture.change(&mut store);
    fs::write(&fixture.source, "# mine\nall_subs_posts_per_group = 4\n[media]\nimage_preview = true # keep\n[accounts.other.filters]\nkeywords = [\"old\"] # keep array\n").unwrap();

    let plan = fixture.plan(&store);
    assert_eq!(
        plan.changes,
        [
            "all_subs_posts_per_group: 3 → 4",
            "media.image_preview: true → false"
        ]
    );
    store.managed_finished(plan.verification(), plan.apply());

    assert_eq!(store.persistence, Persistence::Saved);
    assert!(!store.current.media.image_preview && store.current.all_subs_posts_per_group == 4);
    assert_eq!(
        fs::read_to_string(&fixture.source).unwrap(),
        "# mine\nall_subs_posts_per_group = 4\n[media]\nimage_preview = false # keep\n[accounts.other.filters]\nkeywords = [\"old\"] # keep array\n"
    );
    assert_eq!(
        fs::read(&fixture.target).unwrap(),
        fs::read(&fixture.source).unwrap()
    );
    assert_eq!(
        fs::metadata(&fixture.source).unwrap().permissions().mode() & 0o777,
        0o640
    );
    assert_eq!(
        fs::metadata(&fixture.target).unwrap().permissions().mode() & 0o777,
        0o444
    );
    let args = fs::read_to_string(fixture.directory.path().join("applied-args")).unwrap();
    assert!(args.contains("--include=files\n--recursive=false\n--\n"));
}

#[test]
fn when_source_changes_after_confirmation_it_should_remain_untouched() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    fixture.change(&mut store);
    let plan = fixture.plan(&store);
    fs::write(&fixture.source, "# external edit\n").unwrap();

    store.managed_finished(plan.verification(), plan.apply());

    assert_eq!(
        fs::read_to_string(&fixture.source).unwrap(),
        "# external edit\n"
    );
    assert!(
        store
            .error
            .as_ref()
            .unwrap()
            .contains("changed while confirming")
    );
    assert_eq!(store.persistence, Persistence::Unsaved);
    assert!(!store.current.media.image_preview);
    assert!(!fixture.directory.path().join("applied-args").exists());
}

#[test]
fn when_target_is_retargeted_after_confirmation_it_should_preserve_the_new_file() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    fixture.change(&mut store);
    let plan = fixture.plan(&store);
    let other = fixture.directory.path().join("other.toml");
    fs::write(&other, "# different target\n").unwrap();
    fs::remove_file(&fixture.target).unwrap();
    symlink(&other, &fixture.target).unwrap();

    let result = plan.apply();

    assert!(result.is_err());
    assert_eq!(fs::read_to_string(&other).unwrap(), "# different target\n");
    assert!(
        fs::symlink_metadata(&fixture.target)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(!fixture.directory.path().join("applied-args").exists());
}

#[test]
fn when_source_is_deleted_after_confirmation_it_should_not_be_recreated() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    fixture.change(&mut store);
    let plan = fixture.plan(&store);
    fs::remove_file(&fixture.source).unwrap();

    let result = plan.apply();

    assert!(result.is_err());
    assert!(!fixture.source.exists());
    assert!(
        Config::load(Some(&fixture.target))
            .unwrap()
            .media
            .image_preview
    );
}

#[test]
fn when_a_confirmed_source_becomes_read_only_it_should_not_bypass_its_permissions() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    fixture.change(&mut store);
    let plan = fixture.plan(&store);
    fs::set_permissions(&fixture.source, Permissions::from_mode(0o444)).unwrap();

    let result = plan.apply();

    assert!(result.is_err());
    assert!(
        Config::load(Some(&fixture.source))
            .unwrap()
            .media
            .image_preview
    );
    assert_eq!(
        fs::metadata(&fixture.source).unwrap().permissions().mode() & 0o777,
        0o444
    );
}
