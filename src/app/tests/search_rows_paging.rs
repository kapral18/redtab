use super::*;

#[test]
fn search_rows_should_take_listing_icons_and_request_only_the_missing_ones_when_drawn() {
    use crate::model::SubscriptionIcon;
    use crate::reddit::experience::{Entry, Page, Query, Response};
    let community = |name: &str, icon| Entry {
        title: format!("r/{name}"),
        body: String::new(),
        target: None,
        query: Some(Query::Community(name.into())),
        post: None,
        icon,
    };
    let mut app = app_with(&[]);
    show_thumbnails(&mut app);
    app.open_search(None);
    app.on_search(
        app.search.discovered_generation(),
        Ok(Response::Page(Page {
            title: "Discover".into(),
            text: String::new(),
            entries: vec![
                community("zig", SubscriptionIcon::Url("/icon/zig.png".into())),
                community("odin", SubscriptionIcon::Unknown),
                community("nim", SubscriptionIcon::Missing),
            ],
            after: None,
            url: "/".into(),
        })),
    );
    assert_eq!(
        app.icons.get("zig").and_then(|icon| icon.url.as_deref()),
        Some("/icon/zig.png")
    );

    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 24)).unwrap();
    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();

    assert!(
        app.icon_lookups.contains("odin"),
        "a drawn row without a listing icon asks for one"
    );
    assert!(
        !app.icon_lookups.contains("zig") && !app.icon_lookups.contains("nim"),
        "a listing icon, or a listing's lack of one, needs no lookup: {:?}",
        app.icon_lookups
    );
}

#[test]
fn search_rows_should_ask_for_icons_of_exactly_the_drawn_rows_when_scrolled() {
    use crate::reddit::experience::{Entry, Page, Query, Response};
    let names: Vec<String> = (0..40).map(|i| format!("c{i:02}")).collect();
    let mut app = app_with(&[]);
    show_thumbnails(&mut app);
    app.open_search(None);
    app.on_search(
        app.search.discovered_generation(),
        Ok(Response::Page(Page {
            title: "Discover".into(),
            text: String::new(),
            entries: names
                .iter()
                .map(|name| Entry {
                    title: format!("r/{name}"),
                    body: String::new(),
                    target: None,
                    query: Some(Query::Community(name.clone())),
                    post: None,
                    icon: Default::default(),
                })
                .collect(),
            after: None,
            url: "/".into(),
        })),
    );
    app.search.selected = 30;

    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 24)).unwrap();
    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    let shown: String = buffer.content().iter().map(|cell| cell.symbol()).collect();

    for name in &names {
        assert_eq!(
            app.icon_lookups.contains(name.as_str()),
            shown.contains(name.as_str()),
            "{name}: icons are asked for the drawn rows only"
        );
    }
}

#[test]
fn search_rows_should_stop_paging_after_a_failure_and_drop_a_page_for_an_old_query() {
    use crate::reddit::experience::{Entry, Page, Query, Response, SearchKind};
    let page = |names: &[&str], after: Option<&str>| {
        Ok(Response::Page(Page {
            title: "Communities".into(),
            text: String::new(),
            entries: names
                .iter()
                .map(|name| Entry {
                    icon: Default::default(),
                    title: format!("r/{name}"),
                    body: String::new(),
                    query: Some(Query::Community((*name).into())),
                    target: None,
                    post: None,
                })
                .collect(),
            after: after.map(str::to_string),
            url: "/".into(),
        }))
    };
    let mut app = app_with(&["vim"]);
    app.open_search(Some(SearchKind::Communities));
    assert_eq!(
        app.search.rows_status(),
        Some(search::RowsStatus::Discovering)
    );
    app.on_search(app.search.discovered_generation(), Err("offline".into()));
    assert_eq!(
        app.search.rows_status(),
        Some(search::RowsStatus::DiscoveryFailed)
    );
    app.search_preview_drawn(true);
    let _ = app.tick_at(Instant::now() + search::PREVIEW_SETTLE);
    app.on_msg(Msg::Feed {
        tab: crate::worker::PREVIEW_FEED,
        generation: app.search.preview.generation,
        append: false,
        result: Err("offline".into()),
    });
    let failed = app.search.discovered_generation();
    app.search_arrived();
    let _ = app.tick_at(Instant::now() + search::PREVIEW_SETTLE);
    assert!(
        app.search.discovered_generation() == failed
            && app.search.preview.error.is_some()
            && !app.search.preview.loading,
        "failures wait for r instead of the next arrival or poll"
    );
    press(&mut app, crossterm::event::KeyCode::Down);
    assert_eq!(app.search.selected, 1);
    crate::keys::handle(
        &mut app,
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('a'),
            crossterm::event::KeyModifiers::CONTROL | crossterm::event::KeyModifiers::ALT,
        ),
    );
    assert_eq!(app.search.text(), "", "chords do not type into the query");
    crate::keys::paste(&mut app, "ru\nst");
    assert_eq!(app.search.text(), "ru st", "line breaks paste as spaces");
    assert_eq!(
        app.search.selected, 0,
        "a pasted query starts at the first result"
    );
    crate::keys::handle(
        &mut app,
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('u'),
            crossterm::event::KeyModifiers::CONTROL,
        ),
    );
    crate::keys::paste(&mut app, "rust");
    assert_eq!(
        app.search.selected, 0,
        "a pasted query starts at the first result"
    );
    assert_eq!(
        app.search.rows_status(),
        None,
        "the typing pause adds no line"
    );
    press(&mut app, crossterm::event::KeyCode::Enter);
    app.on_search(
        app.search.results_generation(),
        page(&["a", "b"], Some("next")),
    );
    press(&mut app, crossterm::event::KeyCode::Down);
    assert_eq!(
        app.search.rows_status(),
        Some(search::RowsStatus::LoadingMore)
    );
    let interrupted = app.search.results_generation();
    let rows = app.search.rows();
    let selected = app.search.selected;
    app.switch_tab(BOOKMARKS_TAB);
    app.on_search(interrupted, Err("late failure".into()));
    app.switch_tab(SEARCH_TAB);
    app.tick_at(Instant::now() + Duration::from_secs(1));
    assert_eq!(app.search.rows(), rows);
    assert_eq!(app.search.selected, selected);
    assert!(app.search.results.loading);
    assert_ne!(app.search.results_generation(), interrupted);
    let failed = app.search.results_generation();
    app.on_search(failed, Err("offline".into()));
    assert_eq!(app.search.rows().len(), 2);
    assert_eq!(
        app.search.rows_status(),
        Some(search::RowsStatus::MoreFailed)
    );
    press(&mut app, crossterm::event::KeyCode::Up);
    press(&mut app, crossterm::event::KeyCode::Down);
    assert!(
        app.search.results_generation() == failed && !app.search.results.loading,
        "a failed page is not asked for again by moving"
    );

    press(&mut app, crossterm::event::KeyCode::Tab);
    let _ = app.tick_at(Instant::now());
    assert_eq!(app.search.kind, SearchKind::People);
    app.on_search(failed, page(&["late"], None));
    assert!(
        app.search.rows().iter().all(|row| row.label != "r/late"),
        "a page for the old query is dropped"
    );

    press(&mut app, crossterm::event::KeyCode::Char('/'));
    crate::keys::paste(&mut app, " next");
    app.switch_tab(BOOKMARKS_TAB);
    app.switch_tab(SEARCH_TAB);
    app.tick_at(Instant::now() + Duration::from_secs(1));
    app.tick_at(Instant::now() + Duration::from_secs(1));
    assert_eq!(
        app.search.results_query(),
        Some(&Query::Search {
            text: "rust next".into(),
            kind: crate::reddit::experience::ListSearch::People,
            time: TimeRange::All,
        }),
        "returning during a debounce must search the edited query"
    );

    app.switch_tab(BOOKMARKS_TAB);
    app.reset_search_loads();
    app.tick_at(Instant::now() + Duration::from_secs(1));
    assert!(
        !app.search.results.loading,
        "pool reset must not restart hidden Search work"
    );
    app.switch_tab(SEARCH_TAB);
    app.tick_at(Instant::now() + Duration::from_secs(1));
    app.tick_at(Instant::now() + Duration::from_secs(1));
    assert!(app.search.results.loading);
    assert_eq!(app.search.text(), "rust next");
}

#[test]
fn clearing_the_search_query_should_drop_a_posts_answer_still_on_its_way() {
    let mut app = app_with(&["vim"]);
    app.open_search(None);
    crate::keys::paste(&mut app, "neovim");
    press(&mut app, crossterm::event::KeyCode::Enter);
    let (tab, generation) = (
        app.tabs[SEARCH_TAB].id,
        app.tabs[SEARCH_TAB].feed.generation,
    );
    assert!(app.tabs[SEARCH_TAB].feed.loading());
    press(&mut app, crossterm::event::KeyCode::Char('/'));

    crate::keys::handle(
        &mut app,
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('u'),
            crossterm::event::KeyModifiers::CONTROL,
        ),
    );
    let _ = app.tick_at(Instant::now() + search::DEBOUNCE);
    app.on_msg(Msg::Feed {
        tab,
        generation,
        append: false,
        result: Ok(listing("vim", &["late"], None)),
    });

    assert!(app.search.exploring());
    assert!(app.tabs[SEARCH_TAB].feed.posts.is_empty());
    assert!(!app.tabs[SEARCH_TAB].feed.loading());

    crate::keys::paste(&mut app, "rust");
    press(&mut app, crossterm::event::KeyCode::Enter);
    assert!(app.tabs[SEARCH_TAB].feed.search.is_some());
    app.tabs[SEARCH_TAB].feed.search_sort = crate::reddit::experience::SearchSort::Top;
    app.on_msg(Msg::Feed {
        tab: app.tabs[SEARCH_TAB].id,
        generation: app.tabs[SEARCH_TAB].feed.generation,
        append: false,
        result: Ok(listing("rust", &["r1"], None)),
    });
    app.resize(240);
    app.open_current_post();
    app.focus_main();
    assert!(app.post_view.is_some());
    press(&mut app, crossterm::event::KeyCode::Tab);
    assert!(
        app.post_view.is_none(),
        "another kind's rows replace the post"
    );
    let _ = app.tick_at(Instant::now());
    assert!(
        app.tabs[SEARCH_TAB].feed.search.is_none() && !app.tabs[SEARCH_TAB].feed.loading(),
        "community rows drop the hidden posts search"
    );
    press(&mut app, crossterm::event::KeyCode::BackTab);
    let _ = app.tick_at(Instant::now());
    let feed = &app.tabs[SEARCH_TAB].feed;
    assert_eq!(
        (feed.search.as_deref(), feed.search_sort),
        (Some("rust"), crate::reddit::experience::SearchSort::Top),
        "back on Posts the same query keeps its sort"
    );
}
