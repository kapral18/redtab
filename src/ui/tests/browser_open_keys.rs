use super::*;

#[cfg(unix)]
#[test]
fn private_community_browser_key_should_open_the_community_without_a_selected_post() {
    let mut app = denied_community(AccessDenied::PrivateCommunity);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("opened-url");
    app.config.commands.browser = vec![
        "/bin/sh".into(),
        "-c".into(),
        "printf '%s' \"$1\" > \"$0\"".into(),
        file.to_string_lossy().into_owned(),
    ];
    press_keys(&mut app, "o");
    let deadline = Instant::now() + std::time::Duration::from_secs(2);
    let actual = loop {
        if let Ok(text) = std::fs::read_to_string(&file)
            && !text.is_empty()
        {
            break text;
        }
        assert!(
            Instant::now() < deadline,
            "native browser probe did not receive a URL"
        );
        std::thread::yield_now();
    };
    assert_eq!(actual, "https://www.reddit.com/r/s/");
}

#[cfg(unix)]
#[test]
fn empty_community_browser_key_should_open_the_community_without_a_post() {
    let mut app = app(&["s"]);
    app.active = 4;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("opened-url");
    app.config.commands.browser = vec![
        "/bin/sh".into(),
        "-c".into(),
        "printf '%s' \"$1\" > \"$0\"".into(),
        file.to_string_lossy().into_owned(),
    ];
    press_keys(&mut app, "o");
    let deadline = Instant::now() + std::time::Duration::from_secs(2);
    let actual = loop {
        if let Ok(text) = std::fs::read_to_string(&file)
            && !text.is_empty()
        {
            break text;
        }
        assert!(
            Instant::now() < deadline,
            "native browser probe did not receive a URL"
        );
        std::thread::yield_now();
    };
    assert_eq!(actual, "https://www.reddit.com/r/s/");
}

#[cfg(unix)]
#[test]
fn sidebar_browser_key_should_open_its_community_instead_of_its_selected_post() {
    let mut app = feed_with_next_page(ListView::Card);
    app.focus = Focus::Sidebar;

    let actual = opened_browser_url(&mut app, " oc");

    assert_eq!(actual, "https://www.reddit.com/r/s/");
}

#[cfg(unix)]
#[test]
fn community_browser_key_on_home_should_open_the_selected_posts_community() {
    let mut app = app(&[]);
    app.active = 0;
    let mut post = post_with("p", Media::None);
    post.subreddit = "fromhome".into();
    app.tabs[0].feed.posts = vec![post];

    let actual = opened_browser_url(&mut app, " oc");

    assert_eq!(actual, "https://www.reddit.com/r/fromhome/");
}

#[cfg(unix)]
#[test]
fn community_browser_key_in_detail_should_open_the_shown_posts_community() {
    let mut app = app(&["underlying"]);
    app.active = 4;
    let mut post = post_with("p", Media::None);
    post.subreddit = "detail".into();
    app.tabs[4].feed.posts = vec![post];
    app.open_current_post();
    app.focus = Focus::Post;

    let actual = opened_browser_url(&mut app, " oc");

    assert_eq!(actual, "https://www.reddit.com/r/detail/");
}

#[cfg(unix)]
#[test]
fn lowercase_browser_key_in_a_list_should_keep_opening_the_selected_post() {
    let mut app = feed_with_next_page(ListView::Card);
    app.tabs[4].feed.posts[0].permalink = "/r/s/comments/existing/title/".into();
    app.focus = Focus::List;

    let actual = opened_browser_url(&mut app, "o");

    assert_eq!(
        actual,
        "https://www.reddit.com/r/s/comments/existing/title/"
    );
}

#[test]
fn followed_user_browser_keys_should_target_the_user_profile_even_when_empty() {
    let mut app = app(&[]);
    app.open_user("blackvault");
    app.focus = Focus::Sidebar;
    assert_eq!(
        opened_browser_url(&mut app, "o"),
        "https://www.reddit.com/user/blackvault/"
    );
    app.focus = Focus::List;
    assert_eq!(
        opened_browser_url(&mut app, " oc"),
        "https://www.reddit.com/user/blackvault/"
    );
    assert_eq!(
        opened_browser_url(&mut app, "o"),
        "https://www.reddit.com/user/blackvault/"
    );
}

#[cfg(unix)]
#[test]
fn key_context_audit_browser_should_follow_the_item_and_keep_the_parent_shortcut() {
    use crate::app::experience::Screen;
    use crate::keys::map::{self, Action};
    use crate::reddit::experience::{Entry, Page, Query, Target};
    let mut app = app(&[]);
    let post = post_with("selected", Media::None);
    let item_url = post.reddit_url();
    app.overlay = Some(Overlay::Experience);
    app.experience.screens = vec![Screen::Browse {
        query: Query::Profile("reader".into()),
        page: Page {
            title: "Profile".into(),
            text: String::new(),
            entries: vec![Entry {
                icon: Default::default(),
                title: "Selected".into(),
                body: String::new(),
                query: None,
                target: Some(Target::from_post(&post)),
                post: Some(post),
            }],
            after: None,
            url: "/user/reader/".into(),
        },
        selected: 0,
        scroll: 0,
        pagination: Default::default(),
    }];
    assert_eq!(opened_browser_url(&mut app, "o"), item_url);
    assert_eq!(
        opened_browser_url(&mut app, "\u{f}"),
        "https://www.reddit.com/user/reader/"
    );
    app.overlay = None;
    for name in ["popular", "all"] {
        app.open_subreddit(name);
        assert!(map::available(&app, Action::SourceBrowser));
        assert_eq!(
            opened_browser_url(&mut app, "o"),
            format!("https://www.reddit.com/r/{name}/")
        );
        app.focus_sidebar();
        assert_eq!(
            opened_browser_url(&mut app, "o"),
            format!("https://www.reddit.com/r/{name}/")
        );
        app.focus_main();
    }
}
