use super::*;

#[test]
fn a_rate_limit_shows_paused_requests_not_an_unanswering_reddit() {
    use std::io::{BufRead, BufReader, Write};
    let listener = crate::reddit::tests::local_listener();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                line.clear();
            }
            let _ = write!(
                stream,
                "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 60\r\n\
                     Content-Length: 0\r\nConnection: close\r\n\r\n"
            );
        }
    });
    let (mut app, messages) = app_at(&url, &[]);
    app.status = None;
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(110, 30)).unwrap();
    let mut footer = |app: &mut App| {
        let buffer = terminal
            .draw(|frame| crate::ui::draw(frame, app))
            .unwrap()
            .buffer
            .clone();
        (0..110)
            .map(|x| buffer[(x, 29)].symbol())
            .collect::<String>()
    };

    assert!(app.reddit.search("rust", "x", None).is_err());
    let started = Instant::now();
    let mut shown = footer(&mut app);
    while !shown.contains("paused")
        && !shown.contains("not answering")
        && started.elapsed() < Duration::from_secs(5)
    {
        if let Ok(msg) = messages.recv_timeout(Duration::from_millis(20)) {
            app.on_msg(msg);
        }
        app.tick();
        shown = footer(&mut app);
    }

    assert!(shown.contains("paused"), "{shown}");
}

#[test]
fn when_a_cooldown_counts_down_tick_should_redraw_only_when_the_display_changes() {
    use crate::reddit::tests::{Fixture, Reply};
    let fixture = Fixture::new(|_| Reply {
        status: 429,
        body: String::new(),
        headers: vec![("Retry-After".into(), "2".into())],
    });
    let mut app = app_with(&[]);
    app.reddit = fixture.reddit();
    app.status = None;
    assert!(app.reddit.search("rust", "SECRET", None).is_err());
    assert!(app.tick());
    assert!(!app.tick(), "unchanged countdown must not spin redraws");
    std::thread::sleep(Duration::from_millis(1100));
    assert!(app.tick());
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(110, 20)).unwrap();
    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    let buffer = terminal.backend().buffer();
    let footer: String = (0..110).map(|x| buffer[(x, 19)].symbol()).collect();
    assert!(
        footer.contains("paused 1s")
            && footer.contains("r/rust listing")
            && footer.contains("HTTP 429"),
        "{footer}"
    );
    assert!(!footer.contains("SECRET"));
    app.error("r/rust: Reddit rate limit reached; wait before retrying");
    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    let buffer = terminal.backend().buffer();
    let footer: String = (0..110).map(|x| buffer[(x, 19)].symbol()).collect();
    assert!(
        footer.contains("paused 1s") && footer.contains("HTTP 429"),
        "{footer}"
    );
}

#[test]
fn when_reconnecting_it_should_not_replay_completed_subreddit_pages() {
    use crate::reddit::tests::{Fixture, Reply};
    let fixture = Fixture::new(|_| {
        Reply::json(serde_json::json!({"data":{"id":"sameaccount", "name":"reader"}}))
    });
    let (mut app, _messages) = app_at(&fixture.base, &[]);
    let directory = tempfile::tempdir().unwrap();
    app.state_path = directory.path().join("state.json");
    app.reddit.verify_account().unwrap();
    app.open_subreddit("rust");
    let index = app.active;
    app.on_msg(Msg::Feed {
        tab: app.tabs[index].id,
        generation: app.tabs[index].feed.generation,
        append: false,
        result: Ok(listing("rust", &["first"], Some("completed-cursor"))),
    });
    app.load_feed(index, true);
    app.on_msg(Msg::Feed {
        tab: app.tabs[index].id,
        generation: app.tabs[index].feed.generation,
        append: true,
        result: Ok(listing("rust", &["second"], None)),
    });
    assert_eq!(app.tabs[index].feed.requests.len(), 2);
    assert!(!app.tabs[index].feed.loading());
    let connection = fixture.reddit();
    connection.verify_account().unwrap();
    app.install_connection(connection);
    assert!(
        !app.tabs[index].feed.loading(),
        "Completed pages must stay idle"
    );
    assert!(app.tabs[index].feed.requests.is_empty());
    assert_eq!(app.tabs[index].feed.posts.len(), 2);
}

#[test]
fn when_reconnecting_it_should_resume_interrupted_search_pages_and_islands() {
    use crate::reddit::tests::{Fixture, Reply};
    let identity = serde_json::json!({"data":{"id":"sameaccount", "name":"reader"}});
    let old_identity = identity.clone();
    let old = Fixture::new(move |_| Reply::json(old_identity.clone()));
    let requested = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let requests = requested.clone();
    let new = Fixture::new(move |request| {
        requests.lock().unwrap().push(request.url.clone());
        if request.url == "/api/me.json" {
            Reply::json(identity.clone())
        } else {
            Reply::json(serde_json::json!({"data":{"children":[], "after":null}}))
        }
    });
    let (mut app, _messages) = app_at(&old.base, &[]);
    let directory = tempfile::tempdir().unwrap();
    app.state_path = directory.path().join("state.json");
    app.reddit.verify_account().unwrap();
    app.open_subreddit("rust");
    let search = app.active;
    app.tabs[search].feed.search = Some("needle".into());
    app.fetch_feed(search, None, false);
    app.open_subreddit("python");
    let paged = app.active;
    app.on_msg(Msg::Feed {
        tab: app.tabs[paged].id,
        generation: app.tabs[paged].feed.generation,
        append: false,
        result: Ok(listing("python", &["previous"], Some("next-page"))),
    });
    app.fetch_feed(paged, Some("next-page".into()), false);
    app.active = search;
    app.all_subs.groups = vec![
        Group {
            subreddit: "unfinished".into(),
            loading: true,
            ..Group::default()
        },
        Group {
            subreddit: "failed".into(),
            error: Some("Sign in".into()),
            ..Group::default()
        },
    ];
    let connection = new.reddit();
    connection.verify_account().unwrap();
    app.install_connection(connection);
    let messages = app.take_replacement_messages().unwrap();

    assert!(app.tabs[search].feed.loading());
    assert!(app.tabs[paged].feed.loading_more());
    assert!(
        app.all_subs
            .groups
            .iter()
            .all(|group| !group.loading && group.needs_refresh)
    );
    for index in 0..app.all_subs.groups.len() {
        app.ensure_group_loaded(index);
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        while let Ok(message) = messages.try_recv() {
            app.on_msg(message);
        }
        let urls = requested.lock().unwrap().clone();
        if urls
            .iter()
            .any(|url| url.contains("/r/rust/search?") && url.contains("needle"))
            && urls.iter().any(|url| url.contains("/r/unfinished/"))
            && urls.iter().any(|url| url.contains("/r/failed/"))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Replacement requests missing: {urls:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !requested
            .lock()
            .unwrap()
            .iter()
            .any(|url| url.contains("/r/python/")),
        "inactive pagination must stay deferred after reconnect"
    );
    app.feed_settle.last_move = None;
    app.switch_tab(paged);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        while let Ok(message) = messages.try_recv() {
            app.on_msg(message);
        }
        if requested
            .lock()
            .unwrap()
            .iter()
            .any(|url| url.contains("/r/python/") && url.contains("next-page"))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "return must resume the deferred pagination cursor"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn search_revision_connection_reset_should_end_the_retained_parent_pane() {
    let mut app = app_with(&[]);
    app.open_search(None);
    app.search.discovered.entries.push(search::Row {
        profile: None,
        label: "r/rust".into(),
        detail: "Parent communities".into(),
        target: FeedTarget::Subreddit("rust".into()),
    });
    app.search.selected = 2;
    app.open_search_row();
    assert!(app.search_feed_open());
    app.reset_search_loads();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(160, 24)).unwrap();
    terminal
        .draw(|frame| crate::ui::draw(frame, &mut app))
        .unwrap();
    let actual: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(
        !actual.contains("Explore"),
        "a cleared Search origin must not leave unrelated parent rows: {actual}"
    );
}
