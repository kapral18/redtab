use super::*;

#[test]
fn cache_refresh_keeps_post_content_visible_while_loading() {
    let mut app = app_with(&[]);
    app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
    app.on_msg(Msg::Post {
        generation: app.post_view.as_ref().unwrap().generation,
        result: Ok(Box::new(PostPage {
            post: post("p", "s"),
            body: "saved body".into(),
            comments: vec![comment("c1", vec![], None), comment("c2", vec![], None)],
        })),
    });
    let view = app.post_view.as_mut().unwrap();
    view.scroll = 12;
    view.cursor = 1;
    view.free_scroll = false;
    view.collapsed.insert("c1".into());

    app.reload();

    let view = app.post_view.as_ref().unwrap();
    assert_eq!(view.page.as_ref().unwrap().body, "saved body");
    assert_eq!((view.scroll, view.cursor, view.free_scroll), (12, 1, false));
    assert!(view.collapsed.contains("c1"));
    assert!(view.loading);
}

#[test]
fn cached_post_refresh_failure_still_renders_the_saved_body() {
    let mut app = app_with(&[]);
    app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
    let generation = app.post_view.as_ref().unwrap().generation;
    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Post {
            generation,
            result: Ok(Box::new(PostPage {
                post: post("p", "s"),
                body: "offline reading works".into(),
                comments: vec![],
            })),
        }),
        fetched_at: 1000,
        source: ordered_source(1, true),
        refreshing: true,
    });
    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Post {
            generation,
            result: Err("connection refused".into()),
        }),
        fetched_at: 2000,
        source: ordered_source(2, false),
        refreshing: false,
    });
    let view = app.post_view.as_ref().unwrap();
    assert_eq!(view.freshness.unwrap().fetched_at, 1000);
    assert!(!view.loading);
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(110, 30)).unwrap();

    terminal
        .draw(|frame| crate::ui::draw(frame, &mut app))
        .unwrap();

    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(text.contains("offline reading works"), "{text}");
}

#[test]
fn manual_post_refresh_sets_a_pinned_first_comment_aside() {
    let mut app = app_with(&[]);
    app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
    app.focus = Focus::Post;
    let pin = || Comment {
        stickied: true,
        ..comment("pin", vec![], None)
    };
    let generation = app.post_view.as_ref().unwrap().generation;
    app.on_msg(thread_message(
        generation,
        rooted(vec![
            pin(),
            comment("c1", vec![], None),
            comment("c2", vec![], None),
        ]),
    ));
    let view = app.post_view.as_mut().unwrap();
    view.cursor = 2;
    view.free_scroll = false;
    view.row_starts = vec![5, 10, 15];
    view.drawn_cursor = Some(2);
    view.scroll = 16;

    app.reload();
    let generation = app.post_view.as_ref().unwrap().generation;
    app.on_msg(thread_message(
        generation,
        rooted(vec![
            pin(),
            comment("new", vec![], None),
            comment("c1", vec![], None),
            comment("c2", vec![], None),
        ]),
    ));

    let view = app.post_view.as_mut().unwrap();
    assert_eq!(view.cursor, 0);
    assert_eq!(view.scroll, 0);
}

#[test]
fn refreshed_cached_replies_update_text_without_losing_expanded_children() {
    let mut app = app_with(&[]);
    app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
    let generation = app.post_view.as_ref().unwrap().generation;
    app.on_msg(thread_message(
        generation,
        vec![comment(
            "parent",
            vec![comment("reply", vec![comment("child", vec![], None)], None)],
            None,
        )],
    ));
    let mut updated = comment("reply", vec![], Some("/more"));
    updated.body = "edited reply".into();

    app.on_msg(Msg::MoreReplies {
        generation,
        comment_id: "parent".into(),
        result: Ok(ReplyPage::complete(vec![updated])),
    });

    let reply = &app
        .post_view
        .as_ref()
        .unwrap()
        .page
        .as_ref()
        .unwrap()
        .comments[0]
        .replies[0];
    assert_eq!(reply.body, "edited reply");
    assert_eq!(reply.replies[0].id, "child");
}

#[test]
fn a_leaf_gaining_replies_keeps_the_new_continuation_on_refresh() {
    let mut app = app_with(&[]);
    app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
    let generation = app.post_view.as_ref().unwrap().generation;
    app.on_msg(thread_message(
        generation,
        vec![comment("leaf", vec![], None)],
    ));

    app.on_msg(thread_message(
        generation,
        vec![comment("leaf", vec![], Some("/new-replies"))],
    ));

    assert!(matches!(app.post_view.as_ref().unwrap().rows()[1].kind,
            RowKind::More { more, .. } if more.cursor == "/new-replies"));
}

#[test]
fn older_cached_replies_add_missing_children_without_undoing_edits() {
    let mut app = app_with(&[]);
    app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
    let generation = app.post_view.as_ref().unwrap().generation;
    let mut edited = comment("child", vec![], None);
    edited.body = "edited live reply".into();
    app.on_msg(ordered_snapshot(
        Msg::Post {
            generation,
            result: Ok(Box::new(PostPage {
                post: post("p", "s"),
                body: String::new(),
                comments: vec![comment("parent", vec![edited], Some("/more"))],
            })),
        },
        2,
        false,
        false,
    ));

    app.on_msg(ordered_snapshot(
        Msg::MoreReplies {
            generation,
            comment_id: "parent".into(),
            result: Ok(ReplyPage::complete(vec![
                comment("child", vec![comment("removed", vec![], None)], None),
                comment("missing", vec![], None),
            ])),
        },
        1,
        true,
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
    assert_eq!(parent.replies[0].body, "edited live reply");
    assert!(
        parent.replies[0].replies.is_empty(),
        "an older branch must not restore removed descendants"
    );
    assert_eq!(parent.replies[1].id, "missing");
    assert!(parent.more.is_some());
}

#[test]
fn reply_revalidation_keeps_children_confirmed_by_a_parent_refresh() {
    let mut app = app_with(&[]);
    app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
    let generation = app.post_view.as_ref().unwrap().generation;
    app.on_msg(ordered_snapshot(
        thread_message(generation, vec![comment("parent", vec![], Some("/more"))]),
        1,
        true,
        false,
    ));
    app.on_msg(ordered_snapshot(
        Msg::MoreReplies {
            generation,
            comment_id: "parent".into(),
            result: Ok(ReplyPage::complete(vec![
                comment("confirmed", vec![], None),
                comment("removed", vec![], None),
            ])),
        },
        2,
        true,
        false,
    ));
    app.on_msg(ordered_snapshot(
        thread_message(
            generation,
            vec![comment(
                "parent",
                vec![comment("confirmed", vec![], None)],
                Some("/more"),
            )],
        ),
        3,
        false,
        false,
    ));
    app.on_msg(ordered_snapshot(
        Msg::MoreReplies {
            generation,
            comment_id: "parent".into(),
            result: Ok(ReplyPage::complete(vec![
                comment("confirmed", vec![], None),
                comment("removed", vec![], None),
            ])),
        },
        2,
        true,
        true,
    ));
    app.on_msg(ordered_snapshot(
        Msg::MoreReplies {
            generation,
            comment_id: "parent".into(),
            result: Ok(ReplyPage::complete(vec![comment("new", vec![], None)])),
        },
        4,
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
        ["confirmed", "new"]
    );
}

#[test]
fn an_older_parent_response_confirms_children_without_undoing_newer_content() {
    for already_expanded in [false, true] {
        let mut app = app_with(&[]);
        app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
        let generation = app.post_view.as_ref().unwrap().generation;
        app.on_msg(ordered_snapshot(
            thread_message(
                generation,
                vec![comment(
                    "ancestor",
                    vec![comment("parent", vec![], Some("/parent-more"))],
                    Some("/ancestor-more"),
                )],
            ),
            1,
            true,
            false,
        ));
        app.reload();
        let generation = app.post_view.as_ref().unwrap().generation;
        app.on_msg(ordered_snapshot(
            Msg::MoreReplies {
                generation,
                comment_id: "ancestor".into(),
                result: Ok(ReplyPage::complete(vec![comment(
                    "parent",
                    vec![],
                    Some("/parent-more"),
                )])),
            },
            3,
            false,
            false,
        ));
        if already_expanded {
            let mut child = comment("confirmed", vec![], None);
            child.body = "newer expansion text".into();
            app.on_msg(ordered_snapshot(
                Msg::MoreReplies {
                    generation,
                    comment_id: "parent".into(),
                    result: Ok(ReplyPage::complete(vec![child])),
                },
                4,
                true,
                true,
            ));
        }
        let mut child = comment("confirmed", vec![], None);
        child.body = "older parent text".into();
        app.on_msg(ordered_snapshot(
            thread_message(
                generation,
                vec![comment(
                    "ancestor",
                    vec![comment("parent", vec![child], Some("/parent-more"))],
                    Some("/ancestor-more"),
                )],
            ),
            2,
            false,
            false,
        ));
        app.on_msg(ordered_snapshot(
            Msg::MoreReplies {
                generation,
                comment_id: "parent".into(),
                result: Ok(ReplyPage::complete(vec![comment("new", vec![], None)])),
            },
            5,
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
            .comments[0]
            .replies[0];
        assert_eq!(
            parent
                .replies
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            ["confirmed", "new"]
        );
        assert_eq!(
            parent.replies[0].body,
            if already_expanded {
                "newer expansion text"
            } else {
                "older parent text"
            }
        );
    }
}
