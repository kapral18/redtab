use super::*;

/// Shared decoded pixels and a small retained preview, prepared by image workers.
#[derive(Debug, Clone)]
pub struct LoadedImage {
    pub image: Arc<DynamicImage>,
    pub preview: Arc<DynamicImage>,
    pub metadata: ImageMetadata,
}

#[derive(Debug, Clone, Copy)]
pub struct ImageMetadata {
    pub size: (u32, u32),
    pub fingerprint: [u8; 32],
}

impl From<DynamicImage> for LoadedImage {
    fn from(image: DynamicImage) -> Self {
        let mut hash = Sha256::new();
        hash.update(image.width().to_le_bytes());
        hash.update(image.height().to_le_bytes());
        hash.update(image.to_rgba8().as_raw());
        let fingerprint = hash.finalize().into();
        let preview = if image.width() > 256 || image.height() > 256 {
            image.thumbnail(256, 256)
        } else {
            image.clone()
        };
        let preview = preview.to_rgba8();
        Self {
            metadata: ImageMetadata {
                size: (image.width(), image.height()),
                fingerprint,
            },
            image: Arc::new(image),
            preview: Arc::new(DynamicImage::ImageRgba8(preview)),
        }
    }
}

pub enum Msg {
    FeedFallback {
        tab: u64,
        generation: u64,
        listing: Listing,
        cursor: String,
        error: String,
    },
    FeedProgress {
        tab: u64,
        generation: u64,
        post: Box<Post>,
        source: Source,
    },
    GroupProgress {
        subreddit: String,
        generation: u64,
        post: Box<Post>,
        source: Source,
    },
    SortedPost {
        generation: u64,
        sort: String,
        result: Result<Box<PostPage>, String>,
    },
    Experience {
        generation: u64,
        result: Result<crate::reddit::experience::Response, String>,
    },
    Search {
        generation: u64,
        result: Result<crate::reddit::experience::Response, String>,
    },
    Membership {
        generation: u64,
        result: Result<(), String>,
    },
    Subscriptions {
        generation: u64,
        result: Result<Vec<Subscription>, String>,
    },
    /// Cached or freshly fetched content, with its original fetch time.
    Snapshot {
        message: Box<Msg>,
        fetched_at: i64,
        refreshing: bool,
        source: Source,
    },
    CacheWarning(String),
    Feed {
        tab: u64,
        generation: u64,
        append: bool,
        result: Result<Listing, FeedError>,
    },
    Group {
        subreddit: String,
        generation: u64,
        result: Result<Listing, FeedError>,
    },
    Post {
        generation: u64,
        result: Result<Box<PostPage>, String>,
    },
    MoreReplies {
        generation: u64,
        comment_id: String,
        result: Result<ReplyPage, String>,
    },
    Image {
        url: String,
        result: Result<LoadedImage, String>,
    },
    Thumb {
        url: String,
        result: Result<DynamicImage, String>,
    },
    Icon {
        subreddit: String,
        result: Result<Option<String>, String>,
    },
    Profile {
        user: String,
        result: Result<crate::model::UserProfile, String>,
    },
    MediaResolved {
        permalink: String,
        result: Result<Media, String>,
    },
    ImagesDownloaded {
        permalink: String,
        result: Result<Vec<PathBuf>, String>,
    },
    /// A queued thumbnail left the screen before its turn.
    ThumbSkipped {
        url: String,
    },
    /// A queued image no longer wanted when its turn came (its post closed, or it was
    /// scrolled far away).
    ImageSkipped {
        url: String,
    },
}
