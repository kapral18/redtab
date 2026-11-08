use super::*;

#[test]
fn when_info_is_forbidden_comment_actions_should_load_through_the_focused_thread() {
    let fixture = Fixture::new(|request| {
        if request.url.starts_with("/api/info.json") {
            return Reply::status(403);
        }
        let url = Url::parse(&format!("http://fixture{}", request.url)).unwrap();
        let query: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(request.method, "GET");
        assert_eq!(url.path(), "/comments/post.json");
        assert_eq!(query["comment"], "comment");
        assert_eq!(query["context"], "3");
        assert_eq!(query["raw_json"], "1");
        assert_eq!(query["limit"], "100");
        assert_eq!(query["depth"], "10");
        Reply::json(action_thread())
    });

    let Response::Details(actual) = read(&fixture.reddit(), Query::Inspect(comment_target()))
    else {
        panic!("Expected comment actions");
    };

    assert_eq!(actual.target.id, "t1_comment");
    assert_eq!(
        actual.target.permalink,
        "/r/rust/comments/post/title/comment/"
    );
    assert_eq!(actual.raw_text, ">!spoiler!<\n**bold**");
    assert!(actual.saved);
    assert_eq!(actual.vote, 1);
    assert!(actual.editable);
}

#[test]
fn post_actions_should_use_the_post_fullname_and_preserve_its_metadata() {
    let fixture = Fixture::new(|request| {
        assert!(request.url.starts_with("/comments/post.json?"));
        assert!(!request.url.contains("comment="));
        let mut thread = action_thread();
        thread[0]["data"]["children"][0]["data"]["crosspost_parent_list"] =
            json!([{"permalink":"/comments/original/title/"}]);
        Reply::json(thread)
    });
    let target = Target {
        id: "t3_post".into(),
        permalink: String::new(),
        ..comment_target()
    };

    let Response::Details(actual) = read(&fixture.reddit(), Query::Inspect(target)) else {
        panic!("Expected post actions");
    };

    assert_eq!(actual.target.id, "t3_post");
    assert_eq!(actual.raw_text, "post source");
    assert!(actual.hidden);
    assert!(!actual.saved);
    assert_eq!(actual.vote, 0);
    assert!(actual.editable);
    assert_eq!(
        actual.crosspost.as_deref(),
        Some("/comments/original/title/")
    );
}

#[test]
fn a_missing_comment_should_not_borrow_the_ancestors_action_state() {
    let fixture = Fixture::new(|_| Reply::json(action_thread()));
    let target = Target {
        id: "t1_missing".into(),
        ..comment_target()
    };

    let actual = fixture.reddit().experience(&Request::Read {
        query: Query::Inspect(target),
        after: None,
    });

    assert!(actual.is_err());
}

#[test]
fn a_conflicting_fullname_should_not_match_only_the_bare_comment_id() {
    let fixture = Fixture::new(|_| {
        let mut thread = action_thread();
        thread[1]["data"]["children"][0]["data"]["replies"]["data"]["children"][0]["data"]["name"] =
            json!("t1_other");
        Reply::json(thread)
    });

    let actual = fixture.reddit().experience(&Request::Read {
        query: Query::Inspect(comment_target()),
        after: None,
    });

    assert!(actual.is_err());
}

#[test]
fn malformed_thread_metadata_should_not_create_an_actions_menu() {
    let fixture = Fixture::new(|_| Reply::json(json!({"kind":"Listing","data":{"children":[]}})));

    let actual = fixture.reddit().experience(&Request::Read {
        query: Query::Inspect(comment_target()),
        after: None,
    });

    assert!(actual.is_err());
}
