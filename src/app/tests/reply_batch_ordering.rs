use super::*;

#[test]
fn review_regression_focused_descendant_expansion_should_keep_an_existing_sibling() {
    use crate::reddit::tests::{Fixture, Reply};
    use serde_json::json;
    let fixture = Fixture::new(|request| {
        let url = url::Url::parse("http://localhost")
            .unwrap()
            .join(&request.url)
            .unwrap();
        let query: HashMap<_, _> = url.query_pairs().into_owned().collect();
        let child = match query["comment"].as_str() {
            "leaf" => json!({"id":"leaf","parent_id":"t1_branch","body":"new reply"}),
            "branch" => {
                json!({"id":"branch","parent_id":"t1_parent","body":"ancestor","replies":{"kind":"Listing","data":{"children":[{"kind":"more","data":{"children":["next"],"count":1,"parent_id":"t1_branch"}}]}}})
            }
            _ => panic!("unexpected focus"),
        };
        Reply::json(json!([
            {"kind":"Listing","data":{"children":[{"kind":"t3","data":{"id":"p","title":"post","subreddit":"s","permalink":"/comments/p/"}}]}},
            {"kind":"Listing","data":{"children":[{"kind":"t1","data":child}]}}
        ]))
    });
    let expanded = fixture
        .reddit()
        .more_replies(&json!({"post":"p","parent":"t1_parent","children":["leaf"]}).to_string())
        .unwrap();
    let mut app = app_with(&[]);
    app.open_post_permalink(post("p", "s"), "/comments/p/".into());
    let generation = app.post_view.as_ref().unwrap().generation;
    app.on_msg(thread_message(
        generation,
        vec![comment(
            "parent",
            vec![comment(
                "branch",
                vec![comment("earlier", vec![], None)],
                Some(r#"{"post":"p","parent":"t1_branch","children":["leaf","pending"]}"#),
            )],
            Some("/more"),
        )],
    ));
    app.on_msg(Msg::MoreReplies {
        generation,
        comment_id: "parent".into(),
        result: Ok(expanded),
    });
    assert_eq!(reply_ids(&app, "branch"), ["leaf", "earlier"]);
    let page = app.post_view.as_ref().unwrap().page.as_ref().unwrap();
    let cursor: serde_json::Value = serde_json::from_str(
        &find_comment(&page.comments, "branch")
            .unwrap()
            .more
            .as_ref()
            .unwrap()
            .cursor,
    )
    .unwrap();
    assert_eq!(cursor["children"], json!(["pending", "next"]));
    app.on_msg(Msg::MoreReplies {
        generation,
        comment_id: "branch".into(),
        result: Ok(ReplyPage {
            comments: Vec::new(),
            more: None,
            refreshed: Some(vec!["leaf".into()]),
            order: Vec::new(),
        }),
    });
    assert_eq!(reply_ids(&app, "branch"), ["earlier"]);
}

#[test]
fn growing_reply_batches_stay_before_retained_later_comments() {
    let mut outcomes = Vec::new();
    for returned in [&["x", "b", "c"][..], &["x", "y"][..]] {
        let mut app = app_with(&[]);
        app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
        let generation = app.post_view.as_ref().unwrap().generation;
        let parent_page = || {
            thread_message(
                generation,
                vec![comment(
                    "parent",
                    vec![comment("a", vec![], None)],
                    Some("/more"),
                )],
            )
        };
        app.on_msg(parent_page());
        let batch =
            |comments: Vec<Comment>, covered: &[&str], remaining: &[&str]| Msg::MoreReplies {
                generation,
                comment_id: "parent".into(),
                result: Ok(ReplyPage {
                    order: comments
                        .iter()
                        .map(|c| c.id.clone())
                        .chain(remaining.iter().map(|id| id.to_string()))
                        .collect(),
                    comments,
                    refreshed: Some(covered.iter().map(|id| id.to_string()).collect()),
                    more: (!remaining.is_empty()).then(|| MoreReplies {
                        cursor: "/next".into(),
                        count: None,
                    }),
                }),
            };
        app.on_msg(batch(
            vec![
                comment("b", vec![], None),
                comment("c", vec![], None),
                comment("d", vec![comment("child", vec![], None)], Some("/child")),
            ],
            &["b", "c", "d"],
            &[],
        ));
        let view = app.post_view.as_mut().unwrap();
        view.free_scroll = false;
        view.reselect(Some(RowKey::Comment("child".into())));
        app.on_msg(parent_page());
        let remaining = if returned.contains(&"b") {
            &[][..]
        } else {
            &["b", "c"][..]
        };
        let remaining: Vec<_> = remaining.iter().copied().chain(["d"]).collect();
        app.on_msg(batch(
            returned
                .iter()
                .map(|id| comment(id, vec![], None))
                .collect(),
            returned,
            &remaining,
        ));
        let mut expected: Vec<_> = ["a"]
            .into_iter()
            .chain(returned.iter().copied())
            .chain(remaining.iter().copied())
            .map(str::to_string)
            .collect();
        outcomes.push((reply_ids(&app, "parent"), expected.clone()));
        app.on_msg(batch(
            remaining
                .iter()
                .map(|id| comment(id, vec![], (*id == "d").then_some("/child")))
                .collect(),
            &remaining,
            &[],
        ));
        outcomes.push((reply_ids(&app, "parent"), std::mem::take(&mut expected)));
        let view = app.post_view.as_ref().unwrap();
        assert!(matches!(view.cursor_key(), Some(RowKey::Comment(id)) if id == "child"));
        assert_eq!(
            view.page.as_ref().unwrap().comments[0]
                .replies
                .iter()
                .find(|c| c.id == "d")
                .unwrap()
                .replies[0]
                .id,
            "child"
        );
    }
    assert!(
        outcomes.iter().all(|(actual, expected)| actual == expected),
        "{outcomes:?}"
    );
}

#[test]
fn top_level_refresh_and_reexpansion_keep_later_batches_and_selection() {
    use serde_json::json;
    let page = |ids: &[&str], more: &[&str]| {
        let mut value: serde_json::Value =
            serde_json::from_str(include_str!("../../../tests/fixtures/post.json")).unwrap();
        let more_of = |children: &[&str]| {
            let count = children.len();
            json!({"kind": "more", "data": {"children": children, "count": count}})
        };
        let hidden = |child| json!({"kind": "Listing", "data": {"children": [more_of(&[child])]}});
        let mut children: Vec<_> = ids
            .iter()
            .map(|id| {
                let replies = match *id {
                    "c" => hidden("child"),
                    "d" => hidden("dchild"),
                    _ => json!(""),
                };
                json!({"kind": "t1", "data": {"id": id, "body": id, "replies": replies}})
            })
            .collect();
        if !more.is_empty() {
            children.push(more_of(more));
        }
        value[1]["data"]["children"] = json!(children);
        crate::reddit::parse::parse_post_page(&value.to_string()).unwrap()
    };
    let batch = |comments: Vec<Comment>, refreshed: &[&str], next: Option<&str>| ReplyPage {
        order: comments.iter().map(|c| c.id.clone()).collect(),
        comments,
        refreshed: Some(refreshed.iter().map(|s| s.to_string()).collect()),
        more: next.map(|cursor| MoreReplies {
            cursor: cursor.into(),
            count: None,
        }),
    };
    let mut app = app_with(&[]);
    let initial = page(&["a"], &["b", "c", "d"]);
    app.open_post_permalink(initial.post.clone(), initial.post.permalink.clone());
    let generation = app.post_view.as_ref().unwrap().generation;
    let root = initial.comments[0].id.clone();
    app.on_msg(Msg::Post {
        generation,
        result: Ok(Box::new(initial)),
    });
    for result in [
        batch(
            vec![
                comment("b", vec![], None),
                comment("c", vec![comment("child", vec![], None)], Some("/child")),
            ],
            &["b", "c"],
            Some("/second"),
        ),
        batch(
            vec![comment(
                "d",
                vec![comment("dchild", vec![], None)],
                Some("/dchild"),
            )],
            &["d"],
            None,
        ),
    ] {
        app.on_msg(Msg::MoreReplies {
            generation,
            comment_id: root.clone(),
            result: Ok(result),
        });
    }
    let view = app.post_view.as_mut().unwrap();
    view.free_scroll = false;
    view.reselect(Some(RowKey::Comment("dchild".into())));
    let check = |app: &App| {
        let view = app.post_view.as_ref().unwrap();
        let ids: Vec<_> = view
            .rows()
            .iter()
            .filter_map(|r| match r.kind {
                RowKind::Comment { comment, .. } => Some(comment.id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(ids, ["a", "b", "c", "child", "d", "dchild"]);
        assert!(matches!(view.cursor_key(), Some(RowKey::Comment(id)) if id == "dchild"));
    };
    check(&app);
    app.on_msg(Msg::Post {
        generation,
        result: Ok(Box::new(page(&["a", "b"], &["c", "d"]))),
    });
    check(&app);
    for result in [
        batch(
            vec![comment("c", vec![], Some("/child"))],
            &["c"],
            Some("/second"),
        ),
        batch(vec![comment("d", vec![], Some("/dchild"))], &["d"], None),
    ] {
        app.on_msg(Msg::MoreReplies {
            generation,
            comment_id: root.clone(),
            result: Ok(result),
        });
        check(&app);
    }
    app.on_msg(Msg::Post {
        generation,
        result: Ok(Box::new(page(&["a", "b", "c", "d"], &[]))),
    });
    check(&app);
    let loaded = app.post_view.as_ref().unwrap().page.as_ref().unwrap();
    assert!(
        find_comment(&loaded.comments, &root)
            .unwrap()
            .more
            .is_none()
    );
}
