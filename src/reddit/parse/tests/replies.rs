use super::*;

#[test]
fn sorted_expansion_should_keep_both_returned_and_unattempted_continuations() {
    let request = MoreRequest {
        post: "p".into(),
        parent: "t1_parent".into(),
        children: ["a", "b", "c", "d", "tail"].map(str::to_string).into(),
        sort: "new".into(),
    };
    let page = expanded_replies(&request, |id| Ok(json!([
            {"kind":"Listing","data":{"children":[{"kind":"t3","data":{"id":"p","title":"post","subreddit":"s","permalink":"/comments/p/"}}]}},
            {"kind":"Listing","data":{"children":[{"kind":"t1","data":{"id":"parent","parent_id":"t3_p","body":"parent","replies":{"kind":"Listing","data":{"children":[
                {"kind":"t1","data":{"id":id,"parent_id":"t1_parent","body":"reply"}},
                {"kind":"more","data":{"parent_id":"t1_parent","children":["next"],"count":1}}
            ]}}}}]}}
        ]))).unwrap();
    let next: MoreRequest = serde_json::from_str(&page.more.unwrap().cursor).unwrap();
    assert_eq!(next.children, ["next", "tail"]);
    assert_eq!(next.sort, "new");
}

#[test]
fn focused_expansions_should_preserve_nested_replies_and_skip_returned_or_deleted_ids() {
    let more = MoreRequest {
        post: "p".into(),
        parent: "t3_p".into(),
        children: ["c0", "c1", "gone", "c2", "tail"].map(str::to_owned).into(),
        sort: "new".into(),
    };
    let actual = expanded_replies(&more, |id| Ok(focused_thread("p", match id {
            "c0" => json!([{"kind":"t1","data":{"id":"c0","parent_id":"t3_p","body":"first","replies":{
                "kind":"Listing","data":{"children":[
                    {"kind":"t1","data":{"id":"c1","parent_id":"t1_c0","body":"nested"}},
                    {"kind":"more","data":{"parent_id":"t1_c0","children":["c1","deeper"],"count":2}}
                ]}
            }}}]),
            "c2" => json!([{"kind":"t1","data":{"id":"c2","parent_id":"t3_p","body":"second"}}]),
            "gone" => json!([]),
            _ => panic!("returned descendants and later pages must not be fetched"),
        }))).unwrap();
    assert_eq!(actual.order, ["c0", "c2", "tail"]);
    assert_eq!(actual.comments[0].replies[0].id, "c1");
    let nested: MoreRequest =
        serde_json::from_str(&actual.comments[0].more.as_ref().unwrap().cursor).unwrap();
    assert_eq!(nested.children, ["deeper"]);
    let next: MoreRequest = serde_json::from_str(&actual.more.as_ref().unwrap().cursor).unwrap();
    assert_eq!(next.children, ["tail"]);
    assert_eq!(next.sort, "new");
    assert!(actual.refreshed.as_ref().unwrap().contains(&"gone".into()));
    assert!(!actual.refreshed.as_ref().unwrap().contains(&"tail".into()));
    assert_eq!(
        actual,
        serde_json::from_slice::<ReplyPage>(&serde_json::to_vec(&actual).unwrap()).unwrap()
    );
}

#[test]
fn a_focused_descendant_should_fetch_its_missing_ancestor_without_reparenting_it() {
    let more = MoreRequest {
        post: "p".into(),
        parent: "t3_p".into(),
        children: vec!["leaf".into()],
        sort: String::new(),
    };
    let actual = expanded_replies(&more, |id| Ok(focused_thread("p", match id {
            "leaf" => json!([{"kind":"t1","data":{"id":"leaf","parent_id":"t1_branch","body":"leaf"}}]),
            "branch" => json!([{"kind":"t1","data":{"id":"branch","parent_id":"t3_p","body":"branch"}}]),
            _ => panic!("unexpected ancestor"),
        }))).unwrap();
    assert_eq!(actual.comments[0].id, "branch");
    assert_eq!(actual.comments[0].replies[0].id, "leaf");
    assert!(actual.more.is_none());
    assert!(actual.refreshed.is_some());
}

#[test]
fn a_focused_comment_from_another_post_or_parent_should_be_rejected() {
    let more = MoreRequest {
        post: "p".into(),
        parent: "t1_parent".into(),
        children: vec!["child".into()],
        sort: String::new(),
    };
    assert!(expanded_replies(&more, |_| Ok(focused_thread("other", json!([])))).is_err());
    assert!(
        expanded_replies(&more, |_| Ok(focused_thread(
            "p",
            json!([
                {"kind":"t1","data":{"id":"child","parent_id":"t3_p","body":"outside"}}
            ])
        )))
        .is_err()
    );
}
