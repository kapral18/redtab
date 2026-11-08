use super::*;

#[test]
fn expanding_replies_should_read_focused_comments_with_the_signed_in_session() {
    let fixture = Fixture::new(|request| {
        assert_eq!(request.headers["cookie"], "reddit_session=fixture-session");
        let url = Url::parse("http://localhost")
            .unwrap()
            .join(&request.url)
            .unwrap();
        assert_eq!(url.path(), "/comments/1x4v4sn.json");
        let query: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(query["comment"], "c3");
        assert_eq!(query["sort"], "new");
        let mut value: Value =
            serde_json::from_str(include_str!("../../../tests/fixtures/post.json")).unwrap();
        value[1]["data"]["children"] = json!([{"kind":"t1","data":{
            "id":"c3","name":"t1_c3","parent_id":"t3_1x4v4sn","link_id":"t3_1x4v4sn",
            "body":"expanded reply","replies":""
        }}]);
        Reply::json(value)
    });
    let cursor = json!({"post":"1x4v4sn","parent":"t3_1x4v4sn","children":["c3"],"sort":"new"});

    let actual = fixture.reddit().more_replies(&cursor.to_string()).unwrap();

    assert_eq!(actual.comments[0].id, "c3");
    assert_eq!(actual.comments[0].body, "expanded reply");
    assert_eq!(actual.refreshed, Some(vec!["c3".into()]));
    assert!(actual.more.is_none());
}

#[test]
fn continuing_a_thread_should_request_its_focused_parent() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let fixture = Fixture::new(move |r| {
        log.lock().push(r.url);
        Reply::status(404)
    });
    let reddit = Reddit::test_at(&fixture.base, false);
    let cursor = json!({"post": "abc", "parent": "t1_def", "children": []});
    assert!(reddit.more_replies(&cursor.to_string()).is_err());
    assert!(seen.lock()[0].starts_with("/comments/abc.json?"));
}
