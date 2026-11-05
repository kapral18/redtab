use super::*;

#[test]
fn manual_post_refresh_shows_the_top_only_when_it_brings_new_comments() {
    let mut app = app_with(&[]);
    app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
    app.focus = Focus::Post;
    let generation = app.post_view.as_ref().unwrap().generation;
    app.on_msg(thread_message(
        generation,
        rooted(vec![
            comment("c1", vec![], None),
            comment("c2", vec![], None),
        ]),
    ));
    let read_c2 = |app: &mut App| {
        let view = app.post_view.as_mut().unwrap();
        view.cursor = 1;
        view.free_scroll = false;
        view.row_starts = vec![5, 10];
        view.drawn_cursor = Some(1);
        view.scroll = 11;
    };
    read_c2(&mut app);

    app.reload();
    let generation = app.post_view.as_ref().unwrap().generation;
    app.on_msg(thread_message(
        generation,
        rooted(vec![
            comment("c1", vec![], None),
            comment("c2", vec![], None),
        ]),
    ));
    let view = app.post_view.as_mut().unwrap();
    assert!(matches!(view.cursor_key(), Some(RowKey::Comment(id)) if id == "c2"));
    assert!(view.take_reading_anchor().is_some());

    // Unseen comments below the thread's first comment arrive in place.
    read_c2(&mut app);
    app.reload();
    let generation = app.post_view.as_ref().unwrap().generation;
    app.on_msg(thread_message(
        generation,
        rooted(vec![
            comment("c1", vec![], None),
            comment("lower", vec![], None),
            comment("c2", vec![comment("reply", vec![], None)], None),
        ]),
    ));
    let view = app.post_view.as_mut().unwrap();
    assert!(matches!(view.cursor_key(), Some(RowKey::Comment(id)) if id == "c2"));
    assert!(view.take_reading_anchor().is_some());

    read_c2(&mut app);
    app.reload();
    let generation = app.post_view.as_ref().unwrap().generation;
    app.on_msg(thread_message(
        generation,
        rooted(vec![
            comment("new", vec![], None),
            comment("c1", vec![], None),
            comment("c2", vec![], None),
        ]),
    ));
    let view = app.post_view.as_mut().unwrap();
    assert_eq!(view.cursor, 0);
    assert_eq!(view.scroll, 0);
    assert_eq!(view.take_reading_anchor(), None);
}

#[test]
fn cached_post_refresh_preserves_expanded_replies_and_comment_anchor() {
    let mut app = app_with(&[]);
    app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
    let generation = app.post_view.as_ref().unwrap().generation;
    app.on_msg(thread_message(
        generation,
        vec![
            comment("c1", vec![comment("reply", vec![], None)], None),
            comment("c2", vec![], None),
        ],
    ));
    let view = app.post_view.as_mut().unwrap();
    view.cursor = 2;
    view.free_scroll = false;
    view.row_starts = vec![5, 10, 20];
    view.drawn_cursor = Some(2);
    view.scroll = 21;

    app.on_msg(thread_message(
        generation,
        vec![
            comment("new", vec![], None),
            comment("c1", vec![], Some("/more")),
            comment("c2", vec![], None),
        ],
    ));

    let view = app.post_view.as_mut().unwrap();
    assert!(matches!(view.cursor_key(), Some(RowKey::Comment(id)) if id == "c2"));
    assert_eq!(view.take_reading_anchor(), Some((4, 1)));
    assert_eq!(
        view.page.as_ref().unwrap().comments[1].replies[0].id,
        "reply"
    );
}

#[test]
fn cached_reply_expansion_keeps_selection_on_new_replies_through_revalidation() {
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
        2,
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
            result: Ok(ReplyPage::complete(vec![
                comment("child", vec![], None),
                comment("missing", vec![], None),
            ])),
        },
        1,
        true,
        true,
    ));
    app.on_msg(ordered_snapshot(
        Msg::MoreReplies {
            generation,
            comment_id: "parent".into(),
            result: Ok(ReplyPage::complete(vec![
                comment("child", vec![], None),
                comment("missing", vec![], None),
            ])),
        },
        3,
        false,
        false,
    ));

    assert!(matches!(app.post_view.as_ref().unwrap().cursor_key(),
            Some(RowKey::Comment(id)) if id == "missing"));
}

#[test]
fn live_expansion_reorders_cached_replies_without_losing_selection() {
    for nested in [false, true] {
        let mut app = app_with(&[]);
        app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
        let generation = app.post_view.as_ref().unwrap().generation;
        app.on_msg(ordered_snapshot(
            thread_message(
                generation,
                vec![comment(
                    "parent",
                    vec![comment("confirmed", vec![], None)],
                    Some("/more"),
                )],
            ),
            2,
            true,
            false,
        ));
        let more = |ids: &[&str]| Msg::MoreReplies {
            generation,
            comment_id: "parent".into(),
            result: Ok(ReplyPage::complete(
                ids.iter()
                    .map(|id| comment(id, vec![], (*id == "a").then_some("/nested")))
                    .collect(),
            )),
        };
        app.on_msg(ordered_snapshot(more(&["a", "b"]), 1, true, true));
        let view = app.post_view.as_mut().unwrap();
        view.cursor = view.row_index(&RowKey::Comment("b".into())).unwrap();
        view.free_scroll = false;
        if nested {
            app.on_msg(ordered_snapshot(
                Msg::MoreReplies {
                    generation,
                    comment_id: "a".into(),
                    result: Ok(ReplyPage::complete(vec![comment("nested", vec![], None)])),
                },
                4,
                false,
                false,
            ));
        }
        app.on_msg(ordered_snapshot(more(&["b", "c", "a"]), 3, false, false));
        let view = app.post_view.as_ref().unwrap();
        assert_eq!(
            view.page.as_ref().unwrap().comments[0]
                .replies
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            ["confirmed", "b", "c", "a"]
        );
        assert!(matches!(view.cursor_key(), Some(RowKey::Comment(id)) if id == "b"));
        // An older response arriving afterward cannot roll the refreshed order back.
        app.on_msg(ordered_snapshot(more(&["a", "b"]), 1, true, false));
        let view = app.post_view.as_ref().unwrap();
        assert_eq!(
            view.page.as_ref().unwrap().comments[0]
                .replies
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            ["confirmed", "b", "c", "a"]
        );
        assert!(matches!(view.cursor_key(), Some(RowKey::Comment(id)) if id == "b"));
        if nested {
            let a = view.page.as_ref().unwrap().comments[0]
                .replies
                .iter()
                .find(|c| c.id == "a")
                .unwrap();
            assert_eq!(a.replies[0].id, "nested");
            assert!(a.more.is_none());
        }
    }
}

#[test]
fn a_refreshed_spoiler_post_can_be_revealed_without_a_stale_selection() {
    let mut app = app_with(&[]);
    app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
    let generation = app.post_view.as_ref().unwrap().generation;
    let mut page = PostPage {
        post: post("p", "s"),
        body: "secret body".into(),
        comments: vec![comment("secret reply", vec![], None)],
    };
    app.on_msg(Msg::Post {
        generation,
        result: Ok(Box::new(page.clone())),
    });
    app.post_view.as_mut().unwrap().free_scroll = false;
    page.post.spoiler = true;
    app.on_msg(Msg::Post {
        generation,
        result: Ok(Box::new(page)),
    });
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(110, 30)).unwrap();
    terminal
        .draw(|frame| crate::ui::draw(frame, &mut app))
        .unwrap();

    app.toggle_spoilers();
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
    assert!(text.contains("secret reply"), "{text}");
    assert!(app.post_view.as_ref().unwrap().free_scroll);
}
