use super::*;

#[test]
fn empty_nsfw_disabled_feed_should_explain_how_to_show_nsfw() {
    for (width, height) in [(40, 12), (24, 8)] {
        let mut app = feed_app(Picker::halfblocks());
        app.config.show_nsfw = false;
        app.show_sidebar = false;
        app.reload();
        app.on_msg(Msg::Feed {
            tab: app.active_tab().id,
            generation: app.active_tab().feed.generation,
            append: false,
            result: Ok(crate::model::Listing {
                posts: vec![],
                after: None,
            }),
        });

        let actual = screen(&mut app, width, height);

        assert!(actual.contains("No visible posts"), "{actual}");
        let words = actual.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(words.contains("NSFW is off"), "{actual}");
        let compact: String = actual.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(
            compact.contains("F2") && compact.contains("ShowNSFW"),
            "{actual}"
        );
        assert!(!words.contains("restart"), "{actual}");
        assert!(!actual.contains("No posts here"), "{actual}");
    }
}

#[test]
fn empty_nsfw_enabled_feed_should_keep_the_normal_empty_message() {
    let mut app = feed_app(Picker::halfblocks());
    app.config.show_nsfw = true;
    app.reload();
    app.on_msg(Msg::Feed {
        tab: app.active_tab().id,
        generation: app.active_tab().feed.generation,
        append: false,
        result: Ok(crate::model::Listing {
            posts: vec![],
            after: None,
        }),
    });

    let actual = screen(&mut app, 100, 20);

    assert!(actual.contains("No posts here"), "{actual}");
    assert!(!actual.contains("show_nsfw"), "{actual}");
}

#[test]
fn empty_nsfw_disabled_search_should_explain_the_setting() {
    let mut app = feed_app(Picker::halfblocks());
    app.config.show_nsfw = false;
    app.search("something".into());
    app.on_msg(Msg::Feed {
        tab: app.active_tab().id,
        generation: app.active_tab().feed.generation,
        append: false,
        result: Ok(crate::model::Listing {
            posts: vec![],
            after: None,
        }),
    });

    let actual = screen(&mut app, 100, 20);

    assert!(actual.contains("No results"), "{actual}");
    assert!(
        actual.contains("F2") && actual.contains("Show NSFW"),
        "{actual}"
    );
    assert!(!actual.contains("restart"), "{actual}");
}

#[test]
fn empty_nsfw_disabled_island_should_suggest_the_existing_setting() {
    for width in [40, 100] {
        let mut app = app(&["empty"]);
        app.config.show_nsfw = false;
        app.show_sidebar = false;
        app.on_msg(Msg::Group {
            subreddit: "empty".into(),
            generation: app.all_subs.groups[0].generation,
            result: Ok(crate::model::Listing {
                posts: vec![],
                after: None,
            }),
        });

        let actual = screen(&mut app, width, 20);

        assert!(actual.contains("NSFW off"), "{actual}");
        assert!(
            actual.contains("F2") && actual.contains("Show NSFW"),
            "{actual}"
        );
        assert!(!actual.contains("restart"), "{actual}");
    }
}

#[test]
fn empty_nsfw_disabled_content_tools_should_fit_the_setting_guidance() {
    use crate::app::experience::Screen;
    use crate::reddit::experience::{Page, Query};
    let mut app = app(&[]);
    app.config.show_nsfw = false;
    app.overlay = Some(Overlay::Experience);
    app.experience.screens = vec![Screen::Browse {
        pagination: Default::default(),
        query: Query::Communities,
        page: Page {
            title: "Empty collection".into(),
            text: String::new(),
            entries: vec![],
            after: None,
            url: String::new(),
        },
        selected: 0,
        scroll: 0,
    }];

    let actual = screen(&mut app, 60, 16);

    assert!(actual.contains("NSFW is off"), "{actual}");
    assert!(
        actual.contains("F2") && actual.contains("Show NSFW"),
        "{actual}"
    );
    assert!(!actual.contains("restart"), "{actual}");
}

#[test]
fn empty_nsfw_guidance_should_preserve_notifications_and_supplied_text() {
    use crate::app::experience::Screen;
    use crate::reddit::experience::{Inbox, Page, Query};
    for (show_nsfw, query, text, expected) in [
        (false, Query::Inbox(Inbox::Unread), "", "No items."),
        (true, Query::Communities, "", "No items."),
        (
            false,
            Query::Communities,
            "A supplied explanation",
            "A supplied explanation",
        ),
    ] {
        let mut app = app(&[]);
        app.config.show_nsfw = show_nsfw;
        app.overlay = Some(Overlay::Experience);
        app.experience.screens = vec![Screen::Browse {
            pagination: Default::default(),
            query,
            page: Page {
                title: "Empty collection".into(),
                text: text.into(),
                entries: vec![],
                after: None,
                url: String::new(),
            },
            selected: 0,
            scroll: 0,
        }];

        let actual = screen(&mut app, 60, 16);

        assert!(actual.contains(expected), "{actual}");
        assert!(!actual.contains("show_nsfw"), "{actual}");
    }
}
