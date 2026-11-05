use super::*;

#[test]
fn sidebar_keeps_global_navigation_fixed_while_feed_sections_scroll() {
    let mut app = app(&["neovim", "rust"]);

    let s = screen(&mut app, 120, 30);
    let lines: Vec<&str> = s.lines().collect();
    // Only the sidebar columns: Home cards also mention subreddit names.
    let row_of = |needle: &str| {
        lines
            .iter()
            .position(|l| l.chars().take(30).collect::<String>().contains(needle))
            .unwrap_or(usize::MAX)
    };

    assert!(row_of("Home") < row_of("Bookmarks"));
    assert!(row_of("Bookmarks") < row_of("SUBSCRIPTIONS"));
    assert!(row_of("SUBSCRIPTIONS") < row_of("neovim"));
    assert!(row_of("neovim") < row_of("rust"));
    let globals = ["Home", "Bookmarks", "All subs", "Search"];
    let expected_rows = globals.map(row_of);
    assert!(!expected_rows.contains(&usize::MAX));
    app.on_msg(Msg::Subscriptions {
        generation: 0,
        result: Ok((0..24)
            .map(|i| crate::model::Subscription::Community {
                name: format!("sub{i:02}"),
                icon: crate::model::SubscriptionIcon::Unknown,
            })
            .collect()),
    });
    app.open_subreddit("opened");
    let sidebar = |app: &mut App, height| {
        screen(app, 120, height)
            .lines()
            .map(|line| line.chars().take(30).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    };
    for height in [30, 11] {
        let actual = sidebar(&mut app, height);
        let actual_rows = globals.map(|name| {
            actual
                .lines()
                .position(|line| line.contains(name))
                .unwrap_or(usize::MAX)
        });
        assert_eq!(actual_rows, expected_rows, "{actual}");
        assert!(actual.contains("opened"), "{actual}");
    }
    let actual = sidebar(&mut app, 8);
    assert!(actual.contains("opened"), "{actual}");
    app.switch_tab(crate::app::SEARCH_TAB);
    let actual = sidebar(&mut app, 8);
    assert!(actual.contains("Search"), "{actual}");
}

#[test]
fn when_account_lists_load_the_sidebar_should_show_progress_and_retain_refreshed_entries() {
    let mut app = app(&[]);
    app.active = 0;
    app.tabs[0].feed.loaded = true;
    app.all_subs.freshness = None;
    app.all_subs.loading = true;
    let sidebar = |app: &mut App| {
        screen(app, 120, 30)
            .lines()
            .map(|line| line.chars().take(28).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    };

    let actual = sidebar(&mut app);
    assert!(actual.contains("SUBSCRIPTIONS"), "{actual}");
    assert!(actual.contains("FOLLOWING"), "{actual}");
    assert_eq!(actual.matches("⠋ Loading").count(), 2, "{actual}");

    app.on_msg(Msg::Subscriptions {
        generation: 0,
        result: Ok(vec![
            crate::model::Subscription::Community {
                name: "rust".into(),
                icon: crate::model::SubscriptionIcon::Unknown,
            },
            crate::model::Subscription::User {
                name: "reader".into(),
                icon: None,
            },
        ]),
    });
    let actual = sidebar(&mut app);
    assert!(
        actual.contains("rust") && actual.contains("reader"),
        "{actual}"
    );
    assert!(!actual.contains("Loading"), "{actual}");

    let rows = |actual: &str| {
        actual
            .lines()
            .position(|line| line.contains("rust"))
            .zip(actual.lines().position(|line| line.contains("reader")))
    };
    let settled = rows(&actual);
    app.all_subs.loading = true;
    let actual = sidebar(&mut app);
    assert_eq!(rows(&actual), settled, "{actual}");
    assert!(!actual.contains("Loading"), "{actual}");
    assert!(actual.contains("SUBSCRIPTIONS ⠋"), "{actual}");
    assert!(actual.contains("FOLLOWING ⠋"), "{actual}");

    app.on_msg(Msg::Subscriptions {
        generation: 0,
        result: Err("offline".into()),
    });
    let actual = sidebar(&mut app);
    assert!(
        actual.contains("rust") && actual.contains("reader"),
        "{actual}"
    );
    assert!(!actual.contains("Loading"), "{actual}");

    // An emptied section does not reappear as a placeholder while syncing.
    app.on_msg(Msg::Subscriptions {
        generation: 0,
        result: Ok(vec![crate::model::Subscription::Community {
            name: "rust".into(),
            icon: crate::model::SubscriptionIcon::Unknown,
        }]),
    });
    let settled = sidebar(&mut app);
    assert!(!settled.contains("FOLLOWING"), "{settled}");
    app.all_subs.loading = true;
    let actual = sidebar(&mut app);
    assert!(!actual.contains("Loading"), "{actual}");
    assert_eq!(rows(&actual), rows(&settled), "{actual}");
}

#[test]
fn h_climbs_from_a_comment_to_the_sidebar_and_l_comes_back() {
    let (mut app, _) = open_long_post(|_| {});
    press(&mut app, 'j');
    assert!(
        !app.post_view.as_ref().unwrap().free_scroll,
        "a comment is selected"
    );

    press(&mut app, 'h');
    assert!(
        app.post_view.as_ref().unwrap().free_scroll,
        "back to reading the post"
    );
    press(&mut app, 'h');
    assert!(app.post_view.is_none(), "back to the list");
    assert_eq!(app.focus, Focus::List);
    press(&mut app, 'h');
    assert_eq!(app.focus, Focus::Sidebar);
    press(&mut app, 'h');
    assert_eq!(app.focus, Focus::Sidebar, "the sidebar is the top");

    press(&mut app, 'l');
    assert_eq!(app.focus, Focus::List);
    assert!(
        app.post_view.is_none(),
        "l from the sidebar only returns focus"
    );
}

#[test]
fn sidebar_j_k_switch_tabs_in_sidebar_order_and_stop_at_the_ends() {
    let mut app = app(&["a", "b"]);
    app.open_subreddit("c");
    app.switch_tab(0);
    let name = |app: &App| app.active_tab().title();

    press(&mut app, 'h');
    let mut seen = vec![name(&app)];
    for _ in 0..7 {
        press(&mut app, 'j');
        seen.push(name(&app));
    }
    press(&mut app, 'k');
    let after_k = name(&app);
    press_keys(&mut app, "gg");
    let first = name(&app);
    let shown = screen(&mut app, 100, 20);
    assert!(shown.contains("⌕ Search"), "{shown}");

    assert_eq!(
        seen,
        [
            "Home",
            "Bookmarks",
            "All subs",
            "Search",
            "r/a",
            "r/b",
            "r/c",
            "r/c"
        ]
    );
    assert_eq!(after_k, "r/b");
    assert_eq!(first, "Home");
    assert_eq!(app.focus, Focus::Sidebar);
}

#[test]
fn sidebar_distinguishes_unknown_from_zero_and_keeps_badges_aligned() {
    let mut app = app(&["longcommunityname"]);
    let i = 4;
    for width in [24, 30] {
        let unknown = sidebar_item(&app, i, width);
        assert_eq!(line_width(&unknown.spans), width);
        assert!(unknown.to_string().trim_end().ends_with('?'));
    }
    app.state.observe(
        Some("longcommunityname"),
        &[],
        crate::cache::Freshness {
            fetched_at: 1,
            source: crate::cache::Source::default(),
        },
        1,
    );
    let zero = sidebar_item(&app, i, 24).to_string();
    assert!(!zero.contains('?'));
    let mut post = post_with("fetched", Media::None);
    post.subreddit = "longcommunityname".into();
    app.state.observe(
        Some("longcommunityname"),
        &[post],
        crate::cache::Freshness {
            fetched_at: 2,
            source: crate::cache::Source::default(),
        },
        2,
    );
    app.focus = Focus::Sidebar;
    app.active = i;
    for width in [24, 30] {
        let known = sidebar_item(&app, i, width);
        assert_eq!(line_width(&known.spans), width);
        assert!(known.to_string().trim_end().ends_with('1'));
    }
}

#[test]
fn the_sidebar_reflects_a_displayed_post_read_in_the_same_frame() {
    let mut app = app(&["s"]);
    let first = post_with("p", Media::None);
    let second = post_with("q", Media::None);
    app.on_msg(Msg::Group {
        generation: app.all_subs.groups[0].generation,
        subreddit: "s".into(),
        result: Ok(crate::model::Listing {
            posts: vec![first.clone(), second],
            after: None,
        }),
    });
    app.open_current_post();
    let generation = app.post_view.as_ref().unwrap().generation;
    app.on_msg(Msg::Post {
        generation,
        result: Ok(Box::new(PostPage {
            post: first,
            body: "Readable post".into(),
            comments: vec![],
        })),
    });
    assert_eq!(app.badge(&app.tabs[4]), UnreadBadge::Count(2));

    let buffer = screen_buffer(&mut app, 100, 30);

    assert_eq!(app.badge(&app.tabs[4]), UnreadBadge::Count(1));
    let rows: Vec<_> = (0..30)
        .map(|y| {
            (0..app.sidebar_width())
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect();
    let row = rows.iter().find(|row| row.contains("# s")).unwrap();
    assert!(row.trim_end().ends_with('1'), "{row}");
}
