use super::*;

#[test]
fn private_community_malformed_body_should_remain_a_generic_denial() {
    let fixture = Fixture::new(|_| Reply {
        status: 403,
        body: "<html>private SECRET</html>".into(),
        headers: vec![],
    });
    let actual = fixture.reddit().search("doit", "query", None).unwrap_err();
    assert_eq!(
        actual.downcast_ref::<AccessDenied>(),
        Some(&AccessDenied::Forbidden)
    );
    assert!(!actual.to_string().contains("SECRET"));
}

#[test]
fn private_community_explicit_reason_should_survive_the_denied_listing() {
    let fixture = Fixture::new(|_| Reply {
        status: 403,
        body: json!({"error":403,"message":"Forbidden","reason":"private"}).to_string(),
        headers: vec![],
    });
    let reddit = fixture.reddit();

    let actual = reddit
        .listing("doit", Sort::Hot, TimeRange::Day, None)
        .unwrap_err();

    assert!(actual.to_string().contains("private"), "{actual}");
    assert_eq!(reddit.health(), Health::Up);
    assert!(!reddit.needs_sign_in());
}

#[test]
fn private_community_oversized_body_should_stay_bounded_and_redacted() {
    let fixture = Fixture::new(|_| Reply {
        status: 403,
        body: json!({"reason":"private","message":"SECRET".repeat(12_000)}).to_string(),
        headers: vec![],
    });
    let reddit = fixture.reddit();
    let actual = reddit
        .listing("doit", Sort::Hot, TimeRange::Day, None)
        .unwrap_err();
    assert_eq!(
        actual.downcast_ref::<AccessDenied>(),
        Some(&AccessDenied::Forbidden)
    );
    assert!(!actual.to_string().contains("SECRET"));
    assert_eq!(reddit.health(), Health::Up);
}

#[test]
fn private_community_reason_in_a_server_error_should_preserve_service_failure() {
    let fixture = Fixture::new(|_| Reply {
        status: 500,
        body: json!({"reason":"private"}).to_string(),
        headers: vec![],
    });
    let reddit = fixture.reddit();
    let actual = reddit
        .listing("doit", Sort::Hot, TimeRange::Day, None)
        .unwrap_err();
    assert!(actual.downcast_ref::<AccessDenied>().is_none());
    assert_eq!(reddit.health(), Health::Down);
}

#[test]
fn private_community_unknown_reason_should_remain_a_generic_denial() {
    let fixture = Fixture::new(|_| Reply {
        status: 403,
        body: json!({"reason":"quarantined","message":"private SECRET"}).to_string(),
        headers: vec![],
    });
    let actual = fixture
        .reddit()
        .listing("doit", Sort::Hot, TimeRange::Day, None)
        .unwrap_err();
    assert_eq!(
        actual.downcast_ref::<AccessDenied>(),
        Some(&AccessDenied::Forbidden)
    );
    assert!(!actual.to_string().contains("SECRET"));
    assert!(!actual.to_string().contains("private"));
}
