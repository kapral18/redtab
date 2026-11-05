use super::*;

#[test]
fn when_a_wiki_page_is_missing_it_should_show_an_unavailable_page_without_a_request_error() {
    let fixture = Fixture::new(|_| Reply::status(404));
    let actual = read(
        &fixture.reddit(),
        Query::Wiki {
            subreddit: "rust".into(),
            page: "index".into(),
        },
    );
    assert!(
        matches!(actual, Response::Page(page) if page.entries.is_empty()
            && page.text == "This wiki page is unavailable. Reddit did not find a public page at this address."
            && page.url == "https://www.reddit.com/r/rust/wiki/index")
    );
}

#[test]
fn when_a_write_is_unacknowledged_it_should_not_treat_http_success_as_api_success() {
    let fixture = Fixture::new(|request| {
        if request.url == "/api/me.json" {
            me()
        } else {
            Reply::json(json!({"unexpected":true}))
        }
    });
    assert!(
        fixture
            .reddit()
            .experience(&Request::Write(Action::Save {
                id: "t3_post".into(),
                save: true
            }))
            .unwrap_err()
            .to_string()
            .contains("acknowledge")
    );
}

#[test]
fn when_an_edit_targets_another_author_it_should_not_send_the_mutation() {
    let fixture = Fixture::new(|request| {
        if request.url == "/api/me.json" {
            return me();
        }
        assert_eq!(request.method, "GET");
        let mut thread = action_thread();
        thread[1]["data"]["children"][0]["data"]["replies"]["data"]["children"][0]["data"]["author"] =
            json!("other");
        Reply::json(thread)
    });
    let reddit = fixture.saved_reddit(&saved_export("reader", "reader"));
    assert!(
        reddit
            .experience(&Request::Write(Action::Edit {
                target: comment_target(),
                text: "changed".into()
            }))
            .unwrap_err()
            .to_string()
            .contains("Only your own")
    );
}

#[test]
fn when_the_verified_account_changes_it_should_not_write_through_the_old_account() {
    let fixture = Fixture::new(|request| {
        assert_eq!(request.method, "GET");
        Reply::json(json!({"data":{"name":"someoneelse","modhash":"fixture-csrf"}}))
    });
    let reddit = fixture.saved_reddit(&saved_export("reader", "reader"));
    assert!(
        reddit
            .experience(&Request::Write(Action::Save {
                id: "t3_post".into(),
                save: true
            }))
            .unwrap_err()
            .to_string()
            .contains("account changed")
    );
}

#[test]
fn when_inputs_are_invalid_the_client_should_reject_them_before_any_request() {
    let fixture = Fixture::new(|_| panic!("Invalid input reached HTTP"));
    for action in [
        Action::Vote {
            id: "t3_post".into(),
            direction: 2,
        },
        Action::Save {
            id: "../bad".into(),
            save: true,
        },
        Action::Hide {
            id: "t1_comment".into(),
            hide: true,
        },
        Action::Reply {
            parent: "t3_post".into(),
            text: " ".into(),
        },
        Action::Submit {
            subreddit: "all".into(),
            title: "title".into(),
            text: "body".into(),
            link: false,
        },
        Action::Submit {
            subreddit: "rust".into(),
            title: "title".into(),
            text: "file:///private/file".into(),
            link: true,
        },
    ] {
        assert!(
            fixture
                .reddit()
                .experience(&Request::Write(action))
                .is_err()
        );
    }
}

#[test]
fn when_the_connection_is_retired_no_collection_or_write_should_contact_reddit() {
    let fixture = Fixture::new(|_| panic!("Retired session reached HTTP"));
    let reddit = fixture.reddit();
    reddit.retire();
    assert!(
        reddit
            .experience(&Request::Write(Action::Save {
                id: "t3_post".into(),
                save: true
            }))
            .is_err()
    );
    assert!(
        reddit
            .experience(&Request::Read {
                query: Query::Multis,
                after: None
            })
            .is_err()
    );
}

#[test]
fn when_searching_communities_the_query_time_and_cursor_should_be_encoded() {
    let fixture = Fixture::new(|request| {
        let url = Url::parse(&format!("http://fixture{}", request.url)).unwrap();
        assert_eq!(url.path(), "/subreddits/search");
        let params: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(params["q"], "Հայերեն & rust");
        assert_eq!(params["t"], "week");
        assert_eq!(params["after"], "cursor&next");
        assert!(!params.contains_key("restrict_sr"));
        Reply::json(json!({"kind":"Listing","data":{"children":[
                {"kind":"t5","data":{"display_name":"rust","community_icon":"https://styles.redditmedia.com/rust.png"}}
            ],"after":null}}))
    });
    let Response::Page(page) = fixture
        .reddit()
        .experience(&Request::Read {
            query: Query::Search {
                text: "Հայերեն & rust".into(),
                kind: ListSearch::Communities,
                time: TimeRange::Week,
            },
            after: Some("cursor&next".into()),
        })
        .unwrap()
    else {
        panic!("expected a page");
    };
    assert_eq!(
        page.entries[0].icon,
        crate::model::SubscriptionIcon::Url("https://styles.redditmedia.com/rust.png".into()),
        "a community result carries its listing icon"
    );
}

#[test]
fn when_reading_notifications_it_should_not_implicitly_mark_them_read() {
    let fixture = Fixture::new(|request| {
        let url = Url::parse(&format!("http://fixture{}", request.url)).unwrap();
        assert_eq!(url.path(), "/message/comments.json");
        assert!(url.query_pairs().any(|(k, v)| k == "mark" && v == "false"));
        Reply::json(json!({"kind":"Listing","data":{"children":[],"after":null}}))
    });
    assert!(matches!(
        read(&fixture.reddit(), Query::Inbox(Inbox::Replies)),
        Response::Page(_)
    ));
}

#[test]
fn invalid_comment_locations_should_fail_before_contacting_reddit() {
    let fixture = Fixture::new(|_| panic!("Invalid location reached HTTP"));
    for permalink in [
        "",
        "https://other.example/comments/post/title/comment/",
        "/comments/../title/comment/",
    ] {
        let target = Target {
            permalink: permalink.into(),
            ..comment_target()
        };
        assert!(
            fixture
                .reddit()
                .experience(&Request::Read {
                    query: Query::Inspect(target),
                    after: None
                })
                .is_err()
        );
    }
}
