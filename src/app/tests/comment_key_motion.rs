use super::*;

#[test]
fn folding_more_replies_and_cursor_anchoring() {
    let mut app = app_with(&[]);
    app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
    let generation = app.post_view.as_ref().unwrap().generation;
    let page = PostPage {
        post: post("p", "s"),
        body: String::new(),
        comments: vec![
            comment(
                "c1",
                vec![comment("c1a", vec![], None), comment("c1b", vec![], None)],
                Some("/r/s/comments/p/x/c1"),
            ),
            comment("c2", vec![], None),
        ],
    };
    app.on_msg(Msg::Post {
        generation,
        result: Ok(Box::new(page)),
    });
    let view = app.post_view.as_mut().unwrap();
    assert_eq!(view.rows().len(), 5); // c1, c1a, c1b, more, c2
    assert!(view.free_scroll, "a post opens at its top");
    view.row_starts = vec![0, 4, 8, 12, 14];
    view.scroll = 9;
    view.page_height = 10;

    // The first comment key selects the first comment on screen.
    app.move_comment_cursor(1);
    assert_eq!(app.post_view.as_ref().unwrap().cursor, 3);
    // That row is "more replies": Enter loads them in place.
    app.activate_comment();
    app.on_msg(Msg::MoreReplies {
        generation,
        comment_id: "c1".into(),
        result: Ok(ReplyPage::complete(vec![comment("c1c", vec![], None)])),
    });
    let view = app.post_view.as_mut().unwrap();
    assert_eq!(view.rows().len(), 5); // c1, c1a, c1b, c1c, c2
    view.cursor = 0;
    app.activate_comment();
    let rows = app.post_view.as_ref().unwrap().rows();
    assert_eq!(rows.len(), 2);
    assert!(matches!(rows[0].kind, RowKind::Comment { hidden: 3, .. }));
}

#[test]
fn next_top_level_from_the_top_of_a_post_selects_the_first_comment() {
    let mut app = app_with(&[]);
    app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
    let generation = app.post_view.as_ref().unwrap().generation;
    app.on_msg(thread_message(
        generation,
        vec![comment("c1", vec![], None), comment("c2", vec![], None)],
    ));
    app.post_view.as_mut().unwrap().row_starts = vec![4, 8];

    app.jump_top_level(true);
    assert_eq!(app.post_view.as_ref().unwrap().cursor, 0);
    app.jump_top_level(true);
    assert_eq!(app.post_view.as_ref().unwrap().cursor, 1);
}

#[test]
fn upward_keys_at_the_top_of_a_post_stay_there() {
    let mut app = app_with(&[]);
    app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
    let generation = app.post_view.as_ref().unwrap().generation;
    app.on_msg(thread_message(
        generation,
        vec![comment("c1", vec![], None), comment("c2", vec![], None)],
    ));
    app.post_view.as_mut().unwrap().row_starts = vec![4, 8];
    let at_top = |app: &App| {
        let view = app.post_view.as_ref().unwrap();
        view.free_scroll && view.scroll == 0
    };

    // On the first comment, `k` goes up to the post; more `k` and `[[` stay there
    // instead of flipping back down to the first comment.
    app.move_comment_cursor(1);
    assert!(!at_top(&app));
    let mut states = Vec::new();
    for _ in 0..3 {
        app.move_comment_cursor(-1);
        states.push(at_top(&app));
    }
    app.jump_top_level(false);
    states.push(at_top(&app));
    assert_eq!(states, [true, true, true, true]);

    // `j` from the top still selects the first comment.
    app.move_comment_cursor(1);
    let view = app.post_view.as_ref().unwrap();
    assert!(!view.free_scroll);
    assert_eq!(view.cursor, 0);
    // `[[` on the selected first thread goes up to the post, as `k` does.
    app.post_view.as_mut().unwrap().scroll = 4;
    app.jump_top_level(false);
    assert!(at_top(&app));
}

#[test]
fn comment_keys_while_reading_move_the_way_they_point() {
    let mut app = app_with(&[]);
    app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
    let generation = app.post_view.as_ref().unwrap().generation;
    app.on_msg(thread_message(
        generation,
        ["c1", "c2", "c3"]
            .map(|id| comment(id, vec![], None))
            .into(),
    ));
    // A long body, then comments at lines 100-139 (tall), 140-149 and 150 on
    // (the last, tall); 20 lines show.
    let press_at = |app: &mut App, scroll: usize, key: fn(&mut App)| {
        let view = app.post_view.as_mut().unwrap();
        view.row_starts = vec![100, 140, 150];
        view.page_height = 20;
        view.scroll = scroll;
        view.free_scroll = true;
        key(app);
        let view = app.post_view.as_ref().unwrap();
        (view.free_scroll, view.cursor, view.scroll)
    };
    let k = |app: &mut App| app.move_comment_cursor(-1);
    let j = |app: &mut App| app.move_comment_cursor(1);
    let up = |app: &mut App| app.jump_top_level(false);
    let down = |app: &mut App| app.jump_top_level(true);

    // The first comment's header on the bottom line: up keys keep reading up.
    assert_eq!(press_at(&mut app, 81, k), (true, 0, 80));
    assert!(matches!(press_at(&mut app, 81, up), (true, _, 0)));
    // The first thread's header on the top line: the thread before is the post.
    assert!(matches!(press_at(&mut app, 100, up), (true, _, 0)));
    // Inside the tall first comment: up keys select it, down keys the next one.
    assert_eq!(press_at(&mut app, 110, k), (false, 0, 110));
    assert_eq!(press_at(&mut app, 110, up), (false, 0, 110));
    // A thread header exactly on the top line: `[[` goes to the thread before.
    assert_eq!(press_at(&mut app, 140, up), (false, 0, 140));
    assert_eq!(press_at(&mut app, 110, j), (false, 1, 110));
    assert_eq!(press_at(&mut app, 110, down), (false, 1, 110));
    // Inside a later thread, `[[` selects that thread, not the one before.
    assert_eq!(press_at(&mut app, 145, up), (false, 1, 145));
    // Inside the last comment: `j` reads on by a line, `]]` stays.
    assert!(matches!(press_at(&mut app, 155, j), (true, _, 156)));
    assert!(matches!(press_at(&mut app, 155, down), (true, _, 155)));

    // `l` selects the comment being read: the first starting on screen, else
    // the one holding the top line; after `h`, the comment it left.
    let l = |app: &mut App| app.activate_comment();
    assert_eq!(press_at(&mut app, 135, l), (false, 1, 135));
    assert_eq!(press_at(&mut app, 110, l), (false, 0, 110));
    app.post_view.as_mut().unwrap().cursor = 2;
    app.post_view.as_mut().unwrap().free_scroll = false;
    app.leave_comment();
    assert_eq!(press_at(&mut app, 135, l), (false, 2, 135));
    // Scrolling away without `h` forgets it: `l` selects what is being read.
    assert_eq!(press_at(&mut app, 135, l), (false, 1, 135));
    // `G` selects too, so it also forgets the comment `h` left.
    app.post_view.as_mut().unwrap().cursor = 0;
    app.post_view.as_mut().unwrap().free_scroll = false;
    app.leave_comment();
    app.post_bottom();
    assert_eq!(press_at(&mut app, 135, l), (false, 1, 135));
}

#[test]
fn next_thread_while_reading_the_last_one_keeps_reading() {
    let mut app = app_with(&[]);
    app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
    let generation = app.post_view.as_ref().unwrap().generation;
    app.on_msg(thread_message(
        generation,
        vec![comment(
            "c",
            vec![comment("r1", vec![], None), comment("r2", vec![], None)],
            None,
        )],
    ));
    // Rows c, r1, r2 at lines 100 (tall), 125 and 135; the screen shows 120-139.
    let view = app.post_view.as_mut().unwrap();
    view.row_starts = vec![100, 125, 135];
    view.page_height = 20;
    view.cursor = 2;
    view.free_scroll = false;
    app.leave_comment(); // `h` from r2
    app.post_view.as_mut().unwrap().scroll = 120;

    app.jump_top_level(true);
    let view = app.post_view.as_ref().unwrap();
    let after_n = (view.free_scroll, view.scroll);
    app.activate_comment();
    let after_l = app.post_view.as_ref().unwrap().cursor;

    assert_eq!(
        after_n,
        (true, 120),
        "`]]` selects no reply: no thread follows"
    );
    assert_eq!(after_l, 2, "and `l` still returns to the comment `h` left");
}

#[test]
fn l_with_the_left_comment_scrolled_off_selects_what_is_read() {
    let mut app = app_with(&[]);
    app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
    let generation = app.post_view.as_ref().unwrap().generation;
    app.on_msg(thread_message(
        generation,
        ["c0", "c1", "c2", "c3"]
            .map(|id| comment(id, vec![], None))
            .into(),
    ));
    // Rows of 6 lines plus a blank line; `h` left c1 (lines 12-17).
    let view = app.post_view.as_mut().unwrap();
    view.row_starts = vec![5, 12, 19, 26];
    view.page_height = 20;
    view.cursor = 1;
    view.free_scroll = false;
    app.leave_comment();
    // Read on until the top line is the blank line under c1.
    app.post_view.as_mut().unwrap().scroll = 18;

    app.activate_comment();
    let view = app.post_view.as_ref().unwrap();

    assert_eq!(view.cursor, 2, "c1 is off screen: the comment read is c2");
}

#[test]
fn comment_keys_after_a_fold_in_the_same_batch_wait_for_the_draw() {
    let mut app = app_with(&[]);
    app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
    let generation = app.post_view.as_ref().unwrap().generation;
    let replies = ["a1", "a2", "a3"]
        .map(|id| comment(id, vec![], None))
        .into();
    app.on_msg(thread_message(
        generation,
        vec![
            comment("a", replies, None),
            comment("b", vec![], None),
            comment("c", vec![], None),
        ],
    ));
    // Positions of the last draw: a, a1, a2, a3, b, c.
    let view = app.post_view.as_mut().unwrap();
    view.row_starts = vec![10, 14, 18, 22, 26, 30];
    view.page_height = 20;
    view.free_scroll = false;
    // Fold `a`, `h`, then `j`, `]]`, `[[` and `l`, all before the next draw.
    app.activate_comment();
    app.leave_comment();
    app.post_view.as_mut().unwrap().scroll = 23;
    app.move_comment_cursor(1);
    app.jump_top_level(true);
    app.jump_top_level(false);
    app.activate_comment();
    let view = app.post_view.as_ref().unwrap();

    assert!(view.free_scroll, "nothing selected from stale positions");
    assert_eq!(view.scroll, 23);
    assert!(view.cursor < view.rows().len());
}

#[test]
fn a_load_landing_after_a_fold_in_the_same_batch_keeps_the_selected_comment() {
    let mut app = app_with(&[]);
    app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
    app.focus = Focus::Post;
    let generation = app.post_view.as_ref().unwrap().generation;
    let thread = |extra: bool| {
        let mut long = comment("c2", vec![], None);
        long.body = (1..=40)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n\n");
        let mut comments = vec![
            comment("c1", vec![comment("reply", vec![], None)], None),
            long,
        ];
        if extra {
            comments.insert(0, comment("new", vec![], None));
        }
        thread_message(generation, comments)
    };
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 20)).unwrap();
    let mut selected_row = |app: &mut App| {
        terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .find(|&y| (0..buffer.area.width).any(|x| buffer[(x, y)].symbol() == "▌"))
    };
    app.on_msg(thread(false));
    let view = app.post_view.as_mut().unwrap();
    view.cursor = 0;
    view.free_scroll = false;
    selected_row(&mut app);
    let view = app.post_view.as_mut().unwrap();
    view.scroll += 3;
    let before = selected_row(&mut app);
    assert!(before.is_some());

    app.activate_comment();
    app.on_msg(thread(true));

    assert_eq!(selected_row(&mut app), before);
    let view = app.post_view.as_ref().unwrap();
    assert!(matches!(view.cursor_key(), Some(RowKey::Comment(id)) if id == "c1"));
}
