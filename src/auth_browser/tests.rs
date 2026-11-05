use super::*;

fn dia_cookie_fixture(root: &Path, relative: &str) -> PathBuf {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"").unwrap();
    path
}

#[test]
fn when_dia_profiles_exist_discovery_should_keep_their_ids_and_clean_display_names() {
    let directory = tempfile::tempdir().unwrap();
    dia_cookie_fixture(directory.path(), "Default/Cookies");
    dia_cookie_fixture(directory.path(), "Profile 二/Network/Cookies");
    dia_cookie_fixture(directory.path(), "System Profile/Cookies");
    std::fs::write(
            directory.path().join("Local State"),
            r#"{"profile":{"info_cache":{"Default":{"name":"Personal"},"Profile 二":{"name":"仕事\n"}}}}"#,
        )
        .unwrap();

    let actual = dia_profiles(directory.path());

    assert_eq!(
        actual,
        vec![
            Source::Browser {
                browser: "dia".into(),
                profile: "Default".into(),
                label: "Dia · Personal".into(),
            },
            Source::Browser {
                browser: "dia".into(),
                profile: "Profile 二".into(),
                label: "Dia · 仕事".into(),
            },
        ]
    );
}

#[test]
fn when_dia_metadata_is_invalid_discovery_should_use_the_profile_directory_name() {
    let directory = tempfile::tempdir().unwrap();
    dia_cookie_fixture(directory.path(), "Default/Cookies");
    std::fs::write(directory.path().join("Local State"), "invalid JSON").unwrap();

    let actual = dia_profiles(directory.path());

    assert_eq!(actual[0].label(), "Dia · Default");
}

#[test]
fn when_dia_metadata_is_missing_discovery_should_use_the_profile_directory_name() {
    let directory = tempfile::tempdir().unwrap();
    dia_cookie_fixture(directory.path(), "Default/Cookies");

    let actual = dia_profiles(directory.path());

    assert_eq!(actual[0].label(), "Dia · Default");
}

#[test]
fn when_dia_metadata_has_a_blank_name_discovery_should_use_the_profile_directory_name() {
    let directory = tempfile::tempdir().unwrap();
    dia_cookie_fixture(directory.path(), "Default/Cookies");
    std::fs::write(
        directory.path().join("Local State"),
        r#"{"profile":{"info_cache":{"Default":{"name":"  "}}}}"#,
    )
    .unwrap();

    let actual = dia_profiles(directory.path());

    assert_eq!(actual[0].label(), "Dia · Default");
}

#[test]
fn when_dia_is_not_installed_discovery_should_find_no_dia_profiles() {
    let directory = tempfile::tempdir().unwrap();

    let actual = dia_profiles(&directory.path().join("missing"));

    assert!(actual.is_empty());
}

#[test]
fn when_dia_has_both_cookie_locations_import_should_prefer_network_cookies() {
    let directory = tempfile::tempdir().unwrap();
    dia_cookie_fixture(directory.path(), "Default/Cookies");
    let expected = dia_cookie_fixture(directory.path(), "Default/Network/Cookies")
        .canonicalize()
        .unwrap();

    let actual = dia_cookie_path(directory.path(), "Default");

    assert_eq!(actual, Some(expected));
}

#[test]
fn when_the_selected_dia_cookie_file_disappears_it_should_not_resolve() {
    let directory = tempfile::tempdir().unwrap();
    let path = dia_cookie_fixture(directory.path(), "Default/Cookies");
    std::fs::remove_file(path).unwrap();

    let actual = dia_cookie_path(directory.path(), "Default");

    assert_eq!(actual, None);
}

#[test]
fn when_the_dia_profile_selector_contains_traversal_it_should_not_resolve() {
    let directory = tempfile::tempdir().unwrap();
    dia_cookie_fixture(directory.path(), "Default/Cookies");

    let actual = dia_cookie_path(directory.path(), "Profile 2/../Default");

    assert_eq!(actual, None);
}

#[test]
fn when_the_dia_profile_selector_is_empty_it_should_not_resolve() {
    let directory = tempfile::tempdir().unwrap();
    dia_cookie_fixture(directory.path(), "Default/Cookies");

    let actual = dia_cookie_path(directory.path(), "");

    assert_eq!(actual, None);
}

#[cfg(unix)]
#[test]
fn when_a_dia_profile_symlink_escapes_the_browser_root_it_should_not_be_discovered() {
    let directory = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    dia_cookie_fixture(outside.path(), "Cookies");
    std::os::unix::fs::symlink(outside.path(), directory.path().join("Default")).unwrap();

    let actual = dia_profiles(directory.path());

    assert!(actual.is_empty());
}

#[cfg(unix)]
#[test]
fn when_dia_cookies_are_symlinked_outside_the_profile_they_should_not_resolve() {
    let directory = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let path = dia_cookie_fixture(outside.path(), "Cookies");
    std::fs::create_dir(directory.path().join("Default")).unwrap();
    std::os::unix::fs::symlink(path, directory.path().join("Default/Cookies")).unwrap();

    let actual = dia_cookie_path(directory.path(), "Default");

    assert_eq!(actual, None);
}

fn cookie(domain: &str, name: &str, value: &str) -> BrowserCookie {
    BrowserCookie {
        domain: domain.into(),
        path: "/".into(),
        name: name.into(),
        value: value.into(),
        secure: true,
        expires: 0,
    }
}

#[test]
fn when_containers_supply_different_sessions_the_import_should_not_choose_the_last_account() {
    let actual = cookie_export([
        cookie(".reddit.com", "reddit_session", "fixture-session"),
        cookie(".reddit.com", "reddit_session", "other-session"),
    ])
    .unwrap_err()
    .to_string();
    assert!(actual.contains("multiple Reddit accounts"));
    assert!(!actual.contains("other-session"));
}

#[test]
fn when_other_sites_share_a_profile_only_reddit_cookies_should_be_imported() {
    let actual = cookie_export([
        cookie(".reddit.com", "reddit_session", "fixture-session"),
        cookie(".example.com", "private", "unrelated"),
    ])
    .unwrap();
    assert_eq!(
        actual,
        ".reddit.com\tTRUE\t/\tTRUE\t0\treddit_session\tfixture-session\n"
    );
}

#[test]
fn when_a_cookie_contains_a_row_separator_the_import_should_fail_without_its_value() {
    let actual = cookie_export([cookie(".reddit.com", "reddit_session", "private\nvalue")])
        .unwrap_err()
        .to_string();
    assert!(!actual.contains("private"));
}

#[test]
fn private_login_pipe_reads_events_and_split_cookie_responses() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("browser-pipe-fixture");
    std::fs::write(&executable, r#"#!/bin/sh
pipe=
for argument in "$@"; do
    case "$argument" in
        --remote-debugging-pipe) pipe=yes ;;
        --remote-debugging-port*) exit 4 ;;
    esac
done
test "$pipe" = yes || exit 5
dd bs=1 count=39 <&3 > /dev/null 2>/dev/null || exit 6
printf '%s\000' '{"method":"event"}' >&4
printf '%s' '{"id":1,"result":{"cookies":[' >&4
printf '%s\000' '{"domain":".reddit.com","path":"/","secure":true,"expires":-1,"name":"reddit_session","value":"fixture-session"}]}}' >&4
"#).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let (mut child, mut connection) = spawn_login(&executable, directory.path()).unwrap();
    connection
        .write_all(b"{\"id\":1,\"method\":\"Storage.getCookies\"}\0")
        .unwrap();
    let actual = cookie_export(read_cookies(&mut BufReader::new(connection), 1).unwrap()).unwrap();
    assert!(child.wait().unwrap().success());
    assert_eq!(
        actual,
        ".reddit.com\tTRUE\t/\tTRUE\t0\treddit_session\tfixture-session\n"
    );
}

#[test]
fn login_pipe_rejects_oversized_and_unterminated_messages_without_contents() {
    let mut oversized = json!({"id":1,"result":{"cookies":[{"domain":".reddit.com","path":"/","name":"reddit_session","value":"private".repeat(PACKET_LIMIT as usize / 7 + 1)}]}}).to_string().into_bytes();
    oversized.push(0);
    for bytes in [oversized, b"private-incomplete".to_vec()] {
        let error = read_cookies(&mut std::io::Cursor::new(bytes), 1)
            .err()
            .unwrap()
            .to_string();
        assert!(!error.contains("private"));
    }
}
