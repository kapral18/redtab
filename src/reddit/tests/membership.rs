use super::*;

#[test]
fn following_users_should_write_their_verified_profile_subscription() {
    let followed = Arc::new(AtomicBool::new(false));
    let actual = followed.clone();
    let fixture = Fixture::new(move |request| match request.url.as_str() {
        "/api/me.json" => Reply::json(json!({"data":{"name":"reader","modhash":"fixture-csrf"}})),
        "/user/black-vault/about.json" => {
            Reply::json(json!({"data":{"name":"black-vault","subreddit":{
                "name":"t5_profile", "subreddit_type":"user", "url":"/user/black-vault/"
            }}}))
        }
        "/api/subscribe" => {
            assert_eq!(request.method, "POST");
            assert_eq!(request.headers["x-modhash"], "fixture-csrf");
            let form: HashMap<_, _> = url::form_urlencoded::parse(request.body.as_bytes())
                .into_owned()
                .collect();
            assert_eq!(form["sr"], "t5_profile");
            assert!(!form.contains_key("sr_name"));
            actual.store(form["action"] == "sub", Ordering::SeqCst);
            Reply::json(json!({}))
        }
        _ => panic!("unexpected follow request"),
    });
    let target = MembershipTarget::User("black-vault".into());
    let reddit = fixture.saved_reddit(&saved_export("reader", "reader"));
    for join in [true, true, false, false, true] {
        reddit.set_membership(&target, join).unwrap();
        assert_eq!(followed.load(Ordering::SeqCst), join);
    }
}

#[test]
fn invalid_profile_identity_should_never_change_membership() {
    for profile in [
        json!({}),
        json!({"name":"other","subreddit":{"name":"t5_valid","subreddit_type":"user","url":"/user/reader/"}}),
        json!({"name":"reader","subreddit":{"name":"t5_valid","subreddit_type":"public","url":"/user/reader/"}}),
        json!({"name":"reader","subreddit":{"name":"t5_valid","subreddit_type":"user","url":"/user/other/"}}),
        json!({"name":"reader","subreddit":{"name":"t5_bad,other","subreddit_type":"user","url":"/user/reader/"}}),
    ] {
        let fixture = Fixture::new(move |request| {
            assert_eq!(
                request.method, "GET",
                "invalid profile must not reach a write"
            );
            match request.url.as_str() {
                "/api/me.json" => {
                    Reply::json(json!({"data":{"name":"reader","modhash":"fixture-csrf"}}))
                }
                "/user/reader/about.json" => Reply::json(json!({"data":profile})),
                _ => panic!("unexpected profile request"),
            }
        });
        assert!(
            fixture
                .reddit()
                .set_membership(&MembershipTarget::User("reader".into()), true)
                .is_err()
        );
    }
}

#[test]
fn invalid_user_membership_names_should_fail_before_http() {
    let fixture = Fixture::new(|_| panic!("invalid user reached HTTP"));
    for name in ["", "../reader", "u/reader", "Հայաստան"] {
        assert!(
            fixture
                .reddit()
                .set_membership(&MembershipTarget::User(name.into()), true)
                .is_err()
        );
    }
}

#[test]
fn membership_changes_should_not_write_through_a_different_signed_in_account() {
    let writes = std::sync::Arc::new(AtomicUsize::new(0));
    let actual = writes.clone();
    let fixture = Fixture::new(move |request| {
        if request.url == "/api/me.json" {
            return Reply::json(json!({"data":{"name":"someoneelse","modhash":"fixture-csrf"}}));
        }
        assert_eq!(request.url, "/api/subscribe");
        actual.fetch_add(1, Ordering::SeqCst);
        Reply::json(json!({}))
    });
    let reddit = fixture.saved_reddit(&saved_export("reader", "reader"));

    for join in [true, false] {
        let result = reddit.set_membership(
            &crate::model::MembershipTarget::Community("rust".into()),
            join,
        );
        assert!(result.unwrap_err().to_string().contains("account changed"));
    }
    assert_eq!(writes.load(Ordering::SeqCst), 0);
}

#[test]
fn when_community_membership_changes_it_should_mutate_the_server_for_join_leave_and_rejoin() {
    let joined = Arc::new(AtomicBool::new(false));
    let actual = joined.clone();
    let fixture = Fixture::new(move |request| match request.url.as_str() {
        "/api/me.json" => {
            assert_eq!(request.method, "GET");
            assert!(!request.headers.contains_key("x-modhash"));
            Reply::json(json!({"kind":"t2","data":{"name":"READER","modhash":"fixture-csrf"}}))
        }
        "/api/subscribe" => {
            assert_eq!(request.method, "POST");
            assert_eq!(request.headers["x-modhash"], "fixture-csrf");
            let form: HashMap<_, _> = url::form_urlencoded::parse(request.body.as_bytes())
                .into_owned()
                .collect();
            assert_eq!(form["sr_name"], "armenia");
            assert_eq!(form["api_type"], "json");
            let join = form["action"] == "sub";
            assert_eq!(
                form.get("skip_initial_defaults").map(String::as_str),
                join.then_some("true")
            );
            let was_joined = actual.swap(join, Ordering::SeqCst);
            if !join && !was_joined {
                return Reply::status(404);
            }
            Reply::json(json!({}))
        }
        _ => panic!("unexpected membership request"),
    });
    let reddit = fixture.saved_reddit(&saved_export("reader", "reader"));

    reddit
        .set_membership(
            &crate::model::MembershipTarget::Community("armenia".into()),
            true,
        )
        .unwrap();
    assert!(joined.load(Ordering::SeqCst));
    reddit
        .set_membership(
            &crate::model::MembershipTarget::Community("armenia".into()),
            true,
        )
        .unwrap();
    assert!(joined.load(Ordering::SeqCst));
    reddit
        .set_membership(
            &crate::model::MembershipTarget::Community("armenia".into()),
            false,
        )
        .unwrap();
    assert!(!joined.load(Ordering::SeqCst));
    assert!(
        reddit
            .set_membership(
                &crate::model::MembershipTarget::Community("armenia".into()),
                false
            )
            .is_err()
    );
    reddit
        .set_membership(
            &crate::model::MembershipTarget::Community("armenia".into()),
            true,
        )
        .unwrap();
    assert!(joined.load(Ordering::SeqCst));
}

#[test]
fn when_membership_identity_is_incomplete_it_should_never_post() {
    for data in [
        json!({}),
        json!({"name":"reader"}),
        json!({"modhash":"fixture-secret"}),
        json!({"name":"reader","modhash":""}),
        json!({"name":"reader","modhash":"fixture-secret\ninvalid"}),
    ] {
        let fixture = Fixture::new(move |request| {
            assert_eq!(request.method, "GET");
            assert_eq!(request.url, "/api/me.json");
            Reply::json(json!({"data":data}))
        });
        let error = fixture
            .reddit()
            .set_membership(
                &crate::model::MembershipTarget::Community("armenia".into()),
                true,
            )
            .unwrap_err();
        assert!(!format!("{error:#}").contains("fixture-secret"));
    }
}

#[test]
fn when_membership_target_is_not_a_community_it_should_not_request_anything() {
    let fixture = Fixture::new(|_| panic!("invalid membership target reached HTTP"));
    for name in [
        "",
        "../armenia",
        "r/armenia",
        "a+b",
        "all",
        "POPULAR",
        "Հայաստան",
    ] {
        assert!(
            fixture
                .reddit()
                .set_membership(
                    &crate::model::MembershipTarget::Community(name.into()),
                    true
                )
                .is_err()
        );
    }
}

#[test]
fn when_membership_has_no_success_acknowledgement_it_should_fail_without_echoing_the_body() {
    for (body, success) in [
        ("{}", true),
        (r#"{"json":{"errors":[]}}"#, true),
        (
            r#"{"json":{"errors":[["fixture-secret","denied"]]}}"#,
            false,
        ),
        (r#"{"error":"fixture-secret"}"#, false),
        (r#"{"json":{"errors":"fixture-secret"}}"#, false),
        ("null", false),
        ("[]", false),
        ("fixture-secret", false),
    ] {
        let posts = Arc::new(AtomicUsize::new(0));
        let count = posts.clone();
        let fixture = Fixture::new(move |request| {
            if request.url == "/api/me.json" {
                Reply::json(json!({"data":{"name":"reader","modhash":"fixture-csrf"}}))
            } else {
                assert_eq!(request.url, "/api/subscribe");
                count.fetch_add(1, Ordering::SeqCst);
                Reply {
                    status: 200,
                    body: body.into(),
                    headers: vec![],
                }
            }
        });
        let result = fixture.reddit().set_membership(
            &crate::model::MembershipTarget::Community("armenia".into()),
            true,
        );
        assert_eq!(result.is_ok(), success, "response {body}");
        if let Err(error) = result {
            assert!(!format!("{error:#}").contains("fixture-secret"));
        }
        assert_eq!(posts.load(Ordering::SeqCst), 1, "writes must never retry");
    }
}

#[test]
fn when_membership_http_fails_it_should_not_redirect_retry_or_echo_the_body() {
    for status in [401, 403, 404, 429, 302, 500] {
        let posts = Arc::new(AtomicUsize::new(0));
        let count = posts.clone();
        let fixture = Fixture::new(move |request| {
            if request.url == "/api/me.json" {
                Reply::json(json!({"data":{"name":"reader","modhash":"fixture-csrf"}}))
            } else {
                assert_eq!(request.url, "/api/subscribe");
                count.fetch_add(1, Ordering::SeqCst);
                Reply {
                    status,
                    body: "fixture-secret".into(),
                    headers: vec![("Location".into(), "https://other.invalid/".into())],
                }
            }
        });
        let error = fixture
            .reddit()
            .set_membership(
                &crate::model::MembershipTarget::Community("armenia".into()),
                true,
            )
            .unwrap_err();
        assert!(!format!("{error:#}").contains("fixture-secret"));
        assert_eq!(posts.load(Ordering::SeqCst), 1);
    }
}
