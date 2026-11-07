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
fn when_people_search_returns_accounts_rows_should_carry_avatar_and_karma_and_hide_nsfw() {
    let person = |name: &str, nsfw: bool, avatar: &str, about: &str| {
        format!(
            r#"<div data-testid="search-author"><search-telemetry-tracker data-faceplate-tracking-context='{{"profile":{{"id":"t2_x","name":"{name}"}}}}'></search-telemetry-tracker><img src="{avatar}"><span data-testid="search-warnings">{}</span><p><faceplate-number number="12"></faceplate-number> karma</p><p data-testid="search-subreddit-desc-text"> {about} </p></div>"#,
            if nsfw {
                r#"<span class="text-category-nsfw">NSFW</span>"#
            } else {
                ""
            }
        )
    };
    let html = [
        person(
            "writer",
            false,
            "https://styles.redditmedia.com/a.png?s=1",
            "Hello",
        ),
        person(
            "adult",
            true,
            "https://www.redditstatic.com/avatar_over18.png",
            "",
        ),
    ]
    .concat();
    let fixture = Fixture::new(move |_| Reply {
        status: 200,
        body: html.clone(),
        headers: vec![],
    });
    let query = Query::Search {
        text: "w".into(),
        kind: ListSearch::People,
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
        [(
            "u/writer",
            "Karma: 12\nHello",
            &crate::model::SubscriptionIcon::Url("https://styles.redditmedia.com/a.png?s=1".into())
        )]
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

#[test]
fn a_search_page_whose_rows_cannot_be_read_should_fail_instead_of_showing_no_matches() {
    let api = Url::parse("https://www.reddit.com/").unwrap();
    let read = |html: &str| {
        super::super::search::search_page(html, "q", ListSearch::Communities, &api, false)
    };
    assert!(read(r#"<div data-testid="search-community"><span>r/changed</span></div>"#).is_err());
    let adult = r#"<div data-testid="search-community"><search-telemetry-tracker data-faceplate-tracking-context='{"subreddit":{"name":"adult","nsfw":true}}'></search-telemetry-tracker></div>"#;
    assert!(
        read(adult).unwrap().entries.is_empty(),
        "hidden NSFW rows are no error"
    );
    assert!(
        read("<main>No results</main>").is_err(),
        "an empty page needs Reddit's message"
    );
    let none =
        r#"<div data-testid="search-error-message">Hm...we couldn’t find any results for q</div>"#;
    assert!(read(none).unwrap().entries.is_empty());
}

#[test]
fn comment_search_rows_should_open_their_thread_at_the_comment_and_hide_nsfw() {
    let unit = |comment: &str, post: &str, sub: &str, nsfw: bool, body: &str| {
        format!(
            r#"<div data-testid="search-sdui-comment-unit"><search-telemetry-tracker data-faceplate-tracking-context='{{"comment":{{"id":"t1_{comment}","post_id":"t3_{post}"}},"post":{{"id":"t3_{post}","nsfw":{nsfw},"title":"Post {post}"}},"subreddit":{{"name":"{sub}","nsfw":false}}}}'></search-telemetry-tracker><faceplate-hovercard data-id="community-hover-card"><img src="https://styles.redditmedia.com/{sub}.png"></faceplate-hovercard><div data-testid="search-comment-content"><faceplate-partial src="/svc/shreddit/user-hover-card/writer?commentId=t1_{comment}"></faceplate-partial><div id="search-comment-t1_{comment}-post-rtjson-content"><p>{body}</p></div><faceplate-number number="12"></faceplate-number></div></div>"#
        )
    };
    let html = [
        unit("c1", "p1", "rust", false, "Use <strong>cargo</strong>"),
        unit("c2", "p2", "adult", true, "hidden"),
        r#"<faceplate-partial src="/svc/shreddit/search/?q=w&amp;type=comments&amp;cursor=next"></faceplate-partial>"#.into(),
    ]
    .concat();
    let fixture = Fixture::new(move |_| Reply {
        status: 200,
        body: html.clone(),
        headers: vec![],
    });
    let query = Query::Search {
        text: "w".into(),
        kind: ListSearch::Comments,
    };
    let Response::Page(actual) = read(&fixture.reddit(), query) else {
        panic!("Expected comments");
    };
    let [entry] = actual.entries.as_slice() else {
        panic!("Expected one shown comment: {:?}", actual.entries);
    };
    assert_eq!(entry.title, "Post p1");
    assert_eq!(
        crate::model::unformatted(&entry.body),
        "r/rust · u/writer · 12 votes\nUse cargo"
    );
    assert_eq!(
        entry.query,
        Some(Query::Thread("/r/rust/comments/p1/_/c1/".into()))
    );
    assert_eq!(
        thread_location("/r/rust/comments/p1/_/c1/")
            .unwrap()
            .1
            .as_deref(),
        Some("c1")
    );
    assert_eq!(
        actual.after.as_deref(),
        Some("/svc/shreddit/search/?q=w&type=comments&cursor=next")
    );
}

#[test]
fn media_search_should_read_the_named_posts_in_reddits_order_and_keep_its_cursor() {
    let fixture = Fixture::new(|request| {
        if let Some(id) = request
            .url
            .split("/comments/")
            .nth(1)
            .and_then(|rest| rest.split(".json").next())
        {
            if id == "gone" {
                return Reply {
                    status: 404,
                    body: String::new(),
                    headers: vec![],
                };
            }
            let title = id.to_uppercase();
            return Reply::json(json!([
                {"kind":"Listing","data":{"children":[{"kind":"t3","data":{"id":id,"title":title,"subreddit":"pics","author":"x","permalink":format!("/r/pics/comments/{id}/x/")}}]}},
                {"kind":"Listing","data":{"children":[]}}
            ]));
        }
        assert!(request.url.contains("type=media"), "{}", request.url);
        if request.url.contains("cursor=") {
            assert!(
                request.url.contains("sort=top") && request.url.contains("t=week"),
                "{}",
                request.url
            );
        }
        let unit = |id: &str| {
            format!(
                r#"<div data-id="search-media-post-unit"><search-telemetry-tracker data-faceplate-tracking-context='{{"post":{{"id":"t3_{id}"}}}}'></search-telemetry-tracker></div>"#
            )
        };
        Reply {
            status: 200,
            body: [
                unit("b"),
                unit("gone"),
                unit("a"),
                unit("b"),
                r#"<faceplate-partial src="/svc/shreddit/search/?q=cats&amp;type=media&amp;cursor=next"></faceplate-partial>"#.into(),
            ]
            .concat(),
            headers: vec![],
        }
    });
    let listing = fixture
        .reddit()
        .media_search_progress(
            "cats",
            SearchSort::Relevance,
            crate::model::TimeRange::All,
            None,
            &mut |_| {},
        )
        .unwrap();
    let titles: Vec<_> = listing
        .posts
        .iter()
        .map(|post| post.title.as_str())
        .collect();
    assert_eq!(titles, ["B", "A"]);
    assert_eq!(
        listing.after.as_deref(),
        Some("/svc/shreddit/search/?q=cats&type=media&cursor=next")
    );
    let next = fixture
        .reddit()
        .media_search_progress(
            "cats",
            SearchSort::Top,
            crate::model::TimeRange::Week,
            listing.after.as_deref(),
            &mut |_| {},
        )
        .unwrap();
    assert_eq!(next.posts.len(), 2, "the next page keeps the chosen sort");
}
