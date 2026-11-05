use super::*;

#[test]
fn when_half_page_keys_are_batched_their_row_distances_should_accumulate() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = two_tall_media_cards();
    app.tabs[4].feed.posts.push(post_with("tail", Media::None));
    app.scroll_card(0);
    screen(&mut app, 100, 20);

    crate::keys::handle(
        &mut app,
        KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
    );
    crate::keys::handle(
        &mut app,
        KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
    );
    let actual = screen(&mut app, 100, 20);

    assert_eq!(app.active_tab().feed.offset, 1);
    assert_eq!(app.active_tab().feed.cards.line, 2);
    assert_eq!(
        actual
            .lines()
            .position(|line| line.contains("Following video")),
        None
    );
    assert_eq!(app.active_tab().feed.cards.pending, 0);
}

#[test]
fn when_paging_a_single_fitted_card_it_should_keep_the_selected_post() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = card_with_image(320, 640);
    screen(&mut app, 100, 20);

    crate::keys::handle(
        &mut app,
        KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
    );
    let cells = placeholder_cells(&mut app, 100, 20);

    assert_eq!(app.current_post().unwrap().id, "p1");
    assert_eq!(app.active_tab().feed.cards.line, 0);
    assert_eq!(cells.iter().map(|cell| cell.0).max(), Some(64));
}

#[test]
fn when_half_paging_past_a_tall_card_the_next_card_should_enter_without_snapping() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = two_tall_media_cards();
    app.scroll_card(0);
    screen(&mut app, 100, 20);

    crate::keys::handle(
        &mut app,
        KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
    );
    let actual = screen(&mut app, 100, 20);

    assert_eq!(
        actual
            .lines()
            .position(|line| line.contains("Following video")),
        Some(10)
    );
    assert_eq!(app.active_tab().feed.cards.line, 7);
    assert_eq!(app.current_post().unwrap().id, "p1");
}

#[test]
fn when_half_paging_back_across_a_card_header_it_should_preserve_the_remaining_distance() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = two_tall_media_cards();
    app.move_selection(1);
    screen(&mut app, 100, 20);

    crate::keys::handle(
        &mut app,
        KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
    );
    let actual = screen(&mut app, 100, 20);

    assert_eq!(app.active_tab().feed.offset, 0);
    assert_eq!(app.active_tab().feed.cards.line, 2);
    assert_eq!(
        actual
            .lines()
            .position(|line| line.contains("Following video")),
        Some(15)
    );
}

#[test]
fn when_a_half_page_spans_short_cards_it_should_consume_all_rows() {
    let mut app = card_feed_app(kitty());
    for post in &mut app.tabs[4].feed.posts {
        post.media = Media::None;
        post.title = post.id.clone();
    }
    screen(&mut app, 100, 20);

    app.page_list(1, 2);
    screen(&mut app, 100, 20);

    assert_eq!(app.active_tab().feed.offset, 1);
    assert_eq!(app.active_tab().feed.cards.line, 3);
    assert_eq!(app.current_post().unwrap().id, "p3");
}

#[test]
fn when_bookmarks_half_page_across_tall_media_the_next_card_should_enter_naturally() {
    let mut app = two_tall_media_cards();
    app.state
        .toggle_bookmark(&app.tabs[4].feed.posts[1].clone(), 1);
    app.state
        .toggle_bookmark(&app.tabs[4].feed.posts[0].clone(), 2);
    app.switch_tab(1);
    screen(&mut app, 100, 20);
    app.scroll_card(0);
    screen(&mut app, 100, 20);

    app.page_list(1, 2);
    let actual = screen(&mut app, 100, 20);

    assert_eq!(app.active_tab().feed.cards.line, 7);
    assert_eq!(
        actual
            .lines()
            .position(|line| line.contains("Following video")),
        Some(10)
    );
}

#[test]
fn when_pressing_g_inside_the_first_card_it_should_return_to_its_top() {
    let mut app = card_with_image(320, 640);
    screen(&mut app, 100, 20);
    app.scroll_card(5);

    press_keys(&mut app, "gg");
    let actual = screen(&mut app, 100, 20);

    assert!(actual.contains("Full width image"), "{actual}");
    assert_eq!(app.active_tab().feed.cards.line, 0);
}

#[test]
fn when_bookmarks_page_a_tall_card_the_saved_post_should_stay_selected() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = card_with_image(320, 640);
    app.state.toggle_bookmark(&app.current_post().unwrap(), 1);
    app.switch_tab(1);
    screen(&mut app, 100, 20);

    crate::keys::handle(
        &mut app,
        KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
    );
    screen(&mut app, 100, 20);

    assert_eq!(app.current_post().unwrap().id, "p1");
    assert_eq!(app.active_tab().feed.cards.line, 0);
}

#[test]
fn when_paging_back_from_a_short_card_it_should_keep_the_full_row_distance() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = card_with_image(320, 640);
    app.tabs[4].feed.posts.push(post_with("p2", Media::None));
    screen(&mut app, 100, 12);
    app.move_selection(1);
    screen(&mut app, 100, 12);
    crate::keys::handle(&mut app, KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
    screen(&mut app, 100, 12);
    assert_eq!(app.active_tab().feed.cards.line, 0);
    assert_eq!(app.current_post().unwrap().id, "p1");
}

#[test]
fn when_paging_to_a_tall_cards_bottom_it_should_expose_the_footer_and_keep_the_last_post() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = card_with_image(320, 640);
    screen(&mut app, 100, 20);
    app.scroll_card(1000);
    screen(&mut app, 100, 20);

    crate::keys::handle(
        &mut app,
        KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
    );
    let actual = screen(&mut app, 100, 20);

    assert!(actual.contains("comments"), "{actual}");
    assert_eq!(app.active_tab().feed.cards.line, 0);
    assert_eq!(app.current_post().unwrap().id, "p1");
}

#[test]
fn when_a_compact_list_pages_it_should_still_move_ten_posts() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = feed_app(kitty());

    crate::keys::handle(
        &mut app,
        KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
    );

    assert_eq!(app.current_post().unwrap().id, "p8");
}

#[test]
fn when_card_mode_uses_compact_rows_in_a_tiny_terminal_paging_should_still_work() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = card_feed_app(kitty());
    screen(&mut app, 100, 6);

    crate::keys::handle(
        &mut app,
        KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
    );

    assert_eq!(app.current_post().unwrap().id, "p8");
}
