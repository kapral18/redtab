use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

mod markup;
mod media;

pub use markup::*;
pub use media::*;

/// A person's avatar and their profile page summary, one entry per non-empty line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserProfile {
    pub icon: Option<String>,
    pub lines: Vec<String>,
}

impl UserProfile {
    pub fn new(icon: Option<String>, text: &str) -> Self {
        Self {
            icon,
            lines: text
                .lines()
                .map(|line| unformatted(line).trim().to_string())
                .filter(|line| !line.is_empty())
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionIcon {
    #[default]
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
    #[serde(default)]
    pub stickied: bool,
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
mod tests;
