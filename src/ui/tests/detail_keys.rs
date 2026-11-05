use super::*;

#[test]
fn upward_keys_in_a_long_body_read_upward() {
    let (mut app, _) = open_long_post(|_| {});
    key(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
    screen(&mut app, 100, 30);
    let paged = app.post_view.as_ref().unwrap().scroll;

    press(&mut app, 'k');
    screen(&mut app, 100, 30);
    let after_k = app.post_view.as_ref().unwrap().scroll;
    press_keys(&mut app, "[[");
    screen(&mut app, 100, 30);
    let view = app.post_view.as_ref().unwrap();

    assert!(paged > 0);
    assert_eq!(after_k, paged - 1, "`k` scrolls up a line, not down");
    assert_eq!(view.scroll, 0, "`[[` from the body goes to the post's top");
    assert!(view.free_scroll);
}

#[test]
fn comment_keys_pressed_while_a_post_loads_keep_it_at_its_top() {
    let (app, s) = open_long_post(|app| {
        app.activate_comment();
        app.move_comment_cursor(1);
        app.jump_top_level(true);
    });

    assert!(s.lines().any(|l| l.contains("paragraph 1 ")), "{s}");
    assert_eq!(app.post_view.as_ref().unwrap().scroll, 0);
}

#[test]
fn detail_comment_navigation_should_move_past_the_full_image_and_return_to_it() {
    let mut app = image_post("short");
    let slot = ready(&app, 320, 1280);
    app.images.insert("https://i.redd.it/big.jpeg".into(), slot);
    detail_pictures(&mut app, 80, 30);
    app.activate_comment();
    detail_pictures(&mut app, 80, 30);
    let view = app.post_view.as_ref().unwrap();
    assert!(!view.free_scroll);
    assert_eq!(
        view.scroll, 0,
        "the first comment already fits below the image"
    );
    assert!(view.row_starts[view.cursor] > view.rendered_pictures[0].2);
    assert!(view.row_starts[view.cursor] < view.scroll + view.page_height);
    press_keys(&mut app, "gg");
    let top = detail_pictures(&mut app, 80, 30);
    assert_eq!(top[0].skipped, 0);
}

#[test]
fn comment_keys_on_a_hidden_spoiler_post_select_nothing() {
    let (mut app, _) = open_long_post(|_| {});
    app.post_view.as_mut().unwrap().post.spoiler = true;
    let reading = |app: &App| app.post_view.as_ref().unwrap().free_scroll;

    screen(&mut app, 100, 30);
    press(&mut app, 'G');
    let after_g = reading(&app);
    // Reveal and draw, then hide and press `j` before the next draw.
    press_keys(&mut app, "zs");
    screen(&mut app, 100, 30);
    press_keys(&mut app, "zs");
    press(&mut app, 'j');
    let after_hide_j = reading(&app);
    // Hidden (positions cleared), then reveal and `G` before the next draw:
    // `G` scrolls down to the last comment, it does not hold the view.
    screen(&mut app, 100, 30);
    press_keys(&mut app, "zs");
    press(&mut app, 'G');
    let g_holds = app.post_view.as_ref().unwrap().hold_view.is_some();

    assert!(after_g, "`G` selects no hidden comment");
    assert!(after_hide_j, "`j` uses no stale comment positions");
    assert!(!g_holds, "`G` trusts no stale comment positions");
}

#[test]
fn ctrl_f_pages_a_post_by_its_visible_height_and_stops_at_the_end() {
    let (mut app, first) = open_long_post(|_| {});
    let paragraphs = |s: &str| -> Vec<usize> {
        s.lines()
            .filter_map(|l| l.split("paragraph ").nth(1))
            .filter_map(|r| r.split_whitespace().next()?.parse().ok())
            .collect()
    };
    let before = paragraphs(&first);

    key(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
    let after = paragraphs(&screen(&mut app, 100, 30));
    for _ in 0..50 {
        key(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
    }
    let end = screen(&mut app, 100, 30);

    // One page later, the last paragraph seen is still on screen: nothing skipped.
    assert!(
        after.contains(before.last().unwrap()),
        "{before:?} → {after:?}"
    );
    assert!(
        end.contains("reply"),
        "the last comment stays on screen:\n{end}"
    );
}
