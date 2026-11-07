use super::*;

pub(super) fn assert_feed_arrives_before_tail(home: bool, append: bool) {
    use std::io::Write;
    use std::sync::{Arc, Mutex, mpsc::channel};
    let server = crate::reddit::tests::local_http_server();
    let base = format!("http://{}", server.server_addr());
    let (release, tail) = channel();
    let (sent, prefix) = channel();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let log = requests.clone();
    let handler = std::thread::spawn(move || {
        loop {
            let request = server
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap();
            let path = request.url().to_string();
            log.lock().unwrap().push(path.clone());
            if home && path.contains("cursor=next") {
                request
                    .respond(crate::reddit::tests::served::Response::from_string(
                        ["d", "e"]
                            .map(|id| crate::reddit::tests::native_post(id, "s"))
                            .join(""),
                    ))
                    .unwrap();
                break;
            }
            if path.contains("about.json") {
                request
                    .respond(crate::reddit::tests::served::Response::from_string(
                        r#"{"data":{}}"#,
                    ))
                    .unwrap();
                continue;
            }
            let child = |id: &str| {
                serde_json::json!({"kind":"t3","data":{
                    "id":id,"title":id,"subreddit":"s","permalink":format!("/r/s/comments/{id}/title/"),
                    "spoiler":true,"url_overridden_by_dest":"https://i.redd.it/picture.png","post_hint":"image"
                }}).to_string()
            };
            let first = format!(
                "{{\"kind\":\"Listing\",\"data\":{{\"children\":[{},",
                child("a")
            );
            let rest = format!(
                "{}],\"after\":null}}}}",
                ["b", "c", "d", "e"].map(child).join(",")
            );
            let (first, rest) = if home {
                let first = crate::reddit::tests::native_post("a", "s").replace(
                    " post-type='text'",
                    " spoiler post-type='image' content-href='https://i.redd.it/picture.png'",
                );
                let rest = format!(
                    "{}<faceplate-partial id='feed-next-page-partial' src='/svc/shreddit/feeds/home-feed?cursor=next'></faceplate-partial>",
                    ["b", "c"]
                        .map(|id| crate::reddit::tests::native_post(id, "s"))
                        .join("")
                );
                (first, rest)
            } else {
                (first, rest)
            };
            let mut writer = request.stream(Some(first.len() + rest.len()));
            writer.write_all(first.as_bytes()).unwrap();
            sent.send(()).unwrap();
            let _ = tail.recv_timeout(Duration::from_secs(3));
            writer.write_all(rest.as_bytes()).unwrap();
            writer.flush().unwrap();
            if !home {
                break;
            }
        }
    });
    let (mut app, messages) = app_at(&base, &["s"]);
    let index = if home { 0 } else { tab_index(&app, "s") };
    app.active = index;
    if append {
        app.on_msg(Msg::Feed {
            tab: app.tabs[index].id,
            generation: 0,
            append: false,
            result: Ok(listing("s", &["head"], Some("next"))),
        });
    }
    app.load_feed(index, append);
    prefix.recv_timeout(Duration::from_secs(3)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(1);
    while !app.tabs[index].feed.posts.iter().any(|p| p.id == "a") && Instant::now() < deadline {
        if let Ok(message) = messages.recv_timeout(Duration::from_millis(25)) {
            app.on_msg(message);
        }
    }
    let appeared = app.tabs[index].feed.posts.iter().any(|p| p.id == "a");
    let pending = app.tabs[index].feed.loading();
    release.send(()).unwrap();
    handler.join().unwrap();

    assert!(appeared, "usable post was hidden until the response tail");
    assert!(pending, "a partial response must not finish pagination");
    let first = app.tabs[index]
        .feed
        .posts
        .iter()
        .find(|p| p.id == "a")
        .unwrap();
    assert!(first.spoiler && matches!(first.media, Media::Image { .. }));
    let deadline = Instant::now() + Duration::from_secs(2);
    while app.tabs[index].feed.loading() {
        assert!(Instant::now() < deadline);
        app.on_msg(messages.recv_timeout(Duration::from_secs(1)).unwrap());
    }
    let expected = if append {
        vec!["head", "a", "b", "c", "d", "e"]
    } else {
        vec!["a", "b", "c", "d", "e"]
    };
    assert_eq!(
        app.tabs[index]
            .feed
            .posts
            .iter()
            .map(|p| p.id.as_str())
            .collect::<Vec<_>>(),
        expected
    );
    if home {
        let requests = requests.lock().unwrap();
        assert!(
            requests
                .iter()
                .all(|p| p.starts_with("/svc/shreddit/feeds/home-feed")),
            "unexpected lookup: {requests:?}"
        );
        assert_eq!(requests.len(), 2);
    }
}

/// A fixture Reddit API that answers each post page `delay` late, as `post.json`
/// retitled with the requested id, and records the post ids asked for. Other paths
/// answer at once, empty.
pub(super) struct SlowReddit {
    pub(super) url: String,
    pub(super) asked: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl SlowReddit {
    pub(super) fn start(delay: Duration) -> Self {
        let server = crate::reddit::tests::local_http_server();
        let url = format!("http://{}", server.server_addr());
        let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = std::sync::Arc::clone(&asked);
        std::thread::spawn(move || {
            while let Ok(request) = server.recv() {
                let log = std::sync::Arc::clone(&log);
                std::thread::spawn(move || {
                    let path = request.url().to_owned();
                    let body = match path
                        .split('/')
                        .nth(2)
                        .filter(|_| path.starts_with("/comments/"))
                        .map(|id| id.split('.').next().unwrap())
                    {
                        Some(id) => {
                            log.lock().unwrap().push(id.to_string());
                            std::thread::sleep(delay);
                            include_str!("../../../tests/fixtures/post.json").replace("1x4v4sn", id)
                        }
                        None => String::new(),
                    };
                    let _ =
                        request.respond(crate::reddit::tests::served::Response::from_string(body));
                });
            }
        });
        Self { url, asked }
    }

    /// Home with one card per post id, the first post open beside the cards and the
    /// cards focused. No subscriptions: nothing else asks this server for pages.
    pub(super) fn home(&self, ids: &[&str]) -> (App, std::sync::mpsc::Receiver<Msg>) {
        self.home_with_client(ids, Reddit::test_at(&self.url, false))
    }

    pub(super) fn home_with_client(
        &self,
        ids: &[&str],
        reddit: Reddit,
    ) -> (App, std::sync::mpsc::Receiver<Msg>) {
        let (mut app, messages) = app_with_client(reddit, &[]);
        app.all_subs.groups = ids
            .iter()
            .enumerate()
            .map(|(i, id)| Group {
                subreddit: format!("s{i}"),
                posts: vec![post(id, &format!("s{i}"))],
                ..Group::default()
            })
            .collect();
        app.resize(200);
        app.open_current_post();
        app.focus_main();
        (app, messages)
    }

    pub(super) fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
}

/// Handle replies and timers until the shown post's page is in; how long that took.
pub(super) fn wait_for_page(app: &mut App, messages: &std::sync::mpsc::Receiver<Msg>) -> Duration {
    let started = Instant::now();
    while app.post_view.as_ref().is_some_and(|v| v.page.is_none())
        && started.elapsed() < Duration::from_secs(10)
    {
        if let Ok(msg) = messages.recv_timeout(Duration::from_millis(20)) {
            app.on_msg(msg);
        }
        app.tick();
    }
    started.elapsed()
}
