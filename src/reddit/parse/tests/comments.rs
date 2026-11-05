use super::*;

#[test]
fn merging_two_large_cursors_stays_usable() {
    let cursor = |prefix: &str| {
        MoreRequest {
            post: "p".into(),
            parent: "t3_p".into(),
            children: (0..9_000).map(|i| format!("{prefix}{i}")).collect(),
            sort: String::new(),
        }
        .cursor(None)
    };
    let merged = merge_reply_continuations(Some(cursor("a")), Some(cursor("b")), &[]).unwrap();
    let request: MoreRequest = serde_json::from_str(&merged.cursor).unwrap();
    request.validate().unwrap();
    assert!(
        request.children.is_empty(),
        "large unions use a parent-focused read"
    );

    let pending = |children: &[&str]| {
        MoreRequest {
            post: "p".into(),
            parent: "t3_p".into(),
            children: children.iter().map(|id| (*id).to_string()).collect(),
            sort: String::new(),
        }
        .cursor(None)
    };
    let loaded = comment(json!({"id":"loaded", "body":"already visible"}), "p", 0).unwrap();
    let merged = merge_reply_continuations(
        Some(pending(&["loaded", "old", "old"])),
        Some(pending(&["new", "loaded"])),
        std::slice::from_ref(&loaded),
    )
    .unwrap();
    let request: MoreRequest = serde_json::from_str(&merged.cursor).unwrap();
    assert_eq!(request.children, ["old", "new"]);
    assert!(merge_reply_continuations(None, Some(pending(&["loaded"])), &[loaded]).is_none());
    for (a, b) in [
        (pending(&[]), pending(&["new"])),
        (pending(&["old"]), pending(&[])),
    ] {
        let merged = merge_reply_continuations(Some(a), Some(b), &[]).unwrap();
        let request: MoreRequest = serde_json::from_str(&merged.cursor).unwrap();
        assert!(
            request.children.is_empty(),
            "parent-focused continuation survives"
        );
    }
    let wrong_scope = MoreRequest {
        post: "q".into(),
        parent: "t3_q".into(),
        children: vec!["new".into()],
        sort: String::new(),
    }
    .cursor(None);
    assert_eq!(
        merge_reply_continuations(Some(pending(&["old"])), Some(wrong_scope), &[]),
        Some(pending(&["old"]))
    );
    let invalid = MoreReplies {
        cursor: "invalid cursor".into(),
        count: None,
    };
    assert_eq!(
        merge_reply_continuations(Some(invalid.clone()), Some(pending(&["new"])), &[]),
        Some(invalid)
    );
    assert!(merge_reply_continuations(None, None, &[]).is_none());
}

#[test]
fn a_huge_more_list_still_gives_a_usable_cursor() {
    let ids: Vec<String> = (0..10_050).map(|i| format!("c{i}")).collect();
    let more = Thing {
        kind: "more".into(),
        data: json!({"count": ids.len(), "children": ids}),
    };
    let (_, cursor) = comments_from(vec![more], "p", "t3_p", 0).unwrap();
    let cursor = cursor.unwrap();
    let request: MoreRequest = serde_json::from_str(&cursor.cursor).unwrap();
    request.validate().unwrap();
    // The row offers what it can load.
    assert_eq!(cursor.count, Some(MORE_LIMIT as u64));
}

#[test]
fn post_ids_come_from_the_permalink_position_not_the_first_comments_word() {
    for (permalink, id) in [
        ("/r/comments/comments/abc/x/", Some("abc")),
        ("/r/rust/comments/abc/comments/", Some("abc")),
        ("/user/me/comments/abc/x/", Some("abc")),
        ("/comments/abc", Some("abc")),
        ("/r/rust/abc/", None),
    ] {
        assert_eq!(post_id(permalink), id, "{permalink}");
    }
}

#[test]
fn json_listing_thread_and_gallery_keep_the_reader_contract() {
    let list = parse_listing(include_str!("../../../../tests/fixtures/sub.json")).unwrap();
    assert_eq!(list.posts.len(), 1);
    let p = &list.posts[0];
    assert_eq!(
        (&*p.id, &*p.subreddit, &*p.title),
        ("1x4v4sn", "neovim", "Error with Neovim and Tree Sitter")
    );
    assert_eq!(p.score, Some(12));
    assert_eq!(p.comments, 3);
    assert_eq!(p.created.unwrap().timestamp(), 1791849600);
    assert_eq!(list.after.as_deref(), Some("t3_1x4v4sn"));
    let page = parse_post_page(include_str!("../../../../tests/fixtures/post.json")).unwrap();
    assert_eq!(
        page.body,
        format!(
            "Body {}secret{}.",
            crate::model::SPOILER_START,
            crate::model::SPOILER_END
        )
    );
    let comments = &page.comments[0].replies;
    assert!(comments[0].is_op);
    assert!(comments[0].replies[0].is_mod);
    assert!(comments[0].replies[0].stickied && !comments[0].stickied);
    let cursor: MoreRequest =
        serde_json::from_str(&comments[0].more.as_ref().unwrap().cursor).unwrap();
    assert_eq!(cursor.children, ["c3"]);
    assert_eq!(cursor.parent, "t1_c1");
    let gallery = parse_post_page(include_str!("../../../../tests/fixtures/gallery.json")).unwrap();
    assert_eq!(
        gallery.post.media,
        Media::Gallery {
            images: vec![
                "https://i.redd.it/a.png".into(),
                "https://i.redd.it/b.png".into()
            ],
            previews: Vec::new(),
            thumb: None
        }
    );
}

#[test]
fn valid_empty_and_deleted_content_differ_from_errors_and_malformed_data() {
    let empty = json!({"kind":"Listing","data":{"children":[],"after":null}});
    assert!(parse_listing_value(empty).unwrap().posts.is_empty());
    for bad in [
        json!({"error":403}),
        json!({}),
        json!({"kind":"Listing","data":{}}),
        json!({"kind":"Listing","data":{"children":[{"kind":"t3","data":{}}]}}),
    ] {
        assert!(parse_listing_value(bad).is_err());
    }
    assert!(parse_post_page_value(json!([])).is_err());
    let deleted = comment(
        json!({"id":"abc","author":null,"body":"[deleted]","score_hidden":true,"score":99}),
        "post",
        0,
    )
    .unwrap();
    assert_eq!(deleted.author, "[deleted]");
    assert_eq!(deleted.body, "[deleted]");
    assert_eq!(deleted.score, None);
}

#[test]
fn top_level_more_is_an_expandable_container_and_poll_text_remains_readable() {
    let mut page: Value =
        serde_json::from_str(include_str!("../../../../tests/fixtures/post.json")).unwrap();
    page[0]["data"]["children"][0]["data"]["poll_data"] =
        json!({"total_vote_count":61,"options":[{"text":"Yes"},{"text":"No"}]});
    page[1]["data"]["children"]
        .as_array_mut()
        .unwrap()
        .push(json!({"kind":"more","data":{"children":["c9"],"count":2}}));
    let page = parse_post_page_value(page).unwrap();
    assert!(page.body.ends_with("Poll: 61 votes\n• Yes\n• No"));
    assert_eq!(page.comments.last().unwrap().id, "more:t3_1x4v4sn");
    assert!(page.comments.last().unwrap().author.is_empty());
}
