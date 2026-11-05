use super::*;

#[test]
fn when_a_subreddit_is_named_comments_the_context_link_should_still_target_the_comment() {
    assert_eq!(
        thread_location("https://www.reddit.com/r/comments/comments/post/title/reply/?context=3")
            .unwrap(),
        (
            "/r/comments/comments/post/title/reply/".into(),
            Some("reply".into())
        )
    );
}

#[test]
fn when_people_search_returns_accounts_rows_should_carry_avatar_and_profile_and_hide_nsfw() {
    let fixture = Fixture::new(|_| {
        Reply::json(json!({"kind":"Listing","data":{"after":null,"children":[
            {"kind":"t2","data":{"name":"writer","icon_img":"https://styles.redditmedia.com/a.png?s=1","link_karma":5,"comment_karma":7,"subreddit":{"public_description":"Hello","over_18":false}}},
            {"kind":"t2","data":{"name":"gone","is_suspended":true}},
            {"kind":"t2","data":{"name":"adult","subreddit":{"over_18":true}}}
        ]}}))
    });
    let query = Query::Search {
        text: "w".into(),
        kind: ListSearch::People,
        time: crate::model::TimeRange::All,
    };
    let Response::Page(actual) = read(&fixture.reddit(), query) else {
        panic!("Expected people");
    };
    let shown: Vec<_> = actual
        .entries
        .iter()
        .map(|entry| (entry.title.as_str(), entry.body.as_str(), &entry.icon))
        .collect();
    assert_eq!(
        shown,
        [
            (
                "u/writer",
                "Post karma: 5    Comment karma: 7\nHello",
                &crate::model::SubscriptionIcon::Url(
                    "https://styles.redditmedia.com/a.png?s=1".into()
                )
            ),
            (
                "u/gone",
                "Suspended account",
                &crate::model::SubscriptionIcon::Unknown
            ),
        ]
    );
}

#[test]
fn collection_post_titles_should_distinguish_profile_posts_from_communities() {
    for (subreddit, permalink, label) in [
        (
            "u_blackvault",
            "/user/blackvault/comments/post/title/",
            "u/blackvault",
        ),
        (
            "u_ordinary",
            "/r/u_ordinary/comments/post/title/",
            "r/u_ordinary",
        ),
    ] {
        let actual = entry(&json!({"kind":"t3", "data":{
            "id":"post", "title":"Submission", "subreddit":subreddit,
            "author":"reader", "permalink":permalink
        }}))
        .unwrap()
        .unwrap();
        assert!(
            actual.title.contains(&format!(" · {label} · ")),
            "{}",
            actual.title
        );
    }
}

#[test]
fn when_an_external_link_is_presented_as_a_native_handoff_it_should_be_rejected() {
    assert!(reddit_url("https://evil.invalid/content").is_err());
    assert!(reddit_url("//evil.invalid/content").is_err());
    assert!(reddit_url("https://user:password@www.reddit.com/").is_err());
    assert_eq!(
        reddit_url("/r/rust").unwrap(),
        "https://www.reddit.com/r/rust"
    );
}

#[test]
fn when_a_wiki_request_is_denied_it_should_preserve_the_permission_error() {
    let fixture = Fixture::new(|_| Reply::status(403));
    let actual = fixture
        .reddit()
        .experience(&Request::Read {
            query: Query::Wiki {
                subreddit: "rust".into(),
                page: "index".into(),
            },
            after: None,
        })
        .unwrap_err();
    assert!(actual.to_string().contains("HTTP 403"));
}

#[test]
fn when_moderators_use_the_userlist_schema_their_profiles_should_be_navigable() {
    let fixture = Fixture::new(|request| {
        assert!(request.url.starts_with("/r/rust/about/moderators.json?"));
        Reply::json(json!({"kind":"UserList","data":{"children":[{"id":"t2_mod","name":"mod"}]}}))
    });
    let Response::Page(actual) = read(&fixture.reddit(), Query::Moderators("rust".into())) else {
        panic!("Expected moderators");
    };
    assert!(matches!(&actual.entries[0].query, Some(Query::Profile(user)) if user == "mod"));
}

#[test]
fn when_a_notification_is_a_comment_its_context_should_open_a_replyable_comment() {
    let fixture = Fixture::new(|_| {
        Reply::json(
            json!({"kind":"Listing","data":{"children":[{"kind":"t4","data":{"id":"reply","name":"t4_reply","was_comment":true,"author":"author","body":"reply","context":"/r/rust/comments/post/title/reply/?context=3"}}],"after":null}}),
        )
    });
    let Response::Page(actual) = read(&fixture.reddit(), Query::Inbox(Inbox::Unread)) else {
        panic!("Expected inbox");
    };
    assert_eq!(actual.entries[0].target.as_ref().unwrap().id, "t1_reply");
    assert_eq!(
        actual.entries[0].target.as_ref().unwrap().permalink,
        "/r/rust/comments/post/title/reply/?context=3"
    );
}

#[test]
fn when_a_saved_listing_contains_posts_and_comments_both_should_be_returned() {
    let fixture = Fixture::new(|request| {
        assert!(request.url.starts_with("/user/reader/saved.json?"));
        Reply::json(json!({"kind":"Listing","data":{"children":[
                {"kind":"t3","data":{"id":"post","title":"Saved post","subreddit":"rust","author":"a","permalink":"/r/rust/comments/post/a/"}},
                {"kind":"t1","data":{"id":"comment","author":"b","body":"saved reply","permalink":"/r/rust/comments/post/a/comment/","link_title":"Saved post","subreddit":"rust"}}
            ],"after":"t1_next"}}))
    });
    let Response::Page(actual) = read(
        &fixture.reddit(),
        Query::User {
            name: "reader".into(),
            feed: UserFeed::Saved,
        },
    ) else {
        panic!("Expected saved items");
    };
    assert_eq!(actual.entries.len(), 2);
    assert_eq!(actual.after.as_deref(), Some("t1_next"));
    assert_eq!(actual.entries[1].target.as_ref().unwrap().id, "t1_comment");
    assert_eq!(actual.entries[1].body, "saved reply");
}
