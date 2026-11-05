use super::*;

#[test]
fn when_username_rules_are_saved_through_chezmoi_the_account_scope_should_survive() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    store.select_account(Some("Reader_One-Two"));
    store
        .change(
            SETTINGS
                .iter()
                .find(|f| f.key == "filters.keywords")
                .unwrap(),
            toml::Value::Array(vec!["mine".into()]),
        )
        .unwrap();

    fixture.save(&mut store);

    assert_eq!(store.persistence, Persistence::Saved);
    assert_eq!(
        store.current.accounts["Reader_One-Two"].filters.keywords,
        ["mine"]
    );
    assert_eq!(
        fs::read(&fixture.source).unwrap(),
        fs::read(&fixture.target).unwrap()
    );
    assert_eq!(
        fs::metadata(&fixture.target).unwrap().permissions().mode() & 0o777,
        0o444
    );
}

#[test]
fn when_username_source_case_differs_managed_saves_should_merge_under_source_spelling() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    store.select_account(Some("Reader"));
    store
        .change(
            SETTINGS
                .iter()
                .find(|f| f.key == "filters.keywords")
                .unwrap(),
            toml::Value::Array(vec!["mine".into()]),
        )
        .unwrap();
    let mut source = fs::read_to_string(&fixture.source).unwrap();
    source.push_str("\n[accounts.reader.filters]\nauthors=[\"bot\"]\n");
    fs::write(&fixture.source, source).unwrap();

    fixture.save(&mut store);

    assert_eq!(store.persistence, Persistence::Saved);
    assert_eq!(store.current.accounts.len(), 2);
    assert_eq!(store.current.accounts["reader"].filters.authors, ["bot"]);
    assert_eq!(store.current.accounts["reader"].filters.keywords, ["mine"]);
}

#[test]
fn when_apply_fails_it_should_keep_source_intent_and_allow_a_verified_retry() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    fixture.change(&mut store);
    fs::write(fixture.directory.path().join("fail"), "").unwrap();
    fixture.save(&mut store);
    assert_eq!(store.persistence, Persistence::Unsaved);
    assert!(store.error.as_ref().unwrap().contains("Source updated"));
    assert!(
        !Config::load(Some(&fixture.source))
            .unwrap()
            .media
            .image_preview
    );
    assert!(
        Config::load(Some(&fixture.target))
            .unwrap()
            .media
            .image_preview
    );
    fs::remove_file(fixture.directory.path().join("fail")).unwrap();
    store.save();

    fixture.save(&mut store);

    assert_eq!(store.persistence, Persistence::Saved);
    assert!(
        !Config::load(Some(&fixture.target))
            .unwrap()
            .media
            .image_preview
    );
}

#[test]
fn when_apply_returns_success_without_the_values_it_should_not_report_saved() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    fixture.change(&mut store);
    fs::write(fixture.directory.path().join("noop"), "").unwrap();

    fixture.save(&mut store);

    assert_eq!(store.persistence, Persistence::Unsaved);
    assert!(store.error.as_ref().unwrap().contains("differs"));
    assert!(
        Config::load(Some(&fixture.target))
            .unwrap()
            .media
            .image_preview
    );
}

#[test]
fn when_the_manager_cannot_find_a_source_it_should_keep_session_changes() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    fixture.change(&mut store);
    fs::write(fixture.directory.path().join("unmanaged"), "").unwrap();

    let actual = store
        .managed_request()
        .unwrap()
        .discover(fixture.tool.clone())
        .unwrap();

    assert!(actual.is_none());
    assert_eq!(store.persistence, Persistence::Temporary);
    assert!(!store.current.media.image_preview);
}

#[test]
fn when_chezmoi_is_missing_it_should_keep_session_changes() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    fixture.change(&mut store);
    let tool = Chezmoi {
        program: fixture.directory.path().join("absent"),
        ..fixture.tool.clone()
    };

    let actual = store.managed_request().unwrap().discover(tool).unwrap();

    assert!(actual.is_none());
    assert_eq!(store.persistence, Persistence::Temporary);
}

#[test]
fn when_a_manager_stalls_it_should_time_out_without_writing() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    fixture.change(&mut store);
    fs::write(fixture.directory.path().join("slow"), "").unwrap();
    let tool = Chezmoi {
        timeout: Duration::from_millis(30),
        ..fixture.tool.clone()
    };

    let actual = store.managed_request().unwrap().discover(tool);

    assert!(actual.err().unwrap().to_string().contains("timed out"));
    assert!(
        Config::load(Some(&fixture.source))
            .unwrap()
            .media
            .image_preview
    );
    assert!(!fixture.directory.path().join("applied-args").exists());
}

#[test]
fn when_source_is_templated_it_should_explain_session_only_without_rewriting_it() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    fixture.change(&mut store);
    let source = fixture.directory.path().join("readonly_config.toml.tmpl");
    fs::write(&source, "[media]\nimage_preview = {{ .autoplay }}\n").unwrap();
    fs::write(
        fixture.directory.path().join("source-override"),
        source.to_str().unwrap(),
    )
    .unwrap();

    let result = store
        .managed_request()
        .unwrap()
        .discover(fixture.tool.clone());

    assert!(
        result
            .err()
            .unwrap()
            .to_string()
            .contains("templated or generated")
    );
    assert_eq!(
        fs::read_to_string(&source).unwrap(),
        "[media]\nimage_preview = {{ .autoplay }}\n"
    );
    assert_eq!(store.persistence, Persistence::Temporary);
}

#[test]
fn when_source_is_read_only_it_should_leave_it_and_the_target_unchanged() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    fixture.change(&mut store);
    fs::set_permissions(&fixture.source, Permissions::from_mode(0o444)).unwrap();

    let result = store
        .managed_request()
        .unwrap()
        .discover(fixture.tool.clone());

    assert!(result.err().unwrap().to_string().contains("read-only too"));
    assert!(
        Config::load(Some(&fixture.source))
            .unwrap()
            .media
            .image_preview
    );
    assert!(
        Config::load(Some(&fixture.target))
            .unwrap()
            .media
            .image_preview
    );
}

#[test]
fn when_a_generated_source_parses_as_toml_it_should_still_be_rejected() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    fixture.change(&mut store);
    let source = fixture.directory.path().join("run_config.toml");
    fs::write(&source, "# script with no commands\n").unwrap();
    fs::write(
        fixture.directory.path().join("source-override"),
        source.to_str().unwrap(),
    )
    .unwrap();

    let result = store
        .managed_request()
        .unwrap()
        .discover(fixture.tool.clone());

    assert!(
        result
            .err()
            .unwrap()
            .to_string()
            .contains("templated or generated")
    );
    assert_eq!(
        fs::read_to_string(&source).unwrap(),
        "# script with no commands\n"
    );
    assert_eq!(store.persistence, Persistence::Temporary);
}

#[test]
fn when_source_has_a_conflicting_value_it_should_require_resolution() {
    let fixture = Fixture::new();
    let mut store = fixture.store();
    let field = SETTINGS
        .iter()
        .find(|f| f.key == "all_subs_posts_per_group")
        .unwrap();
    store.change(field, toml::Value::Integer(4)).unwrap();
    fs::write(&fixture.source, "all_subs_posts_per_group = 5\n").unwrap();

    let result = store
        .managed_request()
        .unwrap()
        .discover(fixture.tool.clone());

    assert!(
        result
            .err()
            .unwrap()
            .to_string()
            .contains("all_subs_posts_per_group changed")
    );
    assert_eq!(
        fs::read_to_string(&fixture.source).unwrap(),
        "all_subs_posts_per_group = 5\n"
    );
    assert_eq!(store.current.all_subs_posts_per_group, 4);
}

#[test]
fn when_installed_chezmoi_applies_an_isolated_source_it_should_change_only_that_target() {
    if Command::new("chezmoi").arg("--version").output().is_err() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let source_dir = directory.path().join("source");
    let destination = directory.path().join("destination");
    fs::create_dir(&source_dir).unwrap();
    fs::create_dir(&destination).unwrap();
    let source = source_dir.join("readonly_config_run_mode.toml");
    let target = destination.join("config_run_mode.toml");
    fs::write(
        &source,
        "# actual chezmoi fixture\n[media]\nimage_preview = true # keep\n",
    )
    .unwrap();
    fs::write(&target, fs::read(&source).unwrap()).unwrap();
    fs::set_permissions(&target, Permissions::from_mode(0o444)).unwrap();
    fs::write(source_dir.join("other.toml"), "must not be applied\n").unwrap();
    let config = directory.path().join("chezmoi.toml");
    fs::write(&config, "").unwrap();
    let tool = Chezmoi {
        program: "chezmoi".into(),
        timeout: Duration::from_secs(5),
        args: vec![
            "--source".into(),
            source_dir.into(),
            "--destination".into(),
            destination.clone().into(),
            "--config".into(),
            config.into(),
            "--cache".into(),
            directory.path().join("cache").into(),
            "--persistent-state".into(),
            directory.path().join("state").into(),
        ],
    };
    let mut store = Store::open(Some(&target)).unwrap();
    store
        .change(
            SETTINGS
                .iter()
                .find(|f| f.key == "media.image_preview")
                .unwrap(),
            toml::Value::Boolean(false),
        )
        .unwrap();
    let plan = store
        .managed_request()
        .unwrap()
        .discover(tool)
        .unwrap()
        .unwrap();

    store.managed_finished(plan.verification(), plan.apply());

    assert_eq!(store.persistence, Persistence::Saved, "{:?}", store.error);
    assert_eq!(
        fs::read_to_string(&source).unwrap(),
        "# actual chezmoi fixture\n[media]\nimage_preview = false # keep\n"
    );
    assert!(!Config::load(Some(&target)).unwrap().media.image_preview);
    assert!(!destination.join("other.toml").exists());
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o444
    );
}
