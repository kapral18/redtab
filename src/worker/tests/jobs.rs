use super::*;

#[test]
fn when_a_comment_link_invalidates_a_post_it_should_use_the_same_key_as_the_canonical_post_link() {
    let original = Job::Post {
        generation: 1,
        permalink: "/r/rust/comments/post/title/".into(),
    }
    .cache_key("account")
    .unwrap()
    .0;
    let invalidation =
        Invalidation::Post("https://www.reddit.com/comments/post/_/comment/?context=3".into());
    let actual = Cache::key("account", &invalidation.request().unwrap());
    assert_eq!(actual, original);
}

#[test]
fn user_submitted_cache_keys_should_not_collide_with_a_same_named_community_or_cursor() {
    let query = FeedQuery::Listing {
        sort: Sort::New,
        time: TimeRange::Day,
    };
    let user = query
        .cache_request(&FeedTarget::User("Same".into()), None)
        .unwrap();
    assert_ne!(
        user,
        query
            .cache_request(&FeedTarget::Subreddit("same".into()), None)
            .unwrap()
    );
    assert_ne!(
        user,
        query
            .cache_request(&FeedTarget::User("same".into()), Some("next"))
            .unwrap()
    );
    assert_eq!(
        user,
        query
            .cache_request(&FeedTarget::User("same".into()), None)
            .unwrap()
    );
}

#[test]
fn when_home_and_a_subreddit_share_a_name_their_cache_keys_should_stay_separate() {
    let query = FeedQuery::Listing {
        sort: Sort::Best,
        time: TimeRange::Day,
    };
    assert_ne!(
        query.cache_request(&FeedTarget::Home, None),
        query.cache_request(&FeedTarget::Subreddit("home".into()), None)
    );
    assert_ne!(
        query.cache_request(&FeedTarget::Home, None),
        query.cache_request(&FeedTarget::Home, Some("t3_next"))
    );
}

#[test]
fn when_bodies_retain_markup_they_should_not_reuse_flattened_cached_documents() {
    let directory = tempfile::tempdir().unwrap();
    let cache = Cache::new(directory.path().to_path_buf());
    for (job, legacy, invalidation) in [
        (
            Job::Post {
                generation: 1,
                permalink: "/comments/post/title/".into(),
            },
            r#"["post-v3","post"]"#,
            Invalidation::Post("/comments/post/title/".into()),
        ),
        (
            Job::MoreReplies {
                generation: 1,
                comment_id: "comment".into(),
                cursor: "cursor".into(),
                minimum: None,
            },
            r#"["replies-v2","cursor"]"#,
            Invalidation::Replies("cursor".into()),
        ),
    ] {
        cache
            .store(
                &Cache::key("account", legacy),
                b"flattened document",
                crate::app::now(),
                cache.reserve_request().unwrap(),
            )
            .unwrap();
        let (actual, _) = job.cache_key("account").unwrap();
        assert!(cache.load(&actual, crate::app::now()).is_none());
        assert_eq!(
            actual,
            Cache::key("account", &invalidation.request().unwrap())
        );
    }
}
