use super::*;

#[test]
fn when_card_style_wraps_bookmark_and_pin_prefixes_the_new_post_dot_should_stay_orange() {
    let mut app = feed_with_next_page(ListView::Card);
    app.show_sidebar = false;
    let mut post = post_with("fresh", Media::None);
    post.title = "New title".into();
    post.stickied = true;
    app.state.toggle_bookmark(&post, 1);
    app.tabs[4].feed.posts[0] = post;

    let actual = screen_buffer(&mut app, 14, 20);

    let dots: Vec<_> = (4..7)
        .flat_map(|y| (1..12).map(move |x| (x, y)))
        .map(|at| &actual[at])
        .filter(|cell| cell.symbol() == "●")
        .map(|cell| cell.fg)
        .collect();
    assert_eq!(dots, vec![ACCENT]);
}

#[test]
fn when_card_style_dims_read_posts_it_should_not_add_a_new_post_dot_or_recolor_body_dots() {
    let mut app = feed_with_next_page(ListView::Card);
    let mut post = post_with("fresh", Media::None);
    post.title = "Read ● body".into();
    app.state.mark_read(&post, 1);
    app.tabs[4].feed.posts[0] = post;

    let actual = screen_buffer(&mut app, 100, 20);

    let dots: Vec<_> = (25..98)
        .map(|x| &actual[(x, 5)])
        .filter(|cell| cell.symbol() == "●")
        .map(|cell| cell.fg)
        .collect();
    assert_eq!(dots, vec![SUBTEXT]);
}

#[test]
fn when_a_feed_opens_it_should_default_to_cards_with_wrapped_titles() {
    let mut app = card_feed_app(Picker::halfblocks());
    app.show_images = false;
    app.tabs[4].feed.posts[0].title =
        "First line of the title followed by a second line ending in 東京".into();

    let actual = screen(&mut app, 70, 20);

    assert!(actual.contains("zc Card"), "{actual}");
    assert!(actual.contains("First line of the title"), "{actual}");
    assert!(actual.contains("ending in 東"), "{actual}");
    assert!(actual.contains('京'), "{actual}");
    assert!(actual.contains("comments"), "{actual}");
}

#[test]
fn when_card_style_marks_new_posts_the_dot_should_stay_orange_in_a_highlighted_title() {
    let mut app = feed_with_next_page(ListView::Card);
    let mut post = post_with("fresh", Media::None);
    post.title = "Target ● body".into();
    post.stickied = true;
    app.state.toggle_bookmark(&post, 1);
    app.tabs[4].feed.posts[0] = post;
    app.config.highlight.keywords = vec!["Target".into()];

    let actual = screen_buffer(&mut app, 100, 20);

    let dots: Vec<_> = (25..98)
        .map(|x| &actual[(x, 5)])
        .filter(|cell| cell.symbol() == "●")
        .map(|cell| cell.fg)
        .collect();
    assert_eq!(dots, vec![ACCENT, YELLOW]);
}

#[test]
fn when_card_style_leaves_the_list_unfocused_the_whole_bar_should_use_its_muted_color() {
    let mut app = card_with_image(640, 320);
    app.focus = Focus::Sidebar;

    let actual = screen_buffer(&mut app, 100, 40);

    assert!(
        (4..24).all(|y| actual[(26, y)].symbol() == "│" && actual[(26, y)].fg == OVERLAY),
        "{}",
        buffer_text(&actual)
    );
}

#[test]
fn when_card_style_selects_an_image_the_bar_should_cover_every_card_row() {
    let mut app = card_with_image(640, 320);

    let actual = screen_buffer(&mut app, 100, 40);

    assert!(
        (4..24).all(|y| actual[(26, y)].symbol() == "▌" && actual[(26, y)].fg == ACCENT),
        "{}",
        buffer_text(&actual)
    );
    assert!(actual[(28, 6)].symbol().contains('\u{10EEEE}'));
    assert_eq!(actual[(26, 24)].symbol(), " ", "inter-card gap stays clear");
}

#[test]
fn when_card_style_scrolls_into_an_image_the_bar_should_remain_visible_with_native_pixels() {
    let mut app = card_with_image(320, 640);
    app.tabs[4].feed.posts.push(post_with("tail", Media::None));
    app.tabs[4].feed.posts.push(post_with("tail2", Media::None));
    app.scroll_card(3);
    std::thread::sleep(crate::app::NAV_SETTLE);
    app.tick();

    let actual = screen_buffer(&mut app, 100, 20);

    assert!(
        (4..12).all(|y| actual[(26, y)].symbol() == "▌" && actual[(26, y)].fg == ACCENT),
        "{}",
        buffer_text(&actual)
    );
    assert!(actual[(57, 4)].symbol().contains('\u{10EEEE}'));
    assert_eq!(app.active_tab().feed.cards.line, 3);
}

#[test]
fn when_a_bookmark_list_opens_it_should_use_the_card_layout() {
    let mut app = card_feed_app(Picker::halfblocks());
    app.state
        .toggle_bookmark(&post_with("saved", Media::None), 1);
    app.switch_tab(1);

    let actual = screen(&mut app, 100, 20);

    assert!(actual.contains("zc Card"), "{actual}");
    assert!(actual.contains("saved"), "{actual}");
    assert!(actual.contains("comments"), "{actual}");
}
