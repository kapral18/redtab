use super::*;

#[test]
fn when_galleries_wrap_in_both_directions_the_preview_should_follow_the_selected_image() {
    let (mut app, _dir, _fixture) = app();
    let mut page = post_page();
    page.post.media = Media::Gallery {
        images: vec![
            "https://i.redd.it/a.png".into(),
            "https://i.redd.it/b.png".into(),
        ],
        previews: vec![
            Some("https://preview.redd.it/a.png?width=1080".into()),
            Some("https://preview.redd.it/b.png?width=1080".into()),
        ],
        thumb: None,
    };
    let post = page.post.clone();
    let link = post.permalink.clone();
    let generation = app.show_post(post, &link).unwrap();
    app.on_msg(Msg::Post {
        generation,
        result: Ok(Box::new(page)),
    });
    app.gallery_step(false);
    assert_eq!(
        app.post_view.as_ref().unwrap().preview_image().as_deref(),
        Some("https://preview.redd.it/b.png?width=1080")
    );
    app.gallery_step(true);
    assert_eq!(
        app.post_view.as_ref().unwrap().preview_image().as_deref(),
        Some("https://preview.redd.it/a.png?width=1080")
    );
}

#[test]
fn when_browsing_to_another_item_its_preview_should_start_at_the_top() {
    for (selected, code, expected) in [
        (0, KeyCode::Char('j'), 1),
        (1, KeyCode::Char('k'), 0),
        (0, KeyCode::Char('G'), 1),
    ] {
        let (mut app, _dir, _fixture) = app();
        app.overlay = Some(Overlay::Experience);
        app.experience.screens.push(Screen::Browse {
            pagination: Default::default(),
            query: Query::Inbox(crate::reddit::experience::Inbox::All),
            page: Page {
                title: "Inbox".into(),
                text: String::new(),
                entries: ["first", "second"]
                    .map(|title| Entry {
                        icon: Default::default(),
                        title: title.into(),
                        body: "short preview".into(),
                        target: None,
                        query: None,
                        post: None,
                    })
                    .into(),
                after: None,
                url: "/message/inbox/".into(),
            },
            selected,
            scroll: 30,
        });

        key(&mut app, code);

        assert!(
            matches!(app.experience.screens.last(), Some(Screen::Browse {
                selected, scroll: 0, ..
            }) if *selected == expected)
        );
    }
}
