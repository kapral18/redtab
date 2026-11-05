use super::*;

#[test]
fn when_card_pagination_starts_it_should_render_a_skeleton_after_the_entries() {
    let mut app = feed_with_next_page(ListView::Card);
    screen(&mut app, 80, 20);
    let geometry = app.active_tab().feed.cards.clone();
    app.load_feed(4, true);

    let actual = screen(&mut app, 80, 20);

    assert!(actual.contains("━"), "{actual}");
    assert!(actual.contains("Loading posts"), "{actual}");
    assert_eq!(
        actual.lines().filter(|line| line.contains("━")).count(),
        2,
        "{actual}"
    );
    assert!(
        !actual.lines().last().unwrap().contains("more posts"),
        "{actual}"
    );
    let cards = &app.active_tab().feed.cards;
    assert_eq!(
        (cards.height, cards.viewport, cards.line),
        (geometry.height, geometry.viewport, geometry.line)
    );
}

#[test]
fn when_compact_pagination_starts_it_should_render_a_skeleton_after_the_entries() {
    let mut app = feed_with_next_page(ListView::Compact);
    app.load_feed(4, true);

    let actual = screen(&mut app, 80, 20);

    assert!(actual.contains("━"), "{actual}");
    assert!(actual.contains("Loading posts"), "{actual}");
    assert_eq!(
        actual.lines().filter(|line| line.contains("━")).count(),
        2,
        "{actual}"
    );
    assert!(
        !actual.lines().last().unwrap().contains("more posts"),
        "{actual}"
    );
    assert!(actual.find("Existing post").unwrap() < actual.find("Loading posts").unwrap());
    assert_eq!(app.active_tab().feed.selected, 0);
    assert_eq!(app.active_tab().feed.offset, 0);
}
