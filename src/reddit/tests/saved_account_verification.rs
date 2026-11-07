use super::*;

#[test]
fn when_loading_a_saved_account_it_should_restore_its_identity_source_and_cache_namespace() {
    let fixture = Fixture::new(|_| panic!("Restoring metadata must not use HTTP"));
    let actual = fixture.saved_reddit(&saved_export("first", "reader"));
    assert_eq!(actual.account().unwrap().id, "first");
    assert_eq!(actual.saved_id().as_deref(), Some("first"));
    assert_eq!(actual.source().unwrap().label(), "Firefox · Work");
    assert!(actual.cache_namespace().contains("account:first"));
}

#[test]
fn when_a_saved_cookie_belongs_to_another_account_verification_should_reject_switching() {
    let fixture =
        Fixture::new(|_| Reply::json(json!({"data":{"id":"other", "name":"other_reader"}})));
    let saved = fixture.saved_reddit(&saved_export("first", "reader"));
    let actual = saved.verify_saved_account().unwrap_err().to_string();
    assert!(actual.contains("different account"));
    assert!(!actual.contains("fixture-session"));
}

#[test]
fn when_a_saved_account_is_verified_it_should_accept_the_same_id_with_an_updated_username() {
    let fixture =
        Fixture::new(|_| Reply::json(json!({"data":{"id":"first", "name":"new_reader"}})));
    let saved = fixture.saved_reddit(&saved_export("first", "reader"));
    saved.verify_saved_account().unwrap();
    assert_eq!(saved.account().unwrap().name, "new_reader");
}

#[test]
fn when_saving_fails_the_runtime_saved_key_should_not_claim_the_new_entry_was_saved() {
    let fixture = Fixture::new(|_| Reply::json(json!({"data":{"id":"first", "name":"reader"}})));
    let candidate = fixture.reddit();
    candidate.verify_account().unwrap();
    let before = candidate.saved_id();
    let actual = candidate.save_connection_with(None, |_| bail!("Keyring locked"));
    assert!(actual.is_err());
    assert_eq!(candidate.saved_id(), before);
}

#[test]
fn when_a_candidate_is_unverified_saving_should_not_touch_the_credential_store() {
    let reddit = Reddit::test_at("http://127.0.0.1:9", false);
    let actual =
        reddit.save_connection_with(None, |_| panic!("Unverified credentials must not be saved"));
    assert!(actual.is_err());
}

#[test]
fn when_account_identity_contains_a_path_it_should_not_be_accepted_or_saved() {
    let fixture = Fixture::new(|_| Reply::json(json!({"data":{"id":"../other", "name":"reader"}})));
    let reddit = fixture.reddit();
    assert!(reddit.verify_account().is_err());
    assert!(
        reddit
            .save_connection_with(None, |_| panic!("Invalid identity must not be saved"))
            .is_err()
    );
}

#[test]
fn when_a_connection_is_retired_queued_account_writes_should_not_reach_reddit() {
    let fixture = Fixture::new(|_| panic!("Retired connection must not issue HTTP"));
    let reddit = fixture.reddit();
    reddit.retire();
    assert!(
        reddit
            .set_membership(
                &crate::model::MembershipTarget::Community("neovim".into()),
                true
            )
            .unwrap_err()
            .to_string()
            .contains("replaced")
    );
}

#[test]
fn when_an_account_is_verified_a_cookie_renewal_should_keep_its_account_cache_namespace() {
    let fixture = Fixture::new(|_| Reply::json(json!({"data":{"id":"fixture", "name":"reader"}})));
    let first = fixture.reddit();
    let second = fixture.reddit();
    first.verify_account().unwrap();
    second.verify_account().unwrap();
    assert_eq!(first.cache_namespace(), second.cache_namespace());
    assert!(first.cache_namespace().contains("account:fixture"));
}

#[test]
fn when_accounts_change_their_cache_namespaces_should_differ_without_containing_cookies() {
    let other = session::Session::parse(
        ".reddit.com\tTRUE\t/\tTRUE\t0\treddit_session\tother-session\n",
        0,
    )
    .unwrap();
    let reddit = Reddit::test_at("http://127.0.0.1:9/", false);
    let other = Reddit::with_transport(
        Transport::new().unwrap(),
        false,
        "http://127.0.0.1:9/",
        other,
    )
    .unwrap();
    let namespace = reddit.cache_namespace();
    assert!(!namespace.contains("fixture-session"));
    assert!(!other.cache_namespace().contains("other-session"));
    assert_ne!(namespace, other.cache_namespace());
    assert_eq!(
        namespace,
        Reddit::test_at("http://127.0.0.1:9/", false).cache_namespace()
    );
}
