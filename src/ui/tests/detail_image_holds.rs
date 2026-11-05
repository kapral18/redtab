use super::*;

#[test]
fn a_held_selection_at_the_top_keeps_the_image() {
    let mut app = image_post_with_comments(37);
    // Two rewrapping spoiler paragraphs in the first comment, so "marker 1"
    // shows under the image.
    let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
    page.comments[0].body = (31..=32)
        .map(|n| format!("aaaaaaaaaa {SPOILER_START}{}{SPOILER_END}", "ab-".repeat(n)))
        .collect::<Vec<_>>()
        .join("\n\n");
    screen(&mut app, 100, 60);
    // Reveal; select "marker 1" (row 2); `h`, `l` hold it at scroll 0.
    for keys in ["zs", "j", "j", "j", "h", "l"] {
        press_keys(&mut app, keys);
        screen(&mut app, 100, 60);
    }
    let view = app.post_view.as_ref().unwrap();
    let held = (view.cursor, view.hold_view.is_some(), view.scroll);

    // Hiding the spoilers again adds lines above the held comment.
    press_keys(&mut app, "zs");
    let s = screen(&mut app, 100, 60);

    assert_eq!(held, (2, true, 0));
    assert!(s.contains("loading image") && s.contains("marker 1"), "{s}");
    assert_eq!(app.post_view.as_ref().unwrap().scroll, 0);
}

#[test]
fn the_image_waits_for_the_post_page() {
    let shown = std::cell::RefCell::new(String::new());
    open_long_post(|app| {
        app.image_picker = kitty();
        app.show_images = true;
        app.post_view.as_mut().unwrap().post.media = Media::Image {
            url: "https://i.redd.it/big.jpeg".into(),
            preview: None,
        };
        *shown.borrow_mut() = screen(app, 100, 40);
    });
    let loading = shown.into_inner();

    // The list's copy may lack a spoiler tag the page has.
    assert!(!loading.contains("loading image"), "{loading}");
}

#[test]
fn a_hold_kept_for_the_image_holds_the_text_once_the_image_is_off() {
    use crate::model::MoreReplies;
    let mut app = marker_selected_under_the_image();
    // "marker 2" gets a "more replies" row of its own, below "marker 1".
    let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
    page.comments[2].more = Some(MoreReplies {
        cursor: "m2".into(),
        count: Some(1),
    });
    screen(&mut app, 100, 60);
    // `h` `l` hold "marker 1"; a load below it at the top pins the hold there.
    for key in ['h', 'l'] {
        press(&mut app, key);
        screen(&mut app, 100, 60);
    }
    let generation = app.post_view.as_ref().unwrap().generation;
    app.on_msg(Msg::MoreReplies {
        generation,
        comment_id: "m2".into(),
        result: Ok(ReplyPage::complete(replies(1))),
    });
    screen(&mut app, 100, 60);
    let top = app
        .post_view
        .as_ref()
        .unwrap()
        .hold_view
        .is_some_and(|h| h.top);
    let row_of = |s: &str| s.lines().position(|l| l.contains("marker 1 "));
    press_keys(&mut app, "zi");
    let before = row_of(&screen(&mut app, 100, 60));

    load_two_replies_above(&mut app);
    let after = row_of(&screen(&mut app, 100, 60));

    assert!(top, "the hold was pinned to the top");
    assert!(before.is_some());
    assert_eq!(after, before, "no image to keep: the text stays");
}

#[test]
fn images_on_and_a_load_in_one_pass_show_the_image() {
    let mut app = marker_selected_under_the_image();
    press_keys(&mut app, "zi");
    screen(&mut app, 100, 60);

    press_keys(&mut app, "zi");
    load_two_replies_above(&mut app);
    let s = screen(&mut app, 100, 60);

    assert!(s.contains("loading image"), "{s}");
    assert_eq!(app.post_view.as_ref().unwrap().scroll, 0);
}

#[test]
fn changes_above_a_selection_at_the_top_keep_the_image() {
    use crate::model::MoreReplies;
    let mut app = image_post("short");
    let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
    let reply = page.comments[0].clone();
    page.comments[0].more = Some(MoreReplies {
        cursor: "c".into(),
        count: Some(2),
    });
    page.comments[0].body = format!("so {SPOILER_START}{}{SPOILER_END}", "ab-".repeat(40));
    page.comments.push(Comment {
        id: "c2".into(),
        body: "marker two".into(),
        ..reply.clone()
    });
    // Select c2 under the image, at the top of the post; tall enough that the
    // loaded replies leave it on screen.
    let draw = |app: &mut App| screen(app, 100, 60);
    draw(&mut app);
    for key in ['j', 'j', 'j'] {
        press(&mut app, key);
        draw(&mut app);
    }
    let before = draw(&mut app);
    assert_eq!(app.post_view.as_ref().unwrap().cursor, 2, "c2 selected");

    // Spoilers above it rewrap, then replies load above it.
    press_keys(&mut app, "zs");
    let after_v = draw(&mut app);
    app.on_msg(Msg::MoreReplies {
        generation: app.post_view.as_ref().unwrap().generation,
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
    let after_load = draw(&mut app);

    assert!(
        before.contains("loading image") && before.contains("marker two"),
        "{before}"
    );
    assert!(after_v.contains("loading image"), "{after_v}");
    assert!(
        after_load.contains("loading image") && after_load.contains("marker two"),
        "{after_load}"
    );
    assert_eq!(app.post_view.as_ref().unwrap().scroll, 0);
}
