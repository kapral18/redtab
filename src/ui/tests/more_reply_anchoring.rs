use super::*;

#[test]
fn replies_loading_above_a_selected_comment_keep_it_in_place() {
    use crate::model::MoreReplies;
    let (mut app, _) = open_long_post(|_| {});
    let generation = app.post_view.as_ref().unwrap().generation;
    let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
    let reply = page.comments[0].clone();
    page.comments[0].more = Some(MoreReplies {
        cursor: "c".into(),
        count: Some(2),
    });
    page.comments.push(Comment {
        id: "c2".into(),
        body: "marker two".into(),
        ..reply.clone()
    });
    screen(&mut app, 100, 30);
    // Rows c, more(c), c2: select c2 with its header mid-screen.
    let view = app.post_view.as_mut().unwrap();
    view.cursor = 2;
    view.free_scroll = false;
    view.scroll = view.row_starts[2] - 10;
    let row_of = |s: &str| s.lines().position(|l| l.contains("marker two"));
    let before = row_of(&screen(&mut app, 100, 30));

    app.on_msg(Msg::MoreReplies {
        generation,
        comment_id: "c".into(),
        result: Ok(ReplyPage::complete(
            (0..2)
                .map(|i| Comment {
                    id: format!("r{i}"),
                    ..reply.clone()
                })
                .collect(),
        )),
    });
    let after = row_of(&screen(&mut app, 100, 30));

    assert!(before.is_some());
    assert_eq!(after, before, "the selected comment did not move");
}

#[test]
fn more_replies_loading_above_keep_the_text_being_read() {
    use crate::model::MoreReplies;
    let first_line = |s: &str| {
        s.lines()
            .find_map(|l| l.split("line ").nth(1))
            .map(|rest| rest.split_whitespace().next().unwrap_or("").to_string())
    };
    // Read into a tall last comment below two "more replies" rows; then `keys`,
    // with a draw after each only when `draw_between`; then 20 replies load
    // under each comment in `loads`, all before one draw.
    let run = |keys: &[&str], draw_between: bool, loads: &[&str]| {
        let (mut app, _) = open_long_post(|_| {});
        let generation = app.post_view.as_ref().unwrap().generation;
        let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
        let reply = page.comments[0].clone();
        let mut tall = reply.clone();
        tall.id = "tall".into();
        tall.body = paragraphs("line", 1..=40);
        page.comments[0].more = Some(MoreReplies {
            cursor: "c".into(),
            count: Some(20),
        });
        page.comments.push(Comment {
            id: "c2".into(),
            ..page.comments[0].clone()
        });
        page.comments.push(tall);
        for key in ["G", "\u{6}"] {
            press_keys(&mut app, key);
            screen(&mut app, 100, 30);
        }
        let before = first_line(&screen(&mut app, 100, 30));
        for &key in keys {
            press_keys(&mut app, key);
            if draw_between {
                screen(&mut app, 100, 30);
            }
        }
        for parent in loads {
            let replies = (0..20)
                .map(|i| Comment {
                    id: format!("{parent}-r{i}"),
                    ..reply.clone()
                })
                .collect();
            app.on_msg(Msg::MoreReplies {
                generation,
                comment_id: parent.to_string(),
                result: Ok(ReplyPage::complete(replies)),
            });
        }
        (before, first_line(&screen(&mut app, 100, 30)))
    };

    let (before, reading) = run(&[], true, &["c"]);
    let (_, selected) = run(&["l"], true, &["c"]);
    // The main loop handles keys, then messages, then draws: `l` and the
    // replies, or two loads, can land before one draw.
    let (_, same_pass) = run(&["l"], false, &["c"]);
    let (_, two_loads) = run(&[], true, &["c", "c2"]);

    assert!(
        before.as_deref().is_some_and(|n| n != "1"),
        "reading inside: {before:?}"
    );
    assert_eq!(reading, before, "while reading");
    assert_eq!(selected, before, "with the comment selected by `l`");
    assert_eq!(same_pass, before, "with `l` in the same pass");
    assert_eq!(two_loads, before, "with two loads in one pass");
}

#[test]
fn reading_a_more_slot_follows_new_replies_not_a_following_sibling() {
    let mut wrong = Vec::new();
    for (toggle, preceding_blank, twice) in [
        (false, false, false),
        (true, false, false),
        (false, true, false),
        (false, false, true),
    ] {
        let (mut app, _) = open_long_post(|_| {});
        let view = app.post_view.as_mut().unwrap();
        let page = view.page.as_mut().unwrap();
        page.body.clear();
        page.comments[0].body = format!("parent {SPOILER_START}short spoiler{SPOILER_END}");
        page.comments[0].more = Some(crate::model::MoreReplies {
            cursor: "/more".into(),
            count: Some(10),
        });
        let mut sibling = page.comments[0].clone();
        sibling.id = "sibling".into();
        sibling.more = None;
        sibling.body = paragraphs("unique sibling passage", 1..=30);
        page.comments.push(sibling);
        screen(&mut app, 100, 30);
        let view = app.post_view.as_mut().unwrap();
        view.cursor = 1;
        view.free_scroll = false;
        view.scroll = view.row_starts[1] - usize::from(preceding_blank);
        screen(&mut app, 100, 30);
        app.activate_comment();
        press(&mut app, 'h');
        screen(&mut app, 100, 30);
        if toggle {
            press_keys(&mut app, "zs");
        }
        let view = app.post_view.as_ref().unwrap();
        let generation = view.generation;
        let mut reply = view.page.as_ref().unwrap().comments[0].clone();
        reply.more = None;
        let replies: Vec<_> = (0..10)
            .map(|i| Comment {
                id: format!("new-{i}"),
                body: format!("new reply {i}"),
                ..reply.clone()
            })
            .collect();
        for _ in 0..if twice { 2 } else { 1 } {
            app.on_msg(Msg::MoreReplies {
                generation,
                comment_id: "c".into(),
                result: Ok(ReplyPage::complete(replies.clone())),
            });
        }
        screen(&mut app, 100, 30);
        let view = app.post_view.as_ref().unwrap();
        let expected = view.row_starts[1] - usize::from(preceding_blank);
        if view.scroll != expected {
            wrong.push((toggle, preceding_blank, twice, view.scroll, expected));
        }
    }
    assert!(
        wrong.is_empty(),
        "toggle/blank/twice/actual/expected: {wrong:?}"
    );
}

#[test]
fn removed_parent_more_follows_a_recovered_descendant_before_drawing() {
    let (mut app, _) = open_long_post(|_| {});
    let source = |sequence, cached| crate::cache::Source {
        order: Some(crate::cache::RequestOrder {
            generation: [1; 16],
            sequence,
        }),
        observed: Some((sequence, sequence)),
        cached,
    };
    let view = app.post_view.as_mut().unwrap();
    let page = view.page.as_mut().unwrap();
    page.body.clear();
    let mut child = page.comments[0].clone();
    child.id = "child".into();
    child.more = Some(crate::model::MoreReplies {
        cursor: "/child".into(),
        count: None,
    });
    child.sources = Some(crate::model::CommentSources {
        content: source(1, true),
        more: source(1, true),
        parent: Some(crate::model::ParentMembership {
            source: source(1, true),
            present: true,
        }),
    });
    let mut parent = child.clone();
    parent.id = "parent".into();
    parent.more.as_mut().unwrap().cursor = "/parent".into();
    parent.replies = vec![child.clone()];
    let mut sibling = child.clone();
    sibling.id = "sibling".into();
    sibling.more = None;
    sibling.body = paragraphs("unique following passage", 0..30);
    page.comments = vec![parent, sibling];
    screen(&mut app, 100, 30);
    let view = app.post_view.as_mut().unwrap();
    let more = view
        .rows()
        .iter()
        .position(|row| {
            matches!(
                row.kind,
                RowKind::More {
                    parent: "parent",
                    ..
                }
            )
        })
        .unwrap();
    view.cursor = more;
    view.free_scroll = false;
    view.scroll = view.row_starts[more];
    screen(&mut app, 100, 30);
    app.activate_comment();
    press(&mut app, 'h');
    screen(&mut app, 100, 30);
    let view = app.post_view.as_ref().unwrap();
    let generation = view.generation;
    let mut fresh = view.page.as_ref().unwrap().clone();
    fresh.comments[0].more = None;
    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Post {
            generation,
            result: Ok(Box::new(fresh)),
        }),
        fetched_at: 1000,
        source: source(3, false),
        refreshing: false,
    });
    let mut recovered = child.clone();
    recovered.id = "recovered".into();
    recovered.more = None;
    child.replies.push(recovered);
    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::MoreReplies {
            generation,
            comment_id: "parent".into(),
            result: Ok(ReplyPage::complete(vec![child])),
        }),
        fetched_at: 1000,
        source: source(2, true),
        refreshing: false,
    });
    screen(&mut app, 100, 30);
    let view = app.post_view.as_ref().unwrap();
    let reply = view
        .rows()
        .iter()
        .position(
            |row| matches!(row.kind, RowKind::Comment { comment, .. } if comment.id == "recovered"),
        )
        .unwrap();
    assert_eq!(
        view.scroll, view.row_starts[reply],
        "follow the reply, not its sibling"
    );
}

#[test]
fn reading_the_blank_line_under_a_loading_more_row_keeps_the_text() {
    let mut app = image_post_with_comments(37);
    app.post_view
        .as_mut()
        .unwrap()
        .page
        .as_mut()
        .unwrap()
        .comments[0]
        .body = "first".into();
    screen(&mut app, 100, 30);
    // The top line is the blank line under "↳ 2 more replies" (row 1).
    let view = app.post_view.as_mut().unwrap();
    view.scroll = view.row_starts[1] + 1;
    let row_of = |s: &str| s.lines().position(|l| l.contains("marker 1 "));
    let before = row_of(&screen(&mut app, 100, 30));

    let generation = app.post_view.as_ref().unwrap().generation;
    app.on_msg(Msg::MoreReplies {
        generation,
        comment_id: "c".into(),
        result: Ok(ReplyPage::complete(replies(2))),
    });
    let after = row_of(&screen(&mut app, 100, 30));

    assert!(before.is_some());
    assert_eq!(after, before);
}

#[test]
fn reading_at_a_more_row_that_loads_with_one_above_keeps_its_place() {
    use crate::model::MoreReplies;
    let (mut app, _) = open_long_post(|_| {});
    let generation = app.post_view.as_ref().unwrap().generation;
    let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
    let reply = page.comments[0].clone();
    page.comments[0].more = Some(MoreReplies {
        cursor: "c".into(),
        count: Some(20),
    });
    page.comments.push(Comment {
        id: "c2".into(),
        ..page.comments[0].clone()
    });
    page.comments.push(Comment {
        id: "tall".into(),
        body: "words\n\n".repeat(40),
        ..reply.clone()
    });
    screen(&mut app, 100, 30);
    // Rows c, more(c), c2, more(c2), tall: read with more(c2) on the top line.
    let view = app.post_view.as_mut().unwrap();
    view.scroll = view.row_starts[3];
    screen(&mut app, 100, 30);

    // Both load before one draw: c's replies above, c2's in place of the row.
    for parent in ["c", "c2"] {
        let replies = (0..20)
            .map(|i| Comment {
                id: format!("{parent}-r{i}"),
                ..reply.clone()
            })
            .collect();
        app.on_msg(Msg::MoreReplies {
            generation,
            comment_id: parent.into(),
            result: Ok(ReplyPage::complete(replies)),
        });
    }
    screen(&mut app, 100, 30);
    let view = app.post_view.as_ref().unwrap();
    let first_new = view
        .rows()
        .iter()
        .position(|r| matches!(r.kind, RowKind::Comment { comment, .. } if comment.id == "c2-r0"))
        .unwrap();

    assert_eq!(
        view.scroll, view.row_starts[first_new],
        "c2's first new reply on top"
    );
}
