use super::*;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum FeedQuery {
    Listing {
        sort: Sort,
        time: TimeRange,
    },
    Search {
        query: String,
        sort: crate::reddit::experience::SearchSort,
        time: TimeRange,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedTarget {
    Home,
    Subreddit(String),
    User(String),
}

impl FeedQuery {
    pub const DEFAULT_LISTING: Self = Self::Listing {
        sort: Sort::Hot,
        time: TimeRange::Day,
    };

    pub(super) fn cache_request(&self, target: &FeedTarget, after: Option<&str>) -> Option<String> {
        match target {
            FeedTarget::Home => serde_json::to_string(&("browser-home-v2", self, after)).ok(),
            FeedTarget::User(name) => {
                serde_json::to_string(&("user-submitted", name.to_lowercase(), self, after)).ok()
            }
            FeedTarget::Subreddit(name) => {
                serde_json::to_string(&("feed", name.to_lowercase(), self, after)).ok()
            }
        }
    }

    pub(super) fn fetch(
        &self,
        reddit: &Reddit,
        target: &FeedTarget,
        after: Option<&str>,
        progress: &mut dyn FnMut(ListingEvent<'_>),
    ) -> anyhow::Result<Listing> {
        match (target, self) {
            (FeedTarget::Home, Self::Listing { sort, time }) => {
                reddit.home_progress(*sort, *time, after, progress)
            }
            (FeedTarget::Subreddit(name), Self::Listing { sort, time }) => {
                reddit.listing_progress(name, *sort, *time, after, progress)
            }
            (FeedTarget::Subreddit(name), Self::Search { query, sort, time }) => {
                reddit.search_progress(name, query, *sort, *time, after, progress)
            }
            (FeedTarget::User(name), Self::Listing { sort, time }) => {
                reddit.user_posts_progress(name, *sort, *time, after, progress)
            }
            (FeedTarget::Home | FeedTarget::User(_), Self::Search { .. }) => {
                anyhow::bail!("search runs inside a subreddit")
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FeedError {
    Access(AccessDenied),
    Other(String),
}

impl FeedError {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Access(access) => access.message(),
            Self::Other(message) => message,
        }
    }

    pub fn access(&self) -> Option<AccessDenied> {
        match self {
            Self::Access(AccessDenied::PrivateCommunity) => Some(AccessDenied::PrivateCommunity),
            _ => None,
        }
    }
}

impl std::fmt::Display for FeedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<anyhow::Error> for FeedError {
    fn from(error: anyhow::Error) -> Self {
        match error.downcast_ref::<AccessDenied>() {
            Some(access) => Self::Access(*access),
            None => Self::Other(err_string(error)),
        }
    }
}

impl From<String> for FeedError {
    fn from(message: String) -> Self {
        Self::Other(message)
    }
}

impl From<&str> for FeedError {
    fn from(message: &str) -> Self {
        Self::Other(message.into())
    }
}

#[derive(Debug)]
pub enum Job {
    /// The Search tab's live community or people results; only the latest is wanted.
    Search {
        generation: u64,
        query: crate::reddit::experience::Query,
        after: Option<String>,
    },
    Experience {
        generation: u64,
        request: crate::reddit::experience::Request,
        invalidations: Vec<Invalidation>,
    },
    SortedPost {
        generation: u64,
        permalink: String,
        sort: String,
    },
    Subscriptions {
        generation: u64,
    },
    Membership {
        generation: u64,
        target: MembershipTarget,
        join: bool,
    },
    Feed {
        tab: u64,
        generation: u64,
        target: FeedTarget,
        query: FeedQuery,
        after: Option<String>,
    },
    /// First page of a subreddit for All subs (also seeds its tab).
    Group {
        subreddit: String,
        generation: u64,
    },
    Post {
        generation: u64,
        permalink: String,
    },
    MoreReplies {
        generation: u64,
        comment_id: String,
        cursor: String,
        minimum: Option<Source>,
    },
    Image {
        url: String,
        /// A picture in post or comment text: fetched on the background queue, so a
        /// thread full of them never delays what the reader waits for.
        background: bool,
    },
    VideoFrame {
        url: String,
        background: bool,
    },
    VideoThumb {
        url: String,
    },
    /// Small image for a list row or a subreddit icon.
    Thumb {
        url: String,
    },
    /// A subreddit's icon URL; it rarely changes, so it is cached like images.
    Icon {
        subreddit: String,
    },
    /// A person's avatar and profile summary for their feed's header.
    Profile {
        user: String,
    },
    /// Fetch the post page to learn full media URLs (galleries, HLS video).
    ResolveMedia {
        permalink: String,
    },
    /// Save images to temp files so a desktop viewer can open them.
    DownloadImages {
        /// One download per post at a time.
        permalink: String,
        urls: Vec<String>,
    },
}

#[derive(Debug, Clone)]
pub enum Invalidation {
    Feed {
        target: FeedTarget,
        query: FeedQuery,
        after: Option<String>,
    },
    Post(String),
    Replies(String),
}

impl Invalidation {
    pub(super) fn request(&self) -> Option<String> {
        match self {
            Self::Feed {
                target,
                query,
                after,
            } => query.cache_request(target, after.as_deref()),
            Self::Post(link) => post_cache_request(link),
            Self::Replies(cursor) => serde_json::to_string(&("replies-v3", cursor)).ok(),
        }
    }
}

pub(super) fn post_cache_request(link: &str) -> Option<String> {
    let (path, _) = crate::reddit::experience::thread_location(link).ok()?;
    serde_json::to_string(&("post-v4", crate::reddit::parse::post_id(&path)?)).ok()
}

impl Job {
    /// This job's reply for error `e`; used on panic so whatever waits still gets an answer.
    pub(super) fn failed(&self, e: String) -> Msg {
        match self {
            Job::Experience { generation, .. } => Msg::Experience {
                generation: *generation,
                result: Err(e),
            },
            Job::Search { generation, .. } => Msg::Search {
                generation: *generation,
                result: Err(e),
            },
            Job::SortedPost {
                generation, sort, ..
            } => Msg::SortedPost {
                generation: *generation,
                sort: sort.clone(),
                result: Err(e),
            },
            Job::Subscriptions { generation } => Msg::Subscriptions {
                generation: *generation,
                result: Err(e),
            },
            Job::Membership { generation, .. } => Msg::Membership {
                generation: *generation,
                result: Err(e),
            },
            Job::Feed {
                tab,
                generation,
                after,
                ..
            } => Msg::Feed {
                tab: *tab,
                generation: *generation,
                append: after.is_some(),
                result: Err(e.into()),
            },
            Job::Group {
                subreddit,
                generation,
            } => Msg::Group {
                generation: *generation,
                subreddit: subreddit.clone(),
                result: Err(e.into()),
            },
            Job::Post { generation, .. } => Msg::Post {
                generation: *generation,
                result: Err(e),
            },
            Job::MoreReplies {
                generation,
                comment_id,
                ..
            } => Msg::MoreReplies {
                generation: *generation,
                comment_id: comment_id.clone(),
                result: Err(e),
            },
            Job::Image { url, .. } | Job::VideoFrame { url, .. } => Msg::Image {
                url: url.clone(),
                result: Err(e),
            },
            Job::Thumb { url } | Job::VideoThumb { url } => Msg::Thumb {
                url: url.clone(),
                result: Err(e),
            },
            Job::Icon { subreddit } => Msg::Icon {
                subreddit: subreddit.clone(),
                result: Err(e),
            },
            Job::Profile { user } => Msg::Profile {
                user: user.clone(),
                result: Err(e),
            },
            Job::ResolveMedia { permalink } => Msg::MediaResolved {
                permalink: permalink.clone(),
                result: Err(e),
            },
            Job::DownloadImages { permalink, .. } => Msg::ImagesDownloaded {
                permalink: permalink.clone(),
                result: Err(e),
            },
        }
    }
}
