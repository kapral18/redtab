use super::*;

#[test]
fn when_signin_selects_a_browser_it_should_keep_a_non_ascii_profile_name() {
    let actual = parse_args_from(
        ["--signin", "--browser", "firefox", "--profile", "Wørk"]
            .into_iter()
            .map(Into::into),
    )
    .unwrap()
    .unwrap();
    assert!(actual.signin);
    assert_eq!(actual.browser.as_deref(), Some("firefox"));
    assert_eq!(actual.profile.as_deref(), Some("Wørk"));
}

#[test]
fn when_profile_selection_has_no_browser_it_should_fail_before_authentication() {
    let actual = parse_args_from(
        ["--signin", "--profile", "Work"]
            .into_iter()
            .map(Into::into),
    );
    assert!(actual.is_err());
}

#[test]
fn when_signin_is_combined_with_check_it_should_fail_before_authentication() {
    let actual = parse_args_from(["--signin", "--check"].into_iter().map(Into::into));
    assert!(actual.is_err());
}
