use super::*;

#[test]
fn when_congestion_subscription_icons_are_available_they_should_survive_the_payload() {
    let fixture = Fixture::new(|_| {
        Reply::json(json!({"kind":"Listing","data":{"children":[
        {"kind":"t5","data":{"display_name":"rust","community_icon":"https://styles.redditmedia.com/main.png","icon_img":"https://i.redd.it/fallback.png"}},
        {"kind":"t5","data":{"display_name":"vim","community_icon":"","icon_img":"https://i.redd.it/fallback.png"}},
        {"kind":"t5","data":{"display_name":"none","community_icon":"https://example.com/untrusted.png"}}
    ],"after":null}}))
    });

    let actual = serde_json::to_value(fixture.reddit().subscriptions().unwrap()).unwrap();

    assert_eq!(actual[0]["community"]["icon"], "missing");
    assert_eq!(
        actual[1]["community"]["icon"]["url"],
        "https://styles.redditmedia.com/main.png"
    );
    assert_eq!(
        actual[2]["community"]["icon"]["url"],
        "https://i.redd.it/fallback.png"
    );
}

#[test]
fn when_subscriptions_span_pages_they_should_all_load_without_duplicates() {
    let fixture = Fixture::new(|request| {
        assert!(request.headers.contains_key("cookie"));
        let url = Url::parse(&format!("http://fixture{}", request.url)).unwrap();
        assert_eq!(url.path(), "/subreddits/mine/subscriber.json");
        let query: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(query["limit"], "100");
        if query.contains_key("after") {
            assert_eq!(query["after"], "t5_cursor&next");
            Reply::json(json!({"kind":"Listing","data":{"children":[
                {"kind":"t5","data":{"display_name":"RUST"}},
                {"kind":"t5","data":{"display_name":"neovim"}}
            ],"after":null}}))
        } else {
            Reply::json(json!({"kind":"Listing","data":{"children":[
                {"kind":"t5","data":{"display_name":"rust"}}
            ],"after":"t5_cursor&next"}}))
        }
    });

    assert_eq!(
        fixture.reddit().subscriptions().unwrap(),
        [
            crate::model::Subscription::Community {
                name: "neovim".into(),
                icon: crate::model::SubscriptionIcon::Unknown
            },
            crate::model::Subscription::Community {
                name: "rust".into(),
                icon: crate::model::SubscriptionIcon::Unknown
            }
        ]
    );
}

#[test]
fn when_subscription_pagination_repeats_a_cursor_it_should_fail() {
    let fixture = Fixture::new(|_| {
        Reply::json(json!({
            "kind":"Listing","data":{"children":[],"after":"t5_loop"}
        }))
    });

    assert!(
        fixture
            .reddit()
            .subscriptions()
            .unwrap_err()
            .to_string()
            .contains("cursor")
    );
}

#[test]
fn when_the_account_has_no_subscriptions_it_should_return_an_empty_list() {
    let fixture = Fixture::new(|_| Reply::json(empty_listing()));
    assert!(fixture.reddit().subscriptions().unwrap().is_empty());
}

#[test]
fn when_a_later_subscription_page_fails_it_should_not_return_a_partial_list() {
    let fixture = Fixture::new(|request| {
        if request.url.contains("after=") {
            Reply::status(403)
        } else {
            Reply::json(json!({"kind":"Listing","data":{"children":[
                {"kind":"t5","data":{"display_name":"rust"}}
            ],"after":"t5_next"}}))
        }
    });
    assert!(fixture.reddit().subscriptions().is_err());
}

#[test]
fn when_subscription_json_is_malformed_it_should_fail_instead_of_clearing_membership() {
    let invalid = [
        json!({}),
        json!({"kind":"Listing","data":{"children":[{"kind":"t3","data":{"display_name":"rust"}}]}}),
        json!({"kind":"Listing","data":{"children":[{"kind":"t5","data":{}}]}}),
        json!({"kind":"Listing","data":{"children":[{"kind":"t5","data":{"display_name":"bad/name"}}]}}),
        json!({"kind":"Listing","data":{"children":[],"after":7}}),
    ];
    for value in invalid {
        let fixture = Fixture::new(move |_| Reply::json(value.clone()));
        assert!(fixture.reddit().subscriptions().is_err());
    }
}

#[test]
fn followed_profiles_should_preserve_type_and_identity_in_subscription_discovery() {
    let fixture = Fixture::new(|_| {
        Reply::json(serde_json::json!({
            "kind":"Listing", "data":{"after":null, "children":[
                {"kind":"t5","data":{"display_name":"u_regular","subreddit_type":"public","url":"/r/u_regular/"}},
                {"kind":"t5","data":{"display_name":"u_blackvault","subreddit_type":"user","display_name_prefixed":"u/blackvault","url":"/user/blackvault/","icon_img":"https://styles.redditmedia.com/avatar.png"}}
            ]}
        }))
    });

    let actual = serde_json::to_value(fixture.reddit().subscriptions().unwrap()).unwrap();

    assert!(
        actual
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["user"]["name"] == "blackvault"),
        "{actual}"
    );
    assert!(
        actual
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["community"]["name"] == "u_regular"),
        "{actual}"
    );
}

#[test]
fn conflicting_follow_identity_should_reject_the_whole_subscription_snapshot() {
    let fixture = Fixture::new(|_| {
        Reply::json(json!({"kind":"Listing","data":{"after":null,"children":[
            {"kind":"t5","data":{"display_name":"u_blackvault","subreddit_type":"user","url":"/user/another/","display_name_prefixed":"u/blackvault"}}
        ]}}))
    });
    assert!(fixture.reddit().subscriptions().is_err());
}
