use super::*;

#[test]
fn when_no_account_is_connected_start_should_not_queue_feed_or_subscription_loads() {
    let reddit = Reddit::disconnected(false).unwrap();
    let (pool, _messages) = Pool::start(reddit.clone(), 1, None);
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::new(
        Config::default(),
        pool,
        reddit,
        directory.path().join("state.json"),
        Picker::halfblocks(),
    );
    app.start();
    assert!(!app.tabs[0].feed.loading());
    assert!(!app.all_subs.loading);
    assert_eq!(app.reddit.health(), Health::SignedOut);
}

#[test]
fn when_startup_has_a_slow_quota_home_and_first_navigation_should_still_load() {
    use crate::reddit::tests::{Fixture, Reply};
    let fixture = Fixture::new(|request| {
        let path = request.url.split('?').next().unwrap();
        let body = match path {
            "/subreddits/mine/subscriber.json" => {
                r#"{"kind":"Listing","data":{"children":[{"kind":"t5","data":{"display_name":"rust"}}],"after":null}}"#
            }
            "/svc/shreddit/feeds/home-feed" => {
                "<shreddit-post id='t3_1x4v4sn' post-title='native' subreddit-name='rust' author='reader' permalink='/r/rust/comments/1x4v4sn/title/' post-type='text'></shreddit-post>"
            }
            "/r/rust/hot" => {
                include_str!("../../../tests/fixtures/sub.json")
            }
            "/r/rust/about.json" => r#"{"data":{}}"#,
            path if path.contains("/comments/") => {
                include_str!("../../../tests/fixtures/post.json")
            }
            _ => panic!("unexpected API path: {path}"),
        };
        Reply {
            status: 200,
            body: body.into(),
            headers: vec![
                ("X-Ratelimit-Remaining".into(), "10".into()),
                ("X-Ratelimit-Reset".into(), "30".into()),
            ],
        }
    });
    let reddit = fixture.reddit();
    reddit.subscriptions().unwrap();
    let (mut app, messages) = app_with_client(reddit.clone(), &["rust"]);
    app.active = 0;
    app.start();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !app.tabs[0].feed.loaded {
        app.on_msg(
            messages
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("Home should hydrate before the three-second pacing slot"),
        );
    }
    let rust = tab_index(&app, "rust");
    app.switch_tab(rust);
    let deadline = Instant::now() + Duration::from_secs(2);
    while !app.tabs[rust].feed.loaded {
        app.on_msg(
            messages
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("first subreddit should load without waiting for background subscriptions"),
        );
    }
    app.open_current_post();
    let deadline = Instant::now() + Duration::from_secs(2);
    while app
        .post_view
        .as_ref()
        .is_none_or(|view| view.page.is_none())
    {
        app.on_msg(
            messages
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("selected post should use the remaining interactive capacity"),
        );
    }
    reddit.retire();

    assert_eq!(app.active_tab().subreddit(), Some("rust"));
    assert!(!app.tabs[rust].feed.posts.is_empty());
    assert!(app.post_view.as_ref().unwrap().error.is_none());
}

#[test]
fn when_the_app_starts_it_should_load_home_and_account_subscriptions_in_background() {
    let server = crate::reddit::tests::local_http_server();
    let base = format!("http://{}", server.server_addr());
    let handler = std::thread::spawn(move || {
        let mut completed = 0;
        while completed < 2 {
            let request = server
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap();
            let path = request.url().split('?').next().unwrap();
            let body = match path {
                "/svc/shreddit/feeds/home-feed" => {
                    "<shreddit-post id='t3_1x4v4sn' post-title='native' subreddit-name='rust' author='reader' permalink='/r/rust/comments/1x4v4sn/title/' post-type='text'></shreddit-post>"
                }
                "/r/rust/hot" => {
                    include_str!("../../../tests/fixtures/sub.json")
                }
                "/subreddits/mine/subscriber.json" => {
                    r#"{"kind":"Listing","data":{"children":[{"kind":"t5","data":{"display_name":"rust"}}],"after":null}}"#
                }
                _ => panic!("unexpected API path: {path}"),
            };
            request
                .respond(crate::reddit::tests::served::Response::from_string(body))
                .unwrap();
            completed += 1;
        }
    });
    let (mut app, messages) = app_at(&base, &[]);
    app.active = 0;
    app.start();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !app.tabs[0].feed.loaded || app.all_subs.groups.first().is_none_or(|g| g.loading) {
        assert!(
            Instant::now() < deadline,
            "Home loaded={} error={:?} groups={:?}",
            app.tabs[0].feed.loaded,
            app.tabs[0].feed.error(),
            app.all_subs
                .groups
                .iter()
                .map(|g| (&g.subreddit, g.loading))
                .collect::<Vec<_>>()
        );
        app.on_msg(
            messages
                .recv_timeout(Duration::from_secs(5))
                .expect("startup results"),
        );
    }
    handler.join().unwrap();

    assert_eq!(app.active_tab().title(), "Home");
    assert!(!app.tabs[0].feed.posts.is_empty());
    assert_eq!(app.all_subs.groups[0].subreddit, "rust");
    assert!(
        app.tabs.iter().any(
            |t| matches!(&t.kind, TabKind::Subreddit { name, pinned: true } if name == "rust")
        )
    );
}
