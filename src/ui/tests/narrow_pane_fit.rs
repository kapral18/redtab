use super::*;

#[test]
fn when_a_retained_preview_is_rounded_to_one_pixel_wide_it_should_keep_the_original_aspect() {
    let mut picker = kitty();
    picker.set_protocol_type(ProtocolType::Iterm2);
    let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(1, 256, |_, y| {
        image::Rgb([y as u8, 0, 0])
    }));

    let encoded = encode_tile(
        &picker,
        &image,
        None,
        (Size::new(68, 27200), (2, 1600), None),
        16384,
        1,
        0,
    )
    .unwrap();
    let actual = card_png(&encoded);

    assert_eq!(actual.dimensions(), (544, 16));
    assert!((153..=154).contains(&actual.get_pixel(272, 0)[0]));
}

#[test]
fn narrow_headers_should_prioritize_failure_and_retry() {
    let mut subs = app(&["s"]);
    subs.show_sidebar = false;
    subs.all_subs.groups[0].posts = vec![post_with("p", Media::None)];
    subs.all_subs.groups[0].loading = false;
    subs.on_msg(Msg::Subscriptions {
        generation: 0,
        result: Err("fixture failure".into()),
    });
    subs.status = None;
    let actual = screen(&mut subs, 24, 8);
    let header = actual.lines().take(3).collect::<Vec<_>>().join("\n");
    assert!(
        header.contains("subs failed") && header.contains("r retry"),
        "{actual}"
    );
    for retained in [false, true] {
        let mut group = app(&["a_very_long_community"]);
        group.show_sidebar = false;
        group.all_subs.groups[0].loading = false;
        group.all_subs.groups[0].error = Some("Fixture refresh failure".into());
        if retained {
            let mut post = post_with("cached", Media::None);
            post.subreddit = "a_very_long_community".into();
            group.all_subs.groups[0].posts = vec![post];
        }
        group.status = None;
        let actual = screen(&mut group, 24, 10);
        assert!(
            actual
                .lines()
                .any(|line| line.contains('✗') && line.contains("r retry")),
            "{actual}"
        );
    }
    for append in [false, true] {
        let mut feed = feed_with_next_page(ListView::Compact);
        feed.show_sidebar = false;
        feed.load_feed(4, append);
        feed.on_msg(Msg::Feed {
            tab: feed.active_tab().id,
            generation: feed.active_tab().feed.generation,
            append,
            result: Err("fixture failure".into()),
        });
        feed.status = None;
        let actual = screen(&mut feed, 24, 8);
        let header = actual.lines().take(3).collect::<Vec<_>>().join("\n");
        assert!(
            header.contains("failed") && header.contains("r retry"),
            "{actual}"
        );
    }
}

#[test]
fn long_post_title_should_leave_room_to_read_the_document() {
    let (mut app, _) = open_long_post(|_| {});
    app.show_sidebar = false;
    let view = app.post_view.as_mut().unwrap();
    view.page.as_mut().unwrap().post.title =
        format!("{}TITLEEND", "A long title with many words ".repeat(24));
    screen(&mut app, 50, 14);
    let view = app.post_view.as_ref().unwrap();
    assert!(view.page_height >= 6);
    assert!(
        view.rendered_document
            .iter()
            .any(|line| line.to_string().contains("TITLEEND"))
    );
}

#[test]
fn deep_comment_body_should_fit_the_available_columns() {
    let (mut app, _) = open_long_post(|_| {});
    app.show_sidebar = false;
    let view = app.post_view.as_mut().unwrap();
    let mut leaf = reply("leaf".into());
    leaf.body = "Every word in this deeply nested reply must remain readable".into();
    for depth in 0..24 {
        let mut parent = reply(format!("p{depth}"));
        parent.replies = vec![leaf];
        leaf = parent;
    }
    view.page.as_mut().unwrap().comments = vec![leaf];
    screen(&mut app, 40, 24);
    let lines = &app.post_view.as_ref().unwrap().rendered_document;
    let actual = lines
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" ");
    assert!(lines.iter().all(|line| line.width() <= 34), "{actual}");
    assert!(actual.contains("readable"));
}

#[test]
fn partial_bottom_posts_should_render_without_hiding_selected_metadata() {
    for bookmarks in [false, true] {
        for (height, selected, offset, bottom) in [
            (8, 0, 0, "Second post"),
            (9, 1, 0, "author2"),
            (8, 1, 1, "Third post"),
            (8, 2, 2, "┃"),
        ] {
            let mut app = feed_app(Picker::halfblocks());
            app.show_sidebar = false;
            app.tabs[4].feed.posts.truncate(3);
            for (index, post) in app.tabs[4].feed.posts.iter_mut().enumerate() {
                post.title = ["First post", "Second post", "Third post"][index].into();
                post.author = format!("author{}", index + 1);
            }
            if bookmarks {
                for post in app.tabs[4].feed.posts.clone().iter().rev() {
                    app.state.toggle_bookmark(post, 1);
                }
                app.switch_tab(1);
            }
            app.tabs[app.active].feed.selected = selected;

            let actual = screen(&mut app, 100, height);

            assert!(
                actual
                    .lines()
                    .nth((height - 2) as usize)
                    .unwrap()
                    .contains(bottom),
                "{actual}"
            );
            assert!(
                actual.contains(&format!("author{}", selected + 1)),
                "{actual}"
            );
            assert_eq!(app.active_tab().feed.offset, offset, "{actual}");
        }
    }
}

#[test]
fn safety_labels_should_precede_a_long_author_in_compact_rows() {
    let mut app = feed_app(Picker::halfblocks());
    app.show_sidebar = false;
    app.config.show_nsfw = true;
    for post in &mut app.tabs[4].feed.posts {
        post.author = "an_author_name_that_is_long_enough_to_fill_the_metadata_row".into();
        post.nsfw = true;
        post.spoiler = true;
    }

    let actual = screen(&mut app, 40, 20);

    assert!(actual.contains("spoiler"), "{actual}");
}

#[test]
fn when_a_post_is_open_the_list_should_only_remain_if_both_panes_fit() {
    for (sidebar, width, list_visible) in [
        (true, 158, true),
        (true, 157, false),
        (false, 128, true),
        (false, 127, false),
    ] {
        let (mut app, _) = open_long_post(|_| {});
        app.show_sidebar = sidebar;

        let actual = screen(&mut app, width, 30);
        let content: String = actual
            .lines()
            .map(|line| {
                line.chars()
                    .skip(if sidebar { 30 } else { 0 })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert_eq!(
            content.contains("▦ All subs"),
            list_visible,
            "sidebar={sidebar}, width={width}\n{actual}"
        );
        assert!(content.contains("paragraph 1 "), "{actual}");
    }
}

#[test]
fn comment_age_counts_from_its_creation_time() {
    let (mut app, _) = open_long_post(|_| {});
    let comment = &mut app
        .post_view
        .as_mut()
        .unwrap()
        .page
        .as_mut()
        .unwrap()
        .comments[0];
    comment.created = Some(Utc::now() - chrono::Duration::days(3));
    let view = app.post_view.as_mut().unwrap();
    view.scroll = usize::MAX / 2;

    let shown = screen(&mut app, 100, 30);
    let header = shown
        .lines()
        .find(|l| l.contains("u/x"))
        .unwrap_or_default()
        .to_owned();

    assert!(header.contains("3d ago"), "{header}");
}
