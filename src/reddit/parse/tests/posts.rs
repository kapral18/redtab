use super::*;

#[test]
fn when_only_the_parent_author_is_deleted_the_post_should_not_be_labeled_deleted() {
    let fields = json!({
        "id":"cross", "title":"Shared video", "subreddit":"mechanical_gifs",
        "permalink":"/r/mechanical_gifs/comments/cross/shared/",
        "crosspost_parent_list":[{
            "author":"[deleted]", "title":"Original video",
            "subreddit":"oddlysatisfying", "permalink":"/r/oddlysatisfying/comments/parent/original/"
        }]
    });

    let (actual, body) = post(fields).unwrap();

    assert!(!actual.original_deleted);
    assert!(!body.contains("Original post deleted"));
}

#[test]
fn crosspost_attribution_should_use_the_parents_canonical_profile_identity() {
    for (subreddit, subtype, permalink, label) in [
        (
            "u_blackvault",
            "user",
            "/r/u_blackvault/comments/parent/title/",
            "u/blackvault",
        ),
        (
            "u_ordinary",
            "public",
            "/r/u_ordinary/comments/parent/title/",
            "r/u_ordinary",
        ),
    ] {
        let (_, body) = post(json!({
                "id":"p", "title":"Crosspost", "subreddit":"rust", "permalink":"/r/rust/comments/p/title/",
                "crosspost_parent_list":[{"subreddit":subreddit, "subreddit_type":subtype,
                    "permalink":permalink, "author":"reader", "title":"Parent"}]
            })).unwrap();
        assert!(
            body.contains(&format!("Crossposted from {label} by u/reader")),
            "{body}"
        );
        if subtype == "user" {
            assert!(body.contains("/user/blackvault/comments/parent/title/"));
        }
    }
    let (_, body) = post(json!({
            "id":"p", "title":"Crosspost", "subreddit":"rust", "permalink":"/r/rust/comments/p/title/",
            "crosspost_parent_list":[{"subreddit":"u_blackvault", "subreddit_type":"user", "permalink":"/invalid"}]
        })).unwrap();
    assert!(!body.contains("r/u_blackvault"));
}

#[test]
fn profile_owned_posts_should_keep_profile_identity_and_an_ordinary_u_community_should_not_change()
{
    let (profile, _) = post(json!({"id":"p","title":"Profile post","author":"blackvault","subreddit":"u_blackvault", "subreddit_type":"user","subreddit_name_prefixed":"u/blackvault", "permalink":"/r/u_blackvault/comments/p/title/"})).unwrap();
    assert_eq!(profile.permalink, "/user/blackvault/comments/p/title/");
    assert_eq!(profile.source_label(), "u/blackvault");
    assert_eq!(profile.community(), None);
    let (community, _) = post(json!({"id":"q","title":"Community post","subreddit":"u_regular","subreddit_type":"public","permalink":"/r/u_regular/comments/q/title/"})).unwrap();
    assert_eq!(community.source_label(), "r/u_regular");
    assert_eq!(community.community(), Some("u_regular"));
}
