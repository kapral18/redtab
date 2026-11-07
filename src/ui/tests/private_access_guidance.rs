use super::*;

#[test]
fn private_community_help_should_prioritize_access_actions() {
    let mut app = denied_community(AccessDenied::PrivateCommunity);
    press(&mut app, '?');
    let actual = screen(&mut app, 100, 14);
    assert!(actual.contains("Feed"), "{actual}");
    assert!(
        actual.contains("Open the current item on Reddit"),
        "{actual}"
    );
    assert!(actual.contains("Reload / retry"), "{actual}");
}

#[test]
fn private_community_cached_feed_should_hide_unavailable_posts_and_keep_recovery_visible() {
    for (width, height) in [(24, 10), (40, 12), (100, 24)] {
        let mut app = denied_community(crate::reddit::AccessDenied::PrivateCommunity);
        app.show_sidebar = false;
        let actual = screen(&mut app, width, height);
        let words = actual.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(words.contains("Private community"), "{actual}");
        assert!(
            words.contains("o Reddit") || words.contains("o to open Reddit"),
            "{actual}"
        );
        assert!(
            !actual.contains("failed") && !actual.contains("retry"),
            "{actual}"
        );
        assert!(!app.active_tab().feed.posts.is_empty(), "cache is retained");
        assert!(
            app.current_post().is_none(),
            "hidden cached posts cannot be acted on"
        );
    }
}

#[test]
fn private_community_home_denial_should_keep_the_generic_home_failure() {
    let mut app = app(&[]);
    app.switch_tab(0);
    app.on_msg(Msg::Feed {
        tab: app.active_tab().id,
        generation: app.active_tab().feed.generation,
        append: false,
        result: Err(crate::worker::FeedError::Access(AccessDenied::Forbidden)),
    });

    assert!(app.status.as_ref().is_some_and(|status| status.error));
    let actual = screen(&mut app, 100, 24);
    assert!(actual.contains("Could not load posts"), "{actual}");
    assert!(!actual.contains("Private community"), "{actual}");
}

#[test]
fn private_community_island_should_show_approval_guidance_with_cached_posts() {
    let mut app = app(&["doit"]);
    app.all_subs.groups[0].posts = (0..3)
        .map(|i| post_with(&format!("cached{i}"), Media::None))
        .collect();
    app.all_subs.post = 2;
    app.on_msg(Msg::Group {
        subreddit: "doit".into(),
        generation: app.all_subs.groups[0].generation,
        result: Err(crate::worker::FeedError::Access(
            AccessDenied::PrivateCommunity,
        )),
    });
    let actual = screen(&mut app, 100, 24);
    assert!(
        actual.contains("Private community") && actual.contains("Approved members only"),
        "{actual}"
    );
    assert!(
        !actual.contains("r retry") && !actual.contains("cached"),
        "{actual}"
    );
    assert!(app.current_post().is_none());
}
