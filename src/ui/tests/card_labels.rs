use super::*;

#[test]
fn when_a_card_is_a_spoiler_it_should_show_the_tag_without_loading_its_image() {
    let mut app = card_feed_app(kitty());
    app.tabs[4].feed.posts[0].spoiler = true;

    let actual = screen(&mut app, 100, 20);

    assert!(actual.contains("spoiler"), "{actual}");
    assert!(!app.card_images.contains_key("https://i.redd.it/p1.jpeg"));
}

#[test]
fn when_a_crosspost_has_a_deleted_original_its_card_should_label_it() {
    let app = card_feed_app(kitty());
    let post = post_with("cross", Media::None);
    let mut stored = serde_json::to_value(post).unwrap();
    stored["original_deleted"] = serde_json::Value::Bool(true);
    let post = serde_json::from_value(stored).unwrap();

    let card = expanded_card(
        &app,
        &post,
        120,
        &CardOpts {
            selected: true,
            show_subreddit: true,
            reserve: 0,
        },
        50,
    );
    let text = card
        .lines
        .iter()
        .flat_map(|line| line.spans.iter().map(|s| s.content.as_ref()))
        .collect::<String>();

    assert!(text.contains("Original post deleted"), "{text}");
}

#[test]
fn when_a_cached_crosspost_lacks_parent_status_its_card_should_keep_the_known_label() {
    let mut app = card_feed_app(kitty());
    app.state.mark_original_deleted("cross", crate::app::now());
    let post = post_with("cross", Media::None);

    let card = expanded_card(
        &app,
        &post,
        120,
        &CardOpts {
            selected: true,
            show_subreddit: true,
            reserve: 0,
        },
        50,
    );
    let text = card
        .lines
        .iter()
        .flat_map(|line| line.spans.iter().map(|s| s.content.as_ref()))
        .collect::<String>();

    assert!(text.contains("Original post deleted"), "{text}");
}

#[test]
fn a_spoiler_post_lists_with_a_chip_and_no_thumbnail() {
    let mut app = feed_app(kitty());
    app.tabs[4].feed.posts[0].spoiler = true;

    let s = screen(&mut app, 100, 20);

    assert!(!app.thumbs.contains_key("https://i.redd.it/p1.jpeg"));
    assert!(s.lines().any(|l| l.contains(" spoiler ")), "{s}");
}
