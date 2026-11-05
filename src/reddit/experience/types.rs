use super::*;

pub(crate) const PAGE_LIMIT: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Query {
    Search {
        text: String,
        kind: ListSearch,
        time: TimeRange,
    },
    User {
        name: String,
        feed: UserFeed,
    },
    Profile(String),
    Inbox(Inbox),
    Communities,
    Community(String),
    Rules(String),
    Moderators(String),
    Wiki {
        subreddit: String,
        page: String,
    },
    Multis,
    Multi {
        owner: String,
        name: String,
    },
    Inspect(Target),
    Thread(String),
}

/// Searches whose results list in the popup; post searches open as feeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListSearch {
    Communities,
    People,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchKind {
    Posts,
    Communities,
    People,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub enum SearchSort {
    #[default]
    Relevance,
    Hot,
    Top,
    New,
    Comments,
}

impl SearchSort {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Relevance => "relevance",
            Self::Hot => "hot",
            Self::Top => "top",
            Self::New => "new",
            Self::Comments => "comments",
        }
    }
    pub fn next(self) -> Self {
        match self {
            Self::Relevance => Self::Hot,
            Self::Hot => Self::Top,
            Self::Top => Self::New,
            Self::New => Self::Comments,
            Self::Comments => Self::Relevance,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserFeed {
    Overview,
    Posts,
    Comments,
    Saved,
    Upvoted,
    Downvoted,
    Hidden,
}

impl UserFeed {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Posts => "Posts",
            Self::Comments => "Comments",
            Self::Saved => "Saved on Reddit",
            Self::Upvoted => "Upvoted posts",
            Self::Downvoted => "Downvoted posts",
            Self::Hidden => "Hidden posts",
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Posts => "submitted",
            Self::Comments => "comments",
            Self::Saved => "saved",
            Self::Upvoted => "upvoted",
            Self::Downvoted => "downvoted",
            Self::Hidden => "hidden",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inbox {
    Unread,
    Replies,
    PostReplies,
    Mentions,
    All,
}

impl Inbox {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Unread => "Unread notifications",
            Self::Replies => "Comment replies",
            Self::PostReplies => "Post replies",
            Self::Mentions => "Mentions",
            Self::All => "Inbox",
        }
    }
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Unread => "unread",
            Self::Replies => "comments",
            Self::PostReplies => "selfreply",
            Self::Mentions => "mentions",
            Self::All => "inbox",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub id: String,
    pub author: String,
    pub subreddit: String,
    pub permalink: String,
}

impl Target {
    pub fn from_post(post: &Post) -> Self {
        Self {
            id: format!("t3_{}", post.id),
            author: post.author.clone(),
            subreddit: post.subreddit.clone(),
            permalink: post.permalink.clone(),
        }
    }
    pub fn comment(&self) -> Option<&str> {
        self.id.strip_prefix("t1_")
    }
    pub fn url(&self) -> Result<String> {
        reddit_url(&self.permalink)
    }

    pub(super) fn inspection_permalink(&self) -> Result<String> {
        fullname(&self.id, true)?;
        if let Some(post) = self.id.strip_prefix("t3_") {
            return Ok(format!("/comments/{post}"));
        }
        let url = Url::parse(&self.url()?)?;
        let post = parse::post_id(url.path()).context("Comment has no post location")?;
        Ok(format!("/comments/{post}"))
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub title: String,
    pub body: String,
    pub target: Option<Target>,
    pub query: Option<Query>,
    pub post: Option<Post>,
    pub icon: crate::model::SubscriptionIcon,
}

#[derive(Debug, Clone)]
pub struct Page {
    pub title: String,
    pub text: String,
    pub entries: Vec<Entry>,
    pub after: Option<String>,
    pub url: String,
}

#[derive(Debug, Clone)]
pub struct Details {
    pub target: Target,
    pub raw_text: String,
    pub saved: bool,
    pub hidden: bool,
    pub vote: i8,
    pub editable: bool,
    pub crosspost: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Request {
    Read { query: Query, after: Option<String> },
    Write(Action),
}

#[derive(Debug)]
pub enum Response {
    Page(Page),
    Details(Details),
    Thread(Box<PostPage>),
    Written { url: Option<String> },
}

impl Query {
    pub(super) fn route(&self) -> Result<(String, Vec<(&'static str, String)>)> {
        let mut params = Vec::new();
        let path = match self {
            Self::Search { text, kind, time } => {
                if text.trim().is_empty() {
                    bail!("Enter a search query");
                }
                params.push(("q", text.clone()));
                params.push(("t", time.as_str().into()));
                match kind {
                    ListSearch::Communities => "subreddits/search".into(),
                    ListSearch::People => "users/search".into(),
                }
            }
            Self::User { name: user, feed } => {
                params.push(("sort", "new".into()));
                format!("user/{}/{}.json", name(user)?, feed.as_str())
            }
            Self::Profile(user) => format!("user/{}/about.json", name(user)?),
            Self::Inbox(kind) => {
                params.push(("mark", "false".into()));
                format!("message/{}.json", kind.as_str())
            }
            Self::Communities => "subreddits/popular.json".into(),
            Self::Community(sub) => format!("r/{}/about.json", name(sub)?),
            Self::Rules(sub) => format!("r/{}/about/rules.json", name(sub)?),
            Self::Moderators(sub) => format!("r/{}/about/moderators.json", name(sub)?),
            Self::Wiki { subreddit, page } => {
                if page.split('/').any(|part| name(part).is_err()) {
                    bail!("Invalid wiki page name");
                }
                format!("r/{}/wiki/{page}.json", name(subreddit)?)
            }
            Self::Multis => "api/multi/mine.json".into(),
            Self::Multi { owner, name: multi } => {
                format!("user/{}/m/{}/hot.json", name(owner)?, name(multi)?)
            }
            Self::Inspect(_) | Self::Thread(_) => bail!("Threads use the comment reader"),
        };
        Ok((path, params))
    }

    pub fn url(&self) -> Result<String> {
        match self {
            Self::Inspect(target) => return target.url(),
            Self::Thread(link) => return reddit_url(link),
            Self::Profile(user) => return reddit_url(&format!("user/{}", name(user)?)),
            Self::Community(sub) => return reddit_url(&format!("r/{}", name(sub)?)),
            Self::Multis => return reddit_url("/"),
            _ => {}
        }
        let (path, params) = self.route()?;
        let path = path.strip_suffix(".json").unwrap_or(&path);
        let mut url = Url::parse(&reddit_url(path)?).unwrap();
        if !params.is_empty() {
            url.query_pairs_mut().extend_pairs(params);
        }
        Ok(url.into())
    }
}
