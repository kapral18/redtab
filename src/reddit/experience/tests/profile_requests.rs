use super::*;

#[test]
fn when_nsfw_is_hidden_an_nsfw_profile_should_keep_only_its_karma() {
    let fixture = Fixture::new(|request| {
        assert!(request.url.starts_with("/user/adult/about.json?"));
        Reply::json(
            json!({"kind":"t2","data":{"name":"adult","icon_img":"https://styles.redditmedia.com/a.png","link_karma":1,"comment_karma":2,"subreddit":{"public_description":"Hidden","over_18":true}}}),
        )
    });
    let actual = fixture.reddit().user_profile("adult").unwrap();
    assert_eq!(actual.icon, None);
    assert_eq!(actual.lines.len(), 1);
    assert!(!actual.lines[0].contains("Hidden"));
}
