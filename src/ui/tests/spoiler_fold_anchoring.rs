use super::*;

#[test]
fn revealing_spoilers_above_keeps_the_text_being_read() {
    let (mut app, _) = open_long_post(|_| {});
    let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
    // Hyphens are break points; the mask has none, so lines break elsewhere.
    page.body = (31..=36)
        .map(|n| format!("aaaaaaaaaa {SPOILER_START}{}{SPOILER_END}", "ab-".repeat(n)))
        .collect::<Vec<_>>()
        .join("\n\n");
    page.comments[0].body = paragraphs("line", 1..=40);
    screen(&mut app, 100, 30);
    let view = app.post_view.as_mut().unwrap();
    let masked_start = view.row_starts[0];
    view.scroll = masked_start + 10;
    let shown = |s: &str| {
        s.lines()
            .enumerate()
            .find_map(|(row, l)| l.split("line ").nth(1).map(|r| (row, r.to_string())))
    };
    let before = shown(&screen(&mut app, 100, 30));

    press_keys(&mut app, "zs");
    let after = shown(&screen(&mut app, 100, 30));
    let revealed_start = app.post_view.as_ref().unwrap().row_starts[0];

    assert_ne!(masked_start, revealed_start, "the reveal rewraps the body");
    assert!(before.is_some());
    assert_eq!(after, before, "the text being read stays");
}

#[test]
fn revealing_spoilers_above_the_body_being_read_keeps_it() {
    let mut app = post_with_rewrapping_spoilers(40);
    let view = app.post_view.as_mut().unwrap();
    // Inside the plain body lines below the spoilers.
    view.scroll = view.row_starts[0] - 30;
    let shown = |s: &str| {
        s.lines()
            .enumerate()
            .find_map(|(row, l)| l.split("body ").nth(1).map(|r| (row, r.to_string())))
    };
    let before = shown(&screen(&mut app, 100, 30));

    press_keys(&mut app, "zs");
    let after = shown(&screen(&mut app, 100, 30));

    assert!(before.is_some());
    assert_eq!(after, before);
}

#[test]
fn revealing_spoilers_below_the_body_being_read_leaves_it_alone() {
    let mut app = post_with_rewrapping_spoilers(0);
    let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
    let head = (1..=40).map(|i| format!("body {i}")).collect::<Vec<_>>();
    page.body = format!("{}\n\n{}", head.join("\n\n"), page.body);
    let shown = |s: &str| {
        s.lines()
            .enumerate()
            .find_map(|(row, l)| l.split("body ").nth(1).map(|r| (row, r.to_string())))
    };
    let mut before = Vec::new();
    let mut after = Vec::new();
    for scroll in [0, 10] {
        screen(&mut app, 100, 30);
        app.post_view.as_mut().unwrap().scroll = scroll;
        before.push(shown(&screen(&mut app, 100, 30)));
        press_keys(&mut app, "zs");
        after.push(shown(&screen(&mut app, 100, 30)));
    }

    assert!(before.iter().all(Option::is_some));
    assert_eq!(after, before);
}

#[test]
fn spoilers_stay_masked_until_v_reveals_them() {
    use crate::model::{SPOILER_END, SPOILER_START};
    let (mut app, _) = open_long_post(|_| {});
    let view = app.post_view.as_mut().unwrap();
    let page = view.page.as_mut().unwrap();
    page.body = "the butler did it".into();
    page.comments[0].body = format!("so {SPOILER_START}Rosebud{SPOILER_END} was the sled");

    // Untagged post: only the `>!…!<` text is masked.
    let masked = screen(&mut app, 100, 30);
    // Spoiler-tagged post: the body and the comments stay hidden.
    app.post_view.as_mut().unwrap().post.spoiler = true;
    let hidden = screen(&mut app, 100, 30);
    // No comment can be selected or folded while unseen.
    let rows_hidden = app.post_view.as_ref().unwrap().rows().is_empty();
    press_keys(&mut app, "zs");
    let shown = screen(&mut app, 100, 30);

    assert!(masked.contains("the butler did it"), "{masked}");
    assert!(masked.contains("so ▒▒▒▒▒▒▒ was the sled"), "{masked}");
    assert!(
        hidden.contains("zs reveals the post and its comment"),
        "{hidden}"
    );
    assert!(
        !hidden.contains("butler") && !hidden.contains("the sled"),
        "{hidden}"
    );
    assert!(!hidden.contains("no comments yet"), "{hidden}");
    assert!(rows_hidden);
    assert!(shown.contains("the butler did it"), "{shown}");
    assert!(shown.contains("so Rosebud was the sled"), "{shown}");
}

#[test]
fn selecting_the_comment_being_read_keeps_the_view() {
    let (mut app, _) = open_long_post(|_| {});
    let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
    page.comments[0].body = paragraphs("line", 1..=40);
    // Select the tall comment, then page down inside it: its header leaves.
    for keys in ["j", "\u{6}", "\u{6}"] {
        press_keys(&mut app, keys);
        screen(&mut app, 100, 30);
    }
    let reading = app.post_view.as_ref().unwrap().scroll;

    press(&mut app, 'l');
    screen(&mut app, 100, 30);
    // Later redraws (a status timing out, a health change) keep it too.
    let s = screen(&mut app, 100, 30);
    let (selected, held) = {
        let view = app.post_view.as_ref().unwrap();
        (!view.free_scroll, view.scroll)
    };
    // `j` on the last comment cannot move the selection: the view stays too.
    press(&mut app, 'j');
    screen(&mut app, 100, 30);
    let after_j = app.post_view.as_ref().unwrap().scroll;
    // `G` while reading inside the last comment stays too.
    press(&mut app, 'h');
    key(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
    screen(&mut app, 100, 30);
    let paged = app.post_view.as_ref().unwrap().scroll;
    press(&mut app, 'G');
    screen(&mut app, 100, 30);
    let after_g = app.post_view.as_ref().unwrap().scroll;

    assert!(selected, "`l` selected the comment");
    assert_eq!(held, reading, "and left the text where it was");
    assert!(
        s.lines().any(|l| l.contains("▌") && l.contains("line ")),
        "the selection bar shows beside its text: {s}"
    );
    assert_eq!(after_j, reading, "no jump up to the header");
    assert!(paged > reading);
    assert_eq!(after_g, paged, "`G` inside the last comment stays");
}

#[test]
fn folding_the_comment_being_read_shows_its_header() {
    let (mut app, _) = open_long_post(|_| {});
    let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
    let mut reply = page.comments[0].clone();
    reply.id = "r".into();
    let tall_body = paragraphs("line", 1..=40);
    page.comments[0].body = tall_body.clone();
    page.comments[0].replies = vec![reply.clone()];
    // A tall thread below, so the folded document still scrolls this far.
    page.comments.push(Comment {
        id: "next".into(),
        body: tall_body,
        ..reply
    });
    // Select the comment, scroll its header just off screen, hold it with `l`.
    press(&mut app, 'j');
    screen(&mut app, 100, 30);
    app.scroll_post(1);
    for key in ['l', 'l'] {
        screen(&mut app, 100, 30);
        press(&mut app, key);
    }
    let s = screen(&mut app, 100, 30);

    assert!(s.contains("+1 folded"), "{s}");
}

#[test]
fn folding_two_threads_before_a_draw_keeps_the_comment_left_by_h() {
    let (mut app, _) = open_long_post(|_| {});
    let view = app.post_view.as_mut().unwrap();
    let page = view.page.as_mut().unwrap();
    let template = page.comments[0].clone();
    let comment = |id: &str, body: &str, replies: Vec<Comment>| Comment {
        id: id.into(),
        body: body.into(),
        replies,
        ..template.clone()
    };
    page.body.clear();
    page.comments = vec![
        comment(
            "A",
            &"long line\n".repeat(40),
            vec![comment("a", "reply", vec![])],
        ),
        comment("B", "short", vec![comment("b", "reply", vec![])]),
        comment("C", &"tail line\n".repeat(40), vec![]),
    ];
    view.collapsed.insert("B".into());
    view.cursor = 0;
    view.free_scroll = false;
    screen(&mut app, 100, 30);
    let view = app.post_view.as_mut().unwrap();
    view.scroll = view.row_starts[0];
    screen(&mut app, 100, 30);

    // Replace A's child with B's before a draw: the row count stays four,
    // but their positions change. Reselect only after the draw places them.
    press_keys(&mut app, "za");
    press(&mut app, 'j');
    press_keys(&mut app, "za");
    press(&mut app, 'h');
    press(&mut app, 'l');

    assert!(app.post_view.as_ref().unwrap().free_scroll);
    screen(&mut app, 100, 30);
    press(&mut app, 'l');
    let view = app.post_view.as_ref().unwrap();
    assert!(matches!(
        &view.rows()[view.cursor].kind,
        RowKind::Comment { comment, .. } if comment.id == "B"
    ));
}

#[test]
fn l_after_h_goes_back_to_the_same_comment_without_folding_it() {
    let (mut app, _) = open_long_post(|_| {});
    let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
    let mut reply = page.comments[0].clone();
    reply.id = "r".into();
    page.comments[0].replies = vec![reply];
    let mut back_to = |keys: &[char]| {
        for &key in keys.iter().chain(&['h']) {
            press(&mut app, key);
            screen(&mut app, 100, 30);
        }
        press(&mut app, 'l');
        let view = app.post_view.as_ref().unwrap();
        (view.free_scroll, view.cursor, view.collapsed.len())
    };

    // The thread with a reply: selected again, not folded.
    let thread = back_to(&['G', 'k']);
    // The reply under it, both on screen: the reply, not the first comment shown.
    let reply = back_to(&['G']);

    assert_eq!(thread, (false, 0, 0));
    assert_eq!(reply, (false, 1, 0));
}

#[test]
fn masked_spoilers_take_the_columns_the_text_shows_in() {
    use crate::model::{SPOILER_END, SPOILER_START};
    let text = format!("a {SPOILER_START}👨‍👩‍👧 ❤️ go 日本{SPOILER_END} b");

    let masked = spoiler_text(&text, false);
    let shown = spoiler_text(&text, true);

    assert_eq!(masked.width(), shown.width(), "{masked:?} vs {shown:?}");
    assert!(!masked.contains("go"));
}

#[test]
fn folding_a_comment_with_only_unloaded_replies_shows_it_folded() {
    use crate::model::MoreReplies;
    let (mut app, _) = open_long_post(|_| {});
    app.post_view
        .as_mut()
        .unwrap()
        .page
        .as_mut()
        .unwrap()
        .comments[0]
        .more = Some(MoreReplies {
        cursor: "c".into(),
        count: Some(3),
    });
    screen(&mut app, 100, 30);
    press(&mut app, 'j');
    press_keys(&mut app, "za");
    let s = screen(&mut app, 100, 30);

    assert!(s.contains("+3 folded"), "{s}");
    assert!(!s.contains("more replies"), "{s}");
}
