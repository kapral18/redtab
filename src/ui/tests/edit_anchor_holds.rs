use super::*;

#[test]
fn a_top_jump_discards_the_old_comment_hold_before_an_edit() {
    let (mut app, _) = open_long_post(|_| {});
    let view = app.post_view.as_mut().unwrap();
    let page = view.page.as_mut().unwrap();
    page.body.clear();
    page.comments[0].body = paragraphs("passage", 1..=60);
    screen(&mut app, 100, 30);
    let view = app.post_view.as_mut().unwrap();
    view.scroll = view.row_starts[0] + 26;
    screen(&mut app, 100, 30);
    press(&mut app, 'l');
    screen(&mut app, 100, 30);
    press_keys(&mut app, "gg");
    let view = app.post_view.as_ref().unwrap();
    let generation = view.generation;
    let mut page = view.page.as_ref().unwrap().clone();
    page.comments[0].body = format!("inserted\n\n{}", page.comments[0].body);
    app.on_msg(Msg::Post {
        generation,
        result: Ok(Box::new(page)),
    });
    screen(&mut app, 100, 30);
    assert_eq!(app.post_view.as_ref().unwrap().scroll, 0);
}

#[test]
fn a_new_hold_takes_precedence_over_the_previously_drawn_comment() {
    let run = |edited: bool| {
        let (mut app, _) = open_long_post(|_| {});
        let view = app.post_view.as_mut().unwrap();
        let page = view.page.as_mut().unwrap();
        page.body.clear();
        page.comments[0].body = "short first comment".into();
        let mut next = page.comments[0].clone();
        next.id = "next".into();
        next.body = paragraphs("second passage", 1..=60);
        page.comments.push(next);
        screen(&mut app, 100, 30);
        press(&mut app, 'j');
        screen(&mut app, 100, 30);
        for keys in ["h", "\u{6}", "l"] {
            press_keys(&mut app, keys);
        }
        if edited {
            let view = app.post_view.as_ref().unwrap();
            let generation = view.generation;
            let mut page = view.page.as_ref().unwrap().clone();
            page.comments[1].body =
                format!("inserted\n\nalso inserted\n\n{}", page.comments[1].body);
            app.on_msg(Msg::Post {
                generation,
                result: Ok(Box::new(page)),
            });
        }
        screen(&mut app, 100, 30)
            .lines()
            .enumerate()
            .find_map(|(row, line)| {
                line.split("second passage ")
                    .nth(1)
                    .and_then(|rest| rest.split_whitespace().next())
                    .map(|rest| (row, rest.to_owned()))
            })
    };
    let before = run(false);
    assert!(before.is_some());
    assert_eq!(run(true), before);
}

#[test]
fn repeated_braces_do_not_move_an_edit_anchor_to_another_passage() {
    for comment in [false, true] {
        let (mut app, _) = open_long_post(|_| {});
        let original = format!(
            "opening\n\n}}\n\nFIRST unique passage\n\n}}\n\nSECOND unique passage\n\n{}",
            paragraphs("tail", 1..=40)
        );
        let view = app.post_view.as_mut().unwrap();
        let page = view.page.as_mut().unwrap();
        if comment {
            page.body.clear();
            page.comments[0].body = original.clone();
        } else {
            page.body = original.clone();
        }
        screen(&mut app, 100, 30);
        let view = app.post_view.as_mut().unwrap();
        view.scroll = if comment { view.row_starts[0] + 7 } else { 7 };
        let second = |shown: String| {
            shown
                .lines()
                .position(|line| line.contains("SECOND unique passage"))
        };
        let before = second(screen(&mut app, 100, 30));
        assert!(before.is_some());
        let view = app.post_view.as_ref().unwrap();
        let generation = view.generation;
        let mut page = view.page.as_ref().unwrap().clone();
        if comment {
            page.comments[0].body = format!("}}\n\n{original}");
        } else {
            page.body = format!("}}\n\n{original}");
        }
        app.on_msg(Msg::Post {
            generation,
            result: Ok(Box::new(page)),
        });
        assert_eq!(
            second(screen(&mut app, 100, 30)),
            before,
            "comment={comment}"
        );
    }
}

#[test]
fn spoiler_then_paging_anchors_the_comment_actually_being_read() {
    let run = |edited: bool, held: bool| {
        let (mut app, _) = open_long_post(|_| {});
        let view = app.post_view.as_mut().unwrap();
        let page = view.page.as_mut().unwrap();
        page.body.clear();
        page.comments[0].body = format!(
            "{}\n{SPOILER_START}short spoiler{SPOILER_END}",
            paragraphs("first passage", 1..=10)
        );
        let mut next = page.comments[0].clone();
        next.id = "next".into();
        next.body = paragraphs("next passage", 1..=60);
        page.comments.push(next);
        screen(&mut app, 100, 30);
        let view = app.post_view.as_mut().unwrap();
        view.scroll = view.row_starts[0] + 8;
        screen(&mut app, 100, 30);
        if held {
            press(&mut app, 'l');
            screen(&mut app, 100, 30);
        }
        press_keys(&mut app, "zs");
        key(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
        if edited {
            let view = app.post_view.as_ref().unwrap();
            let generation = view.generation;
            let mut page = view.page.as_ref().unwrap().clone();
            page.comments[1].body =
                format!("inserted\n\nalso inserted\n\n{}", page.comments[1].body);
            app.on_msg(Msg::Post {
                generation,
                result: Ok(Box::new(page)),
            });
        }
        screen(&mut app, 100, 30)
            .lines()
            .enumerate()
            .find_map(|(row, line)| {
                line.split("next passage ")
                    .nth(1)
                    .and_then(|rest| rest.split_whitespace().next())
                    .map(|rest| (row, rest.to_owned()))
            })
    };
    for held in [false, true] {
        let before = run(false, held);
        assert!(before.is_some());
        assert_eq!(run(true, held), before, "held={held}");
    }
}

#[test]
fn edited_selftext_anchor_respects_batched_spoiler_and_scroll_keys() {
    let run = |edit_body: bool, draw_between: bool, before: &[&str], after: &[&str]| {
        let mut app = post_with_rewrapping_spoilers(40);
        let view = app.post_view.as_mut().unwrap();
        view.scroll = view.row_starts[0] - 30;
        screen(&mut app, 100, 30);
        for &key in before {
            press_keys(&mut app, key);
            if draw_between {
                screen(&mut app, 100, 30);
            }
        }
        if edit_body {
            let view = app.post_view.as_ref().unwrap();
            let generation = view.generation;
            let mut page = view.page.as_ref().unwrap().clone();
            page.body = format!("inserted one\n\ninserted two\n\n{}", page.body);
            app.on_msg(Msg::Post {
                generation,
                result: Ok(Box::new(page)),
            });
        }
        if draw_between {
            screen(&mut app, 100, 30);
        }
        for &key in after {
            press_keys(&mut app, key);
        }
        let shown = screen(&mut app, 100, 30);
        let passage = shown.lines().enumerate().find_map(|(row, line)| {
            line.split("body ")
                .nth(1)
                .and_then(|rest| rest.split_whitespace().next())
                .map(|rest| (row, rest.to_owned()))
        });
        (app.post_view.as_ref().unwrap().scroll, passage)
    };
    for (before, after) in [
        (&[][..], &["zs", "\u{6}"][..]),
        (&["zs"][..], &[][..]),
        (&[][..], &["gg"][..]),
    ] {
        let batched = run(true, false, before, after);
        assert_eq!(batched, run(true, true, before, after));
        if after.contains(&"gg") {
            assert_eq!(batched.0, 0);
        } else {
            let unchanged = run(false, false, before, after);
            assert!(unchanged.1.is_some());
            assert_eq!(batched.1, unchanged.1);
        }
    }
}

#[test]
fn edited_comments_keep_the_passage_read_or_held_across_post_and_more_updates() {
    for more in [false, true] {
        for held in [0, 1, 2] {
            let (mut app, _) = open_long_post(|_| {});
            let original = paragraphs("comment passage", 1..=60);
            let view = app.post_view.as_mut().unwrap();
            let page = view.page.as_mut().unwrap();
            page.body.clear();
            page.comments[0].body = original.clone();
            if more {
                let child = page.comments.remove(0);
                let mut parent = child.clone();
                parent.id = "parent".into();
                parent.body = "parent".into();
                parent.replies = vec![child];
                page.comments.push(parent);
            }
            screen(&mut app, 100, 30);
            let view = app.post_view.as_mut().unwrap();
            view.scroll = view.row_starts[usize::from(more)] + 26;
            screen(&mut app, 100, 30);
            let passage = |text: String| {
                text.lines().enumerate().find_map(|(row, line)| {
                    line.split("comment passage ")
                        .nth(1)
                        .and_then(|text| text.split_whitespace().next())
                        .map(|text| (row, text.to_owned()))
                })
            };
            let before = passage(screen(&mut app, 100, 30));
            assert!(before.is_some());
            if held != 0 {
                press(&mut app, 'l');
                if held == 1 {
                    screen(&mut app, 100, 30);
                }
            }
            for body in [
                format!("new paragraph\n\nanother paragraph\n\n{original}"),
                original,
            ] {
                let view = app.post_view.as_ref().unwrap();
                let generation = view.generation;
                let mut page = view.page.as_ref().unwrap().clone();
                let child = if more {
                    &mut page.comments[0].replies[0]
                } else {
                    &mut page.comments[0]
                };
                child.body = body;
                let msg = if more {
                    Msg::MoreReplies {
                        generation,
                        comment_id: "parent".into(),
                        result: Ok(ReplyPage::complete(std::mem::take(
                            &mut page.comments[0].replies,
                        ))),
                    }
                } else {
                    Msg::Post {
                        generation,
                        result: Ok(Box::new(page)),
                    }
                };
                app.on_msg(msg);
                assert_eq!(
                    passage(screen(&mut app, 100, 30)),
                    before,
                    "more={more} held={held}"
                );
            }
        }
    }
}

#[test]
fn edited_selftext_keeps_the_unchanged_passage_being_read() {
    let (mut app, _) = open_long_post(|_| {});
    let original = paragraphs("body passage", 1..=60);
    app.post_view.as_mut().unwrap().page.as_mut().unwrap().body = original.clone();
    screen(&mut app, 100, 30);
    app.post_view.as_mut().unwrap().scroll = 26; // A blank line before unchanged text.
    let passage = |text: String| {
        text.lines().enumerate().find_map(|(row, line)| {
            line.split("body passage ")
                .nth(1)
                .and_then(|rest| rest.split_whitespace().next())
                .map(|rest| (row, rest.to_owned()))
        })
    };
    let before = passage(screen(&mut app, 100, 30));
    assert!(before.is_some());
    for body in [
        format!("inserted one\n\ninserted two\n\n{original}"),
        original,
    ] {
        let view = app.post_view.as_ref().unwrap();
        let generation = view.generation;
        let mut page = view.page.as_ref().unwrap().clone();
        page.body = body;
        app.on_msg(Msg::Post {
            generation,
            result: Ok(Box::new(page)),
        });
        assert_eq!(passage(screen(&mut app, 100, 30)), before);
    }
}

#[test]
fn spoiler_then_selftext_edit_keeps_the_unchanged_comment_passage() {
    let (mut app, _) = open_long_post(|_| {});
    let view = app.post_view.as_mut().unwrap();
    let page = view.page.as_mut().unwrap();
    page.body = "short selftext".into();
    page.comments[0].body = format!(
        "{}\n\n{SPOILER_START}short spoiler{SPOILER_END}",
        paragraphs("stable passage", 1..=60)
    );
    screen(&mut app, 100, 30);
    let view = app.post_view.as_mut().unwrap();
    view.scroll = view.row_starts[0] + 26;
    let passage = |shown: String| {
        shown.lines().enumerate().find_map(|(row, line)| {
            line.split("stable passage ")
                .nth(1)
                .and_then(|rest| rest.split_whitespace().next())
                .map(|rest| (row, rest.to_owned()))
        })
    };
    let before = passage(screen(&mut app, 100, 30));
    assert!(before.is_some());
    press_keys(&mut app, "zs");
    let view = app.post_view.as_ref().unwrap();
    let generation = view.generation;
    let mut page = view.page.as_ref().unwrap().clone();
    page.body = format!("inserted\n\nalso inserted\n\n{}", page.body);
    app.on_msg(Msg::Post {
        generation,
        result: Ok(Box::new(page)),
    });
    assert_eq!(passage(screen(&mut app, 100, 30)), before);
}
