use super::*;

#[test]
fn a_picture_in_a_comment_should_load_then_take_rows_by_its_shape() {
    let (mut app, _) = open_long_post(|_| {});
    app.image_picker = kitty();
    app.show_images = true;
    let url = "https://preview.redd.it/c.png";
    let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
    page.body.clear();
    let mut source: serde_json::Value =
        serde_json::from_str(include_str!("../../../tests/fixtures/post.json")).unwrap();
    source[1]["data"]["children"][0]["data"]["body_html"] = serde_json::json!(format!(
        "before<a href='https://giphy.com/gifs/x'><img src='{url}'></a>after"
    ));
    let parsed = crate::reddit::parse::parse_post_page(&source.to_string()).unwrap();
    page.comments[0].body = parsed.comments[0].replies[0].body.clone();

    let loading = screen(&mut app, 100, 40);
    assert!(loading.contains("⠋ loading image"), "{loading}");
    assert!(!loading.contains("giphy.com"), "{loading}");
    assert!(matches!(app.images.get(url), Some(ImageSlot::Loading)));

    let slot = ready(&app, 400, 200);
    app.images.insert(url.into(), slot);
    let loaded = screen(&mut app, 100, 40);

    let row = |text: &str| loaded.lines().position(|l| l.contains(text)).unwrap();
    // 400×200 grows twice to 800×400 px, 100×25 cells at 8×16: 12 rows at most.
    assert_eq!(row("after") - row("before"), 13, "{loaded}");
    let first = loaded.lines().nth(row("before") + 1).unwrap();
    assert!(first.contains('\u{10EEEE}'), "{loaded}");
}

#[test]
fn when_a_picture_above_the_text_being_read_loads_that_text_should_stay_put() {
    let (mut app, _) = open_long_post(|_| {});
    app.image_picker = kitty();
    app.show_images = true;
    let url = "https://preview.redd.it/c.png";
    let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
    page.body.clear();
    page.comments[0].body = format!("{PICTURE}{url}\n\n{}", paragraphs("line", 1..=60));
    screen(&mut app, 100, 30);
    key(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
    let top = |s: &str| {
        s.lines().find_map(|l| {
            let rest = l.split("line ").nth(1)?;
            rest.split_whitespace().next().map(str::to_string)
        })
    };
    let before = top(&screen(&mut app, 100, 30));

    let slot = ready(&app, 400, 200);
    app.images.insert(url.into(), slot);
    let after = top(&screen(&mut app, 100, 30));

    assert!(before.is_some());
    assert_eq!(before, after);
}

#[test]
fn when_a_cached_comment_picture_refresh_fails_retry_should_keep_its_rows_and_update_pixels() {
    let url = "https://preview.redd.it/cached-comment.png";
    let mut app = picture_comment(url);
    screen(&mut app, 100, 40);
    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Image {
            url: url.into(),
            result: Ok(image::DynamicImage::new_rgb8(400, 200).into()),
        }),
        fetched_at: crate::app::now() - 8 * 86_400,
        refreshing: true,
        source: crate::cache::Source {
            cached: true,
            ..Default::default()
        },
    });
    screen(&mut app, 100, 40);
    let before = app.post_view.as_ref().unwrap().rendered_pictures.clone();
    app.on_msg(Msg::Image {
        url: url.into(),
        result: Err("offline".into()),
    });

    press(&mut app, 'r');
    let retrying = screen(&mut app, 100, 40);

    assert!(retrying.contains('\u{10EEEE}'), "{retrying}");
    assert!(matches!(
        app.images.get(url),
        Some(ImageSlot::Ready {
            pixels: (400, 200),
            refresh: crate::app::ImageRefresh::Loading,
            ..
        })
    ));
    assert_eq!(app.post_view.as_ref().unwrap().rendered_pictures, before);
    app.on_msg(Msg::Image {
        url: url.into(),
        result: Ok(image::DynamicImage::new_rgb8(800, 400).into()),
    });
    screen(&mut app, 100, 40);
    assert!(matches!(
        app.images.get(url),
        Some(ImageSlot::Ready {
            pixels: (800, 400),
            refresh: crate::app::ImageRefresh::Idle,
            ..
        })
    ));
    assert_eq!(app.post_view.as_ref().unwrap().rendered_pictures, before);
}

#[test]
fn after_r_a_failed_comment_picture_should_load_again() {
    let url = "https://preview.redd.it/c.png";
    let mut app = picture_comment(url);
    screen(&mut app, 100, 40);
    app.images
        .insert(url.into(), ImageSlot::Failed("HTTP 429".into()));
    let failed = screen(&mut app, 100, 40);

    press(&mut app, 'r');
    let again = screen(&mut app, 100, 40);

    assert!(failed.contains("▣ image unavailable"), "{failed}");
    assert!(again.contains("⠋ loading image"), "{again}");
    assert!(matches!(app.images.get(url), Some(ImageSlot::Loading)));
}

#[test]
fn a_held_comment_should_stay_put_when_a_picture_in_it_loads() {
    let url = "https://preview.redd.it/c.png";
    let mut app = picture_comment(url);
    let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
    page.comments[0].body = format!("intro\n{PICTURE}{url}\n\n{}", paragraphs("line", 1..=60));
    screen(&mut app, 100, 30);
    for keys in ["\u{6}", "\u{6}", "\u{6}", "l"] {
        press_keys(&mut app, keys);
    }
    let before = first_numbered_line(&screen(&mut app, 100, 30));

    let slot = ready(&app, 400, 200);
    app.images.insert(url.into(), slot);
    let after = first_numbered_line(&screen(&mut app, 100, 30));

    assert!(before.is_some());
    assert_eq!(before, after);
}

#[test]
fn a_queued_picture_scrolled_far_away_should_be_let_go() {
    let urls: Vec<String> = (0..40)
        .map(|i| format!("https://preview.redd.it/q{i}.png"))
        .collect();
    let mut app = picture_comment(&urls[0]);
    let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
    page.comments[0].body = urls
        .iter()
        .map(|url| format!("{PICTURE}{url}\n\n{}", paragraphs("filler", 1..=10)))
        .collect::<Vec<_>>()
        .join("\n\n");
    screen(&mut app, 100, 30);
    assert!(matches!(app.images.get(&urls[0]), Some(ImageSlot::Loading)));

    let view = app.post_view.as_mut().unwrap();
    view.free_scroll = true;
    view.scroll = 600;
    screen(&mut app, 100, 30);

    assert!(
        !app.images.contains_key(&urls[0]),
        "hundreds of lines above"
    );
}

#[test]
fn r_in_the_list_should_retry_the_open_posts_failed_pictures_too() {
    let url = "https://preview.redd.it/c.png";
    let mut app = picture_comment(url);
    app.images
        .insert(url.into(), ImageSlot::Failed("HTTP 429".into()));
    app.focus = crate::app::Focus::List;

    app.reload();

    assert!(!matches!(app.images.get(url), Some(ImageSlot::Failed(_))));
}

#[test]
fn a_quoted_picture_should_keep_its_quote_bar_on_every_row() {
    let url = "https://preview.redd.it/c.png";
    let mut app = picture_comment(url);
    let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
    page.comments[0].body = format!("│ {PICTURE}{url}");
    let slot = ready(&app, 400, 200);
    app.images.insert(url.into(), slot);

    let s = screen(&mut app, 100, 40);

    let rows: Vec<&str> = s.lines().filter(|l| l.contains('\u{10EEEE}')).collect();
    assert!(rows.len() > 1, "{s}");
    // The first row's first cell also carries the image's escape sequence.
    let barred = |l: &&str| {
        let at = l.find(['\u{10EEEE}', '\x1b']).unwrap();
        l[..at].ends_with("│ ")
    };
    assert!(rows.iter().all(barred), "{s}");
}

#[test]
fn a_queued_picture_in_a_folded_reply_should_be_let_go() {
    let url = "https://preview.redd.it/folded.png";
    let mut app = picture_comment("https://preview.redd.it/unused.png");
    let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
    page.comments[0].body = "parent".into();
    let mut answer = reply("r".into());
    answer.body = format!("{PICTURE}{url}");
    page.comments[0].replies.push(answer);
    screen(&mut app, 100, 30);
    assert!(matches!(app.images.get(url), Some(ImageSlot::Loading)));

    let id = app
        .post_view
        .as_ref()
        .unwrap()
        .page
        .as_ref()
        .unwrap()
        .comments[0]
        .id
        .clone();
    app.post_view.as_mut().unwrap().collapsed.insert(id);
    screen(&mut app, 100, 30);

    assert!(!app.images.contains_key(url));
}

#[test]
fn a_bulleted_picture_in_a_quote_should_show_its_bullet_once() {
    let url = "https://preview.redd.it/c.png";
    let mut app = picture_comment(url);
    let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
    page.comments[0].body = format!("│ • {PICTURE}{url}");
    let slot = ready(&app, 400, 200);
    app.images.insert(url.into(), slot);

    let s = screen(&mut app, 100, 40);

    let rows: Vec<&str> = s.lines().filter(|l| l.contains('\u{10EEEE}')).collect();
    assert!(rows.len() > 1, "{s}");
    let bullets = rows.iter().filter(|l| l.contains("• ")).count();
    assert_eq!(bullets, 1, "{s}");
    assert!(rows.iter().all(|l| l.contains("│ ")), "{s}");
}

#[test]
fn a_picture_across_the_top_shrinking_should_keep_the_text_under_it() {
    let url = "https://preview.redd.it/c.png";
    let mut app = picture_comment(url);
    let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
    page.comments[0].body = format!("{PICTURE}{url}\n\n{}", paragraphs("line", 1..=60));
    let slot = ready(&app, 400, 200);
    app.images.insert(url.into(), slot);
    screen(&mut app, 100, 30);
    // Five of the picture's 12 rows above the top, "line 1" a few rows down.
    let view = app.post_view.as_mut().unwrap();
    let line = view.rendered_pictures[0].1;
    (view.free_scroll, view.scroll) = (true, line + 5);
    let before = screen(&mut app, 100, 30);

    app.show_images = false;
    let after = screen(&mut app, 100, 30);

    assert_eq!(
        first_numbered_line(&before).as_deref(),
        Some("1"),
        "{before}"
    );
    assert_eq!(first_numbered_line(&after).as_deref(), Some("1"), "{after}");
}

#[test]
fn many_loaded_pictures_far_from_view_should_be_let_go() {
    let urls: Vec<String> = (0..40)
        .map(|i| format!("https://preview.redd.it/p{i}.png"))
        .collect();
    let mut app = picture_comment(&urls[0]);
    let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
    page.comments[0].body = urls
        .iter()
        .map(|url| format!("{PICTURE}{url}\n\nfiller"))
        .collect::<Vec<_>>()
        .join("\n\n");
    for url in &urls {
        let slot = ready(&app, 400, 400);
        app.images.insert(url.clone(), slot);
    }

    screen(&mut app, 100, 30);

    assert!(app.images.contains_key(&urls[0]), "in view");
    assert!(
        !app.images.contains_key(&urls[39]),
        "hundreds of lines below"
    );
}

#[test]
fn a_hidden_picture_should_mask_as_a_short_bar_not_its_url() {
    let text = format!(
        "see {SPOILER_START}{PICTURE}https://preview.redd.it/abcdefghijklmnop.png{SPOILER_END}"
    );

    assert_eq!(spoiler_text(&text, false), "see ▒▒▒▒▒▒");
}

#[test]
fn a_picture_loading_as_the_page_refreshes_should_not_move_a_short_selected_comment() {
    let url = "https://preview.redd.it/c.png";
    let mut app = picture_comment(url);
    let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
    page.comments[0].body = format!("{PICTURE}{url}\n\n{}", paragraphs("line", 1..=60));
    let mut short = reply("short".into());
    short.body = "short reply".into();
    page.comments.push(short);
    screen(&mut app, 100, 30);
    // The short comment selected and wholly in view, the picture above the top.
    let view = app.post_view.as_mut().unwrap();
    view.scroll = view.row_starts[1] - 5;
    (view.free_scroll, view.cursor, view.hold_view) = (false, 1, None);
    let row = |s: &str| s.lines().position(|l| l.contains("short reply"));
    let before = row(&screen(&mut app, 100, 30));

    app.post_view.as_mut().unwrap().content_changed = true;
    let slot = ready(&app, 400, 200);
    app.images.insert(url.into(), slot);
    let after = row(&screen(&mut app, 100, 30));

    assert!(before.is_some());
    assert_eq!(before, after);
}

#[test]
fn a_picture_loading_just_above_the_top_should_keep_the_text_being_read() {
    let url = "https://preview.redd.it/c.png";
    let mut app = picture_comment(url);
    let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
    page.comments[0].body = format!("{PICTURE}{url}\n\n{}", paragraphs("line", 1..=60));
    screen(&mut app, 100, 30);
    // Its one-line label 3 lines above the top, fewer than the 12 rows it will take.
    let view = app.post_view.as_mut().unwrap();
    let line = view.rendered_pictures[0].1;
    (view.free_scroll, view.scroll) = (true, line + 3);
    let before = first_numbered_line(&screen(&mut app, 100, 30));

    let slot = ready(&app, 400, 200);
    app.images.insert(url.into(), slot);
    let after = first_numbered_line(&screen(&mut app, 100, 30));

    assert!(before.is_some());
    assert_eq!(before, after);
}
