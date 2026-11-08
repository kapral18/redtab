use super::*;

#[test]
fn editing_then_deleting_a_comment_should_recheck_the_captured_thread_target() {
    let writes = Arc::new(Mutex::new(Vec::new()));
    let actual = writes.clone();
    let fixture = Fixture::new(move |request| {
        if request.url == "/api/me.json" {
            return me();
        }
        if request.method == "GET" {
            assert!(request.url.starts_with("/comments/post.json?"));
            assert!(request.url.contains("comment=comment"));
            return Reply::json(action_thread());
        }
        let form: HashMap<_, _> = url::form_urlencoded::parse(request.body.as_bytes())
            .into_owned()
            .collect();
        match request.url.as_str() {
            "/api/editusertext" => {
                assert_eq!(form["thing_id"], "t1_comment");
                assert_eq!(form["text"], "changed text");
                actual.lock().unwrap().push("edit");
                Reply::json(
                    json!({"json":{"errors":[],"data":{"things":[{"kind":"t1","data":{"id":"comment"}}]}}}),
                )
            }
            "/api/del" => {
                assert_eq!(form["id"], "t1_comment");
                actual.lock().unwrap().push("delete");
                Reply::json(json!({}))
            }
            _ => panic!("Unexpected mutation route"),
        }
    });
    let reddit = fixture.saved_reddit(&saved_export("reader", "reader"));

    reddit
        .experience(&Request::Write(Action::Edit {
            target: comment_target(),
            text: "changed text".into(),
        }))
        .unwrap();
    reddit
        .experience(&Request::Write(Action::Delete {
            target: comment_target(),
        }))
        .unwrap();

    assert_eq!(*writes.lock().unwrap(), ["edit", "delete"]);
}

#[test]
fn editing_a_link_post_should_be_rejected_after_fresh_metadata_is_read() {
    let fixture = Fixture::new(|request| {
        if request.url == "/api/me.json" {
            return me();
        }
        assert_eq!(request.method, "GET");
        let mut thread = action_thread();
        thread[0]["data"]["children"][0]["data"]["is_self"] = json!(false);
        Reply::json(thread)
    });
    let reddit = fixture.saved_reddit(&saved_export("reader", "reader"));
    let target = Target {
        id: "t3_post".into(),
        permalink: "/comments/post".into(),
        ..comment_target()
    };

    let actual = reddit.experience(&Request::Write(Action::Edit {
        target,
        text: "changed".into(),
    }));

    assert!(
        actual
            .unwrap_err()
            .to_string()
            .contains("Only comments and text-post bodies")
    );
}

#[test]
fn when_votes_are_changed_and_removed_the_server_should_receive_each_explicit_direction() {
    let actual = Arc::new(Mutex::new(Vec::new()));
    let votes = actual.clone();
    let fixture = Fixture::new(move |request| {
        if request.url == "/api/me.json" {
            return me();
        }
        assert_eq!(request.url, "/api/vote");
        assert_eq!(request.method, "POST");
        assert_eq!(request.headers["x-modhash"], "fixture-csrf");
        let form: HashMap<_, _> = url::form_urlencoded::parse(request.body.as_bytes())
            .into_owned()
            .collect();
        assert_eq!(form["id"], "t1_comment");
        votes.lock().unwrap().push(form["dir"].clone());
        Reply::json(json!({}))
    });
    let reddit = fixture.reddit();
    for direction in [1, -1, 0] {
        reddit
            .experience(&Request::Write(Action::Vote {
                id: "t1_comment".into(),
                direction,
            }))
            .unwrap();
    }
    assert_eq!(*actual.lock().unwrap(), ["1", "-1", "0"]);
}

#[test]
fn when_saving_hiding_and_following_posts_the_forms_should_match_the_documented_contract() {
    let actual = Arc::new(Mutex::new(Vec::new()));
    let forms = actual.clone();
    let fixture = Fixture::new(move |request| {
        if request.url == "/api/me.json" {
            return me();
        }
        let form: HashMap<_, _> = url::form_urlencoded::parse(request.body.as_bytes())
            .into_owned()
            .collect();
        forms.lock().unwrap().push((request.url, form));
        Reply::json(json!({}))
    });
    let reddit = fixture.reddit();
    for action in [
        Action::Save {
            id: "t1_comment".into(),
            save: true,
        },
        Action::Save {
            id: "t1_comment".into(),
            save: false,
        },
        Action::Hide {
            id: "t3_post".into(),
            hide: true,
        },
        Action::Hide {
            id: "t3_post".into(),
            hide: false,
        },
        Action::FollowPost {
            id: "t3_post".into(),
            follow: true,
        },
        Action::FollowPost {
            id: "t3_post".into(),
            follow: false,
        },
    ] {
        reddit.experience(&Request::Write(action)).unwrap();
    }
    let actual = actual.lock().unwrap();
    assert_eq!(
        actual
            .iter()
            .map(|(path, _)| path.as_str())
            .collect::<Vec<_>>(),
        [
            "/api/save",
            "/api/unsave",
            "/api/hide",
            "/api/unhide",
            "/api/follow_post",
            "/api/follow_post"
        ]
    );
    assert_eq!(actual[0].1["id"], "t1_comment");
    assert_eq!(actual[4].1["fullname"], "t3_post");
    assert_eq!(actual[4].1["follow"], "true");
    assert_eq!(actual[5].1["follow"], "false");
}

#[test]
fn when_replying_with_unicode_and_markdown_the_body_should_round_trip_without_command_interpretation()
 {
    let fixture = Fixture::new(|request| {
        if request.url == "/api/me.json" {
            return me();
        }
        assert_eq!(request.url, "/api/comment");
        let form: HashMap<_, _> = url::form_urlencoded::parse(request.body.as_bytes())
            .into_owned()
            .collect();
        assert_eq!(form["thing_id"], "t1_parent");
        assert_eq!(form["text"], "**Հայերեն**\n$(not-a-command) & x");
        Reply::json(
            json!({"json":{"errors":[],"data":{"things":[{"kind":"t1","data":{"id":"new","parent_id":"t1_parent"}}]}}}),
        )
    });
    assert!(matches!(
        fixture
            .reddit()
            .experience(&Request::Write(Action::Reply {
                parent: "t1_parent".into(),
                text: "**Հայերեն**\n$(not-a-command) & x".into()
            }))
            .unwrap(),
        Response::Written { .. }
    ));
}

#[test]
fn when_submitting_a_link_the_url_should_be_separate_from_the_title_and_return_a_reddit_link() {
    let fixture = Fixture::new(|request| {
        if request.url == "/api/me.json" {
            return me();
        }
        assert_eq!(request.url, "/api/submit");
        let form: HashMap<_, _> = url::form_urlencoded::parse(request.body.as_bytes())
            .into_owned()
            .collect();
        assert_eq!(form["kind"], "link");
        assert_eq!(form["sr"], "rust");
        assert_eq!(form["title"], "A useful link");
        assert_eq!(form["url"], "https://example.org/?x=1&y=2");
        Reply::json(
            json!({"json":{"errors":[],"data":{"url":"https://www.reddit.com/r/rust/comments/post/a/","name":"t3_post"}}}),
        )
    });
    let actual = fixture
        .reddit()
        .experience(&Request::Write(Action::Submit {
            subreddit: "rust".into(),
            title: "A useful link".into(),
            text: "https://example.org/?x=1&y=2".into(),
            link: true,
        }))
        .unwrap();
    assert!(
        matches!(actual, Response::Written { url: Some(url) } if url == "https://www.reddit.com/r/rust/comments/post/a/")
    );
}

#[test]
fn when_submitting_a_text_post_its_markdown_should_remain_in_the_text_field() {
    let fixture = Fixture::new(|request| {
        if request.url == "/api/me.json" {
            return me();
        }
        assert_eq!(request.method, "POST");
        assert_eq!(request.url, "/api/submit");
        let form: HashMap<_, _> = url::form_urlencoded::parse(request.body.as_bytes())
            .into_owned()
            .collect();
        assert_eq!(form["kind"], "self");
        assert_eq!(form["sr"], "rust");
        assert_eq!(form["title"], "A text post");
        assert_eq!(form["text"], "**Հայերեն**\nA & B");
        assert!(!form.contains_key("url"));
        Reply::json(json!({"json":{"errors":[],"data":{"url":"/r/rust/comments/post/text/"}}}))
    });

    let actual = fixture
        .reddit()
        .experience(&Request::Write(Action::Submit {
            subreddit: "rust".into(),
            title: "A text post".into(),
            text: "**Հայերեն**\nA & B".into(),
            link: false,
        }))
        .unwrap();

    assert!(matches!(actual, Response::Written { url: Some(url) }
            if url == "https://www.reddit.com/r/rust/comments/post/text/"));
}

#[test]
fn when_reporting_a_comment_the_reason_should_be_sent_with_its_fullname() {
    let fixture = Fixture::new(|request| {
        if request.url == "/api/me.json" {
            return me();
        }
        assert_eq!(request.method, "POST");
        assert_eq!(request.url, "/api/report");
        assert_eq!(request.headers["x-modhash"], "fixture-csrf");
        let form: HashMap<_, _> = url::form_urlencoded::parse(request.body.as_bytes())
            .into_owned()
            .collect();
        assert_eq!(form["thing_id"], "t1_comment");
        assert_eq!(form["reason"], "Spam & unwanted promotion");
        Reply::json(json!({}))
    });

    let actual = fixture
        .reddit()
        .experience(&Request::Write(Action::Report {
            id: "t1_comment".into(),
            reason: "Spam & unwanted promotion".into(),
        }))
        .unwrap();

    assert!(matches!(actual, Response::Written { url: None }));
}

#[test]
fn when_inbox_items_are_marked_read_unread_and_read_the_explicit_state_should_be_sent() {
    let actual = Arc::new(Mutex::new(Vec::new()));
    let requests = actual.clone();
    let fixture = Fixture::new(move |request| {
        if request.url == "/api/me.json" {
            return me();
        }
        assert_eq!(request.method, "POST");
        let form: HashMap<_, _> = url::form_urlencoded::parse(request.body.as_bytes())
            .into_owned()
            .collect();
        requests
            .lock()
            .unwrap()
            .push((request.url, form["id"].clone()));
        Reply::json(json!({}))
    });
    let reddit = fixture.reddit();

    for (id, read) in [
        ("t4_message", true),
        ("t4_message", false),
        ("t1_comment", true),
    ] {
        reddit
            .experience(&Request::Write(Action::Read {
                id: id.into(),
                read,
            }))
            .unwrap();
    }

    assert_eq!(
        *actual.lock().unwrap(),
        [
            ("/api/read_message".into(), "t4_message".into()),
            ("/api/unread_message".into(), "t4_message".into()),
            ("/api/read_message".into(), "t1_comment".into()),
        ]
    );
}

#[test]
fn when_the_api_reports_an_error_the_write_should_not_claim_success_or_echo_submitted_text() {
    let fixture = Fixture::new(|request| {
        if request.url == "/api/me.json" {
            return me();
        }
        Reply::json(json!({"json":{"errors":[["RATELIMIT","private submitted text","text"]]}}))
    });
    let error = fixture
        .reddit()
        .experience(&Request::Write(Action::Reply {
            parent: "t3_post".into(),
            text: "private submitted text".into(),
        }))
        .unwrap_err()
        .to_string();
    assert!(error.contains("RATELIMIT"));
    assert!(!error.contains("private submitted text"));
}

#[test]
fn when_a_custom_feed_is_created_added_removed_and_deleted_the_methods_should_match_the_multi_contract()
 {
    let actual = Arc::new(Mutex::new(Vec::new()));
    let methods = actual.clone();
    let fixture = Fixture::new(move |request| {
        if request.url == "/api/me.json" {
            return me();
        }
        methods
            .lock()
            .unwrap()
            .push((request.method.clone(), request.url.clone()));
        if request.method == "DELETE" {
            return Reply {
                status: 204,
                body: String::new(),
                headers: vec![],
            };
        }
        if request.url.ends_with("/r/rust") {
            Reply::json(json!({"name":"rust"}))
        } else {
            Reply::json(json!({"kind":"LabeledMulti","data":{"path":"/user/reader/m/tools"}}))
        }
    });
    let reddit = fixture.saved_reddit(&saved_export("reader", "reader"));
    for action in [
        Action::CreateMulti {
            name: "tools".into(),
            description: "Tools".into(),
        },
        Action::MultiCommunity {
            owner: "reader".into(),
            name: "tools".into(),
            subreddit: "rust".into(),
            add: true,
        },
        Action::MultiCommunity {
            owner: "reader".into(),
            name: "tools".into(),
            subreddit: "rust".into(),
            add: false,
        },
        Action::DeleteMulti {
            owner: "reader".into(),
            name: "tools".into(),
        },
    ] {
        reddit.experience(&Request::Write(action)).unwrap();
    }
    assert_eq!(
        *actual.lock().unwrap(),
        [
            ("POST".into(), "/api/multi/user/reader/m/tools".into()),
            ("PUT".into(), "/api/multi/user/reader/m/tools/r/rust".into()),
            (
                "DELETE".into(),
                "/api/multi/user/reader/m/tools/r/rust".into()
            ),
            ("DELETE".into(), "/api/multi/user/reader/m/tools".into())
        ]
    );
}
