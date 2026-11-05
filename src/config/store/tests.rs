use super::*;
use crate::config::schema::SETTINGS;
fn change(store: &mut Store, key: &str, input: &str) {
    let value: toml::Table = toml::from_str(&format!("value = {input}")).unwrap();
    store
        .change(
            SETTINGS.iter().find(|f| f.key == key).unwrap(),
            value["value"].clone(),
        )
        .unwrap();
}

#[test]
fn scoped_settings_should_initialize_local_defaults_without_reddit_preferences() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    Store::open(Some(&path)).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    let values: toml::Value = toml::from_str(&text).unwrap();
    assert_eq!(values["media"]["image_preview"].as_bool(), Some(true));
    assert!(values["media"].get("video_autoplay").is_none());
    assert!(values.get("show_nsfw").is_none());
}

#[test]
fn grouped_settings_should_keep_headers_and_section_annotations_when_inserting_root_keys() {
    let original = "# file header\n\n# media section\n\n[media]\nimage_preview = true # keep\n";
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    fs::write(&path, original).unwrap();
    let mut store = Store::open(Some(&path)).unwrap();
    change(&mut store, "all_subs_posts_per_group", "4");
    let actual = fs::read_to_string(&path).unwrap();
    assert!(
        actual.starts_with("# file header\n\nall_subs_posts_per_group = 4\n"),
        "{actual}"
    );
    assert!(
        actual.contains("# media section\n\n[media]\nimage_preview = true # keep\n"),
        "{actual}"
    );
}

#[test]
fn grouped_settings_should_keep_comment_only_headers_above_new_values() {
    for original in ["# réglages\n", "# réglages", "# réglages\r\n\r\n"] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(&path, original).unwrap();
        let mut store = Store::open(Some(&path)).unwrap();
        change(&mut store, "all_subs_posts_per_group", "4");
        let actual = fs::read_to_string(&path).unwrap();
        assert!(actual.starts_with(original), "{actual}");
        assert_eq!(
            Config::load(Some(&path)).unwrap().all_subs_posts_per_group,
            4
        );
    }
}

#[test]
fn grouped_settings_should_keep_existing_inline_and_dotted_media_styles() {
    for (original, prefix) in [
        ("media = { image_preview = true }\n", "media = {"),
        (
            "media.image_preview = true\n",
            "media.image_preview = false\n",
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(&path, original).unwrap();
        let mut store = Store::open(Some(&path)).unwrap();
        change(&mut store, "media.image_preview", "false");
        let actual = fs::read_to_string(&path).unwrap();
        assert!(actual.starts_with(prefix), "{actual}");
        assert!(!Config::load(Some(&path)).unwrap().media.image_preview);
    }
}

#[test]
fn when_username_casing_differs_saving_should_keep_one_readable_account_table() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        "# mine\n[accounts.TFordragon.filters]\nkeywords=[\"old\"] # keep\n",
    )
    .unwrap();
    let mut store = Store::open(Some(&path)).unwrap();
    store.select_account(Some("tfordragon"));

    change(&mut store, "filters.keywords", "[\"new\"]");

    assert_eq!(store.persistence, Persistence::Saved);
    assert_eq!(store.current.accounts.len(), 1);
    assert_eq!(
        store.current.accounts["TFordragon"].filters.keywords,
        ["new"]
    );
    assert!(fs::read_to_string(&path).unwrap().contains("# keep"));
}

#[test]
fn when_username_table_case_changes_a_pending_edit_should_merge_under_disk_spelling() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, "[accounts.Reader.filters]\nkeywords=[\"old\"]\n").unwrap();
    let mut store = Store::open(Some(&path)).unwrap();
    store.select_account(Some("Reader"));
    store.current = SETTINGS
        .iter()
        .find(|f| f.key == "filters.keywords")
        .unwrap()
        .set(&store.current, toml::Value::Array(vec!["new".into()]))
        .unwrap();
    fs::write(&path, "[accounts.reader.filters]\nkeywords=[\"old\"]\n").unwrap();

    store.save();

    assert_eq!(store.persistence, Persistence::Saved);
    assert_eq!(store.current.accounts.len(), 1);
    assert_eq!(store.current.active_account.as_deref(), Some("reader"));
    assert_eq!(
        Config::load(Some(&path)).unwrap().accounts["reader"]
            .filters
            .keywords,
        ["new"]
    );
}

#[test]
fn when_username_first_writers_use_different_case_distinct_rule_edits_should_merge() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut first = Store::open(Some(&path)).unwrap();
    let mut second = Store::open(Some(&path)).unwrap();
    first.select_account(Some("Reader"));
    second.select_account(Some("reader"));
    change(&mut first, "filters.authors", "[\"bot\"]");

    change(&mut second, "filters.keywords", "[\"mine\"]");

    assert_eq!(second.persistence, Persistence::Saved);
    assert_eq!(second.current.accounts.len(), 1);
    assert_eq!(second.current.accounts["Reader"].filters.authors, ["bot"]);
    assert_eq!(second.current.accounts["Reader"].filters.keywords, ["mine"]);
}

#[test]
fn scoped_settings_should_merge_different_accounts_without_losing_rules_or_comments() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        "# mine\n[accounts.first.filters]\nkeywords = [\"news\"] # custom\n",
    )
    .unwrap();
    let mut first = Store::open(Some(&path)).unwrap();
    let mut second = Store::open(Some(&path)).unwrap();
    first.select_account(Some("first"));
    second.select_account(Some("second"));
    change(&mut second, "filters.authors", "[\"bot\"]");
    change(&mut first, "filters.keywords", "[\"spoiler\"]");
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("# custom"), "{text}");
    assert_eq!(first.current.accounts["second"].filters.authors, ["bot"]);
    assert_eq!(first.current.filters.keywords, ["spoiler"]);
    second.reload();
    assert_eq!(second.current.filters.authors, ["bot"]);
    assert_eq!(
        second.current.accounts["first"].filters.keywords,
        ["spoiler"]
    );
    assert_eq!(first.persistence, Persistence::Saved);
}

#[test]
fn concurrent_same_account_key_conflicts_until_revert_then_reapply() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut local = Store::open(Some(&path)).unwrap();
    local.select_account(Some("Reader"));
    local.current = SETTINGS
        .iter()
        .find(|f| f.key == "filters.keywords")
        .unwrap()
        .set(&local.current, toml::Value::Array(vec!["local".into()]))
        .unwrap();
    fs::write(
        &path,
        "[accounts.reader.filters]\nkeywords=[\"external\"]\n[media]\nimage_preview=false\n",
    )
    .unwrap();
    local.save();
    assert_eq!(local.persistence, Persistence::Unsaved);
    assert_eq!(local.conflicts, ["accounts.reader.filters.keywords"]);
    assert_eq!(local.current.filters.keywords, ["local"]);
    assert!(!local.current.media.image_preview);
    local.revert();
    assert_eq!(local.current.filters.keywords, ["external"]);
    change(&mut local, "filters.keywords", "[\"local\"]");
    assert_eq!(local.persistence, Persistence::Saved);
}

#[test]
fn invalid_deleted_and_retargeted_files_keep_last_working_values() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, "all_subs_posts_per_group=4\n").unwrap();
    let mut store = Store::open(Some(&path)).unwrap();
    fs::write(&path, "[media]\nimage_preview=\"invalid\"\n").unwrap();
    store.reload();
    assert_eq!(store.current.all_subs_posts_per_group, 4);
    assert_eq!(store.persistence, Persistence::Unsaved);
    fs::remove_file(&path).unwrap();
    store.save();
    assert!(!path.exists());
    fs::write(&path, "all_subs_posts_per_group=5\n").unwrap();
    store.revert();
    assert_eq!(store.current.all_subs_posts_per_group, 5);
}

#[test]
fn read_only_is_temporary_and_retry_keeps_mode_and_symlink() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("réglages.toml");
    fs::write(&real, "# user\nall_subs_posts_per_group=3\n").unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o444)).unwrap();
    let link = dir.path().join("config.toml");
    symlink(&real, &link).unwrap();
    let mut store = Store::open(Some(&link)).unwrap();
    change(&mut store, "all_subs_posts_per_group", "4");
    assert_eq!(store.persistence, Persistence::Temporary);
    assert_eq!(
        Config::load(Some(&real)).unwrap().all_subs_posts_per_group,
        3
    );
    fs::set_permissions(&real, fs::Permissions::from_mode(0o640)).unwrap();
    store.save();
    assert_eq!(store.persistence, Persistence::Saved);
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::metadata(&real).unwrap().permissions().mode() & 0o777,
        0o640
    );
    let other = dir.path().join("other.toml");
    fs::write(&other, "all_subs_posts_per_group=5\n").unwrap();
    fs::remove_file(&link).unwrap();
    symlink(&other, &link).unwrap();
    store.reload();
    assert_eq!(store.persistence, Persistence::Unsaved);
    assert_eq!(store.current.all_subs_posts_per_group, 4);
    store.revert();
    assert_eq!(store.current.all_subs_posts_per_group, 5);
}

#[test]
fn empty_file_and_removing_keys_restore_defaults_on_reload() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, "[media]\nimage_preview=false\n").unwrap();
    let mut store = Store::open(Some(&path)).unwrap();
    fs::write(&path, "").unwrap();
    store.reload();
    assert!(store.current.media.image_preview);
    assert_eq!(store.persistence, Persistence::Saved);
}

#[test]
fn scoped_settings_should_remove_and_readd_the_same_account() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut store = Store::open(Some(&path)).unwrap();
    store.select_account(Some("first"));
    change(&mut store, "filters.keywords", "[\"first\"]");
    fs::write(&path, "[accounts.other.filters]\nkeywords=[\"other\"]\n").unwrap();
    store.reload();
    assert!(store.current.filters.keywords.is_empty());
    assert!(!store.current.accounts.contains_key("first"));
    change(&mut store, "filters.keywords", "[\"again\"]");
    assert_eq!(store.current.accounts["first"].filters.keywords, ["again"]);
    assert_eq!(store.current.accounts["other"].filters.keywords, ["other"]);
}

#[test]
fn settings_review_array_edits_should_keep_internal_and_removed_element_comments() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, "# top\n[accounts.reader.filters]\nkeywords = [\n # before first\n \"old\", # first note\n \"removed\", # removed note\n # trailing note\n] # outer\n").unwrap();
    let mut store = Store::open(Some(&path)).unwrap();
    store.select_account(Some("reader"));
    for input in ["[\"new\"]", "[]", "[\"one\", \"two\"]"] {
        change(&mut store, "filters.keywords", input);
        let text = fs::read_to_string(&path).unwrap();
        for note in [
            "# before first",
            "# first note",
            "# removed note",
            "# trailing note",
            "# outer",
        ] {
            assert!(text.contains(note), "{note}: {text}");
        }
    }
}
