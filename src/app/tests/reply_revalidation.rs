use super::*;

#[test]
fn cached_replies_revalidate_when_the_parent_changes_after_submission() {
    let mut app = app_with(&[]);
    app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
    let generation = app.post_view.as_ref().unwrap().generation;
    app.on_msg(ordered_snapshot(
        thread_message(generation, vec![comment("parent", vec![], Some("/more"))]),
        10,
        true,
        true,
    ));
    let view = app.post_view.as_mut().unwrap();
    view.cursor = 1;
    view.free_scroll = false;
    app.activate_comment();
    app.on_msg(ordered_snapshot(
        thread_message(
            generation,
            vec![comment(
                "parent",
                vec![comment("edited", vec![], None)],
                Some("/more"),
            )],
        ),
        21,
        false,
        false,
    ));

    app.on_msg(ordered_snapshot(
        Msg::MoreReplies {
            generation,
            comment_id: "parent".into(),
            result: Ok(ReplyPage::complete(vec![comment("missing", vec![], None)])),
        },
        20,
        true,
        false,
    ));

    let view = app.post_view.as_ref().unwrap();
    assert!(
        view.loading_more.contains("parent"),
        "the newly stale expansion must keep refreshing"
    );
    assert_eq!(
        view.page.as_ref().unwrap().comments[0]
            .replies
            .iter()
            .map(|reply| reply.id.as_str())
            .collect::<Vec<_>>(),
        ["edited", "missing"]
    );
}

#[test]
fn a_newly_complete_parent_rejects_an_older_cached_expansion() {
    let mut app = app_with(&[]);
    app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
    let generation = app.post_view.as_ref().unwrap().generation;
    app.on_msg(ordered_snapshot(
        thread_message(generation, vec![comment("parent", vec![], Some("/more"))]),
        10,
        true,
        true,
    ));
    let view = app.post_view.as_mut().unwrap();
    view.cursor = 1;
    view.free_scroll = false;
    app.activate_comment();
    app.on_msg(ordered_snapshot(
        thread_message(
            generation,
            vec![comment(
                "parent",
                vec![comment("current", vec![], Some("/child-more"))],
                None,
            )],
        ),
        21,
        false,
        false,
    ));

    app.on_msg(ordered_snapshot(
        Msg::MoreReplies {
            generation,
            comment_id: "parent".into(),
            result: Ok(ReplyPage::complete(vec![
                comment("removed", vec![], None),
                comment("current", vec![comment("recovered", vec![], None)], None),
            ])),
        },
        20,
        true,
        false,
    ));

    let view = app.post_view.as_ref().unwrap();
    assert_eq!(
        view.page.as_ref().unwrap().comments[0]
            .replies
            .iter()
            .map(|reply| reply.id.as_str())
            .collect::<Vec<_>>(),
        ["current"]
    );
    assert_eq!(
        view.page.as_ref().unwrap().comments[0].replies[0]
            .replies
            .iter()
            .map(|reply| reply.id.as_str())
            .collect::<Vec<_>>(),
        ["recovered"],
    );
    assert!(!view.loading_more.contains("parent"));
}

#[test]
fn completing_reply_revalidation_removes_disappeared_direct_replies() {
    for keep_b in [true, false] {
        let mut app = app_with(&[]);
        app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
        let generation = app.post_view.as_ref().unwrap().generation;
        app.on_msg(ordered_snapshot(
            thread_message(generation, vec![comment("parent", vec![], Some("/more"))]),
            2,
            true,
            false,
        ));
        let view = app.post_view.as_mut().unwrap();
        view.cursor = 1;
        view.free_scroll = false;
        app.activate_comment();
        app.on_msg(ordered_snapshot(
            Msg::MoreReplies {
                generation,
                comment_id: "parent".into(),
                result: Ok(ReplyPage::complete(vec![
                    comment("a", vec![], None),
                    comment("b", vec![comment("expanded", vec![], None)], None),
                ])),
            },
            1,
            true,
            true,
        ));
        let replies = if keep_b {
            vec![comment("b", vec![], Some("/b-more"))]
        } else {
            vec![]
        };
        app.on_msg(ordered_snapshot(
            Msg::MoreReplies {
                generation,
                comment_id: "parent".into(),
                result: Ok(ReplyPage::complete(replies)),
            },
            3,
            false,
            false,
        ));
        let parent = &app
            .post_view
            .as_ref()
            .unwrap()
            .page
            .as_ref()
            .unwrap()
            .comments[0];
        assert_eq!(
            parent
                .replies
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            if keep_b { vec!["b"] } else { vec![] }
        );
        assert!(parent.more.is_none());
        if keep_b {
            assert_eq!(parent.replies[0].replies[0].id, "expanded");
        }
    }
}

#[test]
fn older_expansion_recovers_incomplete_descendants_beneath_complete_children() {
    let mut app = app_with(&[]);
    app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
    let generation = app.post_view.as_ref().unwrap().generation;
    app.on_msg(ordered_snapshot(
        thread_message(
            generation,
            vec![comment(
                "parent",
                vec![comment(
                    "complete",
                    vec![comment("incomplete", vec![], Some("/nested-more"))],
                    None,
                )],
                Some("/more"),
            )],
        ),
        2,
        true,
        false,
    ));
    let view = app.post_view.as_mut().unwrap();
    view.cursor = 4;
    view.free_scroll = false;
    app.activate_comment();
    app.on_msg(ordered_snapshot(
        Msg::MoreReplies {
            generation,
            comment_id: "parent".into(),
            result: Ok(ReplyPage::complete(vec![comment(
                "complete",
                vec![
                    comment("removed", vec![], None),
                    comment("incomplete", vec![comment("recovered", vec![], None)], None),
                ],
                None,
            )])),
        },
        1,
        true,
        true,
    ));
    let view = app.post_view.as_ref().unwrap();
    let complete = &view.page.as_ref().unwrap().comments[0].replies[0];
    assert_eq!(
        complete
            .replies
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>(),
        ["incomplete"]
    );
    assert_eq!(
        complete.replies[0]
            .replies
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>(),
        ["recovered"]
    );
    assert!(matches!(view.cursor_key(), Some(RowKey::Comment(id)) if id == "recovered"));
}

#[test]
fn incomparable_live_fields_survive_a_later_cached_expansion() {
    for retired in [false, true] {
        let mut app = app_with(&[]);
        app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
        let generation = app.post_view.as_ref().unwrap().generation;
        let mut child = comment("child", vec![], None);
        child.body = "new live text".into();
        app.on_msg(Msg::Snapshot {
            message: Box::new(thread_message(
                generation,
                vec![comment("parent", vec![child], Some("/more"))],
            )),
            fetched_at: now(),
            refreshing: false,
            source: Source {
                observed: Some((4, 4)),
                order: if retired {
                    ordered_source(1, false).order
                } else {
                    None
                },
                ..Source::default()
            },
        });
        let mut old = comment("child", vec![], Some("/obsolete-continuation"));
        old.body = "old cached text".into();
        app.on_msg(Msg::Snapshot {
            message: Box::new(Msg::MoreReplies {
                generation,
                comment_id: "parent".into(),
                result: Ok(ReplyPage::complete(vec![old])),
            }),
            fetched_at: now(),
            refreshing: false,
            source: Source {
                observed: Some((5, 6)),
                order: Some(crate::cache::RequestOrder {
                    generation: [2; 16],
                    sequence: 2,
                }),
                ..ordered_source(2, true)
            },
        });
        let view = app.post_view.as_ref().unwrap();
        let child = &view.page.as_ref().unwrap().comments[0].replies[0];
        assert_eq!(child.body, "new live text");
        assert!(child.more.is_none());
        assert!(
            view.loading_more.contains("parent"),
            "uncertain cached replies must revalidate"
        );
        let mut new = comment("child", vec![], None);
        new.body = "revalidated text".into();
        app.on_msg(Msg::Snapshot {
            message: Box::new(Msg::MoreReplies {
                generation,
                comment_id: "parent".into(),
                result: Ok(ReplyPage::complete(vec![new])),
            }),
            fetched_at: now(),
            refreshing: false,
            source: Source {
                observed: Some((7, 7)),
                order: Some(crate::cache::RequestOrder {
                    generation: [2; 16],
                    sequence: 3,
                }),
                ..ordered_source(3, false)
            },
        });
        assert_eq!(
            app.post_view
                .as_ref()
                .unwrap()
                .page
                .as_ref()
                .unwrap()
                .comments[0]
                .replies[0]
                .body,
            "revalidated text"
        );
    }
}
