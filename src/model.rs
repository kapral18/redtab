use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Post media. URLs are Redlib-relative (`/img/…`, `/hls/…`) or absolute for links.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Media {
    #[default]
    None,
    Image {
        url: String,
    },
    Video {
        /// File or HLS playlist; `None` when Redlib has no playable source.
        url: Option<String>,
        poster: Option<String>,
    },
    Gallery {
        images: Vec<String>,
        /// Listing thumbnail; only post pages list the images.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thumb: Option<String>,
    },
    Link {
        url: String,
        thumb: Option<String>,
    },
}

impl Media {
    pub fn tag(&self) -> Option<String> {
        match self {
            Media::None => None,
            Media::Image { .. } if self.video_url().is_some() => Some("gif".into()),
            Media::Image { .. } => Some("image".into()),
            Media::Video { .. } => Some("video".into()),
            Media::Gallery { images, .. } if images.is_empty() => Some("gallery".into()),
            Media::Gallery { images, .. } => Some(format!("gallery {}", images.len())),
            Media::Link { url, .. } => Some(domain_of(url).unwrap_or_else(|| "link".into())),
        }
    }

    /// Playable media, including GIFs stored as images in Redlib pages and bookmarks.
    pub fn video_url(&self) -> Option<&str> {
        match self {
            Media::Video { url, .. } => url.as_deref(),
            Media::Image { url } if is_gif(url) => Some(url),
            _ => None,
        }
    }

    /// The best still image to preview inline. A link's preview picture is not one: shown
    /// under the post it reads as the post's own image (it stays the list thumbnail).
    pub fn preview_image(&self) -> Option<&str> {
        match self {
            Media::Image { url } => self.video_url().is_none().then_some(url),
            Media::Video { poster, .. } => poster.as_deref(),
            Media::Gallery { images, .. } => images.first().map(String::as_str),
            Media::Link { .. } | Media::None => None,
        }
    }

    /// Whether the post view shows an image or plays a video under the header.
    pub fn shows_inline(&self) -> bool {
        self.preview_image().is_some() || self.video_url().is_some()
    }

    /// GIF image posts list their first frame (the fetch stops after it).
    pub fn list_thumb(&self) -> Option<&str> {
        match self {
            Media::Gallery { images, thumb } => {
                thumb.as_deref().or(images.first().map(String::as_str))
            }
            Media::Image { url } => Some(url),
            Media::Link { thumb, .. } => thumb.as_deref(),
            _ => self.preview_image(),
        }
    }
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
    /// Redlib's fetch-time age text; display prefers `created`.
    pub age: String,
    /// e.g. `/r/neovim/comments/abc/title/`.
    pub permalink: String,
    pub media: Media,
    pub stickied: bool,
    pub nsfw: bool,
    /// Media and body stay hidden until revealed.
    #[serde(default)]
    pub spoiler: bool,
}

/// Wrap inline `>!spoiler!<` text so the view can mask it. Unicode noncharacters;
/// the parser strips any that arrive in Reddit text.
pub const SPOILER_START: char = '\u{FDD0}';
pub const SPOILER_END: char = '\u{FDD1}';
/// Starts a line of post or comment text that stands for a picture, its URL after it. A
/// noncharacter too, stripped from Reddit text like the spoiler marks.
pub const PICTURE: char = '\u{FDD2}';

impl Post {
    pub fn reddit_url(&self) -> String {
        format!("https://www.reddit.com{}", self.permalink)
    }

    pub fn age_at(&self, now: DateTime<Utc>) -> String {
        age_at(self.created, &self.age, now)
    }
}

/// `created` relative to `now`, else Redlib's fetch-time text.
fn age_at(created: Option<DateTime<Utc>>, fetched: &str, now: DateTime<Utc>) -> String {
    let Some(created) = created else {
        return fetched.to_owned();
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
    /// Only subreddit listings carry one.
    pub icon: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Comment {
    pub id: String,
    pub author: String,
    pub flair: Option<String>,
    pub is_op: bool,
    pub is_mod: bool,
    pub score: Option<i64>,
    /// Redlib's relative age ("1h ago") at fetch time; display uses `created` when known.
    pub age: String,
    /// Absolute creation time, so cached comments age with the clock.
    #[serde(default)]
    pub created: Option<DateTime<Utc>>,
    pub body: String,
    pub replies: Vec<Comment>,
    /// "More replies" link Redlib renders instead of deeper levels.
    pub more: Option<MoreReplies>,
    /// In-memory response provenance; cached JSON never serializes it.
    #[serde(skip)]
    pub sources: Option<CommentSources>,
}

impl Comment {
    /// Age relative to `now`, so a cached comment ages with the clock.
    pub fn age_at(&self, now: DateTime<Utc>) -> String {
        age_at(self.created, &self.age, now)
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CommentSources {
    pub content: crate::cache::Source,
    pub more: crate::cache::Source,
    /// A direct More contribution, not also supplied by a parent snapshot.
    pub from_expansion: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MoreReplies {
    /// Redlib path of the thread rooted at the parent comment.
    pub path: String,
    pub count: Option<u64>,
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
    #[default]
    Hot,
    New,
    Top,
    Rising,
    Controversial,
}

impl Sort {
    pub const ALL: [Sort; 5] = [
        Sort::Hot,
        Sort::New,
        Sort::Top,
        Sort::Rising,
        Sort::Controversial,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
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
    fn when_image_urls_have_gif_extensions_they_should_play_as_video() {
        for (url, expected) in [
            ("/img/a.GIF", Some("/img/a.GIF")),
            ("/img/a.gif?width=640&s=x", Some("/img/a.gif?width=640&s=x")),
            ("/img/a.gif#t", Some("/img/a.gif#t")),
            ("/img/a.gifv", None),
            ("/img/a.jpeg", None),
            ("/img/gif", None),
            ("", None),
        ] {
            let media = Media::Image { url: url.into() };

            assert_eq!(media.video_url(), expected, "{url:?}");
        }
    }

    #[test]
    fn when_a_gif_post_lists_it_should_have_a_thumbnail_but_no_still_preview() {
        let media = Media::Image {
            url: "/img/a.gif".into(),
        };

        assert_eq!(media.list_thumb(), Some("/img/a.gif"));
        assert_eq!(media.preview_image(), None, "it plays as video instead");
    }

    #[test]
    fn a_link_post_should_list_its_preview_but_not_show_it_as_the_posts_picture() {
        let media = Media::Link {
            url: "https://claude.ai/artifact/x".into(),
            thumb: Some("/preview/external-pre/t.png".into()),
        };

        assert_eq!(media.list_thumb(), Some("/preview/external-pre/t.png"));
        assert_eq!(media.preview_image(), None);
        assert!(!media.shows_inline());
    }

    #[test]
    fn gallery_media_saved_before_thumbnails_still_loads() {
        // Bookmarks in state.json store a post's media in this shape.
        let old: Media = serde_json::from_str(r#"{"Gallery":{"images":[]}}"#).unwrap();
        assert_eq!(
            old,
            Media::Gallery {
                images: Vec::new(),
                thumb: None,
            }
        );

        let new = Media::Gallery {
            images: Vec::new(),
            thumb: Some("/preview/pre/a.jpg".into()),
        };
        let json = serde_json::to_string(&new).unwrap();
        assert_eq!(serde_json::from_str::<Media>(&json).unwrap(), new);
        assert_eq!(new.list_thumb(), Some("/preview/pre/a.jpg"));
    }
}
