use super::*;

#[test]
fn a_removed_post_stays_out_of_later_listings_and_waiting_island_posts() {
    let (mut app, _dir, _fixture) = app();
    let named = |id: &str| {
        let mut post = post_page().post;
        post.id = id.into();
        post.subreddit = "rust".into();
        post
    };
    app.all_subs.groups.push(Group {
        subreddit: "rust".into(),
        posts: vec![named("kept")],
        loading: false,
        error: None,
        freshness: None,
        generation: 1,
        needs_refresh: false,
        stream: None,
        arrivals: Some(vec![named("gone"), named("kept")]),
        manual: false,
    });
    app.tabs[BOOKMARKS_TAB].feed.selected = 1;
    app.tabs[BOOKMARKS_TAB].feed.offset = 1;
    app.open_search(None);
    app.search_preview_drawn(true);
    let _ = app.tick_at(Instant::now() + crate::app::search::PREVIEW_SETTLE);
    let preview_generation = app.search.preview.generation;
    let preview_listing = || crate::model::Listing {
        posts: vec![named("gone"), named("kept")],
        after: None,
    };
    let preview_answer = |generation, refreshing| Msg::Snapshot {
        message: Box::new(Msg::Feed {
            tab: crate::worker::PREVIEW_FEED,
            generation,
            append: false,
            result: Ok(preview_listing()),
        }),
        fetched_at: 1,
        source: crate::cache::Source::default(),
        refreshing,
    };
    app.on_msg(preview_answer(preview_generation, true));

    app.after_experience_write(&Action::Hide {
        id: "t3_gone".into(),
        hide: true,
    });
    assert!(
        app.search
            .preview
            .posts
            .iter()
            .all(|post| post.id != "gone")
    );
    app.on_msg(preview_answer(preview_generation, false));
    assert!(
        app.search
            .preview
            .posts
            .iter()
            .all(|post| post.id != "gone")
    );
    app.search_preview_drawn(true);
    let _ = app.tick_at(Instant::now() + crate::app::search::PREVIEW_SETTLE);
    app.on_msg(preview_answer(app.search.preview.generation, false));
    assert_eq!(
        app.search
            .preview
            .posts
            .iter()
            .map(|post| post.id.as_str())
            .collect::<Vec<_>>(),
        ["kept"]
    );
    let generation = app.tabs[0].feed.generation;
    app.on_msg(Msg::Feed {
        tab: app.tabs[0].id,
        generation,
        append: false,
        result: Ok(crate::model::Listing {
            posts: vec![named("gone"), named("kept")],
            after: None,
        }),
    });

    let group = &app.all_subs.groups[0];
    assert!(
        group
            .arrivals
            .iter()
            .flatten()
            .all(|post| post.id != "gone")
    );
    assert!(app.tabs[0].feed.posts.iter().all(|post| post.id != "gone"));
    assert_eq!(app.tabs[BOOKMARKS_TAB].feed.selected, 1);
    assert!(!app.all_subs.has_new_posts(&app.config));
}

#[test]
fn a_write_before_r_answers_leaves_island_arrivals_behind_the_notice() {
    let (mut app, _dir, _fixture) = app();
    let named = |id: &str| {
        let mut post = post_page().post;
        post.id = id.into();
        post.subreddit = "rust".into();
        post
    };
    app.all_subs.groups.push(Group {
        subreddit: "rust".into(),
        posts: vec![named("a")],
        loading: false,
        error: None,
        freshness: None,
        generation: 1,
        needs_refresh: true,
        stream: None,
        arrivals: None,
        manual: true,
    });

    app.after_experience_write(&Action::Hide {
        id: "t3_elsewhere".into(),
        hide: true,
    });
    app.on_msg(Msg::Group {
        subreddit: "rust".into(),
        generation: 1,
        result: Ok(crate::model::Listing {
            posts: vec![named("n"), named("a")],
            after: None,
        }),
    });

    assert!(app.all_subs.has_new_posts(&app.config));
}

#[test]
fn hiding_a_post_removes_it_from_the_unread_count() {
    let (mut app, _dir, _fixture) = app();
    let mut post = post_page().post;
    post.id = "hidden".into();
    post.subreddit = "rust".into();
    // Only reading history knows the post: no list, bookmark or open view holds it.
    app.state.observe(
        Some("rust"),
        std::slice::from_ref(&post),
        crate::cache::Freshness {
            fetched_at: crate::app::now(),
            source: Source::default(),
        },
        crate::app::now(),
    );
    assert_eq!(app.state.unread("rust", &app.config), Some(1));

    app.after_experience_write(&Action::Hide {
        id: "t3_hidden".into(),
        hide: true,
    });

    assert_eq!(app.state.unread("rust", &app.config), Some(0));
}

#[test]
fn hiding_a_post_above_the_selection_keeps_the_selected_row_on_screen() {
    let (mut app, _dir, _fixture) = app();
    let posts: Vec<_> = ["a", "b", "c", "d"]
        .into_iter()
        .map(|id| {
            let mut post = post_page().post;
            post.id = id.into();
            post
        })
        .collect();
    app.tabs[0]
        .feed
        .replace_page(None, posts, None, Source::default());
    app.tabs[0].feed.selected = 2;
    app.tabs[0].feed.offset = 1;

    app.after_experience_write(&Action::Hide {
        id: "t3_a".into(),
        hide: true,
    });

    let feed = &app.tabs[0].feed;
    assert_eq!(feed.posts[feed.selected].id, "c");
    assert_eq!(feed.selected - feed.offset, 1);
}

#[test]
fn when_hiding_a_loaded_post_page_lengths_should_remain_valid_for_the_refresh() {
    let (mut app, _dir, _fixture) = app();
    let mut first = post_page().post;
    first.id = "first".into();
    let mut second = first.clone();
    second.id = "second".into();
    app.tabs[0]
        .feed
        .replace_page(None, vec![first, second.clone()], None, Source::default());
    app.after_experience_write(&Action::Hide {
        id: "t3_first".into(),
        hide: true,
    });
    assert_eq!(app.tabs[0].feed.pages[0].len, 1);
    assert!(
        app.tabs[0]
            .feed
            .replace_page(None, vec![second], None, Source::default())
    );
    assert_eq!(app.tabs[0].feed.posts[0].id, "second");
}

#[test]
fn when_a_write_invalidates_an_inactive_feed_its_old_response_should_not_mark_it_loaded() {
    let (mut app, _dir, _fixture) = app();
    let index = app.push_tab(TabKind::Subreddit {
        name: "rust".into(),
        pinned: false,
    });
    let tab = app.tabs[index].id;
    let old = app.tabs[index].feed.generation;
    app.after_experience_write(&Action::Save {
        id: "t3_post".into(),
        save: true,
    });
    app.on_msg(Msg::Feed {
        tab,
        generation: old,
        append: false,
        result: Ok(Listing {
            posts: vec![post_page().post],
            after: None,
        }),
    });
    assert!(!app.tabs[index].feed.loaded);
    assert!(app.tabs[index].feed.posts.is_empty());
}

#[test]
fn when_an_old_read_finishes_after_a_write_only_the_post_write_refresh_should_replace_content() {
    let (mut app, _dir, _fixture) = app();
    let page = post_page();
    let post = page.post.clone();
    let link = post.permalink.clone();
    let old = app.show_post(post, &link).unwrap();
    app.after_experience_write(&Action::Edit {
        target: Target {
            id: "t3_1x4v4sn".into(),
            author: "reader".into(),
            subreddit: "rust".into(),
            permalink: "/comments/1x4v4sn".into(),
        },
        text: "edited body".into(),
    });
    let current = app.post_view.as_ref().unwrap().generation;
    app.on_msg(Msg::Post {
        generation: old,
        result: Ok(Box::new(page.clone())),
    });
    assert_eq!(app.post_view.as_ref().unwrap().generation, current);
    assert!(app.post_view.as_ref().unwrap().page.is_none());
    let mut fresh = page;
    fresh.body = "edited body".into();
    app.on_msg(Msg::Post {
        generation: current,
        result: Ok(Box::new(fresh)),
    });
    assert_eq!(
        app.post_view.as_ref().unwrap().page.as_ref().unwrap().body,
        "edited body"
    );
}

#[test]
fn when_reopening_a_post_after_a_write_pre_write_reads_should_still_be_ignored() {
    let (mut app, _dir, _fixture) = app();
    let page = post_page();
    let post = page.post.clone();
    let link = post.permalink.clone();
    let old = app.show_post(post.clone(), &link).unwrap();
    app.after_experience_write(&Action::Save {
        id: "t3_1x4v4sn".into(),
        save: true,
    });
    let current = app.show_post(post, &link).unwrap();
    app.on_msg(Msg::Post {
        generation: old,
        result: Ok(Box::new(page)),
    });
    assert_eq!(app.post_view.as_ref().unwrap().generation, current);
    assert!(app.post_view.as_ref().unwrap().page.is_none());
}
