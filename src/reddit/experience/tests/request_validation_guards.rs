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
fn when_searching_communities_it_should_read_the_search_page_and_follow_only_its_own_cursor() {
    let fixture = Fixture::new(|request| {
        let url = Url::parse(&format!("http://fixture{}", request.url)).unwrap();
        assert_eq!(url.path(), "/svc/shreddit/search/");
        let params: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(params["q"], "Հայերեն & rust");
        assert_eq!(params["type"], "communities");
        let next = if params.contains_key("cursor") {
            ""
        } else {
            r#"<faceplate-partial loading="lazy" src="/svc/shreddit/search/?q=%D5%80%D5%A1%D5%B5%D5%A5%D6%80%D5%A5%D5%B6+%26+rust&amp;type=communities&amp;cursor=next"></faceplate-partial>"#
        };
        Reply {
            status: 200,
            body: format!(
                r#"<div data-testid="search-community"><search-telemetry-tracker data-faceplate-tracking-context='{{"subreddit":{{"name":"rust","nsfw":false}}}}'></search-telemetry-tracker><img src="https://styles.redditmedia.com/rust.png"><p data-testid="search-subreddit-desc-text">Rust</p></div>{next}"#
            ),
            headers: vec![],
        }
    });
    let reddit = fixture.reddit();
    let search = |after: Option<String>| {
        reddit.experience(&Request::Read {
            query: Query::Search {
                text: "Հայերեն & rust".into(),
                kind: ListSearch::Communities,
            },
            after,
        })
    };
    let Ok(Response::Page(page)) = search(None) else {
        panic!("expected a page");
    };
    assert_eq!(page.entries[0].title, "r/rust");
    assert_eq!(
        page.entries[0].icon,
        crate::model::SubscriptionIcon::Url("https://styles.redditmedia.com/rust.png".into()),
        "a community result carries its icon"
    );
    let Ok(Response::Page(next)) = search(page.after.clone()) else {
        panic!("the page's own cursor must load");
    };
    assert_eq!((next.entries.len(), next.after), (1, None));
    for foreign in [
        "https://example.com/svc/shreddit/search/?q=x&type=communities&cursor=n",
        "/svc/shreddit/feeds/home-feed?cursor=n",
        "/svc/shreddit/search/?q=other&type=communities&cursor=n",
    ] {
        assert!(search(Some(foreign.into())).is_err(), "{foreign}");
    }
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
