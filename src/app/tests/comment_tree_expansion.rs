use super::*;

#[test]
fn broader_expansion_should_remove_covered_descendants_and_keep_protected_replies() {
    let mut outcomes = Vec::new();
    for mode in 0..3 {
        let mut app = app_with(&[]);
        app.open_post_permalink(post("p", "s"), "/comments/p/".into());
        let generation = app.post_view.as_ref().unwrap().generation;
        let mut siblings = vec![comment("earlier", vec![], None)];
        if mode == 1 {
            siblings.push(comment("leaf", vec![], None));
        }
        app.on_msg(ordered_snapshot(
            thread_message(
                generation,
                vec![comment(
                    "parent",
                    vec![comment("branch", siblings, Some("/branch-more"))],
                    Some("/parent-more"),
                )],
            ),
            1,
            false,
            false,
        ));
        let leaf = || Msg::MoreReplies {
            generation,
            comment_id: "branch".into(),
            result: Ok(ReplyPage {
                comments: vec![comment("leaf", vec![], None)],
                refreshed: Some(vec!["leaf".into()]),
                more: None,
                order: Vec::new(),
            }),
        };
        if mode != 1 {
            app.on_msg(ordered_snapshot(
                leaf(),
                if mode == 2 { 4 } else { 2 },
                false,
                false,
            ));
        }
        let omission = || Msg::MoreReplies {
            generation,
            comment_id: "parent".into(),
            result: Ok(ReplyPage {
                comments: Vec::new(),
                refreshed: Some(vec!["leaf".into()]),
                more: Some(MoreReplies {
                    cursor: "/parent-next".into(),
                    count: None,
                }),
                order: Vec::new(),
            }),
        };
        app.on_msg(ordered_snapshot(omission(), 3, false, false));
        outcomes.push(reply_ids(&app, "branch").contains(&"leaf".into()));
        assert!(reply_ids(&app, "branch").contains(&"earlier".into()));
        app.on_msg(ordered_snapshot(leaf(), 5, false, false));
        app.on_msg(ordered_snapshot(omission(), 3, true, true));
        assert!(
            reply_ids(&app, "branch").contains(&"leaf".into()),
            "newer re-add survives stale omission"
        );
    }
    assert_eq!(outcomes, [false, true, true]);
}

#[test]
fn cached_ancestor_cannot_resurrect_a_removed_reply_but_newer_data_can() {
    let mut app = app_with(&[]);
    app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
    let generation = app.post_view.as_ref().unwrap().generation;
    let parent = |id| comment("parent", vec![comment(id, vec![], None)], Some("/parent"));
    let page = |children| {
        thread_message(
            generation,
            vec![comment("ancestor", children, Some("/ancestor"))],
        )
    };
    let ancestor = || Msg::MoreReplies {
        generation,
        comment_id: "ancestor".into(),
        result: Ok(ReplyPage::complete(vec![parent("b")])),
    };
    app.on_msg(ordered_snapshot(page(vec![]), 1, false, false));
    app.on_msg(ordered_snapshot(ancestor(), 2, true, false));
    app.on_msg(ordered_snapshot(page(vec![parent("a")]), 3, false, false));
    app.on_msg(ordered_snapshot(page(vec![]), 4, false, false));
    app.on_msg(ordered_snapshot(
        Msg::MoreReplies {
            generation,
            comment_id: "parent".into(),
            result: Ok(ReplyPage {
                comments: vec![],
                refreshed: Some(vec!["b".into()]),
                order: vec![],
                more: Some(MoreReplies {
                    cursor: "/parent-next".into(),
                    count: None,
                }),
            }),
        },
        5,
        false,
        false,
    ));
    let has_b = |app: &App| reply_ids(app, "parent").iter().any(|id| id == "b");
    let removed = has_b(&app);
    app.on_msg(ordered_snapshot(ancestor(), 2, true, true));
    let stale = has_b(&app);
    app.on_msg(ordered_snapshot(ancestor(), 6, false, false));
    assert_eq!([removed, stale, has_b(&app)], [false, false, true]);
}

#[test]
fn explicit_omissions_remember_unseen_ids_and_advance_without_rolling_back() {
    let mut outcomes = Vec::new();
    for loaded in [false, true] {
        let mut app = app_with(&[]);
        app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
        let generation = app.post_view.as_ref().unwrap().generation;
        let parent = |ids: &[&str]| {
            comment(
                "parent",
                ids.iter().map(|id| comment(id, vec![], None)).collect(),
                Some("/parent"),
            )
        };
        let page = |children| {
            thread_message(
                generation,
                vec![comment("ancestor", children, Some("/ancestor"))],
            )
        };
        app.on_msg(ordered_snapshot(
            page(vec![parent(if loaded { &["a", "b"] } else { &["a"] })]),
            1,
            false,
            false,
        ));
        app.on_msg(ordered_snapshot(
            page(vec![parent(&["a"])]),
            3,
            false,
            false,
        ));
        let omission = || Msg::MoreReplies {
            generation,
            comment_id: "parent".into(),
            result: Ok(ReplyPage {
                comments: vec![],
                refreshed: Some(vec!["b".into()]),
                order: vec![],
                more: Some(MoreReplies {
                    cursor: "/next".into(),
                    count: None,
                }),
            }),
        };
        for sequence in [5, 7, 5] {
            app.on_msg(ordered_snapshot(omission(), sequence, false, false));
        }
        let ancestor = || Msg::MoreReplies {
            generation,
            comment_id: "ancestor".into(),
            result: Ok(ReplyPage::complete(vec![parent(&["b"])])),
        };
        let has_b = |app: &App| reply_ids(app, "parent").iter().any(|id| id == "b");
        app.on_msg(ordered_snapshot(ancestor(), 6, true, true));
        let stale = has_b(&app);
        app.on_msg(ordered_snapshot(ancestor(), 8, false, false));
        outcomes.push([stale, has_b(&app)]);
    }
    assert_eq!(outcomes, [[false, true], [false, true]]);
}

#[test]
fn parent_page_demotion_allows_later_batch_order_and_removal() {
    let mut app = app_with(&[]);
    app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
    let generation = app.post_view.as_ref().unwrap().generation;
    let page = |ids: &[&str]| {
        thread_message(
            generation,
            vec![comment(
                "parent",
                ids.iter().map(|id| comment(id, vec![], None)).collect(),
                Some("/more"),
            )],
        )
    };
    let batch = |ids: &[&str], covered: &[&str], remaining: &[&str]| Msg::MoreReplies {
        generation,
        comment_id: "parent".into(),
        result: Ok(ReplyPage {
            comments: ids.iter().map(|id| comment(id, vec![], None)).collect(),
            refreshed: Some(covered.iter().map(|s| s.to_string()).collect()),
            order: ids.iter().chain(remaining).map(|s| s.to_string()).collect(),
            more: (!remaining.is_empty()).then(|| MoreReplies {
                cursor: "/next".into(),
                count: None,
            }),
        }),
    };
    app.on_msg(ordered_snapshot(page(&["a", "b"]), 1, false, false));
    app.on_msg(ordered_snapshot(
        batch(&["c", "d"], &["c", "d"], &[]),
        2,
        false,
        false,
    ));
    app.on_msg(ordered_snapshot(page(&["a"]), 3, false, false));
    // An older cached parent cannot confirm b again after the newer demotion.
    app.on_msg(ordered_snapshot(page(&["a", "b"]), 1, true, false));
    app.on_msg(ordered_snapshot(
        batch(&["x", "b", "c"], &["x", "b", "c"], &["d"]),
        4,
        false,
        false,
    ));
    let ordered = reply_ids(&app, "parent");
    // A covered omission removes b, and a newer response may add it back.
    app.on_msg(ordered_snapshot(
        batch(&[], &["b"], &["d"]),
        5,
        false,
        false,
    ));
    let removed = reply_ids(&app, "parent");
    app.on_msg(ordered_snapshot(
        batch(&["b"], &["b"], &["c", "d"]),
        6,
        false,
        false,
    ));
    let readded = reply_ids(&app, "parent");
    let expected = [
        vec!["a", "x", "b", "c", "d"],
        vec!["a", "x", "c", "d"],
        vec!["a", "x", "b", "c", "d"],
    ]
    .map(|ids| ids.into_iter().map(str::to_owned).collect::<Vec<_>>());
    assert_eq!([ordered, removed, readded], expected);
}

#[test]
fn top_level_more_expands_without_a_synthetic_author_or_extra_indent() {
    let mut app = app_with(&[]);
    let mut value: serde_json::Value =
        serde_json::from_str(include_str!("../../../tests/fixtures/post.json")).unwrap();
    value[1]["data"]["children"] =
        serde_json::json!([{"kind":"more","data":{"children":["c1"],"count":1}}]);
    let page = crate::reddit::parse::parse_post_page(&value.to_string()).unwrap();
    app.open_post_permalink(page.post.clone(), page.post.permalink.clone());
    let generation = app.post_view.as_ref().unwrap().generation;
    let parent = page.comments[0].id.clone();
    app.on_msg(Msg::Post {
        generation,
        result: Ok(Box::new(page)),
    });
    let rows = app.post_view.as_ref().unwrap().rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].depth, 0);
    assert!(matches!(rows[0].kind, RowKind::More { .. }));
    app.on_msg(Msg::MoreReplies {
        generation,
        comment_id: parent,
        result: Ok(ReplyPage::complete(vec![comment("c1", vec![], None)])),
    });
    let rows = app.post_view.as_ref().unwrap().rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].depth, 0);
    assert!(matches!(&rows[0].kind, RowKind::Comment { comment, .. } if comment.id == "c1"));
}

#[test]
fn more_replies_arriving_keep_the_selected_comment_selected() {
    let mut app = app_with(&[]);
    app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
    let generation = app.post_view.as_ref().unwrap().generation;
    app.on_msg(thread_message(
        generation,
        vec![
            comment("c1", vec![], Some("/r/s/comments/p/x/c1")),
            comment("c2", vec![], None),
            comment("c3", vec![], None),
        ],
    ));
    let view = app.post_view.as_mut().unwrap();
    view.free_scroll = false;
    view.cursor = 3; // rows: c1, more, c2, c3

    app.on_msg(Msg::MoreReplies {
        generation,
        comment_id: "c1".into(),
        result: Ok(ReplyPage::complete(vec![
            comment("r1", vec![], None),
            comment("r2", vec![], None),
        ])),
    });

    let view = app.post_view.as_ref().unwrap();
    let rows = view.rows();
    let selected = match rows[view.cursor].kind {
        RowKind::Comment { comment, .. } => comment.id.clone(),
        RowKind::More { .. } => "more".into(),
    };
    assert_eq!(selected, "c3");
}

#[test]
fn more_replies_arriving_above_keep_a_selected_more_row_selected() {
    let mut app = app_with(&[]);
    app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
    let generation = app.post_view.as_ref().unwrap().generation;
    app.on_msg(thread_message(
        generation,
        vec![
            comment("c1", vec![], Some("/r/s/comments/p/x/c1")),
            comment("c2", vec![], Some("/r/s/comments/p/x/c2")),
        ],
    ));
    let view = app.post_view.as_mut().unwrap();
    view.free_scroll = false;
    view.cursor = 3; // rows: c1, more(c1), c2, more(c2)

    app.on_msg(Msg::MoreReplies {
        generation,
        comment_id: "c1".into(),
        result: Ok(ReplyPage::complete(vec![
            comment("r1", vec![], None),
            comment("r2", vec![], None),
        ])),
    });

    let view = app.post_view.as_ref().unwrap();
    let rows = view.rows();
    assert!(matches!(
        rows[view.cursor].kind,
        RowKind::More { parent: "c2", .. }
    ));
}

#[test]
fn an_interim_expansion_without_new_replies_keeps_its_target() {
    for (empty, navigate) in [(true, false), (false, false), (true, true)] {
        let mut app = app_with(&[]);
        app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
        let generation = app.post_view.as_ref().unwrap().generation;
        app.on_msg(ordered_snapshot(
            thread_message(
                generation,
                vec![
                    comment(
                        "parent",
                        vec![comment("child", vec![], None)],
                        Some("/more"),
                    ),
                    comment("sibling", vec![], None),
                ],
            ),
            1,
            true,
            false,
        ));
        let view = app.post_view.as_mut().unwrap();
        view.cursor = 2;
        view.free_scroll = false;
        app.activate_comment();
        app.on_msg(ordered_snapshot(
            Msg::MoreReplies {
                generation,
                comment_id: "parent".into(),
                result: Ok(ReplyPage::complete(if empty {
                    vec![]
                } else {
                    vec![comment("child", vec![], None)]
                })),
            },
            2,
            true,
            true,
        ));
        assert!(matches!(app.post_view.as_ref().unwrap().cursor_key(),
                Some(RowKey::More(id)) if id == "parent"));
        if navigate {
            app.move_comment_cursor(1);
        }
        app.on_msg(ordered_snapshot(
            Msg::MoreReplies {
                generation,
                comment_id: "parent".into(),
                result: Ok(ReplyPage::complete(vec![
                    comment("child", vec![], None),
                    comment("new", vec![], None),
                ])),
            },
            4,
            false,
            false,
        ));
        let view = app.post_view.as_ref().unwrap();
        assert!(matches!(view.cursor_key(), Some(RowKey::Comment(id))
                if id == if navigate { "sibling" } else { "new" }));
        assert!(!view.loading_more.contains("parent"));
    }
}
