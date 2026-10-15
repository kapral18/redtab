use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Media attached to a post, as Redlib renders it. URLs are Redlib-relative
/// (`/img/…`, `/preview/…`, `/hls/…`) or absolute for external links.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Media {
    #[default]
    None,
    Image {
        url: String,
    },
    Video {
        /// Direct video or HLS playlist; empty when Redlib serves no playable source.
        url: Option<String>,
        poster: Option<String>,
    },
    Gallery {
        images: Vec<String>,
        /// Listing preview; listings carry no image list, only this thumbnail.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thumb: Option<String>,
    },
    /// External link post; `thumb` is the preview image when Redlib has one.
    Link {
        url: String,
        thumb: Option<String>,
    },
}

impl Media {
    /// Short tag for list views.
    pub fn tag(&self) -> Option<String> {
        match self {
            Media::None => None,
            Media::Image { .. } => Some("image".into()),
            Media::Video { .. } => Some("video".into()),
            Media::Gallery { images, .. } if images.is_empty() => Some("gallery".into()),
            Media::Gallery { images, .. } => Some(format!("gallery {}", images.len())),
            Media::Link { url, .. } => Some(domain_of(url).unwrap_or_else(|| "link".into())),
        }
    }

    /// The best still image to preview inline.
    pub fn preview_image(&self) -> Option<&str> {
        match self {
            Media::Image { url } => Some(url),
            Media::Video { poster, .. } => poster.as_deref(),
            Media::Gallery { images, .. } => images.first().map(String::as_str),
            Media::Link { thumb, .. } => thumb.as_deref(),
            Media::None => None,
        }
    }

    /// Whether a post view shows something under the header: a still image, or a
    /// video that plays there.
    pub fn shows_inline(&self) -> bool {
        self.preview_image().is_some() || matches!(self, Media::Video { url: Some(_), .. })
    }

    /// The image for a list-row thumbnail.
    pub fn list_thumb(&self) -> Option<&str> {
        match self {
            Media::Gallery { images, thumb } => {
                thumb.as_deref().or(images.first().map(String::as_str))
            }
            _ => self.preview_image(),
        }
    }
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
    /// Redlib's relative age ("3h ago") at fetch time; display uses `created` when known.
    pub age: String,
    /// Path of the comments page, e.g. `/r/neovim/comments/abc/title/`.
    pub permalink: String,
    pub media: Media,
    pub stickied: bool,
    pub nsfw: bool,
    /// Tagged spoiler: its media and body stay hidden until revealed.
    #[serde(default)]
    pub spoiler: bool,
}

/// Inline spoiler text (`>!…!<`) in a body is wrapped in these two characters, so
/// the post view can mask it until you reveal it. Unicode noncharacters: never meant
/// for interchange, and the parser removes any that arrive in Reddit's text.
pub const SPOILER_START: char = '\u{FDD0}';
pub const SPOILER_END: char = '\u{FDD1}';

impl Post {
    pub fn reddit_url(&self) -> String {
        format!("https://www.reddit.com{}", self.permalink)
    }

    /// Age relative to `now`, falling back to Redlib's text when the time is unknown.
    pub fn age_at(&self, now: DateTime<Utc>) -> String {
        let Some(created) = self.created else {
            return self.age.clone();
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
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Listing {
    pub posts: Vec<Post>,
    /// Cursor for the next page (`t3_…`), absent on the last page.
    pub after: Option<String>,
    /// The subreddit's icon; only subreddit listings carry one.
    pub icon: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Comment {
    pub id: String,
    pub author: String,
    pub flair: Option<String>,
    pub is_op: bool,
    pub is_mod: bool,
    pub score: Option<i64>,
    pub age: String,
    pub body: String,
    pub replies: Vec<Comment>,
    /// "More replies" link Redlib renders instead of deeper levels.
    pub more: Option<MoreReplies>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MoreReplies {
    /// Redlib path of the thread rooted at the parent comment.
    pub path: String,
    pub count: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
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
