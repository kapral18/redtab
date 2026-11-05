use super::*;

#[test]
fn when_home_is_open_it_should_show_a_mixed_list_and_all_subs_should_keep_islands() {
    let mut app = app(&["rust"]);
    app.switch_tab(0);
    let mut post = post_with("personalized", Media::None);
    post.subreddit = "rust".into();
    app.on_msg(Msg::Feed {
        tab: app.active_tab().id,
        generation: app.active_tab().feed.generation,
        append: false,
        result: Ok(crate::model::Listing {
            posts: vec![post.clone()],
            after: None,
        }),
    });
    let home = screen(&mut app, 120, 30);
    assert!(
        home.contains("best") && home.contains("r/rust") && home.contains("personalized"),
        "{home}"
    );
    assert!(home.contains("]s"), "{home}");

    app.switch_tab(crate::app::ALL_SUBS_TAB);
    app.on_msg(Msg::Group {
        generation: app.all_subs.groups[0].generation,
        subreddit: "rust".into(),
        result: Ok(crate::model::Listing {
            posts: vec![post],
            after: None,
        }),
    });
    let islands = screen(&mut app, 120, 30);
    assert!(
        islands.contains("▦ All subs") && islands.contains("personalized"),
        "{islands}"
    );
    assert!(islands.contains("]s"), "{islands}");
}

#[test]
fn island_lookahead_should_load_only_the_visible_and_following_grid_rows() {
    for (width, selected, remaining, visible, expected) in [
        (60, 0, 0, vec![0], vec![0, 1]),
        (180, 0, 0, vec![0, 1, 2], vec![0, 1, 2, 3, 4, 5]),
        (180, 11, 0, vec![9, 10, 11], vec![9, 10, 11]),
        (60, 0, 5, vec![0, 1], vec![0, 1, 2]),
        (
            180,
            0,
            5,
            vec![0, 1, 2, 3, 4, 5],
            vec![0, 1, 2, 3, 4, 5, 6, 7, 8],
        ),
    ] {
        let mut app = app(&["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l"]);
        app.active = 2;
        app.all_subs.group = selected;
        let height = card_height(&app) + 4 + remaining;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();

        terminal
            .draw(|f| {
                draw_all_subs(f, &mut app, f.area(), &mut Vec::new(), &[]);
            })
            .unwrap();

        let loading = |app: &App| {
            app.all_subs
                .groups
                .iter()
                .enumerate()
                .filter_map(|(i, group)| group.loading.then_some(i))
                .collect::<Vec<_>>()
        };
        assert_eq!(loading(&app), visible);
        for &index in &visible {
            let name = app.all_subs.groups[index].subreddit.clone();
            let mut post = post_with(&format!("stream{index}"), Media::None);
            post.subreddit = name.clone();
            app.on_msg(Msg::GroupProgress {
                subreddit: name,
                generation: app.all_subs.groups[index].generation,
                post: Box::new(post),
                source: Default::default(),
            });
        }
        terminal
            .draw(|f| {
                draw_all_subs(f, &mut app, f.area(), &mut Vec::new(), &[]);
            })
            .unwrap();
        let text = buffer_text(terminal.backend().buffer());
        for index in visible {
            assert!(text.contains(&format!("stream{index}")), "{text}");
        }
        let actual: Vec<_> = app
            .all_subs
            .groups
            .iter()
            .enumerate()
            .filter_map(|(i, group)| group.loading.then_some(i))
            .collect();
        assert_eq!(actual, expected);
        assert!(!app.all_subs.has_new_posts(&app.config));
    }
}

#[test]
fn home_shows_a_card_in_a_short_pane() {
    let mut app = app(&["s"]);
    app.all_subs.groups[0].posts = (1..=3)
        .map(|i| Post {
            id: format!("p{i}"),
            subreddit: "s".into(),
            title: format!("title {i}"),
            author: "a".into(),
            flair: None,
            score: Some(1),
            comments: 0,
            created: None,
            permalink: format!("/r/s/comments/p{i}/x/"),
            media: Media::None,
            preview_size: None,
            stickied: false,
            nsfw: false,
            spoiler: false,
            original_deleted: false,
        })
        .collect();

    let top = screen(&mut app, 100, 12);
    app.move_selection(2);
    let bottom = screen(&mut app, 100, 12);

    assert!(top.contains("title 1"), "{top}");
    // The clipped card scrolls so the selected post stays in view.
    assert!(bottom.contains("title 3"), "{bottom}");
}

#[test]
fn distant_island_selection_should_fit_after_more_than_u16_max_rows() {
    let mut app = app(&[]);
    app.config.all_subs_posts_per_group = 10;
    app.all_subs.groups = (0..6000)
        .map(|index| crate::app::Group {
            subreddit: format!("community{index}"),
            loading: true,
            ..Default::default()
        })
        .collect();
    app.all_subs.group = 5999;
    let mut terminal = Terminal::new(TestBackend::new(60, 40)).unwrap();

    terminal
        .draw(|f| {
            draw_all_subs(f, &mut app, f.area(), &mut Vec::new(), &[]);
        })
        .unwrap();

    assert_eq!(app.all_subs.offset, 5999);
    assert!(buffer_text(terminal.backend().buffer()).contains("community5999"));
}

#[test]
fn partial_bottom_islands_should_render_and_load_then_scroll_fully_into_selection() {
    for (width, columns) in [(60, 1), (180, 3)] {
        for remaining in [1, 5] {
            let mut app = app(&["a", "b", "c", "d", "e", "f", "g", "h", "i"]);
            for group in &mut app.all_subs.groups {
                let mut post = post_with(&format!("post_{}", group.subreddit), Media::None);
                post.subreddit = group.subreddit.clone();
                group.posts = vec![post];
            }
            let height = 3 + card_height(&app) + CARD_ROW_GAP + remaining;
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();

            terminal
                .draw(|f| {
                    draw_all_subs(f, &mut app, f.area(), &mut Vec::new(), &[]);
                })
                .unwrap();

            let text = buffer_text(terminal.backend().buffer());
            let name = &app.all_subs.groups[columns].subreddit;
            assert!(
                text.lines()
                    .nth((height - remaining) as usize)
                    .unwrap()
                    .contains(&format!("#  {name}")),
                "{text}"
            );
            if remaining == 5 {
                assert!(text.contains(&format!("post_{name}")), "{text}");
            }
            let loading: Vec<_> = app
                .all_subs
                .groups
                .iter()
                .enumerate()
                .filter_map(|(index, group)| group.loading.then_some(index))
                .collect();
            assert_eq!(loading, (0..columns * 2).collect::<Vec<_>>());

            app.move_all_subs_group(columns as isize);
            terminal
                .draw(|f| {
                    draw_all_subs(f, &mut app, f.area(), &mut Vec::new(), &[]);
                })
                .unwrap();

            let selected = &app.all_subs.groups[columns].subreddit;
            assert_eq!(app.all_subs.offset, 1);
            assert!(buffer_text(terminal.backend().buffer()).contains(&format!("post_{selected}")));
        }
    }
}
