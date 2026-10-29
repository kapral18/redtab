use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionIcon {
    Unknown,
    Missing,
    Url(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Subscription {
    Community {
        name: String,
        icon: SubscriptionIcon,
    },
    User {
        name: String,
        icon: Option<String>,
    },
}

impl Subscription {
    pub fn name(&self) -> &str {
        match self {
            Self::Community { name, .. } | Self::User { name, .. } => name,
        }
    }

    pub fn community(&self) -> Option<&str> {
        match self {
            Self::Community { name, .. } => Some(name),
            Self::User { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MembershipTarget {
    Community(String),
    User(String),
}

impl MembershipTarget {
    pub fn name(&self) -> &str {
        match self {
            Self::Community(name) | Self::User(name) => name,
        }
    }

    pub fn label(&self) -> String {
        format!(
            "{}/{}",
            if matches!(self, Self::User(_)) {
                "u"
            } else {
                "r"
            },
            self.name()
        )
    }

    pub fn action(&self, join: bool) -> &'static str {
        match (self, join) {
            (Self::Community(_), true) => "Join",
            (Self::Community(_), false) => "Leave",
            (Self::User(_), true) => "Follow",
            (Self::User(_), false) => "Unfollow",
        }
    }

    pub fn matches(&self, entry: &Subscription) -> bool {
        match (self, entry) {
            (Self::Community(name), Subscription::Community { name: other, .. })
            | (Self::User(name), Subscription::User { name: other, .. }) => {
                name.eq_ignore_ascii_case(other)
            }
            _ => false,
        }
    }
}

pub fn is_username(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
}

pub fn profile_user(path: &str) -> Option<&str> {
    let path = path
        .strip_prefix("/user/")
        .or_else(|| path.strip_prefix("/u/"))?;
    let mut parts = path.trim_end_matches('/').split('/');
    let name = parts.next().filter(|name| is_username(name))?;
    matches!(parts.next(), None | Some("comments")).then_some(name)
}

pub const PREVIEW_EDGE: u32 = 1600;
pub const PREVIEW_PIXELS: u64 = 16_000_000;

/// Post media. URLs are absolute upstream URLs; temporary media-relay URLs are never persisted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Media {
    #[default]
    None,
    Image {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        preview: Option<String>,
    },
    Video {
        /// File or HLS playlist; `None` when Reddit has no playable source.
        url: Option<String>,
        poster: Option<String>,
        #[serde(default)]
        gif: bool,
    },
    Gallery {
        images: Vec<String>,
        /// Display sources by original index; absent entries use the original.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        previews: Vec<Option<String>>,
        /// Listing thumbnail, also used when gallery entries are unavailable.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thumb: Option<String>,
    },
    Link {
        url: String,
        thumb: Option<String>,
    },
}

impl Media {
    pub fn youtube_url(&self) -> Option<&str> {
        match self {
            Self::Link { url, .. } if youtube_id(url).is_some() => Some(url),
            _ => None,
        }
    }

    pub fn tag(&self) -> Option<String> {
        if self.youtube_url().is_some() {
            return Some("YouTube".into());
        }
        match self {
            Media::None => None,
            Media::Image { .. } if self.video_url().is_some() => Some("gif".into()),
            Media::Image { .. } => Some("image".into()),
            Media::Video { gif: true, .. } => Some("gif".into()),
            Media::Video { .. } => Some("video".into()),
            Media::Gallery { images, .. } if images.is_empty() => Some("gallery".into()),
            Media::Gallery { images, .. } => Some(format!("gallery {}", images.len())),
            Media::Link { url, .. } => Some(domain_of(url).unwrap_or_else(|| "link".into())),
        }
    }

    /// Playback identity, including GIFs and YouTube page URLs resolved by the player.
    pub fn video_url(&self) -> Option<&str> {
        match self {
            Media::Video { url, .. } => url.as_deref(),
            Media::Image { url, .. } if is_gif(url) => Some(url),
            Media::Link { .. } => self.youtube_url(),
            _ => None,
        }
    }

    /// Inline still or video poster. Ordinary link previews stay list thumbnails.
    pub fn preview_image(&self) -> Option<Cow<'_, str>> {
        self.preview_image_at(0)
    }

    pub fn preview_image_at(&self, index: usize) -> Option<Cow<'_, str>> {
        let source = match self {
            Media::Image { url, preview } => self
                .video_url()
                .is_none()
                .then_some(preview.as_deref().unwrap_or(url)),
            Media::Video { poster, .. } => poster.as_deref(),
            Media::Gallery {
                images, previews, ..
            } => {
                let index = index % images.len().max(1);
                images.get(index).map(|original| {
                    previews
                        .get(index)
                        .and_then(Option::as_deref)
                        .unwrap_or(original)
                })
            }
            Media::Link { url, thumb } if self.youtube_url().is_some() => {
                if thumb.is_none() {
                    return youtube_id(url).map(|id| {
                        Cow::Owned(format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg"))
                    });
                }
                thumb.as_deref()
            }
            Media::Link { .. } | Media::None => None,
        };
        source.map(Cow::Borrowed)
    }

    /// Whether the post view shows an image or plays a video under the header.
    pub fn shows_inline(&self) -> bool {
        self.preview_image().is_some() || self.video_url().is_some()
    }

    /// GIF image posts list their first frame (the fetch stops after it).
    pub fn list_thumb(&self) -> Option<Cow<'_, str>> {
        match self {
            Media::Gallery { thumb, .. } => thumb
                .as_deref()
                .map(Cow::Borrowed)
                .or_else(|| self.preview_image()),
            Media::Image { url, preview } => Some(Cow::Borrowed(preview.as_deref().unwrap_or(url))),
            Media::Link { .. } if self.youtube_url().is_some() => self.preview_image(),
            Media::Link { thumb, .. } => thumb.as_deref().map(Cow::Borrowed),
            _ => self.preview_image(),
        }
    }
}

pub(crate) fn youtube_id(url: &str) -> Option<String> {
    if url.len() > 8192 {
        return None;
    }
    let url = url::Url::parse(url).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return None;
    }
    let path: Vec<_> = url
        .path()
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    let id = match (url.host_str()?, path.as_slice()) {
        ("youtu.be" | "www.youtu.be", [id]) => (*id).to_owned(),
        ("youtube.com" | "www.youtube.com" | "m.youtube.com" | "music.youtube.com", ["watch"]) => {
            let mut ids = url.query_pairs().filter(|(key, _)| key == "v");
            let id = ids.next()?.1.into_owned();
            if ids.next().is_some() {
                return None;
            }
            id
        }
        ("youtube.com" | "www.youtube.com" | "m.youtube.com", ["shorts" | "embed", id])
        | ("www.youtube-nocookie.com" | "youtube-nocookie.com", ["embed", id]) => (*id).to_owned(),
        _ => return None,
    };
    (id.len() == 11
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')))
    .then_some(id)
}

/// A `.gif` path, whatever its query or fragment.
fn is_gif(url: &str) -> bool {
    url.split(['?', '#']).next().is_some_and(|path| {
        path.rsplit_once('.')
            .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("gif"))
    })
}

pub fn domain_of(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    Some(host.trim_start_matches("www.").to_string())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Post {
    pub id: String,
    pub subreddit: String,
    pub title: String,
    pub author: String,
    pub flair: Option<String>,
    pub score: Option<i64>,
    pub comments: u64,
    pub created: Option<DateTime<Utc>>,
    /// e.g. `/r/neovim/comments/abc/title/`.
    pub permalink: String,
    pub media: Media,
    /// Width and height of the card's preview, before its pixels are downloaded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_size: Option<(u32, u32)>,
    pub stickied: bool,
    pub nsfw: bool,
    /// Media and body stay hidden until revealed.
    #[serde(default)]
    pub spoiler: bool,
    /// Reddit explicitly marks the crosspost's original as deleted.
    #[serde(default)]
    pub original_deleted: bool,
}

/// Wrap inline `>!spoiler!<` text so the view can mask it. Unicode noncharacters;
/// the parser strips any that arrive in Reddit text.
pub const SPOILER_START: char = '\u{FDD0}';
pub const SPOILER_END: char = '\u{FDD1}';
/// Starts a line of post or comment text that stands for a picture, its URL after it. A
/// noncharacter too, stripped from Reddit text like the spoiler marks.
pub const PICTURE: char = '\u{FDD2}';
pub const TEXT_BOLD: char = '\u{FDD3}';
pub const TEXT_ITALIC: char = '\u{FDD4}';
pub const TEXT_STRIKE: char = '\u{FDD5}';
pub const TEXT_CODE: char = '\u{FDD6}';
pub const TEXT_HEADING: char = '\u{FDD7}';
pub const TEXT_LINK: char = '\u{FDD8}';
pub const TEXT_END: char = '\u{FDD9}';
pub const LINK_TARGET: char = '\u{FDDA}';
pub const LINK_TARGET_END: char = '\u{FDDB}';

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BodyLink {
    pub label: String,
    pub url: String,
}

pub fn without_link_targets(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains(LINK_TARGET) {
        return text.into();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(LINK_TARGET) {
        out.push_str(&rest[..at]);
        let Some(end) = rest[at..].find(LINK_TARGET_END) else {
            return out.into();
        };
        rest = &rest[at + end + LINK_TARGET_END.len_utf8()..];
    }
    out.push_str(rest);
    out.into()
}

pub fn body_links(text: &str, reveal_spoilers: bool) -> Vec<BodyLink> {
    let mut links = Vec::new();
    let mut formats = Vec::new();
    let mut current: Option<(BodyLink, bool)> = None;
    let mut spoiler = false;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            SPOILER_START => {
                spoiler = true;
                if let Some((_, hidden)) = &mut current {
                    *hidden = true;
                }
            }
            SPOILER_END => spoiler = false,
            LINK_TARGET => {
                let url: String = chars
                    .by_ref()
                    .take_while(|c| *c != LINK_TARGET_END)
                    .collect();
                if let Some((link, _)) = &mut current {
                    link.url = url;
                }
            }
            TEXT_LINK => {
                formats.push(c);
                current = Some((
                    BodyLink {
                        label: String::new(),
                        url: String::new(),
                    },
                    spoiler,
                ));
            }
            TEXT_END => {
                if formats.pop() == Some(TEXT_LINK)
                    && let Some((mut link, hidden)) = current.take()
                    && (!hidden || reveal_spoilers)
                    && url::Url::parse(&link.url).is_ok_and(|url| {
                        matches!(url.scheme(), "http" | "https") && url.host_str().is_some()
                    })
                {
                    link.label = if link.label.contains(PICTURE) {
                        "[image]".into()
                    } else {
                        link.label.split_whitespace().collect::<Vec<_>>().join(" ")
                    };
                    if !link.label.is_empty()
                        && !links
                            .iter()
                            .any(|existing: &BodyLink| existing.url == link.url)
                    {
                        links.push(link);
                    }
                }
            }
            _ if text_format(c) => formats.push(c),
            _ => {
                if let Some((link, _)) = &mut current
                    && (!c.is_control() || c.is_whitespace())
                {
                    link.label.push(c);
                }
            }
        }
    }
    links
}

pub fn text_format(c: char) -> bool {
    (TEXT_BOLD..=TEXT_END).contains(&c)
}

pub fn unformatted(text: &str) -> std::borrow::Cow<'_, str> {
    let text = without_link_targets(text);
    if text.contains(text_format) {
        text.chars()
            .filter(|c| !text_format(*c))
            .collect::<String>()
            .into()
    } else {
        text
    }
}
/// Id prefix of the hidden comment that holds a thread's top-level comments.
pub const THREAD_ROOT: &str = "more:";

impl Post {
    pub fn profile_user(&self) -> Option<&str> {
        profile_user(&self.permalink)
    }

    pub fn community(&self) -> Option<&str> {
        self.profile_user()
            .is_none()
            .then_some(self.subreddit.as_str())
    }

    pub fn source_label(&self) -> String {
        self.profile_user().map_or_else(
            || format!("r/{}", self.subreddit),
            |name| format!("u/{name}"),
        )
    }

    pub fn reddit_url(&self) -> String {
        format!("https://www.reddit.com{}", self.permalink)
    }

    pub fn age_at(&self, now: DateTime<Utc>) -> String {
        age_at(self.created, now)
    }
}

/// `created` relative to `now`; empty when unknown.
fn age_at(created: Option<DateTime<Utc>>, now: DateTime<Utc>) -> String {
    let Some(created) = created else {
        return String::new();
    };
    let secs = (now - created).num_seconds().max(0);
    match secs {
        0..60 => "just now".into(),
        60..3_600 => format!("{}m ago", secs / 60),
        3_600..86_400 => format!("{}h ago", secs / 3_600),
        86_400..2_592_000 => format!("{}d ago", secs / 86_400),
        _ => created.format("%b %d '%y").to_string(),
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Listing {
    pub posts: Vec<Post>,
    /// Cursor for the next page (`t3_…`), absent on the last page.
    pub after: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Comment {
    pub id: String,
    pub author: String,
    pub flair: Option<String>,
    pub is_op: bool,
    pub is_mod: bool,
    pub score: Option<i64>,
    /// Absolute creation time, so cached comments age with the clock.
    #[serde(default)]
    pub created: Option<DateTime<Utc>>,
    pub body: String,
    pub replies: Vec<Comment>,
    /// Opaque cursor for replies that Reddit omitted.
    pub more: Option<MoreReplies>,
    /// In-memory response provenance; cached JSON never serializes it.
    #[serde(skip)]
    pub sources: Option<CommentSources>,
}

impl Comment {
    pub fn age_at(&self, now: DateTime<Utc>) -> String {
        age_at(self.created, now)
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CommentSources {
    pub content: crate::cache::Source,
    pub more: crate::cache::Source,
    /// Latest parent-page membership, independent of expansion content freshness.
    pub parent: Option<ParentMembership>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParentMembership {
    pub source: crate::cache::Source,
    pub present: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MoreReplies {
    /// Opaque backend cursor, safe to cache; contains no credentials.
    pub cursor: String,
    pub count: Option<u64>,
}

/// One expansion of a logical parent's children, including its next cursor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplyPage {
    pub comments: Vec<Comment>,
    pub more: Option<MoreReplies>,
    /// IDs whose omission means removal. None means the complete child list;
    /// a partial response must not remove comments outside this set.
    pub refreshed: Option<Vec<String>>,
    /// Relative sibling order, including retained comments still behind the cursor.
    pub order: Vec<String>,
}

impl ReplyPage {
    pub fn complete(comments: Vec<Comment>) -> Self {
        Self {
            order: comments.iter().map(|c| c.id.clone()).collect(),
            comments,
            more: None,
            refreshed: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PostPage {
    pub post: Post,
    /// Full selftext as plain text.
    pub body: String,
    pub comments: Vec<Comment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Sort {
    Best,
    #[default]
    Hot,
    New,
    Top,
    Rising,
    Controversial,
}

impl Sort {
    pub const ALL: [Sort; 6] = [
        Sort::Best,
        Sort::Hot,
        Sort::New,
        Sort::Top,
        Sort::Rising,
        Sort::Controversial,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Sort::Best => "best",
            Sort::Hot => "hot",
            Sort::New => "new",
            Sort::Top => "top",
            Sort::Rising => "rising",
            Sort::Controversial => "controversial",
        }
    }

    pub fn takes_time(self) -> bool {
        matches!(self, Sort::Top | Sort::Controversial)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TimeRange {
    Hour,
    #[default]
    Day,
    Week,
    Month,
    Year,
    All,
}

impl TimeRange {
    pub const ALL: [TimeRange; 6] = [
        TimeRange::Hour,
        TimeRange::Day,
        TimeRange::Week,
        TimeRange::Month,
        TimeRange::Year,
        TimeRange::All,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            TimeRange::Hour => "hour",
            TimeRange::Day => "day",
            TimeRange::Week => "week",
            TimeRange::Month => "month",
            TimeRange::Year => "year",
            TimeRange::All => "all",
        }
    }

    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|t| *t == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn when_a_cached_youtube_link_has_no_reddit_thumbnail_it_should_have_a_public_poster() {
        let media: Media = serde_json::from_value(serde_json::json!({"Link": {
            "url":"https://www.youtube.com/watch?v=n5l_E770Ksw", "thumb":null
        }}))
        .unwrap();

        assert_eq!(
            media.preview_image().as_deref(),
            Some("https://i.ytimg.com/vi/n5l_E770Ksw/hqdefault.jpg")
        );
        assert_eq!(
            media.list_thumb().as_deref(),
            Some("https://i.ytimg.com/vi/n5l_E770Ksw/hqdefault.jpg")
        );
    }

    #[test]
    fn when_a_youtube_link_is_loaded_it_should_have_inline_video_and_poster_semantics() {
        let media = Media::Link {
            url: "https://youtu.be/dQw4w9WgXcQ".into(),
            thumb: Some("https://external-preview.redd.it/poster.jpg".into()),
        };

        assert_eq!(media.video_url(), Some("https://youtu.be/dQw4w9WgXcQ"));
        assert_eq!(media.tag().as_deref(), Some("YouTube"));
        assert_eq!(
            media.preview_image().as_deref(),
            Some("https://external-preview.redd.it/poster.jpg")
        );
        assert!(media.shows_inline());
    }

    #[test]
    fn when_youtube_urls_are_classified_only_supported_video_shapes_should_match() {
        for url in [
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ&feature=share",
            "https://m.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://youtu.be/dQw4w9WgXcQ",
            "https://www.youtube.com/shorts/dQw4w9WgXcQ",
            "https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ",
        ] {
            assert_eq!(youtube_id(url).as_deref(), Some("dQw4w9WgXcQ"), "{url}");
        }
        for url in [
            "",
            "https://youtube.com/playlist?list=foo",
            "https://youtube.com/@channel",
            "https://youtube.com/watch?v=short",
            "https://youtube.com/watch?v=dQw4w9WgXcQ&v=abcdefghijk",
            "https://youtube.com.attacker.test/watch?v=dQw4w9WgXcQ",
            "https://user:secret@youtube.com/watch?v=dQw4w9WgXcQ",
            "https://youtube.com:444/watch?v=dQw4w9WgXcQ",
            "file:///youtube.com/watch?v=dQw4w9WgXcQ",
        ] {
            assert_eq!(youtube_id(url), None, "{url}");
        }
    }

    #[test]
    fn when_a_saved_youtube_link_is_deserialized_it_should_gain_playback_without_rewriting_storage()
    {
        let stored = serde_json::json!({"Link":{"url":"https://www.youtube.com/shorts/dQw4w9WgXcQ","thumb":null}});
        let media: Media = serde_json::from_value(stored.clone()).unwrap();

        assert!(media.shows_inline());
        assert_eq!(serde_json::to_value(media).unwrap(), stored);
    }

    #[test]
    fn card_media_saved_video_without_a_gif_flag_should_still_be_a_video() {
        let actual: Media =
            serde_json::from_str(r#"{"Video":{"url":"https://v.redd.it/a.mp4","poster":null}}"#)
                .unwrap();
        assert_eq!(actual.tag().as_deref(), Some("video"));
    }

    #[test]
    fn when_image_urls_have_gif_extensions_they_should_play_as_video() {
        for (url, expected) in [
            ("https://i.redd.it/a.GIF", Some("https://i.redd.it/a.GIF")),
            (
                "https://i.redd.it/a.gif?width=640&s=x",
                Some("https://i.redd.it/a.gif?width=640&s=x"),
            ),
            (
                "https://i.redd.it/a.gif#t",
                Some("https://i.redd.it/a.gif#t"),
            ),
            ("https://i.redd.it/a.gifv", None),
            ("https://i.redd.it/a.jpeg", None),
            ("https://i.redd.it/gif", None),
            ("", None),
        ] {
            let media = Media::Image {
                url: url.into(),
                preview: None,
            };

            assert_eq!(media.video_url(), expected, "{url:?}");
        }
    }

    #[test]
    fn when_a_gif_post_lists_it_should_have_a_thumbnail_but_no_still_preview() {
        let media = Media::Image {
            url: "https://i.redd.it/a.gif".into(),
            preview: None,
        };

        assert_eq!(
            media.list_thumb().as_deref(),
            Some("https://i.redd.it/a.gif")
        );
        assert_eq!(media.preview_image(), None, "it plays as video instead");
    }

    #[test]
    fn a_link_post_should_list_its_preview_but_not_show_it_as_the_posts_picture() {
        let media = Media::Link {
            url: "https://claude.ai/artifact/x".into(),
            thumb: Some("https://external-preview.redd.it/t.png".into()),
        };

        assert_eq!(
            media.list_thumb().as_deref(),
            Some("https://external-preview.redd.it/t.png")
        );
        assert_eq!(media.preview_image(), None);
        assert!(!media.shows_inline());
    }

    #[test]
    fn when_a_saved_image_has_no_preview_metadata_it_should_keep_its_original() {
        let actual: Media =
            serde_json::from_str(r#"{"Image":{"url":"https://i.redd.it/old.png"}}"#).unwrap();

        assert_eq!(
            actual.preview_image().as_deref(),
            Some("https://i.redd.it/old.png")
        );
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::json!({"Image":{"url":"https://i.redd.it/old.png"}})
        );
    }

    #[test]
    fn when_a_gif_has_a_still_preview_playback_should_keep_the_original() {
        let actual = Media::Image {
            url: "https://i.redd.it/movie.gif".into(),
            preview: Some("https://preview.redd.it/movie.png".into()),
        };

        assert_eq!(actual.video_url(), Some("https://i.redd.it/movie.gif"));
        assert_eq!(actual.preview_image(), None);
        assert_eq!(
            actual.list_thumb().as_deref(),
            Some("https://preview.redd.it/movie.png")
        );
    }

    #[test]
    fn gallery_media_saved_before_thumbnails_still_loads() {
        // Bookmarks in state.json store a post's media in this shape.
        let old: Media = serde_json::from_str(r#"{"Gallery":{"images":[]}}"#).unwrap();
        assert_eq!(
            old,
            Media::Gallery {
                images: Vec::new(),
                previews: Vec::new(),
                thumb: None,
            }
        );

        let new = Media::Gallery {
            images: Vec::new(),
            previews: Vec::new(),
            thumb: Some("https://preview.redd.it/a.jpg".into()),
        };
        let json = serde_json::to_string(&new).unwrap();
        assert_eq!(serde_json::from_str::<Media>(&json).unwrap(), new);
        assert_eq!(
            new.list_thumb().as_deref(),
            Some("https://preview.redd.it/a.jpg")
        );
    }
}
