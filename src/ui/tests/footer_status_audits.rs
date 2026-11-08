use super::*;

#[test]
fn footer_hints_follow_the_post_and_the_tab() {
    let footer = |app: &mut App| {
        app.status = None;
        let s = screen(app, 200, 30);
        s.lines().last().unwrap_or_default().to_string()
    };
    // No media: no `m`; bookmarked: `b` unsaves.
    let (mut app, _) = open_long_post(|_| {});
    let plain = footer(&mut app);
    // In memory: `b` would also save the state file.
    let post = app.post_view.as_ref().unwrap().post.clone();
    app.state.toggle_bookmark(&post, 0);
    let saved = footer(&mut app);
    // A tab opened with `p`, sorted by top: `t` and `x` both apply.
    let mut feed = feed_app(Picker::halfblocks());
    feed.open_subreddit("foo");
    let active = feed.active;
    feed.tabs[active].feed.sort = crate::model::Sort::Top;
    let opened = footer(&mut feed);
    feed.tabs[active].feed.search = Some("rust".into());
    let searching = footer(&mut feed);

    assert!(!plain.contains("media"), "{plain}");
    assert!(plain.contains(" bookmark"), "{plain}");
    assert!(saved.contains("unbookmark"), "{saved}");
    assert!(
        opened.contains("time") && opened.contains("close"),
        "{opened}"
    );
    assert!(
        searching.contains("time") && searching.contains("close"),
        "{searching}"
    );
    feed.search("neovim".into());
    let header = screen(&mut feed, 120, 6);
    assert!(
        header.contains("neovim") && header.contains("⇅ relevance") && header.contains("◷ all"),
        "{header}"
    );
}

#[test]
fn footer_activity_should_keep_the_same_keys_in_the_same_cells() {
    for width in [5, 6, 7, 8, 9, 10, 20, 32, 40, 64, 80, 100, 180] {
        for context in ["list", "sidebar", "detail", "unsaved"] {
            let mut app = if context == "detail" {
                open_video_post()
            } else {
                feed_with_next_page(ListView::Card)
            };
            app.config.media.video_autoplay = false;
            app.image_picker = kitty();
            app.show_images = true;
            app.health = Health::Up;
            app.status = None;
            if context == "sidebar" {
                app.focus_sidebar();
            }
            if context == "unsaved" {
                app.settings.store.persistence = crate::config::store::Persistence::Unsaved;
            }
            let footer = |app: &App| {
                let mut terminal = Terminal::new(TestBackend::new(width, 1)).unwrap();
                terminal
                    .draw(|f| draw_footer(f, app, f.area(), &VisibleLoaders::default(), &[]))
                    .unwrap();
                terminal.backend().buffer().clone()
            };
            let keys = |buffer: &ratatui::buffer::Buffer| {
                (0..width)
                    .filter_map(|x| {
                        let cell = &buffer[(x, 0)];
                        (cell.bg == SURFACE1).then(|| (x, cell.symbol().to_string()))
                    })
                    .collect::<Vec<_>>()
            };
            let idle = footer(&app);
            let expected = keys(&idle);
            assert!(buffer_text(&idle).contains("F1"));
            if width >= 20 {
                assert!(!expected.is_empty());
            }
            app.card_images
                .insert("pending".into(), CardImageSlot::Loading);
            for frame in [0, 4] {
                app.loading_frame = frame;
                let busy = footer(&app);
                assert!(
                    buffer_text(&busy).contains(loading::glyph(frame)),
                    "{width}: {}",
                    buffer_text(&busy)
                );
                assert_eq!(keys(&busy), expected, "{context} at width {width}");
                if width >= 40 {
                    assert!(buffer_text(&busy).contains("images"));
                }
            }
            app.card_images.clear();
            assert_eq!(
                footer(&app),
                idle,
                "completion must restore the idle footer"
            );
            if context == "detail" {
                app.post_view.as_mut().unwrap().loading = true;
            } else {
                app.load_feed(app.active, true);
            }
            let busy = footer(&app);
            assert!(buffer_text(&busy).contains(loading::glyph(app.loading_frame)));
            assert_eq!(keys(&busy), expected, "content loading at width {width}");
        }
    }
}

#[test]
fn narrow_loading_and_text_only_views_should_show_applicable_controls() {
    use crate::app::experience::Screen;
    use crate::reddit::experience::{Page, Query, Request};
    for width in [20, 24, 40, 72] {
        let mut app = app(&[]);
        app.overlay = Some(Overlay::Experience);
        let query = Query::Rules("rust".into());
        app.experience.screens = vec![Screen::Loading(query.clone())];
        app.experience.pending = Some((
            1,
            Request::Read {
                query: query.clone(),
                after: None,
            },
        ));
        let actual = screen(&mut app, width, 8);
        assert!(!actual.contains("r retry"), "{actual}");
        assert!(actual.contains("Esc"), "{actual}");
        app.experience.pending = None;
        app.experience.screens = vec![Screen::Browse {
            pagination: Default::default(),
            query,
            page: Page {
                title: "Rules".into(),
                text: "Community rules".into(),
                entries: vec![],
                after: None,
                url: "/r/rust/about/rules".into(),
            },
            selected: 0,
            scroll: 0,
        }];
        let actual = screen(&mut app, width, 8);
        assert!(!actual.contains("Enter"), "{actual}");
        assert!(actual.contains("Esc"), "{actual}");
    }
}

#[test]
fn feedback_should_precede_background_activity() {
    let mut app = feed_app(Picker::halfblocks());
    app.show_sidebar = false;
    app.images.insert(
        "https://preview.redd.it/pending.png".into(),
        ImageSlot::Loading,
    );
    app.error("Clipboard failure");
    let actual = screen(&mut app, 20, 8);
    let footer = actual.lines().last().unwrap();
    assert!(footer.contains('✗') && footer.contains("Clip"), "{actual}");
}

#[test]
fn health_shows_in_the_sidebar_footer_and_card_ages() {
    let mut app = app(&["rust"]);
    app.status = None;
    app.all_subs.groups[0].freshness = Some(crate::cache::Freshness {
        fetched_at: crate::app::now(),
        source: crate::cache::Source::default(),
    });
    for (health, sidebar) in [
        (Health::Unknown, "○ reddit "),
        (Health::Up, "● reddit "),
        (Health::Paused, "◐ reddit paused"),
        (Health::Down, "✗ reddit down"),
    ] {
        app.health = health;
        let s = screen(&mut app, 120, 30);
        let footer = s.lines().last().unwrap();

        assert!(
            s.lines()
                .any(|l| l.chars().take(30).collect::<String>().contains(sidebar)),
            "{health:?}: {s}"
        );
        assert_eq!(footer.contains("requests paused"), health == Health::Paused);
        assert_eq!(footer.contains("not answering"), health == Health::Down);
        assert_eq!(s.contains("offline"), health == Health::Down, "{health:?}");
    }
    // A narrow sidebar keeps the mark and drops the state.
    let s = screen(&mut app, 100, 30);
    assert!(s.contains("✗ reddit ") && !s.contains("reddit down"), "{s}");
}

#[test]
fn a_cache_warning_is_repeated_until_a_draw_shows_it() {
    let mut app = app(&[]);
    let warning = || Msg::CacheWarning("Could not save cache: disk full".into());
    // Replaced by a load error before any draw: never seen, so it comes back.
    app.on_msg(warning());
    app.error("r/x: connection refused");
    screen(&mut app, 100, 20);
    app.on_msg(warning());
    let shown = screen(&mut app, 100, 20);
    assert!(shown.contains("disk full"), "{shown}");

    app.status = None;
    app.on_msg(warning());

    assert!(app.status.is_none());
}

#[test]
fn the_footer_says_what_c_does_now() {
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
    let footer = |app: &mut App| {
        app.status = None; // the footer shows a status instead of key hints
        let s = screen(app, 100, 30);
        s.lines().last().unwrap_or_default().to_string()
    };
    let reading = footer(&mut app);
    press(&mut app, 'j');
    let on_comment = footer(&mut app);
    press(&mut app, 'j');
    let on_more = footer(&mut app);
    // Asked for: the row says "loading replies", and `c` does nothing more.
    press_keys(&mut app, "za");
    let loading = footer(&mut app);
    press(&mut app, 'k');
    press_keys(&mut app, "za");
    let folded = footer(&mut app);
    // A comment without replies: `c` does nothing there, so no hint.
    let (mut leaf, _) = open_long_post(|_| {});
    press(&mut leaf, 'j');
    let on_leaf = footer(&mut leaf);
    // `x` closes only tabs opened with `p`; r/s is a subscription.
    let mut feed = feed_app(Picker::halfblocks());
    let on_subscription = footer(&mut feed);
    feed.focus = Focus::Sidebar;
    let in_sidebar = footer(&mut feed);

    assert!(reading.contains("select"), "{reading}");
    assert!(on_comment.contains(" fold"), "{on_comment}");
    assert!(on_more.contains("load"), "{on_more}");
    assert!(!loading.contains("load"), "{loading}");
    assert!(folded.contains("unfold"), "{folded}");
    assert!(!on_leaf.contains(" c "), "{on_leaf}");
    assert!(!on_subscription.contains("close"), "{on_subscription}");
    assert!(
        in_sidebar.contains("tabs") && !in_sidebar.contains("close"),
        "{in_sidebar}"
    );
}
