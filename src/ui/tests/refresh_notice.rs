use super::*;

#[test]
fn the_notice_offers_r_only_while_r_would_apply_it() {
    let mut app = app(&["rust", "vim"]);
    app.switch_tab(crate::app::ALL_SUBS_TAB);
    let titled = |id: &str, sub: &str| {
        let mut post = post_with(id, Media::None);
        post.subreddit = sub.into();
        post.title = format!("Title {id}");
        post
    };
    let answer = |app: &App, sub: &str, ids: &[&str]| Msg::Group {
        generation: app
            .all_subs
            .groups
            .iter()
            .find(|group| group.subreddit == sub)
            .unwrap()
            .generation,
        subreddit: sub.into(),
        result: Ok(crate::model::Listing {
            posts: ids.iter().map(|id| titled(id, sub)).collect(),
            after: None,
        }),
    };
    app.on_msg(answer(&app, "rust", &["r1", "r2"]));
    app.on_msg(answer(&app, "vim", &["v1"]));
    app.on_msg(answer(&app, "rust", &["fresh", "r1", "r2"]));
    screen(&mut app, 200, 40);

    press_keys(&mut app, "l");
    assert!(app.post_focused());
    let reading = screen(&mut app, 200, 40);

    assert!(reading.contains("New posts"), "{reading}");
    assert!(!reading.contains("r refresh"), "{reading}");
    app.focus = Focus::List;
    assert!(screen(&mut app, 200, 40).contains("New posts · r refresh"));
}

#[test]
fn island_arrivals_wait_behind_the_notice_and_r_shows_them_from_the_top() {
    let mut app = app(&["rust", "vim"]);
    app.switch_tab(crate::app::ALL_SUBS_TAB);
    let titled = |id: &str, sub: &str| {
        let mut post = post_with(id, Media::None);
        post.subreddit = sub.into();
        post.title = format!("Title {id}");
        post
    };
    let answer = |app: &App, sub: &str, posts: Vec<Post>| Msg::Group {
        generation: app
            .all_subs
            .groups
            .iter()
            .find(|group| group.subreddit == sub)
            .unwrap()
            .generation,
        subreddit: sub.into(),
        result: Ok(crate::model::Listing { posts, after: None }),
    };
    let rust = vec![titled("r1", "rust"), titled("r2", "rust")];
    app.on_msg(answer(&app, "rust", rust.clone()));
    app.on_msg(answer(&app, "vim", vec![titled("v1", "vim")]));
    screen(&mut app, 120, 40);
    app.move_selection(2);
    assert_eq!(app.current_post().unwrap().id, "v1");

    let mut refreshed = vec![titled("fresh", "rust")];
    refreshed.extend(rust);
    refreshed[1].score = Some(99);
    app.on_msg(answer(&app, "rust", refreshed));
    let pending = screen(&mut app, 120, 40);

    assert!(pending.contains("New posts · r refresh"), "{pending}");
    assert!(!pending.contains("Title fresh"), "{pending}");
    assert!(pending.contains("99"), "{pending}");
    assert_eq!(app.current_post().unwrap().id, "v1");

    press_keys(&mut app, "r");
    let applied = screen(&mut app, 120, 40);

    assert!(!applied.contains("r refresh"), "{applied}");
    assert!(applied.contains("Title fresh"), "{applied}");
    assert_eq!(app.current_post().unwrap().id, "fresh");
}

#[test]
fn post_notice_preview_should_announce_only_new_leading_posts() {
    for (ids, new_posts) in [
        (vec!["a", "b"], false),
        (vec!["a", "tail", "b"], false),
        (vec!["b", "a"], false),
        (vec!["leader", "b", "a"], true),
    ] {
        let mut app = app(&[]);
        app.open_search(None);
        app.search.discovered.fetch = Default::default();
        app.search.preview.target = Some(app.search.selected_row().unwrap().target);
        app.search.preview.posts = vec![post_with("a", Media::None), post_with("b", Media::None)];
        app.search.preview.update = Some(
            ids.iter()
                .map(|id| {
                    let mut post = post_with(id, Media::None);
                    post.score = Some(123);
                    post.comments = 42;
                    post
                })
                .collect(),
        );
        let shown = screen(&mut app, 200, 30);
        assert_eq!(shown.contains("New posts"), new_posts, "{ids:?}: {shown}");
    }
}

#[test]
fn post_notice_preview_status_should_belong_to_the_selected_source() {
    let mut app = app(&[]);
    app.open_search(None);
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    app.search.discovered.fetch = Default::default();
    app.search.preview.target = Some(app.search.selected_row().unwrap().target);
    app.search.preview.posts = vec![post_with("a", Media::None)];
    app.search.preview.update = Some(vec![post_with("leader", Media::None)]);
    app.search.preview.error = Some("previous source failed".into());
    let current = screen(&mut app, 200, 30);
    assert!(
        current.contains("New posts") && current.contains("refresh failed"),
        "{current}"
    );

    press(&mut app, 'j');
    let next = screen(&mut app, 200, 30);
    assert!(
        next.contains("r/all") && next.contains("Loading preview"),
        "{next}"
    );
    assert!(
        !next.contains("New posts") && !next.contains("refresh failed"),
        "{next}"
    );
}
