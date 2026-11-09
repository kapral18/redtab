//! Application state and key actions; rendering is in `ui`, dispatch in `keys`.

pub mod experience;
pub mod jumps;
pub mod search;
pub mod settings;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::layout::{Rect, Size};
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::StatefulProtocol;

use crate::cache::{Freshness, Source};
use crate::config::{Config, is_subreddit_name, normalize_subreddit};
use crate::media;
use crate::model::{
    Comment, CommentSources, Listing, Media, MembershipTarget, MoreReplies, ParentMembership, Post,
    PostPage, ReplyPage, SPOILER_START, Sort, Subscription, THREAD_ROOT, TimeRange,
};
use crate::reddit::{AccessDenied, Health, Reddit, is_community};
use crate::state::{ListView, State};
use crate::video::{self, Player};
use crate::worker::{FeedError, FeedQuery, FeedTarget, ImageMetadata, Job, Msg, Pool};

mod comment_merge;
mod community;
mod connection;
mod external;
mod feed;
mod feed_results;
mod group_results;
mod groups;
mod icons;
mod image_results;
mod images;
mod layout;
mod list_nav;
mod loading;
mod membership;
mod messages;
mod persistence;
mod picker;
mod playback;
mod playback_sync;
mod post_nav;
mod post_results;
mod post_view;
mod settle;
mod snapshots;
mod status;
mod tabs;
mod thread;

use comment_merge::*;
pub use feed::*;
use icons::*;
pub use images::*;
use membership::*;
pub use picker::*;
use playback::*;
pub use post_view::*;
use settle::*;
pub use tabs::*;

const SAVE_INTERVAL: Duration = Duration::from_secs(60);
const STATUS_TTL: Duration = Duration::from_secs(6);
/// How often the shown feed or islands check whether their posts passed `CONTENT_TTL`.
const FEED_POLL: Duration = Duration::from_secs(60);
const PREFETCH_MARGIN: usize = 5;
/// Bound automatic Home fill when filters or missing post details leave pages empty.
const HOME_AUTO_PAGES: usize = 4;
/// Built-in tabs stay before account subscriptions and tabs opened with the picker.
const BOOKMARKS_TAB: usize = 1;
pub const ALL_SUBS_TAB: usize = 2;
pub const SEARCH_TAB: usize = 3;
/// Past this many cached thumbnails, those not on screen are dropped.
const THUMB_CACHE_MAX: usize = 400;
/// Recent decoded card previews; visible images are not part of this offscreen budget.
const CARD_CACHE_MAX: usize = 64;
const CARD_CACHE_BYTES: usize = 64 * 1024 * 1024;
/// Small retained previews keep revisits visible while full pixels return from disk.
const CARD_PREVIEW_MAX: usize = 256;
/// Last frames for recent videos; decoders independently bound each frame's pixels.
const VIDEO_PREVIEW_MAX: usize = 8;
const VIDEO_PLAYER_MAX: usize = 3;
const VIDEO_PREVIEW_BYTES: usize = 32 * 1024 * 1024;
/// A video area that changed size restarts the video once the size holds this long.
const VIDEO_RESIZE_SETTLE: Duration = Duration::from_millis(250);
pub(crate) const NAV_SETTLE: Duration = Duration::from_millis(200);
pub const CARD_MIN_WIDTH: u16 = 54;
/// Pane widths include horizontal margins.
pub const MAIN_MIN_WIDTH: u16 = CARD_MIN_WIDTH + 2;
pub const DETAIL_MIN_WIDTH: u16 = 72;
const SIDEBAR_WIDE: u16 = 30;
const SIDEBAR_NARROW: u16 = 24;

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

pub struct Status {
    pub text: String,
    pub error: bool,
    pub at: Instant,
    /// A cache warning: once a draw shows it, the same text is not repeated.
    pub cache_warning: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    List,
    Post,
}

pub struct App {
    pub keymap: crate::keys::State,
    pub experience: experience::Experience,
    pub search: search::Search,
    jumps: jumps::Jumps,
    collections: VecDeque<experience::CachedCollection>,
    pub config: Config,
    pub settings: settings::Settings,
    pub reddit: Reddit,
    pool: Pool,
    pub auth: crate::auth::Controller,
    replacement_messages: Option<std::sync::mpsc::Receiver<Msg>>,
    pub state: State,
    state_path: PathBuf,
    pub tabs: Vec<Tab>,
    pub active: usize,
    next_id: u64,
    next_generation: u64,
    content_generation_floor: u64,
    pub all_subs: AllSubs,
    community_access: HashMap<String, CommunityAccess>,
    read_redraw: bool,
    membership: Option<MembershipChange>,
    pub post_view: Option<PostView>,
    ancestor_follow: Option<AncestorFollow>,
    /// Posts hidden, reported or deleted here: listings that still carry them leave them out.
    removed_posts: HashSet<String>,
    pub overlay: Option<Overlay>,
    /// Help covers the current dialog without replacing its state.
    pub help_scroll: Option<usize>,
    pub status: Option<Status>,
    clipboard: Option<(media::CopyTask, &'static str)>,
    pub health: Health,
    pause: Option<crate::reddit::Pause>,
    pub loading_frame: usize,
    loading_since: Option<Instant>,
    last_save: Instant,
    feed_poll: Instant,
    pub show_images: bool,
    /// The user's sidebar choice; a hidden sidebar still shows while it has focus.
    pub show_sidebar: bool,
    /// NO_COLOR is set: half-block images, which are only colors, are not drawn.
    pub no_color: bool,
    pub focus: Focus,
    /// Last drawable terminal width, shared by layout and key dispatch.
    width: u16,
    pub image_picker: Picker,
    pub images: HashMap<String, ImageSlot>,
    /// Full-size list previews have their own protocols so split detail can draw the same URL.
    pub card_images: HashMap<String, CardImageSlot>,
    /// Measured geometry and preview classification survive decoded-pixel eviction.
    pub card_image_metadata: HashMap<String, ImageMetadata>,
    card_image_order: VecDeque<String>,
    card_previews: HashMap<String, std::sync::Arc<image::DynamicImage>>,
    pub card_renderer: crate::card_image::Renderer,
    card_preview_order: VecDeque<String>,
    /// Image fetches submitted and not yet answered, and whether each went to the
    /// background queue: wanted again while queued, an image is not fetched twice.
    queued_images: HashMap<String, bool>,
    /// Default thread reads awaiting a terminal reply; returns join their generation.
    pending_posts: HashMap<String, u64>,
    /// List-row thumbnails and subreddit icons, keyed by image URL.
    pub thumbs: HashMap<String, ImageSlot>,
    /// Subreddit icon URLs, keyed by lowercase subreddit name; `None` when it has none.
    icons: HashMap<String, IconMetadata>,
    /// Subreddits whose icon lookup has not answered yet.
    icon_lookups: HashSet<String>,
    icon_demands: HashSet<String>,
    /// People's avatars and profile summaries, keyed by lowercase username.
    profiles: HashMap<String, crate::model::UserProfile>,
    /// Profile lookups on their way or failed; a failed one waits for `r` on its tab.
    profile_lookups: HashMap<String, Lookup>,
    pending_media: HashSet<String>,
    /// Cache warnings a draw has shown; a broken or busy cache repeats them per job.
    cache_warnings: HashSet<String>,
    /// The selected post's inline video while it plays (or loads, or failed).
    video: Option<ActiveVideo>,
    card_video: Option<CardPlayback>,
    video_requested: Option<String>,
    video_previews: VecDeque<VideoPreview>,
    /// Requested sound; a fresh card selection starts muted.
    video_sound_on: bool,
    /// The box the video decodes for; the drawn picture follows each frame's shape inside.
    pub video_area: Option<Rect>,
    /// The video area's new size and when it first appeared; the video restarts once it holds.
    video_resize: Option<(ratatui::layout::Size, Instant)>,
    /// Post loads of list moves beside an open post, by post view generation.
    post_settle: Settle,
    /// Feed loads of tab switches, by tab id.
    feed_settle: Settle,
    /// Thumbnails, icons and pictures in post text that moves bring on screen.
    picture_settle: Settle<()>,
    list_prefetch: Option<(u64, usize)>,
    pub quit: bool,
}

impl App {
    pub fn new(
        mut config: Config,
        pool: Pool,
        reddit: Reddit,
        state_path: PathBuf,
        image_picker: Picker,
    ) -> Self {
        if let Some(account) = reddit.account() {
            config.select_account(Some(&account.name));
            config.show_nsfw = false;
            config.media.video_autoplay = false;
        }
        let (state, warning) = State::load(&state_path);
        let show_images = config.media.image_preview;
        let mut app = Self {
            keymap: crate::keys::State::default(),
            experience: experience::Experience::default(),
            search: search::Search::default(),
            collections: VecDeque::new(),
            settings: settings::Settings::new(crate::config::store::Store::memory(config.clone())),
            config,
            reddit,
            pool,
            auth: crate::auth::Controller::default(),
            replacement_messages: None,
            state,
            state_path,
            tabs: Vec::new(),
            active: 0,
            next_id: 0,
            next_generation: 0,
            content_generation_floor: 0,
            all_subs: AllSubs::default(),
            community_access: HashMap::new(),
            read_redraw: false,
            post_view: None,
            overlay: None,
            help_scroll: None,
            status: None,
            clipboard: None,
            health: Health::Unknown,
            pause: None,
            loading_frame: 0,
            loading_since: None,
            last_save: Instant::now(),
            feed_poll: Instant::now(),
            show_images,
            show_sidebar: true,
            no_color: false,
            focus: Focus::List,
            width: 0,
            image_picker,
            images: HashMap::new(),
            card_images: HashMap::new(),
            card_image_metadata: HashMap::new(),
            card_image_order: VecDeque::new(),
            card_previews: HashMap::new(),
            card_renderer: crate::card_image::Renderer::default(),
            card_preview_order: VecDeque::new(),
            queued_images: HashMap::new(),
            pending_posts: HashMap::new(),
            thumbs: HashMap::new(),
            icons: HashMap::new(),
            icon_lookups: HashSet::new(),
            icon_demands: HashSet::new(),
            profiles: HashMap::new(),
            profile_lookups: HashMap::new(),
            pending_media: HashSet::new(),
            cache_warnings: HashSet::new(),
            video: None,
            card_video: None,
            video_requested: None,
            video_previews: VecDeque::new(),
            video_sound_on: false,
            video_area: None,
            video_resize: None,
            post_settle: Settle::default(),
            ancestor_follow: None,
            removed_posts: HashSet::new(),
            membership: None,
            feed_settle: Settle::default(),
            picture_settle: Settle::default(),
            list_prefetch: None,
            jumps: jumps::Jumps::default(),
            quit: false,
        };
        app.push_tab(TabKind::Home);
        app.push_tab(TabKind::Bookmarks);
        app.push_tab(TabKind::AllSubs);
        app.push_tab(TabKind::Search);
        app.tabs[0].feed.sort = Sort::Best;
        if let Some(w) = warning {
            app.error(w);
        }
        app
    }

    // ----- tabs -------------------------------------------------------------

    // ----- feeds ------------------------------------------------------------

    // ----- post view --------------------------------------------------------

    // ----- focus, layout and sidebar ----------------------------------------

    // ----- pictures ---------------------------------------------------------

    // ----- actions: current post, Reddit ------------------------------------

    // ----- picker -----------------------------------------------------------

    // ----- worker results -----------------------------------------------------
}

#[cfg(test)]
mod tests;
