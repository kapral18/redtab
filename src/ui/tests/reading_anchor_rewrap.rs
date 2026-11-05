use super::*;

#[test]
fn unchanged_redraws_reuse_document_text_and_content_updates_replace_it() {
    let mut app = image_post("Stable layout marker");
    let marker_pointer = |app: &App| {
        app.post_view
            .as_ref()
            .unwrap()
            .rendered_document
            .iter()
            .flat_map(|line| &line.spans)
            .find(|span| span.content.contains("Stable layout marker"))
            .unwrap()
            .content
            .as_ptr()
    };
    // The document cache key includes the clock second, so only two draws within one
    // second must share the wrapped text.
    for attempt in 1.. {
        assert!(attempt <= 3, "draws never shared a clock second");
        let second = chrono::Utc::now().timestamp();
        screen(&mut app, 100, 40);
        let pointer = marker_pointer(&app);
        screen(&mut app, 100, 40);
        if chrono::Utc::now().timestamp() == second {
            assert_eq!(
                marker_pointer(&app),
                pointer,
                "unchanged text should not be allocated and wrapped on every media draw"
            );
            break;
        }
    }
    let view = app.post_view.as_ref().unwrap();
    let generation = view.generation;
    let mut page = view.page.clone().unwrap();
    page.body = "Changed layout marker".into();
    app.on_msg(Msg::Post {
        generation,
        result: Ok(Box::new(page)),
    });
    assert!(screen(&mut app, 100, 40).contains("Changed layout marker"));
}

#[test]
fn when_body_markup_wraps_it_should_style_visible_text_without_changing_column_width() {
    use crate::model::{LINK_TARGET, LINK_TARGET_END, TEXT_BOLD, TEXT_END, TEXT_ITALIC, TEXT_LINK};
    let mut lines = Vec::new();
    wrap_into(
        &mut lines,
        &format!(
            "plain {TEXT_BOLD}bold {TEXT_ITALIC}both{TEXT_END} end{TEXT_END} {TEXT_LINK}{LINK_TARGET}https://example.org/a-long-destination{LINK_TARGET_END}link{TEXT_END}"
        ),
        12,
        &[Span::raw("│ ")],
        Style::new().fg(TEXT),
    );
    let actual: String = lines
        .iter()
        .flat_map(|line| line.spans.iter().map(|span| span.content.as_ref()))
        .collect();
    assert!(!actual.contains(crate::model::text_format), "{actual:?}");
    assert!(!actual.contains("example.org"), "{actual:?}");
    assert!(lines.iter().all(|line| line.width() <= 14), "{lines:?}");
    let both = lines
        .iter()
        .flat_map(|line| &line.spans)
        .find(|span| span.content.contains("both"))
        .unwrap();
    assert!(
        both.style
            .add_modifier
            .contains(Modifier::BOLD | Modifier::ITALIC),
        "{both:?}"
    );
    let link = lines
        .iter()
        .flat_map(|line| &line.spans)
        .find(|span| span.content.contains("link"))
        .unwrap();
    assert!(
        link.style.add_modifier.contains(Modifier::UNDERLINED),
        "{link:?}"
    );
}

#[test]
fn a_tall_held_comment_at_the_top_stays_there_when_text_below_changes() {
    let mut app = image_post("short");
    let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
    let first = page.comments[0].clone();
    page.comments[0].body = "para\n\n".repeat(40);
    page.comments.push(Comment {
        id: "below".into(),
        body: (31..=36)
            .map(|n| format!("aaaaaaaaaa {SPOILER_START}{}{SPOILER_END}", "ab-".repeat(n)))
            .collect::<Vec<_>>()
            .join("\n\n"),
        ..first
    });
    screen(&mut app, 100, 60);
    // `l` selects and holds the tall first comment under the image.
    press(&mut app, 'l');
    screen(&mut app, 100, 60);

    // A spoiler only below it is revealed.
    press_keys(&mut app, "zs");
    let s = screen(&mut app, 100, 60);

    assert!(s.contains("loading image"), "{s}");
    assert_eq!(app.post_view.as_ref().unwrap().scroll, 0);
}

#[test]
fn a_header_age_rewrapping_above_keeps_the_text_being_read() {
    let opened = |age: i64, width: u16| {
        let (mut app, _) = open_long_post(|_| {});
        let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
        page.comments[0].body = paragraphs("line", 1..=40);
        page.comments[0].created = Some(Utc::now() - chrono::Duration::seconds(age));
        screen(&mut app, width, 30);
        app
    };
    let len = |app: &App| app.post_view.as_ref().unwrap().rendered_document.len();
    // A width where "just now" wraps the header and "1m ago" does not.
    let width = (40..140)
        .find(|&width| len(&opened(10, width)) > len(&opened(120, width)))
        .expect("some width rewraps the header");
    let shown = |s: &str, needle: &str| {
        s.lines()
            .enumerate()
            .find_map(|(row, l)| l.split(needle).nth(1).map(|r| (row, r.to_string())))
    };
    let created = Utc::now() - chrono::Duration::seconds(58);
    let mut apps: Vec<_> = (0..2)
        .map(|_| {
            let (mut app, _) = open_long_post(|_| {});
            let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
            page.comments[0].body = paragraphs("line", 1..=40);
            page.comments[0].created = Some(created);
            let mut second = page.comments[0].clone();
            second.id = "second".into();
            second.body = "short".into();
            second.created = None;
            page.comments.push(second);
            screen(&mut app, width, 30);
            app
        })
        .collect();
    // Reading by scrolling, then with a later comment selected.
    let view = apps[0].post_view.as_mut().unwrap();
    view.scroll = view.row_starts[0] + 10;
    let view = apps[1].post_view.as_mut().unwrap();
    view.cursor = 1;
    view.free_scroll = false;
    view.scroll = view.row_starts[1] - 5;
    screen(&mut apps[1], width, 30);
    let befores: Vec<_> = apps
        .iter_mut()
        .map(|app| {
            let screen = screen(app, width, 30);
            (shown(&screen, "line "), len(app))
        })
        .collect();
    let selected = shown(&screen(&mut apps[1], width, 30), "▌");

    let wait = created + chrono::Duration::milliseconds(60_300) - Utc::now();
    std::thread::sleep(wait.to_std().unwrap_or_default());

    for (app, (before, lines_before)) in apps.iter_mut().zip(befores) {
        let after = shown(&screen(app, width, 30), "line ");
        assert!(len(app) < lines_before, "the header lost a line");
        assert!(before.is_some());
        assert_eq!(after, before, "the text being read stays");
    }
    assert!(selected.is_some());
    assert_eq!(shown(&screen(&mut apps[1], width, 30), "▌"), selected);
}

#[test]
fn a_header_shrinking_above_while_one_grows_below_keeps_the_text_being_read() {
    let opened = |age: i64, width: u16| {
        let (mut app, _) = open_long_post(|_| {});
        let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
        page.comments[0].body = paragraphs("line", 1..=40);
        page.comments[0].created = Some(Utc::now() - chrono::Duration::seconds(age));
        screen(&mut app, width, 30);
        app
    };
    let len = |app: &App| app.post_view.as_ref().unwrap().rendered_document.len();
    // A width where "just now" wraps and "1m ago" does not, while "9m ago" fits and
    // "10m ago" wraps: the document keeps its length as one header shrinks and one grows.
    let width = (40..140)
        .find(|&width| {
            len(&opened(10, width)) > len(&opened(120, width))
                && len(&opened(590, width)) < len(&opened(610, width))
        })
        .expect("some width rewraps both headers");
    let created = Utc::now() - chrono::Duration::seconds(58);
    let (mut app, _) = open_long_post(|_| {});
    let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
    page.comments[0].body = paragraphs("line", 1..=40);
    page.comments[0].created = Some(created);
    let mut below = page.comments[0].clone();
    below.id = "below".into();
    below.body = "short".into();
    below.created = Some(created - chrono::Duration::seconds(540));
    page.comments.push(below);
    screen(&mut app, width, 30);
    let view = app.post_view.as_mut().unwrap();
    view.scroll = view.row_starts[0] + 10;
    let shown = |s: &str| {
        s.lines()
            .enumerate()
            .find_map(|(row, l)| l.split("line ").nth(1).map(|r| (row, r.to_string())))
    };
    let lines_before = len(&app);
    let before = shown(&screen(&mut app, width, 30));

    let wait = created + chrono::Duration::milliseconds(60_300) - Utc::now();
    std::thread::sleep(wait.to_std().unwrap_or_default());
    // A spinner advancing in the same draw, as while anything loads.
    app.loading_frame += 1;
    let after = shown(&screen(&mut app, width, 30));

    assert_eq!(len(&app), lines_before, "the rewraps cancel out");
    assert!(before.is_some());
    assert_eq!(after, before, "the text being read stays");
}
