use super::*;

#[test]
fn when_target_is_writable_it_should_save_directly_without_a_managed_plan() {
    let fixture = Fixture::new();
    fs::set_permissions(&fixture.target, Permissions::from_mode(0o600)).unwrap();
    let mut store = fixture.store();

    fixture.change(&mut store);

    assert_eq!(store.persistence, Persistence::Saved);
    assert!(store.managed_request().is_none());
    assert!(
        Config::load(Some(&fixture.source))
            .unwrap()
            .media
            .image_preview
    );
    assert!(
        !Config::load(Some(&fixture.target))
            .unwrap()
            .media
            .image_preview
    );
}

#[test]
fn when_verification_fails_a_reload_should_not_hide_the_save_failure() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    fixture.change(&mut store);
    fs::write(fixture.directory.path().join("writable"), "").unwrap();
    fixture.save(&mut store);
    assert_eq!(store.persistence, Persistence::Unsaved);

    store.reload();

    assert_eq!(store.persistence, Persistence::Unsaved);
    assert!(store.error.as_ref().unwrap().contains("permissions"));
    store.save();
    assert_ne!(
        store.persistence,
        Persistence::Saved,
        "retry still needs verification"
    );
    fixture.save(&mut store);
    assert_eq!(
        store.persistence,
        Persistence::Unsaved,
        "retry must retain the original permissions"
    );
    fs::remove_file(fixture.directory.path().join("writable")).unwrap();
    store.save();

    fixture.save(&mut store);

    assert_eq!(store.persistence, Persistence::Saved);
    assert_eq!(
        fs::metadata(&fixture.target).unwrap().permissions().mode() & 0o777,
        0o444
    );
}

#[test]
fn when_reverting_an_unverified_save_it_should_explicitly_accept_the_current_file() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    fixture.change(&mut store);
    fs::write(fixture.directory.path().join("writable"), "").unwrap();
    fixture.save(&mut store);

    store.revert();

    assert_eq!(store.persistence, Persistence::Saved);
    assert!(store.error.is_none());
    assert!(store.managed_request().is_none());
    assert!(!store.current.media.image_preview);
}

#[test]
fn when_an_empty_config_is_saved_twice_it_should_keep_the_latest_value_once() {
    let fixture = Fixture::new();
    fs::write(&fixture.source, "").unwrap();
    fs::set_permissions(&fixture.target, Permissions::from_mode(0o600)).unwrap();
    fs::write(&fixture.target, "").unwrap();
    fs::set_permissions(&fixture.target, Permissions::from_mode(0o444)).unwrap();
    let mut store = fixture.store();
    fixture.change(&mut store);
    fixture.save(&mut store);
    assert_eq!(store.persistence, Persistence::Saved);
    store
        .change(
            SETTINGS
                .iter()
                .find(|f| f.key == "media.image_preview")
                .unwrap(),
            toml::Value::Boolean(true),
        )
        .unwrap();

    fixture.save(&mut store);

    assert_eq!(store.persistence, Persistence::Saved);
    assert_eq!(
        fs::read_to_string(&fixture.source).unwrap(),
        "[media]\nimage_preview = true\n"
    );
    assert!(store.current.media.image_preview);
}
