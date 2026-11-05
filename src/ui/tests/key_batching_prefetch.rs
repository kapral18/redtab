use super::*;

#[test]
fn a_scroll_key_after_v_on_a_selected_comment_moves_on_from_its_text() {
    // Returns the first visible "line N" and the scroll.
    let after = |batches: &[&[&str]]| {
        let mut app = post_with_rewrapping_spoilers(0);
        let view = app.post_view.as_mut().unwrap();
        view.cursor = 0;
        view.free_scroll = false;
        view.scroll = view.row_starts[0] + 10;
        screen(&mut app, 100, 30);
        for batch in batches {
            for &key in *batch {
                press_keys(&mut app, key);
            }
            screen(&mut app, 100, 30);
        }
        let s = screen(&mut app, 100, 30);
        let first = s
            .lines()
            .find_map(|l| l.split("line ").nth(1).map(str::to_string));
        (first, app.post_view.as_ref().unwrap().scroll)
    };

    assert_eq!(
        after(&[&["zs", "\u{6}"]]).0,
        after(&[&["zs"], &["\u{6}"]]).0
    );
    // Hiding again adds lines above; `g` in the same batch still goes to the top.
    assert_eq!(after(&[&["zs"], &["zs", "gg"]]).1, 0);
}

#[test]
fn a_page_key_in_the_same_pass_as_a_load_keeps_the_reading_position() {
    let first_marker = |load: bool| {
        let mut app = image_post_with_comments(37);
        // A short first comment: a page down puts the top line in the markers,
        // below where the replies load.
        let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
        page.comments[0].body = "first".into();
        screen(&mut app, 100, 60);
        key(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
        if load {
            let generation = app.post_view.as_ref().unwrap().generation;
            app.on_msg(Msg::MoreReplies {
                generation,
                comment_id: "c".into(),
                result: Ok(ReplyPage::complete(replies(2))),
            });
        }
        let s = screen(&mut app, 100, 60);
        s.lines()
            .enumerate()
            .find_map(|(row, l)| l.split("marker ").nth(1).map(|r| (row, r.to_string())))
    };

    let without = first_marker(false);
    assert!(without.is_some());
    assert_eq!(first_marker(true), without);
}

#[test]
fn a_scroll_key_after_v_in_the_same_batch_still_scrolls() {
    // Each batch of keys is handled before one draw, as the main loop does.
    let scroll_after = |batches: &[&[&str]]| {
        let mut app = post_with_rewrapping_spoilers(0);
        let view = app.post_view.as_mut().unwrap();
        view.scroll = view.row_starts[0] + 10;
        screen(&mut app, 100, 30);
        for batch in batches {
            for &key in *batch {
                press_keys(&mut app, key);
            }
            screen(&mut app, 100, 30);
        }
        app.post_view.as_ref().unwrap().scroll
    };

    // Paging after `v` moves on from the kept text, drawn or not in between.
    assert_eq!(
        scroll_after(&[&["zs", "\u{6}"]]),
        scroll_after(&[&["zs"], &["\u{6}"]])
    );
    // Hiding again adds lines above; `g` in the same batch still goes to the top.
    assert_eq!(scroll_after(&[&["zs"], &["zs", "gg"]]), 0);
}

#[test]
fn navigation_should_prefetch_near_the_visible_end_without_draining_filtered_pages() {
    for (view, height, delayed_draw) in [
        (ListView::Compact, 72, false),
        (ListView::Card, 96, false),
        (ListView::Compact, 72, true),
        (ListView::Card, 96, true),
    ] {
        let mut app = feed_with_next_page(view);
        app.config.filters.keywords = vec!["hidden".into()];
        app.tabs[4].feed.posts = (0..25)
            .map(|i| {
                let mut post = post_with(&format!("p{i}"), Media::None);
                post.title = format!("Entry {i:02} end");
                post
            })
            .collect();
        screen(&mut app, 100, height);
        screen(&mut app, 100, height + 1);
        assert!(
            !app.active_tab().feed.loading_more(),
            "redraws must not fetch"
        );

        app.move_selection(1);
        if delayed_draw {
            std::thread::sleep(crate::app::NAV_SETTLE);
            app.tick();
        }
        let actual = screen(&mut app, 100, height);
        assert!(actual.contains("Entry 20 end"), "view={view:?}: {actual}");
        assert!(!actual.contains("Entry 24 end"), "view={view:?}: {actual}");
        assert_eq!(app.active_tab().feed.selected, 1);
        if !delayed_draw {
            assert!(
                !app.active_tab().feed.loading_more(),
                "navigation must settle first"
            );
            std::thread::sleep(crate::app::NAV_SETTLE);
            app.tick();
        }

        assert!(
            app.active_tab().feed.loading_more(),
            "view={view:?}: the visible edge should prefetch before selection reaches the end"
        );
        let generation = app.active_tab().feed.generation;
        let mut hidden = post_with("filtered", Media::None);
        hidden.title = "hidden entry".into();
        app.on_msg(Msg::Feed {
            tab: app.active_tab().id,
            generation,
            append: true,
            result: Ok(crate::model::Listing {
                posts: vec![hidden],
                after: Some("again".into()),
            }),
        });
        screen(&mut app, 100, height);
        screen(&mut app, 100, height + 1);
        app.tick();

        assert!(
            !app.active_tab().feed.loading_more(),
            "a filtered append must wait for new navigation"
        );
        assert_eq!(app.active_tab().feed.generation, generation);
        assert_eq!(app.active_tab().feed.selected, 1);

        app.select_edge(true);
        std::thread::sleep(crate::app::NAV_SETTLE);
        app.tick();
        assert!(app.active_tab().feed.loading_more());
        let generation = app.active_tab().feed.generation;
        let mut hidden = post_with("filtered-again", Media::None);
        hidden.title = "hidden again".into();
        app.on_msg(Msg::Feed {
            tab: app.active_tab().id,
            generation,
            append: true,
            result: Ok(crate::model::Listing {
                posts: vec![hidden],
                after: Some("last".into()),
            }),
        });
        screen(&mut app, 100, height);
        assert!(
            !app.active_tab().feed.loading_more(),
            "the request before drawing consumes navigation intent"
        );
        assert_eq!(app.active_tab().feed.generation, generation);
    }
}

#[test]
fn when_scrolling_a_short_loaded_feed_it_should_prefetch_at_the_visible_end() {
    let mut app = card_feed_app(kitty());
    app.tabs[4].feed.posts = (0..20)
        .map(|i| {
            let mut post = post_with(&format!("short{i}"), Media::None);
            post.title = post.id.clone();
            post
        })
        .collect();
    screen(&mut app, 100, 100);
    app.tabs[4].feed.after = Some("next".into());

    app.scroll_card(1);
    screen(&mut app, 100, 100);

    assert!(!app.active_tab().feed.loading());
    std::thread::sleep(crate::app::NAV_SETTLE);
    app.tick();
    assert!(app.active_tab().feed.loading());
    assert_eq!(app.active_tab().feed.selected, 0);
    assert_eq!(app.active_tab().feed.cards.line, 0);
}

#[test]
fn spoiler_then_comment_refresh_in_one_batch_keeps_the_old_passage() {
    for more in [false, true] {
        let (mut app, _) = open_long_post(|_| {});
        let original = paragraphs("unchanged comment passage", 1..=60);
        let original = format!("{original}\n\n{SPOILER_START}short spoiler{SPOILER_END}");
        let view = app.post_view.as_mut().unwrap();
        let page = view.page.as_mut().unwrap();
        page.body.clear();
        page.comments[0].body = original;
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
        let passage = |text: String| {
            text.lines().enumerate().find_map(|(row, line)| {
                line.split("unchanged comment passage ")
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
        let child = if more {
            &mut page.comments[0].replies[0]
        } else {
            &mut page.comments[0]
        };
        child.body = format!("inserted\n\nalso inserted\n\n{}", child.body);
        app.on_msg(if more {
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
        });
        assert_eq!(passage(screen(&mut app, 100, 30)), before, "more={more}");
    }
}

#[test]
fn moving_the_selection_in_the_same_pass_as_a_load_keeps_the_text() {
    use crate::model::MoreReplies;
    // Rows c, more(c), a, b, tall; `a` selected, then `j` and a load of 2 replies.
    let run = |draw_between: bool| {
        let (mut app, _) = open_long_post(|_| {});
        let generation = app.post_view.as_ref().unwrap().generation;
        let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
        let reply = page.comments[0].clone();
        page.comments[0].more = Some(MoreReplies {
            cursor: "c".into(),
            count: Some(2),
        });
        for (id, body) in [("a", "marker a"), ("b", "marker b")] {
            page.comments.push(Comment {
                id: id.into(),
                body: body.into(),
                ..reply.clone()
            });
        }
        page.comments.push(Comment {
            id: "tall".into(),
            body: "words\n\n".repeat(40),
            ..reply.clone()
        });
        screen(&mut app, 100, 30);
        let view = app.post_view.as_mut().unwrap();
        view.cursor = 2;
        view.free_scroll = false;
        view.scroll = view.row_starts[2] - 10;
        screen(&mut app, 100, 30);
        press(&mut app, 'j');
        if draw_between {
            screen(&mut app, 100, 30);
        }
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
        let s = screen(&mut app, 100, 30);
        s.lines().position(|l| l.contains("marker b"))
    };

    let same_pass = run(false);
    assert!(same_pass.is_some());
    assert_eq!(same_pass, run(true));
}

#[test]
fn selecting_a_comment_below_an_image_post_settles_in_one_frame() {
    let (mut app, _) = open_long_post(|_| {});
    app.image_picker = kitty();
    app.show_images = true;
    let image = Media::Image {
        url: "https://i.redd.it/big.jpeg".into(),
        preview: None,
    };
    let view = app.post_view.as_mut().unwrap();
    view.post.media = image.clone();
    view.page.as_mut().unwrap().post.media = image;
    let at_top = screen(&mut app, 100, 40);

    // `j` selects the comment under the 60-paragraph body.
    press(&mut app, 'j');
    let first = screen(&mut app, 100, 40);
    let scroll = app.post_view.as_ref().unwrap().scroll;
    let second = screen(&mut app, 100, 40);

    assert!(at_top.contains("loading image"), "{at_top}");
    assert!(!first.contains("loading image"), "{first}");
    assert_eq!(first, second, "no jump on the next redraw");
    assert_eq!(app.post_view.as_ref().unwrap().scroll, scroll);
}

#[test]
fn a_selection_moved_in_the_same_pass_as_a_load_still_shows() {
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
        id: "tall".into(),
        body: "words\n\n".repeat(40),
        ..reply.clone()
    });
    page.comments.push(Comment {
        id: "b".into(),
        ..reply.clone()
    });
    screen(&mut app, 100, 30);
    // Rows c, more(c), tall, b: select b, then `k` and a load in one pass.
    let view = app.post_view.as_mut().unwrap();
    view.cursor = 3;
    view.free_scroll = false;
    view.scroll = view.row_starts[3] - 10;
    screen(&mut app, 100, 30);
    press(&mut app, 'k');
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
    screen(&mut app, 100, 30);
    let view = app.post_view.as_ref().unwrap();

    assert_eq!(
        view.scroll, view.row_starts[view.cursor],
        "the header of `tall` shows"
    );
}
