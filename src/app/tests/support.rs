use super::*;

pub(super) fn app_with_pending_pagination() -> App {
    let mut app = app_with(&["s"]);
    app.switch_tab(4);
    app.on_msg(Msg::Feed {
        tab: app.tabs[4].id,
        generation: app.tabs[4].feed.generation,
        append: false,
        result: Ok(listing("s", &["existing"], Some("next"))),
    });
    app.load_feed(4, true);
    app
}

pub(super) fn card_playback_app() -> App {
    let mut app = app_with(&[]);
    app.active = 0;
    app.show_images = true;
    let mut first = post("first", "s");
    first.media = Media::Video {
        gif: false,
        url: Some("https://v.redd.it/example.mp4".into()),
        poster: Some("https://i.redd.it/poster.jpg".into()),
    };
    first.preview_size = Some((320, 640));
    let mut second = first.clone();
    second.id = "second".into();
    second.permalink = "/comments/second/".into();
    app.tabs[0].feed.posts = vec![first, second];
    app.tabs[0].feed.loaded = true;
    app.card_video_drawn(Some(Size::new(68, 68)));
    app
}

pub(super) fn failed_card_playback_app() -> App {
    let mut app = card_playback_app();
    app.card_pictures_drawn(&["https://i.redd.it/poster.jpg"]);
    app.on_msg(Msg::Image {
        url: "https://i.redd.it/poster.jpg".into(),
        result: Ok(image::DynamicImage::new_rgb8(320, 640).into()),
    });
    let later = app.card_video.as_ref().unwrap().since + NAV_SETTLE;
    app.sync_video_with(later, |url, area, _| Player::unavailable(&url, area));
    app
}

pub(super) fn playback_frame(shade: u8) -> video::Frame {
    video::Frame {
        width: 16,
        height: 9,
        pixels: video::Pixels::Raw(std::sync::Arc::new(image::DynamicImage::ImageRgb8(
            image::RgbImage::from_pixel(16, 9, image::Rgb([shade, 0, 0])),
        ))),
        same_as_first: false,
    }
}

/// The ids of the replies under comment `parent` in the open post.
pub(super) fn reply_ids(app: &App, parent: &str) -> Vec<String> {
    let page = app.post_view.as_ref().unwrap().page.as_ref().unwrap();
    find_comment(&page.comments, parent)
        .unwrap()
        .replies
        .iter()
        .map(|c| c.id.clone())
        .collect()
}

/// A graphics protocol with images on: icons and thumbnails draw.
pub(super) fn show_thumbnails(app: &mut App) {
    app.image_picker = crate::ui::tests::kitty();
    app.show_images = true;
}

pub(super) fn app_with(subscriptions: &[&str]) -> App {
    app_with_messages(subscriptions).0
}

pub(super) fn press(app: &mut App, code: crossterm::event::KeyCode) {
    crate::keys::handle(
        app,
        crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE),
    );
}

pub(super) fn sequence(app: &mut App, keys: &str) {
    for c in keys.chars() {
        press(app, crossterm::event::KeyCode::Char(c));
    }
}

/// An app plus the replies its jobs produce. Port 9 is discard: nothing answers,
/// so every network job fails quickly and no test touches the network.
pub(super) fn app_with_messages(subscriptions: &[&str]) -> (App, std::sync::mpsc::Receiver<Msg>) {
    app_at("http://127.0.0.1:9", subscriptions)
}

/// An app whose Reddit API is the server at `url`, without a disk cache, plus the replies
/// its jobs produce.
pub(super) fn app_at(url: &str, subscriptions: &[&str]) -> (App, std::sync::mpsc::Receiver<Msg>) {
    app_with_client(Reddit::test_at(url, false), subscriptions)
}

pub(super) fn app_with_client(
    reddit: Reddit,
    subscriptions: &[&str],
) -> (App, std::sync::mpsc::Receiver<Msg>) {
    let (pool, messages) = Pool::start(reddit.clone(), 3, None);
    let state = crate::test_support::state_path();
    let mut app = App::new(Config::default(), pool, reddit, state, Picker::halfblocks());
    app.config.select_account(Some("tester"));
    app.settings.store.select_account(Some("tester"));
    app.on_msg(Msg::Subscriptions {
        generation: 0,
        result: Ok((subscriptions
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<String>>())
        .into_iter()
        .map(|name| crate::model::Subscription::Community {
            name,
            icon: crate::model::SubscriptionIcon::Unknown,
        })
        .collect()),
    });
    app.active = ALL_SUBS_TAB;
    (app, messages)
}

pub(super) fn post(id: &str, sub: &str) -> Post {
    Post {
        id: id.into(),
        subreddit: sub.into(),
        title: format!("title {id}"),
        author: "a".into(),
        flair: None,
        score: Some(1),
        comments: 0,
        created: None,
        permalink: format!("/r/{sub}/comments/{id}/x/"),
        media: Media::None,
        preview_size: None,
        stickied: false,
        nsfw: false,
        spoiler: false,
        original_deleted: false,
    }
}

pub(super) fn listing(sub: &str, ids: &[&str], after: Option<&str>) -> Listing {
    Listing {
        posts: ids.iter().map(|i| post(i, sub)).collect(),
        after: after.map(str::to_string),
    }
}

pub(super) fn comment(id: &str, replies: Vec<Comment>, more: Option<&str>) -> Comment {
    Comment {
        sources: None,
        created: None,
        id: id.into(),
        author: "a".into(),
        flair: None,
        is_op: false,
        is_mod: false,
        stickied: false,
        score: None,
        body: id.into(),
        replies,
        more: more.map(|p| MoreReplies {
            cursor: p.into(),
            count: Some(1),
        }),
    }
}

pub(super) fn tab_index(app: &App, name: &str) -> usize {
    app.tabs
        .iter()
        .position(|t| t.subreddit() == Some(name))
        .unwrap()
}

/// r/s listing `ids`, its first post open beside the list and the list focused.
pub(super) fn split_feed(ids: &[&str]) -> (App, std::sync::mpsc::Receiver<Msg>) {
    let (mut app, messages) = app_with_messages(&["s"]);
    let i = tab_index(&app, "s");
    app.switch_tab(i);
    let (tab, generation) = (app.tabs[i].id, app.tabs[i].feed.generation);
    app.on_msg(Msg::Feed {
        tab,
        generation,
        append: false,
        result: Ok(listing("s", ids, None)),
    });
    app.resize(200);
    app.open_current_post();
    app.focus_main();
    (app, messages)
}

pub(super) fn entry_title(app: &App, entry: &PickerEntry) -> Option<String> {
    match entry {
        PickerEntry::Tab { index, .. } => Some(app.tabs[*index].title()),
        PickerEntry::Open(_) | PickerEntry::OpenUser(_) => None,
    }
}

pub(super) fn island_ids(app: &App) -> Vec<String> {
    app.group_posts(&app.all_subs.groups[0])
        .map(|post| post.id.clone())
        .collect()
}

pub(super) fn island_answer(app: &App, ids: &[&str]) -> Msg {
    Msg::Group {
        subreddit: "vim".into(),
        generation: app.all_subs.groups[0].generation,
        result: Ok(listing("vim", ids, None)),
    }
}

pub(super) fn ordered_source(order: u64, cached: bool) -> Source {
    Source {
        order: Some(crate::cache::RequestOrder {
            generation: [1; 16],
            sequence: order,
        }),
        observed: Some((order, order)),
        cached,
    }
}

pub(super) fn ordered_snapshot(message: Msg, order: u64, cached: bool, refreshing: bool) -> Msg {
    Msg::Snapshot {
        message: Box::new(message),
        fetched_at: 1000,
        source: ordered_source(order, cached),
        refreshing,
    }
}

/// Top-level comments under the thread root, as the parser delivers them.
pub(super) fn rooted(comments: Vec<Comment>) -> Vec<Comment> {
    vec![comment(&format!("{THREAD_ROOT}t3_p"), comments, None)]
}

pub(super) fn thread_message(generation: u64, comments: Vec<Comment>) -> Msg {
    Msg::Post {
        generation,
        result: Ok(Box::new(PostPage {
            post: post("p", "s"),
            body: String::new(),
            comments,
        })),
    }
}

pub(in crate::app) fn confirm_preferences(app: &mut App, show_nsfw: bool) {
    app.settings.account.values = Some(crate::reddit::Preferences {
        show_nsfw,
        video_autoplay: true,
        mature_content_editable: true,
    });
    app.apply_settings();
}

pub(super) fn unread_badge_app() -> (App, usize) {
    let mut app = app_with(&["s"]);
    app.state.mark_read(&post("old", "s"), now());
    app.on_msg(Msg::Group {
        generation: 0,
        subreddit: "s".into(),
        result: Ok(listing("s", &["old", "new1", "new2"], None)),
    });
    let i = tab_index(&app, "s");
    app.active = i;
    assert_eq!(app.badge(&app.tabs[i]), UnreadBadge::Count(2));
    (app, i)
}
