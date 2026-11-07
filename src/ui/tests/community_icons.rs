use super::*;

#[test]
fn a_subreddit_icon_shows_in_sidebar_and_home_card() {
    let mut app = app(&["s"]);
    app.image_picker = kitty();
    app.show_images = true;
    app.on_msg(Msg::Group {
        generation: 0,
        subreddit: "s".into(),
        result: Ok(crate::model::Listing {
            posts: vec![post_with("p1", Media::None)],
            after: None,
        }),
    });
    app.on_msg(Msg::Icon {
        subreddit: "s".into(),
        result: Ok(Some("/icon/s.png".into())),
    });
    // A later load of the subreddit keeps the known icon.
    app.switch_tab(4);
    let generation = app.tabs[4].feed.generation;
    let id = app.tabs[4].id;
    app.on_msg(Msg::Feed {
        tab: id,
        generation,
        append: false,
        result: Ok(crate::model::Listing {
            posts: vec![post_with("p1", Media::None)],
            after: None,
        }),
    });
    app.switch_tab(crate::app::ALL_SUBS_TAB);
    // Two quick tab switches hold new icons until they settle.
    app.tick_at(std::time::Instant::now() + crate::app::NAV_SETTLE);

    let s = screen(&mut app, 120, 30);

    assert!(app.thumbs.contains_key("/icon/s.png"));
    let image = image::DynamicImage::new_rgb8(16, 16);
    let protocol = app.image_picker.new_resize_protocol(image);
    app.thumbs.insert(
        "/icon/s.png".into(),
        ImageSlot::Ready {
            protocol: Box::new(protocol),
            refresh: crate::app::ImageRefresh::Idle,
            pixels: (16, 16),
        },
    );
    let cells = placeholder_cells(&mut app, 120, 30);

    let sidebar_row = s
        .lines()
        .position(|l| {
            l.chars()
                .take(app.sidebar_width() as usize)
                .collect::<String>()
                .split_whitespace()
                .any(|word| word == "s")
        })
        .unwrap() as u16;
    // Row 0 holds the focus outline's top edge.
    let card_row = s
        .lines()
        .skip(1)
        .position(|l| l.contains('╭') || l.contains('╔'))
        .unwrap() as u16
        + 1;
    let card_x = s
        .lines()
        .nth(card_row as usize)
        .unwrap()
        .chars()
        .position(|c| c == '╭' || c == '╔')
        .unwrap() as u16;
    // Sidebar: inset 2 + bar + number = 4; card title: corner + space = 2.
    let mut expected = vec![
        (5, sidebar_row),
        (6, sidebar_row),
        (card_x + 2, card_row),
        (card_x + 3, card_row),
    ];
    expected.sort_by_key(|&(x, y)| (y, x));
    let mut cells = cells;
    cells.sort_by_key(|&(x, y)| (y, x));
    assert_eq!(cells, expected);
}

#[test]
fn a_tab_opened_by_name_shows_its_icon_whatever_the_letter_case() {
    let mut app = app(&[]);
    app.image_picker = kitty();
    app.show_images = true;
    app.open_subreddit("Pics");
    let tab = &app.tabs[app.active];
    let (id, generation) = (tab.id, tab.feed.generation);

    app.on_msg(Msg::Feed {
        tab: id,
        generation,
        append: false,
        result: Ok(crate::model::Listing {
            posts: vec![post_with("p1", Media::None)],
            after: None,
        }),
    });
    app.on_msg(Msg::Icon {
        subreddit: "pics".into(),
        result: Ok(Some("/icon/pics.png".into())),
    });

    assert_eq!(app.shown_icon("Pics"), Some("/icon/pics.png"));
    assert_eq!(app.shown_icon("pics"), Some("/icon/pics.png"));
    app.show_images = false;
    assert_eq!(app.shown_icon("pics"), None);
}
