use super::*;

/// Opens a 60-paragraph post; `while_loading` runs before the page arrives.
pub(super) fn open_long_post(while_loading: impl FnOnce(&mut App)) -> (App, String) {
    let mut app = app(&[]);
    let post = Post {
        id: "p".into(),
        subreddit: "s".into(),
        title: "A long post".into(),
        author: "a".into(),
        flair: None,
        score: Some(1),
        comments: 1,
        created: None,
        permalink: "/r/s/comments/p/x/".into(),
        media: Media::None,
        preview_size: None,
        stickied: false,
        nsfw: false,
        spoiler: false,
        original_deleted: false,
    };
    app.all_subs.groups = vec![crate::app::Group {
        subreddit: "s".into(),
        posts: vec![post.clone()],
        freshness: Some(crate::cache::Freshness {
            fetched_at: crate::app::now(),
            source: Default::default(),
        }),
        ..Default::default()
    }];
    app.open_current_post();
    screen(&mut app, 100, 30);
    while_loading(&mut app);
    let generation = app.post_view.as_ref().unwrap().generation;
    let body = paragraphs("paragraph", 1..=60);
    let comment = reply("c".into());
    app.on_msg(Msg::Post {
        generation,
        result: Ok(Box::new(PostPage {
            post,
            body,
            comments: vec![comment],
        })),
    });
    let s = screen(&mut app, 100, 30);
    (app, s)
}

pub(super) fn reply(id: String) -> Comment {
    Comment {
        sources: None,
        created: None,
        id,
        author: "x".into(),
        flair: None,
        is_op: false,
        is_mod: false,
        stickied: false,
        score: None,
        body: "reply".into(),
        replies: vec![],
        more: None,
    }
}

pub(super) fn paragraphs(prefix: &str, range: impl Iterator<Item = usize>) -> String {
    range
        .map(|i| format!("{prefix} {i}"))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// An open post whose only comment is one picture, kitty and images on.
pub(super) fn picture_comment(url: &str) -> App {
    let (mut app, _) = open_long_post(|_| {});
    app.image_picker = kitty();
    app.show_images = true;
    let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
    page.body.clear();
    page.comments[0].body = format!("{PICTURE}{url}");
    app
}

/// The number after "line " on the first line that has one.
pub(super) fn first_numbered_line(s: &str) -> Option<String> {
    s.lines().find_map(|l| {
        let rest = l.split("line ").nth(1)?;
        rest.split_whitespace().next().map(str::to_string)
    })
}

pub(super) fn denied_community(access: AccessDenied) -> App {
    let mut app = card_feed_app(Picker::halfblocks());
    app.on_msg(Msg::Feed {
        tab: app.active_tab().id,
        generation: app.active_tab().feed.generation,
        append: false,
        result: Err(crate::worker::FeedError::Access(access)),
    });
    app
}

/// `open_long_post` with an image and `kitty` graphics.
pub(super) fn image_post(body: &str) -> App {
    let (mut app, _) = open_long_post(|_| {});
    app.image_picker = kitty();
    app.show_images = true;
    let image = Media::Image {
        url: "https://i.redd.it/big.jpeg".into(),
        preview: None,
    };
    let view = app.post_view.as_mut().unwrap();
    view.post.media = image.clone();
    let page = view.page.as_mut().unwrap();
    page.post.media = image;
    page.body = body.into();
    app
}

pub(super) fn detail_pictures(app: &mut App, width: u16, height: u16) -> Vec<CardPicture> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    let mut pictures = ListPictures::default();
    terminal
        .draw(|f| {
            draw_post(f, app, f.area(), &mut pictures);
        })
        .unwrap();
    pictures.cards
}

/// `image_post("short")` with a rewrapping spoiler in its first comment, a
/// "more replies" row under it and `n` "marker N" comments after; drawn.
pub(super) fn image_post_with_comments(n: usize) -> App {
    use crate::model::MoreReplies;
    let mut app = image_post("short");
    let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
    let reply = page.comments[0].clone();
    page.comments[0].body = (31..=36)
        .map(|n| format!("aaaaaaaaaa {SPOILER_START}{}{SPOILER_END}", "ab-".repeat(n)))
        .collect::<Vec<_>>()
        .join("\n\n");
    page.comments[0].more = Some(MoreReplies {
        cursor: "c".into(),
        count: Some(2),
    });
    for i in 1..=n {
        page.comments.push(Comment {
            id: format!("m{i}"),
            body: format!("marker {i}"),
            ..reply.clone()
        });
    }
    screen(&mut app, 100, 60);
    app
}

pub(super) fn replies(n: usize) -> Vec<Comment> {
    (0..n)
        .map(|i| Comment {
            sources: Some(Default::default()),
            ..reply(format!("r{i}"))
        })
        .collect()
}

/// `image_post_with_comments(37)` with a short first comment and "marker 1"
/// selected under the image (`j j j`), at scroll 0.
pub(super) fn marker_selected_under_the_image() -> App {
    let mut app = image_post_with_comments(37);
    app.post_view
        .as_mut()
        .unwrap()
        .page
        .as_mut()
        .unwrap()
        .comments[0]
        .body = "first".into();
    screen(&mut app, 100, 60);
    for key in ['j', 'j', 'j'] {
        press(&mut app, key);
        screen(&mut app, 100, 60);
    }
    assert_eq!(app.post_view.as_ref().unwrap().cursor, 2);
    app
}

pub(super) fn load_two_replies_above(app: &mut App) {
    let generation = app.post_view.as_ref().unwrap().generation;
    app.on_msg(Msg::MoreReplies {
        generation,
        comment_id: "c".into(),
        result: Ok(ReplyPage::complete(replies(2))),
    });
}

/// A post whose body has spoilers that rewrap when revealed, then `body_tail`
/// lines, then a tall comment; drawn once.
pub(super) fn post_with_rewrapping_spoilers(body_tail: usize) -> App {
    let (mut app, _) = open_long_post(|_| {});
    let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
    // Hyphens are break points; the mask has none, so lines break elsewhere.
    let spoilers =
        (31..=36).map(|n| format!("aaaaaaaaaa {SPOILER_START}{}{SPOILER_END}", "ab-".repeat(n)));
    let tail = (1..=body_tail).map(|i| format!("body {i}"));
    page.body = spoilers.chain(tail).collect::<Vec<_>>().join("\n\n");
    page.comments[0].body = paragraphs("line", 1..=40);
    screen(&mut app, 100, 30);
    app
}

pub(super) fn post_with(id: &str, media: Media) -> Post {
    Post {
        id: id.into(),
        subreddit: "s".into(),
        title: format!("{id} {}", "x".repeat(200)),
        author: "a".into(),
        flair: None,
        score: Some(1),
        comments: 0,
        created: None,
        permalink: format!("/r/s/comments/{id}/x/"),
        media,
        preview_size: None,
        stickied: false,
        nsfw: false,
        spoiler: false,
        original_deleted: false,
    }
}

/// r/s with eight posts; all but `p2` have an image.
pub(super) fn card_feed_app(picker: Picker) -> App {
    let mut app = app(&["s"]);
    app.image_picker = picker;
    app.show_images = true;
    app.switch_tab(4);
    let tab = app.tabs[4].id;
    let generation = app.tabs[4].feed.generation;
    let posts = (1..=8)
        .map(|i| {
            let media = if i == 2 {
                Media::None
            } else {
                Media::Image {
                    url: format!("https://i.redd.it/p{i}.jpeg"),
                    preview: None,
                }
            };
            post_with(&format!("p{i}"), media)
        })
        .collect();
    app.on_msg(Msg::Feed {
        tab,
        generation,
        append: false,
        result: Ok(crate::model::Listing { posts, after: None }),
    });
    app
}

pub(super) fn feed_app(picker: Picker) -> App {
    let mut app = card_feed_app(picker);
    app.state.toggle_list_view();
    app
}

pub(super) fn feed_with_next_page(view: ListView) -> App {
    feed_with_next_page_in(app(&["s"]), view)
}

pub(super) fn feed_with_next_page_in(mut app: App, view: ListView) -> App {
    app.switch_tab(4);
    if app.state.list_view() != view {
        app.state.toggle_list_view();
    }
    let mut post = post_with("existing", Media::None);
    post.title = "Existing post".into();
    app.on_msg(Msg::Feed {
        tab: app.active_tab().id,
        generation: app.active_tab().feed.generation,
        append: false,
        result: Ok(crate::model::Listing {
            posts: vec![post],
            after: Some("next".into()),
        }),
    });
    app
}

pub(super) fn card_with_decoded_image() -> App {
    let mut app = card_feed_app(kitty());
    let url = "https://i.redd.it/p1.jpeg";
    app.card_pictures_drawn(&[url]);
    app.on_msg(Msg::Image {
        url: url.into(),
        result: Ok(image::DynamicImage::new_rgb8(640, 640).into()),
    });
    app
}

pub(super) fn refreshing_thumbnail_app() -> App {
    let mut app = feed_app(kitty());
    app.tabs[4].feed.posts.truncate(1);
    app.thumbs_drawn(&["https://i.redd.it/p1.jpeg"]);
    app.on_msg(Msg::Snapshot {
        message: Box::new(Msg::Thumb {
            url: "https://i.redd.it/p1.jpeg".into(),
            result: Ok(image::DynamicImage::new_rgb8(32, 32)),
        }),
        fetched_at: crate::app::now() - 600,
        refreshing: true,
        source: crate::cache::Source {
            cached: true,
            ..Default::default()
        },
    });
    app
}

pub(super) fn card_with_image(width: u32, height: u32) -> App {
    let mut app = card_feed_app(kitty());
    app.tabs[4].feed.posts.truncate(1);
    app.tabs[4].feed.posts[0].title = "Full width image".into();
    screen(&mut app, 100, 20);
    app.on_msg(Msg::Image {
        url: "https://i.redd.it/p1.jpeg".into(),
        result: Ok(image::DynamicImage::new_rgb8(width, height).into()),
    });
    app
}

pub(super) fn video_card_with_poster(image: image::DynamicImage) -> App {
    let mut app = card_feed_app(kitty());
    app.tabs[4].feed.posts.truncate(1);
    app.tabs[4].feed.posts[0].media = Media::Video {
        gif: false,
        url: Some("https://v.redd.it/example/HLSPlaylist.m3u8".into()),
        poster: Some("https://i.redd.it/p1.jpeg".into()),
    };
    app.tabs[4].feed.posts[0].preview_size = Some((image.width(), image.height()));
    first_screen(&mut app, 100, 20);
    app.on_msg(Msg::Image {
        url: "https://i.redd.it/p1.jpeg".into(),
        result: Ok(image.into()),
    });
    app
}

pub(super) fn card_png(protocol: &ratatui_image::protocol::Protocol) -> image::RgbImage {
    use base64::Engine;
    let ratatui_image::protocol::Protocol::ITerm2(protocol) = protocol else {
        panic!("iTerm2 card protocol");
    };
    let png = protocol.data.split("doNotMoveCursor=1:").nth(1).unwrap();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(png.split('\u{7}').next().unwrap())
        .unwrap();
    image::load_from_memory(&bytes).unwrap().to_rgb8()
}

pub(super) fn two_tall_media_cards() -> App {
    let mut app = card_feed_app(kitty());
    let mut first = app.tabs[4].feed.posts[0].clone();
    first.title = "Full width image".into();
    first.preview_size = Some((320, 640));
    let mut next = post_with(
        "p2",
        Media::Video {
            gif: false,
            url: Some("https://v.redd.it/example/HLSPlaylist.m3u8".into()),
            poster: Some("https://i.redd.it/p2.jpeg".into()),
        },
    );
    next.title = "Following video".into();
    next.preview_size = Some((320, 640));
    app.reload();
    app.on_msg(Msg::Feed {
        tab: app.active_tab().id,
        generation: app.active_tab().feed.generation,
        append: false,
        result: Ok(crate::model::Listing {
            posts: vec![first, next],
            after: None,
        }),
    });
    first_screen(&mut app, 100, 20);
    app.on_msg(Msg::Image {
        url: "https://i.redd.it/p1.jpeg".into(),
        result: Ok(image::DynamicImage::new_rgb8(320, 640).into()),
    });
    screen(&mut app, 100, 20);
    app
}

pub(super) const VIDEO: &str = "https://v.redd.it/v/HLSPlaylist.m3u8";

/// A loaded video post in a kitty terminal with images on, drawn once.
pub(super) fn open_video_post() -> App {
    let (mut app, _) = open_long_post(|_| {});
    app.image_picker = kitty();
    app.show_images = true;
    let view = app.post_view.as_mut().unwrap();
    view.page.as_mut().unwrap().post.media = Media::Video {
        gif: false,
        url: Some(VIDEO.into()),
        poster: None,
    };
    screen(&mut app, 100, 30);
    app
}
