use super::*;

#[test]
fn when_nsfw_is_disabled_streamed_callbacks_should_only_publish_visible_posts() {
    let fixture = Fixture::new(|_| {
        let mut hidden = home_post("hidden", "s");
        hidden["data"]["over_18"] = json!(true);
        Reply::json(
            json!({"kind":"Listing","data":{"children":[hidden,home_post("safe","s")],"after":null}}),
        )
    });
    let mut published = Vec::new();

    let actual = fixture
        .reddit()
        .listing_progress("s", Sort::Hot, TimeRange::Day, None, &mut |event| {
            if let ListingEvent::Post(post) = event {
                published.push(post.id.clone());
            }
        })
        .unwrap();

    assert_eq!(published, ["safe"]);
    assert_eq!(
        actual
            .posts
            .iter()
            .map(|post| post.id.as_str())
            .collect::<Vec<_>>(),
        ["safe"]
    );
}

#[test]
fn user_submissions_should_stream_posts_with_sort_time_and_cursor() {
    let fixture = Fixture::new(|request| {
        let url = Url::parse(&format!("http://fixture{}", request.url)).unwrap();
        assert_eq!(url.path(), "/user/black-vault/submitted.json");
        let params: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(params["sort"], "top");
        assert_eq!(params["t"], "week");
        assert_eq!(params["after"], "t3_cursor&next");
        Reply::json(
            json!({"kind":"Listing","data":{"after":"t3_next","children":[
                {"kind":"t3","data":{"id":"p","title":"A submitted post","subreddit":"UFOs","author":"black-vault","permalink":"/r/UFOs/comments/p/title/"}}
            ]}}),
        )
    });
    let mut streamed = Vec::new();

    let actual = fixture
        .reddit()
        .user_posts_progress(
            "black-vault",
            crate::model::Sort::Top,
            crate::model::TimeRange::Week,
            Some("t3_cursor&next"),
            &mut |event| {
                if let super::super::ListingEvent::Post(post) = event {
                    streamed.push(post.id.clone());
                }
            },
        )
        .unwrap();

    assert_eq!(streamed, ["p"]);
    assert_eq!(actual.posts[0].subreddit, "UFOs");
    assert_eq!(actual.after.as_deref(), Some("t3_next"));
}

#[test]
fn user_submissions_should_reject_unsupported_sort_before_http() {
    let fixture = Fixture::new(|_| panic!("unsupported sort reached HTTP"));
    let actual = fixture.reddit().user_posts_progress(
        "blackvault",
        crate::model::Sort::Rising,
        crate::model::TimeRange::Day,
        None,
        &mut |_| {},
    );
    assert!(actual.is_err());
}
