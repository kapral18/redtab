//! Application state and the actions keys trigger. Rendering lives in `ui`,
//! key dispatch in `keys`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::layout::Rect;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::StatefulProtocol;

use crate::config::{Config, is_subreddit_name, normalize_subreddit};
use crate::media;
use crate::model::{
    Comment, Listing, Media, MoreReplies, Post, PostPage, SPOILER_START, Sort, TimeRange,
};
use crate::redlib::Redlib;
use crate::state::State;
use crate::video::{self, Player};
use crate::worker::{CommandEnd, FeedQuery, Job, Msg, Pool};

const HEALTH_INTERVAL: Duration = Duration::from_secs(30);
const SAVE_INTERVAL: Duration = Duration::from_secs(60);
const STATUS_TTL: Duration = Duration::from_secs(6);
/// A tab's posts count as seen once you stay this long or interact with it.
const SEEN_DWELL: Duration = Duration::from_secs(2);
/// Load the next page when the selection gets this close to the end.
const PREFETCH_MARGIN: usize = 5;
/// Tabs 0 and 1 are always Home and Bookmarks.
const BOOKMARKS_TAB: usize = 1;
/// Past this many cached thumbnails, those not on screen are dropped.
const THUMB_CACHE_MAX: usize = 400;
/// A video area that changed size restarts the video once the size holds this long.
const VIDEO_RESIZE_SETTLE: Duration = Duration::from_millis(250);

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

#[derive(Debug, Default)]
pub struct Feed {
    pub posts: Vec<Post>,
    pub after: Option<String>,
    pub sort: Sort,
    pub time: TimeRange,
    pub search: Option<String>,
    pub loading: bool,
    pub error: Option<String>,
    pub generation: u64,
    pub selected: usize,
    pub loaded: bool,
    /// First visible list item; kept in sync by the UI.
    pub offset: usize,
    /// The query `posts` came from. A failed load of a different query clears them;
    /// a failed reload of the same query keeps them.
    shown_query: Option<FeedQuery>,
    /// The last error came from loading a further page (`L` retries it), not page 1.
    pub page_failed: bool,
}

impl Feed {
    fn query(&self) -> FeedQuery {
        match &self.search {
            Some(q) => FeedQuery::Search { query: q.clone() },
            None => FeedQuery::Listing {
                sort: self.sort,
                time: self.time,
            },
        }
    }

    pub fn is_default_view(&self) -> bool {
        self.search.is_none() && self.sort == Sort::Hot
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabKind {
    Home,
    Bookmarks,
    /// `pinned` tabs come from subscriptions; others were opened with the picker.
    Subreddit {
        name: String,
        pinned: bool,
    },
}

#[derive(Debug)]
pub struct Tab {
    pub id: u64,
    pub kind: TabKind,
    pub feed: Feed,
    /// When the tab became active, and whether you moved or opened anything since.
    entered: Option<Instant>,
    engaged: bool,
}

impl Tab {
    pub fn subreddit(&self) -> Option<&str> {
        match &self.kind {
            TabKind::Subreddit { name, .. } => Some(name),
            _ => None,
        }
    }

    pub fn title(&self) -> String {
        match &self.kind {
            TabKind::Home => "Home".into(),
            TabKind::Bookmarks => "Bookmarks".into(),
            TabKind::Subreddit { name, .. } => format!("r/{name}"),
        }
    }
}

#[derive(Debug, Default)]
pub struct Group {
    pub subreddit: String,
    /// The subreddit's first page (unfiltered).
    pub posts: Vec<Post>,
    pub loading: bool,
    pub error: Option<String>,
}

#[derive(Debug, Default)]
pub struct Home {
    pub groups: Vec<Group>,
    pub group: usize,
    pub post: usize,
    /// First visible row of groups; kept in sync by the UI.
    pub offset: usize,
}

#[derive(Debug)]
pub struct PostView {
    pub post: Post,
    pub page: Option<PostPage>,
    pub loading: bool,
    pub error: Option<String>,
    pub generation: u64,
    pub collapsed: HashSet<String>,
    pub loading_more: HashSet<String>,
    /// Index into [`PostView::rows`].
    pub cursor: usize,
    /// First rendered line of the document.
    pub scroll: usize,
    /// True while you read by scrolling; the next comment key then selects a
    /// comment near the screen's top (see [`Anchor`]).
    pub free_scroll: bool,
    /// The comment `h` left, which `l` returns to while it is on screen.
    left: Option<RowKey>,
    /// A comment `l` selected while reading (or `G` inside the last comment):
    /// draws keep its text where it was while it stays selected and on screen,
    /// instead of showing its header.
    pub hold_view: Option<Hold>,
    /// The row holding the top line, the offset into it and the scroll then, taken
    /// before the document changed above it: the next draw puts the text back, and
    /// keys that scrolled since move on from it (see `take_reading_anchor`).
    keep_reading: Option<(RowKey, isize, usize)>,
    /// The selected row the last draw placed on screen; None while reading.
    pub drawn_cursor: Option<usize>,
    /// `v` while reading: the spoiler state the last draw showed and the scroll
    /// then. The next draw finds the line that was on top in the new wrapping.
    pub rewrapped: Option<(bool, usize)>,
    /// The inline image was scrolled away. It stays away, even when a short post
    /// clamps the scroll back to 0, until a key goes back to the top.
    pub image_away: bool,
    /// The last draw showed the inline image (so the post was at its top).
    pub image_shown: bool,
    /// Document line where each row starts; kept in sync by the UI.
    pub row_starts: Vec<usize>,
    /// Document lines visible in the last draw; pages are this tall.
    pub page_height: usize,
    /// `v` shows spoilers: a spoiler-tagged post's body and image, and `>!…!<` text.
    pub reveal_spoilers: bool,
}

pub enum RowKind<'a> {
    Comment {
        comment: &'a Comment,
        hidden: usize,
    },
    More {
        parent: &'a str,
        more: &'a MoreReplies,
    },
}

pub struct Row<'a> {
    pub depth: usize,
    pub kind: RowKind<'a>,
}

impl PostView {
    /// A spoiler-tagged post whose body and image stay hidden until `v`.
    pub fn spoiler_hidden(&self) -> bool {
        !self.reveal_spoilers
            && (self.post.spoiler || self.page.as_ref().is_some_and(|p| p.post.spoiler))
    }

    /// Anything `v` would reveal: a spoiler tag or `>!…!<` text in the body or comments.
    pub fn has_spoilers(&self) -> bool {
        fn marked(comments: &[Comment]) -> bool {
            comments
                .iter()
                .any(|c| c.body.contains(SPOILER_START) || marked(&c.replies))
        }
        self.post.spoiler
            || self.page.as_ref().is_some_and(|p| {
                p.post.spoiler || p.body.contains(SPOILER_START) || marked(&p.comments)
            })
    }

    /// The comment tree flattened in display order, honoring folded threads. A
    /// hidden spoiler post has none: its comments discuss what the body hides.
    pub fn rows(&self) -> Vec<Row<'_>> {
        let mut rows = Vec::new();
        if let Some(page) = &self.page
            && !self.spoiler_hidden()
        {
            for c in &page.comments {
                self.flatten(c, 0, &mut rows);
            }
        }
        rows
    }

    fn flatten<'a>(&'a self, c: &'a Comment, depth: usize, rows: &mut Vec<Row<'a>>) {
        let folded = self.collapsed.contains(&c.id);
        rows.push(Row {
            depth,
            kind: RowKind::Comment {
                comment: c,
                hidden: if folded { descendants(c) } else { 0 },
            },
        });
        if folded {
            return;
        }
        for r in &c.replies {
            self.flatten(r, depth + 1, rows);
        }
        if let Some(more) = &c.more {
            rows.push(Row {
                depth: depth + 1,
                kind: RowKind::More {
                    parent: &c.id,
                    more,
                },
            });
        }
    }

    /// The comment a key pressed while reading (free scroll) selects; None leaves
    /// the post as it is. Rows are the comment spans from the last draw. No key
    /// moves the view against its direction.
    fn anchor_row(&self, anchor: Anchor) -> Option<usize> {
        let starts = &self.row_starts;
        let bottom = self.scroll + self.page_height.max(1);
        // The comment holding the top line: selecting it moves the view up, if at all.
        let covering_top = || starts.iter().rposition(|&s| s <= self.scroll);
        // The first comment from the top line down: selecting it moves the view down.
        let next_down = || starts.iter().position(|&s| s >= self.scroll);
        match anchor {
            Anchor::Up => covering_top(),
            Anchor::Down => next_down(),
            Anchor::Select => {
                let on_screen = |i: usize| {
                    // A row ends before the blank line under it, which belongs to
                    // the next row (as in `reading_position`).
                    let end = starts.get(i + 1).map_or(usize::MAX, |&s| s - 1);
                    starts
                        .get(i)
                        .is_some_and(|&s| s < bottom && end > self.scroll)
                };
                let left = self.left.as_ref().and_then(|key| self.row_index(key));
                left.filter(|&i| on_screen(i))
                    .or_else(|| next_down().filter(|&i| starts[i] < bottom))
                    .or_else(covering_top)
                    .or_else(next_down)
            }
        }
    }

    /// Leave free scroll by selecting the comment `anchor` picks. Returns true
    /// when it did.
    fn anchor_cursor(&mut self, anchor: Anchor) -> bool {
        // Before the first draw of a loaded post there are no row positions yet:
        // keep the post in free scroll so it still opens at its top. After a fold in
        // the same key batch they belong to other rows: wait for the draw.
        if !self.free_scroll || self.row_starts.is_empty() || !self.positions_current() {
            return false;
        }
        let Some(i) = self.anchor_row(anchor) else {
            return false;
        };
        self.cursor = i;
        self.free_scroll = false;
        self.left = None;
        // Selecting what you read must not move it; the other keys point a way.
        self.hold_view = matches!(anchor, Anchor::Select).then(|| self.hold(i));
        true
    }

    /// Whether `row_starts` from the last draw still describe [`PostView::rows`]:
    /// a fold, a reveal or loaded replies since then change the rows.
    fn positions_current(&self) -> bool {
        self.row_starts.len() == self.rows().len()
    }

    /// Up to the post's top, reading. The inline image there shows again.
    fn go_to_top(&mut self) {
        self.scroll = 0;
        self.free_scroll = true;
        self.image_away = false;
    }

    /// Hold row `i` where the last draw showed it.
    fn hold(&self, i: usize) -> Hold {
        Hold {
            row: i,
            offset: self.scroll as isize - self.row_starts[i] as isize,
            top: false,
        }
    }

    /// Before the document changes above what is being read (replies loading,
    /// spoilers rewrapping): keep the text the last draw showed where it is. Several
    /// changes before one draw keep the first one's anchor: positions are stale
    /// after it.
    fn pin_view(&mut self) {
        // At the top with the inline image shown, the top stays the top: an anchor
        // there would move the scroll off 0 and dismiss the image, so a hold keeps
        // scroll 0 instead of its text, and without one the draw keeps a selected
        // comment in view. (A key in the same pass may have scrolled already: then
        // the reading position counts again.)
        if self.image_shown && self.scroll == 0 {
            if let Some(hold) = &mut self.hold_view {
                hold.top = true;
            }
            return;
        }
        if self.keep_reading.is_none() {
            self.keep_reading = if self.free_scroll {
                self.reading_position()
                    .map(|(key, offset)| (key, offset, self.scroll))
            } else {
                // The selected row the last draw placed: a key may have moved the
                // selection since; the draw then brings the new one into view.
                self.drawn_cursor
                    .filter(|&d| self.positions_current() && d < self.row_starts.len())
                    .map(|d| {
                        let row = RowKey::of(&self.rows()[d]);
                        (
                            row,
                            self.scroll as isize - self.row_starts[d] as isize,
                            self.scroll,
                        )
                    })
            };
        }
        // A selection the last draw placed also holds its text over the next draws.
        if !self.free_scroll
            && self.hold_view.is_none()
            && self.positions_current()
            && self.drawn_cursor == Some(self.cursor)
        {
            self.hold_view = Some(self.hold(self.cursor));
        }
    }

    /// The comment holding the top line and the top line's offset into it, by
    /// the last draw's positions. None in the body (loaded replies land below it),
    /// or when the positions are stale.
    fn reading_position(&self) -> Option<(RowKey, isize)> {
        let rows = self.rows();
        if !self.free_scroll || self.row_starts.len() != rows.len() {
            return None;
        }
        let mut i = self.row_starts.iter().rposition(|&s| s <= self.scroll)?;
        // The blank line under a row belongs to the row after it: a "more replies"
        // row that loads is replaced, so its blank line must not anchor to it.
        if self.row_starts.get(i + 1) == Some(&(self.scroll + 1)) {
            i += 1;
        }
        Some((
            RowKey::of(&rows[i]),
            self.scroll as isize - self.row_starts[i] as isize,
        ))
    }

    /// For the draw: the row and offset to put the top line back at after the
    /// document changed above it, once. Keys that scrolled since (in the same key
    /// batch) move on from the kept text; a jump to the post's top (`g`) stays.
    pub fn take_reading_anchor(&mut self) -> Option<(usize, isize)> {
        let (key, offset, pinned) = self.keep_reading.take()?;
        if self.scroll == 0 && pinned != 0 {
            return None;
        }
        let moved = self.scroll as isize - pinned as isize;
        Some((self.row_index(&key)?, offset + moved))
    }

    /// What the cursor is on.
    fn cursor_key(&self) -> Option<RowKey> {
        self.rows().get(self.cursor).map(RowKey::of)
    }

    fn row_index(&self, key: &RowKey) -> Option<usize> {
        self.rows().iter().position(|r| match (key, &r.kind) {
            (RowKey::Comment(id), RowKind::Comment { comment, .. }) => comment.id == *id,
            (RowKey::More(id), RowKind::More { parent, .. }) => parent == id,
            _ => false,
        })
    }

    /// Put the cursor back on the same row after rows changed above it. A "more
    /// replies" row that just loaded is gone: its index now holds its first new reply.
    fn reselect(&mut self, key: Option<RowKey>) {
        let found = key.and_then(|key| self.row_index(&key));
        let held = self.hold_view.is_some_and(|h| h.row == self.cursor);
        self.cursor = found
            .unwrap_or(self.cursor)
            .min(self.rows().len().saturating_sub(1));
        // The comment `l` selected moved: its hold on the view moves with it.
        if held && let Some(hold) = &mut self.hold_view {
            hold.row = self.cursor;
        }
    }
}

/// See [`PostView::hold_view`].
#[derive(Debug, Clone, Copy)]
pub struct Hold {
    /// The held comment's row.
    pub row: usize,
    /// The top line's distance from the comment's first line when the hold began:
    /// rows loading above move the comment, and the view moves with it.
    pub offset: isize,
    /// Hold the post's top instead (the inline image is shown there).
    pub top: bool,
}

/// A comment row by its id, or a "more replies" row by its parent comment's id.
#[derive(Debug, Clone)]
enum RowKey {
    Comment(String),
    More(String),
}

impl RowKey {
    fn of(row: &Row) -> Self {
        match row.kind {
            RowKind::Comment { comment, .. } => RowKey::Comment(comment.id.clone()),
            RowKind::More { parent, .. } => RowKey::More(parent.to_string()),
        }
    }
}

/// Which comment a key pressed while reading the post selects.
#[derive(Clone, Copy)]
enum Anchor {
    /// `k` `N`: the comment holding the top line.
    Up,
    /// `j` `n`: the first comment from the top line down.
    Down,
    /// `l` `c` Enter: the comment `h` left while it is on screen, else the one
    /// being read: the first starting on screen, the one holding the top line,
    /// or the next one below.
    Select,
}

/// Replies under `c`, loaded or not: a fold hides both, and says how many.
fn descendants(c: &Comment) -> usize {
    let unloaded = c
        .more
        .as_ref()
        .map_or(0, |m| m.count.map_or(1, |n| n as usize));
    unloaded + c.replies.iter().map(|r| 1 + descendants(r)).sum::<usize>()
}

fn find_comment_mut<'a>(comments: &'a mut [Comment], id: &str) -> Option<&'a mut Comment> {
    for c in comments {
        if c.id == id {
            return Some(c);
        }
        if let Some(found) = find_comment_mut(&mut c.replies, id) {
            return Some(found);
        }
    }
    None
}

#[derive(Debug, Default)]
pub struct PickerState {
    pub input: String,
    pub selected: usize,
}

pub enum Overlay {
    /// `scroll` is the first key row shown; the UI clamps it to what fits.
    Help {
        scroll: usize,
    },
    Picker(PickerState),
    Sort {
        selected: usize,
    },
    Search {
        input: String,
    },
}

pub enum ImageSlot {
    Loading,
    Ready(Box<StatefulProtocol>),
    Failed(String),
}

pub struct Status {
    pub text: String,
    pub error: bool,
    pub at: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerEntry {
    Tab { index: usize, title: String },
    Open(String),
}

pub struct App {
    pub config: Config,
    pub redlib: Redlib,
    pool: Pool,
    pub state: State,
    state_path: PathBuf,
    pub tabs: Vec<Tab>,
    pub active: usize,
    next_id: u64,
    next_generation: u64,
    pub home: Home,
    pub post_view: Option<PostView>,
    pub overlay: Option<Overlay>,
    pub status: Option<Status>,
    pub redlib_up: Option<bool>,
    last_health: Option<Instant>,
    last_save: Instant,
    pub show_images: bool,
    pub show_sidebar: bool,
    /// NO_COLOR is set: half-block images, which are only colors, are not drawn.
    pub no_color: bool,
    /// `j`/`k` move through the sidebar's tabs instead of the list.
    pub sidebar_focused: bool,
    /// The sidebar was hidden and is shown only while it has focus.
    sidebar_peek: bool,
    pub image_picker: Picker,
    pub images: HashMap<String, ImageSlot>,
    /// List-row thumbnails and subreddit icons, keyed by image URL.
    pub thumbs: HashMap<String, ImageSlot>,
    /// Subreddit icon URLs, keyed by lowercase subreddit name.
    icons: HashMap<String, String>,
    pending_media: HashSet<String>,
    /// The open post's inline video while it plays (or loads, or failed).
    pub video: Option<Player>,
    /// `a` turned the sound on: later videos start with it too.
    video_sound_on: bool,
    /// Where the last draw put the video picture; it sizes the decoded frames.
    pub video_area: Option<Rect>,
    /// The frame number and cells the last draw sent to the terminal (kitty), so a
    /// frame goes out once.
    pub video_sent: Option<(u64, Rect)>,
    /// The new size of the video area and when the draws first showed it: the
    /// video restarts for it once it holds still.
    video_resize: Option<(ratatui::layout::Size, Instant)>,
    command_running: bool,
    pub quit: bool,
}

impl App {
    pub fn new(
        config: Config,
        subscriptions: Vec<String>,
        pool: Pool,
        redlib: Redlib,
        state_path: PathBuf,
        image_picker: Picker,
    ) -> Self {
        let (state, warning) = State::load(&state_path);
        let show_images = config.image_preview;
        let mut app = Self {
            config,
            redlib,
            pool,
            state,
            state_path,
            tabs: Vec::new(),
            active: 0,
            next_id: 0,
            next_generation: 0,
            home: Home::default(),
            post_view: None,
            overlay: None,
            status: None,
            redlib_up: None,
            last_health: None,
            last_save: Instant::now(),
            show_images,
            show_sidebar: true,
            no_color: false,
            sidebar_focused: false,
            sidebar_peek: false,
            image_picker,
            images: HashMap::new(),
            thumbs: HashMap::new(),
            icons: HashMap::new(),
            pending_media: HashSet::new(),
            video: None,
            video_sound_on: false,
            video_area: None,
            video_sent: None,
            video_resize: None,
            command_running: false,
            quit: false,
        };
        app.push_tab(TabKind::Home);
        app.push_tab(TabKind::Bookmarks);
        for name in &subscriptions {
            app.push_tab(TabKind::Subreddit {
                name: name.clone(),
                pinned: true,
            });
        }
        app.tabs[0].entered = Some(Instant::now());
        app.home.groups = subscriptions
            .iter()
            .map(|s| Group {
                subreddit: s.clone(),
                loading: true,
                ..Group::default()
            })
            .collect();
        app.check_health();
        for name in subscriptions {
            app.pool.submit(Job::Group { subreddit: name });
        }
        if let Some(w) = warning {
            app.error(w);
        } else if app.home.groups.is_empty() {
            app.info("No subscriptions yet: press p to open a subreddit, or list them in the subscriptions file");
        }
        app
    }

    fn push_tab(&mut self, kind: TabKind) -> usize {
        self.next_id += 1;
        self.tabs.push(Tab {
            id: self.next_id,
            kind,
            feed: Feed::default(),
            entered: None,
            engaged: false,
        });
        self.tabs.len() - 1
    }

    fn generation(&mut self) -> u64 {
        self.next_generation += 1;
        self.next_generation
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.status = Some(Status {
            text: text.into(),
            error: false,
            at: Instant::now(),
        });
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.status = Some(Status {
            text: text.into(),
            error: true,
            at: Instant::now(),
        });
    }

    /// Timers: status expiry, health checks, periodic saves, video frames. Returns
    /// true when something visible changed.
    pub fn tick(&mut self) -> bool {
        let mut changed = self.sync_video(Instant::now());
        if self
            .status
            .as_ref()
            .is_some_and(|s| s.at.elapsed() > STATUS_TTL)
        {
            self.status = None;
            changed = true;
        }
        if self
            .last_health
            .is_none_or(|t| t.elapsed() > HEALTH_INTERVAL)
        {
            self.check_health();
        }
        if self.last_save.elapsed() > SAVE_INTERVAL && self.persist() {
            changed = true;
        }
        changed
    }

    /// How long the main loop may wait for input before `tick` has work: the next
    /// video frame, a resized video settling, or the timers' pace.
    pub fn wake_in(&self) -> Duration {
        const IDLE: Duration = Duration::from_millis(100);
        let now = Instant::now();
        let settle = self.video_resize.map(|(_, since)| {
            VIDEO_RESIZE_SETTLE.saturating_sub(now.saturating_duration_since(since))
        });
        self.video
            .as_ref()
            .and_then(|p| p.wake_in(now))
            .into_iter()
            .chain(settle)
            .fold(IDLE, Duration::min)
    }

    /// The stream the open post plays inline: its page has loaded, images are on
    /// and no spoiler hides it.
    fn wanted_video(&self) -> Option<String> {
        let view = self.post_view.as_ref()?;
        let page = view.page.as_ref()?;
        if !self.inline_images_on() || view.spoiler_hidden() {
            return None;
        }
        match &page.post.media {
            Media::Video { url: Some(url), .. } => Some(self.redlib.absolute(url)),
            _ => None,
        }
    }

    /// Start, stop, pause and advance the inline video to match the post view.
    /// It starts once a draw placed its area (and starts again, at the same place,
    /// when the area changes size), and pauses while that area is off screen or
    /// under an overlay. Returns true when something visible changed.
    fn sync_video(&mut self, now: Instant) -> bool {
        let Some(url) = self.wanted_video() else {
            // The next video starts from its own post's draw, not this one's area.
            self.video_area = None;
            self.video_resize = None;
            let changed = self.video.take().is_some();
            self.video_sent = None;
            return changed;
        };
        let mut changed = false;
        if let Some(area) = self.video_area.map(Rect::as_size) {
            let start = match &self.video {
                Some(player) if player.url == url && player.area == area => {
                    self.video_resize = None;
                    false
                }
                // A new size: once it holds still, as a window drag passes many.
                Some(player) if player.url == url => match self.video_resize {
                    Some((size, since)) if size == area => {
                        now.saturating_duration_since(since) >= VIDEO_RESIZE_SETTLE
                    }
                    _ => {
                        self.video_resize = Some((area, now));
                        false
                    }
                },
                _ => true,
            };
            if start {
                // Same video, new size: go on from where it was.
                let resume = match self.video.take() {
                    Some(old) if old.url == url => old.resume(now),
                    _ => video::Resume {
                        sound: self.video_sound_on,
                        ..video::Resume::default()
                    },
                };
                let output = if self.image_picker.protocol_type() == ProtocolType::Kitty {
                    video::Output::Kitty
                } else {
                    video::Output::Image(self.image_picker.clone(), area)
                };
                let font = self.image_picker.font_size();
                self.video = Some(Player::start(url, area, font, output, resume, now));
                self.video_sent = None;
                self.video_resize = None;
                changed = true;
            }
        }
        let shown =
            self.overlay.is_none() && self.post_view.as_ref().is_some_and(|v| v.image_shown);
        if let Some(player) = &mut self.video {
            player.set_hidden(!shown, now);
            changed |= player.tick(now);
            if let Some((text, error)) = player.take_notice() {
                if error {
                    // No ffplay: no later video can play sound either.
                    self.video_sound_on = false;
                    self.error(text);
                } else {
                    self.info(text);
                }
                changed = true;
            }
        }
        changed
    }

    /// `P` in a post: pause or play its video.
    pub fn toggle_video_pause(&mut self) {
        match &mut self.video {
            Some(player) => player.toggle_pause(Instant::now()),
            None => self.info("No video plays here"),
        }
    }

    /// `a` in a post: sound on or off, for this video and the next ones. It turns
    /// the preference over; a video without sound keeps its own off.
    pub fn toggle_video_sound(&mut self) {
        let Some(player) = &mut self.video else {
            self.info("No video plays here");
            return;
        };
        let wanted = !self.video_sound_on;
        if player.sound() != wanted {
            player.toggle_sound(Instant::now());
        }
        self.video_sound_on = wanted;
        match player.take_notice() {
            Some((text, true)) => {
                self.video_sound_on = false;
                self.error(text);
            }
            Some((text, false)) => self.info(format!("Sound on for later videos · {text}")),
            None => self.info(if wanted { "Sound on" } else { "Sound off" }),
        }
    }

    /// Videos start with sound (`a`); a video without sound still plays silent.
    pub fn video_sound_wanted(&self) -> bool {
        self.video_sound_on
    }

    /// `,` and `.` in a post: seek the video back or forward.
    pub fn seek_video(&mut self, forward: bool) {
        let by = if forward {
            video::SEEK_STEP
        } else {
            -video::SEEK_STEP
        };
        match &mut self.video {
            Some(player) if player.error().is_some() => self.info("The video is unavailable"),
            Some(player) if player.ready() => player.seek(by, Instant::now()),
            Some(_) => self.info("The video is still loading"),
            None => self.info("No video plays here"),
        }
    }

    pub fn check_health(&mut self) {
        self.last_health = Some(Instant::now());
        self.pool.submit(Job::Health);
    }

    /// Save state. Returns true when something visible changed: merging another
    /// instance's saves changed the bookmark list (the Bookmarks selection then
    /// follows its post), or saving failed. `save` merges before it writes, so
    /// the list can change even when the write fails.
    fn persist(&mut self) -> bool {
        self.last_save = Instant::now();
        let before = self.bookmark_ids();
        let selected = self.selected_bookmark_id();
        let failed = match self.state.save(&self.state_path, now()) {
            Ok(_) => false,
            Err(e) => {
                self.error(format!("cannot save state: {e:#}"));
                true
            }
        };
        let changed = self.bookmark_ids() != before;
        if changed {
            self.reselect_bookmark(selected);
        }
        changed || failed
    }

    fn bookmark_ids(&self) -> Vec<String> {
        self.state
            .bookmarks()
            .iter()
            .map(|b| b.post.id.clone())
            .collect()
    }

    fn selected_bookmark_id(&self) -> Option<String> {
        let selected = self.tabs[BOOKMARKS_TAB].feed.selected;
        self.state
            .bookmarks()
            .get(selected)
            .map(|b| b.post.id.clone())
    }

    /// After the bookmark list changed: keep the Bookmarks selection on its post,
    /// or clamp it when that post is gone.
    fn reselect_bookmark(&mut self, id: Option<String>) {
        let ids = self.bookmark_ids();
        let feed = &mut self.tabs[BOOKMARKS_TAB].feed;
        feed.selected = id
            .and_then(|id| ids.iter().position(|a| *a == id))
            .unwrap_or(feed.selected)
            .min(ids.len().saturating_sub(1));
    }

    /// Final save on quit.
    pub fn save_state(&mut self) {
        self.mark_active_seen();
        self.persist();
    }

    // ----- tabs -------------------------------------------------------------

    pub fn active_tab(&self) -> &Tab {
        &self.tabs[self.active]
    }

    fn engage(&mut self) {
        self.tabs[self.active].engaged = true;
    }

    pub fn switch_tab(&mut self, index: usize) {
        if index >= self.tabs.len() || index == self.active {
            return;
        }
        self.mark_active_seen();
        self.enter_tab(index);
    }

    /// Make `index` the active tab and start its visit.
    fn enter_tab(&mut self, index: usize) {
        self.active = index;
        let tab = &mut self.tabs[index];
        tab.entered = Some(Instant::now());
        tab.engaged = false;
        self.close_post();
        self.ensure_feed_loaded(index);
    }

    pub fn cycle_tab(&mut self, forward: bool) {
        let n = self.tabs.len();
        let next = if forward {
            (self.active + 1) % n
        } else {
            (self.active + n - 1) % n
        };
        self.switch_tab(next);
    }

    /// Switch to the tab for `name`, opening a new tab when needed.
    pub fn open_subreddit(&mut self, name: &str) {
        let name = normalize_subreddit(name);
        if !is_subreddit_name(name) {
            self.error(format!("{name:?} is not a subreddit name"));
            return;
        }
        let existing = self
            .tabs
            .iter()
            .position(|t| t.subreddit().is_some_and(|s| s.eq_ignore_ascii_case(name)));
        let index = existing.unwrap_or_else(|| {
            self.push_tab(TabKind::Subreddit {
                name: name.to_string(),
                pinned: false,
            })
        });
        self.switch_tab(index);
        self.ensure_feed_loaded(index);
    }

    pub fn close_active_tab(&mut self) {
        match &self.active_tab().kind {
            TabKind::Subreddit { pinned: false, .. } => {
                self.mark_active_seen();
                self.tabs.remove(self.active);
                self.enter_tab(self.active.min(self.tabs.len() - 1));
            }
            _ => self.info("Only tabs opened with p can be closed; subscriptions stay"),
        }
    }

    /// Listed posts count as seen when you leave a subreddit tab you actually
    /// looked at: you moved or opened something, or stayed a moment. Passing
    /// through with Tab keeps its "new" badges.
    fn mark_active_seen(&mut self) {
        let tab = &self.tabs[self.active];
        let looked = tab.engaged || tab.entered.is_some_and(|t| t.elapsed() >= SEEN_DWELL);
        if !looked || !tab.feed.loaded || !tab.feed.is_default_view() {
            return;
        }
        if let Some(name) = tab.subreddit() {
            let name = name.to_string();
            let ids: Vec<String> = tab.feed.posts.iter().map(|p| p.id.clone()).collect();
            self.state
                .mark_seen(&name, ids.iter().map(String::as_str), now());
        }
    }

    pub fn badge(&self, tab: &Tab) -> usize {
        let Some(name) = tab.subreddit() else {
            return 0;
        };
        if tab.feed.loaded && tab.feed.is_default_view() {
            return self.state.unseen(name, self.visible(&tab.feed.posts));
        }
        self.home
            .groups
            .iter()
            .find(|g| g.subreddit.eq_ignore_ascii_case(name))
            .map_or(0, |g| self.state.unseen(name, self.visible(&g.posts)))
    }

    pub fn visible<'a>(&self, posts: &'a [Post]) -> Vec<&'a Post> {
        posts.iter().filter(|p| !self.config.hides(p)).collect()
    }

    // ----- feeds ------------------------------------------------------------

    fn ensure_feed_loaded(&mut self, index: usize) {
        let tab = &self.tabs[index];
        if tab.subreddit().is_some() && !tab.feed.loaded && !tab.feed.loading {
            self.load_feed(index, false);
        }
    }

    pub fn load_feed(&mut self, index: usize, more: bool) {
        let generation = self.generation();
        let tab = &mut self.tabs[index];
        let Some(subreddit) = tab.subreddit().map(str::to_string) else {
            return;
        };
        let feed = &mut tab.feed;
        if more && (feed.loading || feed.after.is_none()) {
            return;
        }
        feed.loading = true;
        feed.error = None;
        feed.generation = generation;
        let after = if more { feed.after.clone() } else { None };
        let job = Job::Feed {
            tab: tab.id,
            generation,
            subreddit,
            query: feed.query(),
            after,
        };
        self.pool.submit(job);
    }

    pub fn reload(&mut self) {
        // Thumbnails that failed (Redlib down, rate limit) get another try.
        self.thumbs
            .retain(|_, slot| !matches!(slot, ImageSlot::Failed(_)));
        if let Some(view) = &self.post_view {
            let post = view.post.clone();
            for url in post.media.preview_image().into_iter() {
                if matches!(self.images.get(url), Some(ImageSlot::Failed(_))) {
                    self.images.remove(url);
                }
            }
            let permalink = post.permalink.clone();
            self.open_post_permalink(post, permalink);
            return;
        }
        match self.active_tab().kind {
            TabKind::Home => {
                for g in &mut self.home.groups {
                    g.loading = true;
                    g.error = None;
                    self.pool.submit(Job::Group {
                        subreddit: g.subreddit.clone(),
                    });
                }
            }
            TabKind::Bookmarks => {}
            TabKind::Subreddit { .. } => {
                self.load_feed(self.active, false);
            }
        }
        self.check_health();
    }

    /// After `S` started Redlib: reload only what failed or never loaded, so the
    /// screen you are reading keeps its place.
    fn reload_failed(&mut self) {
        self.thumbs
            .retain(|_, slot| !matches!(slot, ImageSlot::Failed(_)));
        if let Some(view) = &self.post_view
            && view.error.is_some()
        {
            let post = view.post.clone();
            let permalink = post.permalink.clone();
            self.open_post_permalink(post, permalink);
        }
        for g in &mut self.home.groups {
            if g.error.is_some() || (g.posts.is_empty() && !g.loading) {
                g.loading = true;
                g.error = None;
                self.pool.submit(Job::Group {
                    subreddit: g.subreddit.clone(),
                });
            }
        }
        for i in 0..self.tabs.len() {
            let feed = &self.tabs[i].feed;
            if self.tabs[i].subreddit().is_some() && feed.error.is_some() && !feed.loading {
                // A failed next page is retried as a next page: the pages above stay.
                let more = feed.page_failed;
                self.load_feed(i, more);
            }
        }
    }

    pub fn set_sort(&mut self, sort: Sort) {
        let feed = &mut self.tabs[self.active].feed;
        feed.sort = sort;
        feed.search = None;
        feed.selected = 0;
        self.load_feed(self.active, false);
    }

    pub fn cycle_time(&mut self) {
        let feed = &mut self.tabs[self.active].feed;
        if !feed.sort.takes_time() || feed.search.is_some() {
            self.info("Time range applies to Top and Controversial (press s)");
            return;
        }
        feed.time = feed.time.next();
        feed.selected = 0;
        self.load_feed(self.active, false);
    }

    pub fn search(&mut self, query: String) {
        let query = query.trim().to_string();
        if query.is_empty() {
            return;
        }
        if self.active_tab().subreddit().is_none() {
            self.info("Search runs inside a subreddit tab");
            return;
        }
        let feed = &mut self.tabs[self.active].feed;
        feed.search = Some(query);
        feed.selected = 0;
        self.load_feed(self.active, false);
    }

    pub fn clear_search(&mut self) -> bool {
        let feed = &mut self.tabs[self.active].feed;
        if feed.search.take().is_none() {
            return false;
        }
        feed.selected = 0;
        self.load_feed(self.active, false);
        true
    }

    /// Move the selection in a list view; `delta` may be negative.
    pub fn move_selection(&mut self, delta: isize) {
        self.engage();
        match self.active_tab().kind {
            TabKind::Home => self.move_home(delta),
            TabKind::Bookmarks => {
                let len = self.state.bookmarks().len();
                let feed = &mut self.tabs[self.active].feed;
                feed.selected = step(feed.selected, delta, len);
            }
            TabKind::Subreddit { .. } => {
                let len = self.visible(&self.tabs[self.active].feed.posts).len();
                let feed = &mut self.tabs[self.active].feed;
                feed.selected = step(feed.selected, delta, len);
                // After a failed page load, wait for L or r instead of retrying on every key.
                if feed.selected + PREFETCH_MARGIN >= len
                    && feed.after.is_some()
                    && !feed.loading
                    && feed.error.is_none()
                {
                    self.load_feed(self.active, true);
                }
            }
        }
    }

    pub fn select_edge(&mut self, end: bool) {
        match self.active_tab().kind {
            TabKind::Home => {
                self.home.group = if end {
                    self.home.groups.len().saturating_sub(1)
                } else {
                    0
                };
                self.home.post = 0;
            }
            _ => self.move_selection(if end { isize::MAX / 2 } else { isize::MIN / 2 }),
        }
    }

    /// Posts shown in a Home group: the hot listing without pinned mod posts.
    pub fn home_group_posts<'a>(&self, group: &'a Group) -> Vec<&'a Post> {
        let mut posts: Vec<&Post> = self
            .visible(&group.posts)
            .into_iter()
            .filter(|p| !p.stickied)
            .collect();
        posts.truncate(self.config.home_posts_per_group);
        posts
    }

    fn move_home(&mut self, delta: isize) {
        if self.home.groups.is_empty() {
            return;
        }
        let mut remaining = delta;
        while remaining != 0 {
            let count = self
                .home_group_posts(&self.home.groups[self.home.group])
                .len();
            if remaining > 0 {
                if self.home.post + 1 < count {
                    self.home.post += 1;
                } else if self.home.group + 1 < self.home.groups.len() {
                    self.home.group += 1;
                    self.home.post = 0;
                } else {
                    break;
                }
                remaining -= 1;
            } else {
                if self.home.post > 0 {
                    self.home.post -= 1;
                } else if self.home.group > 0 {
                    self.home.group -= 1;
                    let count = self
                        .home_group_posts(&self.home.groups[self.home.group])
                        .len();
                    self.home.post = count.saturating_sub(1);
                } else {
                    break;
                }
                remaining += 1;
            }
        }
    }

    /// Jump between Home groups; `delta` counts groups.
    pub fn move_home_group(&mut self, delta: isize) {
        self.home.group = step(self.home.group, delta, self.home.groups.len());
        self.home.post = 0;
    }

    pub fn open_home_group_tab(&mut self) {
        if let Some(g) = self.home.groups.get(self.home.group) {
            let name = g.subreddit.clone();
            self.open_subreddit(&name);
        }
    }

    /// The post under the cursor in whatever view is active.
    pub fn current_post(&self) -> Option<Post> {
        if let Some(view) = &self.post_view {
            return Some(
                view.page
                    .as_ref()
                    .map_or_else(|| view.post.clone(), |p| p.post.clone()),
            );
        }
        let tab = self.active_tab();
        match tab.kind {
            TabKind::Home => {
                let group = self.home.groups.get(self.home.group)?;
                self.home_group_posts(group)
                    .get(self.home.post)
                    .map(|p| (*p).clone())
            }
            TabKind::Bookmarks => self
                .state
                .bookmarks()
                .get(tab.feed.selected)
                .map(|b| b.post.clone()),
            TabKind::Subreddit { .. } => self
                .visible(&tab.feed.posts)
                .get(tab.feed.selected)
                .map(|p| (*p).clone()),
        }
    }

    // ----- post view --------------------------------------------------------

    pub fn open_current_post(&mut self) {
        self.engage();
        if let Some(post) = self.current_post() {
            let permalink = post.permalink.clone();
            self.open_post_permalink(post, permalink);
        }
    }

    fn open_post_permalink(&mut self, post: Post, permalink: String) {
        if permalink.is_empty() {
            self.error("This post has no comments link");
            return;
        }
        let generation = self.generation();
        self.state.mark_read(&post, now());
        // The image is queued after the post: the text must not wait behind a download.
        let image = self
            .inline_images_on()
            .then(|| post.media.preview_image().map(str::to_string))
            .flatten();
        self.post_view = Some(PostView {
            post,
            page: None,
            loading: true,
            error: None,
            generation,
            collapsed: HashSet::new(),
            loading_more: HashSet::new(),
            cursor: 0,
            scroll: 0,
            // Start at the top of the post; the first comment key selects a comment.
            free_scroll: true,
            left: None,
            hold_view: None,
            keep_reading: None,
            drawn_cursor: None,
            rewrapped: None,
            image_away: false,
            image_shown: false,
            row_starts: Vec::new(),
            page_height: 0,
            reveal_spoilers: false,
        });
        self.pool.submit(Job::Post {
            generation,
            permalink,
        });
        if let Some(url) = image {
            self.request_image(&url);
        }
    }

    /// `v` in a post: show or hide its spoilers.
    pub fn toggle_spoilers(&mut self) {
        let Some(view) = &mut self.post_view else {
            return;
        };
        if !view.has_spoilers() {
            self.info("No spoilers in this post");
            return;
        }
        // Masking changes how lines wrap: keep the text being read in place. While
        // reading, the draw finds the top line again; a selection holds its text.
        if view.free_scroll {
            let (was, scroll) = (view.reveal_spoilers, view.scroll);
            view.rewrapped.get_or_insert((was, scroll));
        } else {
            view.pin_view();
        }
        view.reveal_spoilers = !view.reveal_spoilers;
        view.row_starts.clear();
        let shown = view.reveal_spoilers;
        if view.spoiler_hidden() {
            // The comments went away with the post: read again from the top. Their
            // positions are stale until the next draw.
            view.go_to_top();
            view.keep_reading = None;
            view.hold_view = None;
            view.rewrapped = None;
        }
        self.info(if shown {
            "Spoilers shown"
        } else {
            "Spoilers hidden"
        });
    }

    pub fn close_post(&mut self) {
        self.post_view = None;
        // Full-size previews are megabytes each: keep them only while their post is open.
        self.images.clear();
        self.pool.forget_images();
    }

    /// `h` in a post: a selected comment first goes back to reading the post.
    /// Returns false when there is no comment selection to leave.
    pub fn leave_comment(&mut self) -> bool {
        match &mut self.post_view {
            Some(view) if !view.free_scroll => {
                view.left = view.cursor_key();
                view.free_scroll = true;
                true
            }
            _ => false,
        }
    }

    pub fn move_comment_cursor(&mut self, delta: isize) {
        let Some(view) = &mut self.post_view else {
            return;
        };
        if view.free_scroll {
            // At the post's top nothing is above: `k` stays. Anchoring would select
            // the first comment, a jump down, so held `k` flipped between the two.
            if delta < 0 && view.scroll == 0 {
                view.go_to_top();
                return;
            }
            // Before the post loads and draws, nothing is known: the post stays at
            // its top; after a fold in the same key batch, the positions are stale.
            // A post without comments reads by line.
            if view.page.is_none() || !view.positions_current() {
                return;
            }
            let anchor = if delta < 0 { Anchor::Up } else { Anchor::Down };
            // With no comment that way, read on by a line, like C-y / C-e.
            if !view.anchor_cursor(anchor) {
                if delta < 0 && view.scroll <= 1 {
                    // Up to the top, as C-y does: the image there shows again.
                    view.go_to_top();
                } else {
                    view.scroll = view.scroll.saturating_add_signed(delta.signum());
                }
            }
            return;
        }
        let len = view.rows().len();
        if delta < 0 && view.cursor == 0 {
            view.go_to_top();
            return;
        }
        view.cursor = step(view.cursor, delta, len);
    }

    pub fn post_top(&mut self) {
        if let Some(view) = &mut self.post_view {
            view.cursor = 0;
            view.go_to_top();
        }
    }

    pub fn post_bottom(&mut self) {
        if let Some(view) = &mut self.post_view {
            let len = view.rows().len();
            // A hidden spoiler post has no comments to select.
            if len > 0 {
                view.cursor = len - 1;
                view.free_scroll = false;
                view.left = None;
                // Already reading inside the last comment: stay there instead of
                // jumping up to its header. Otherwise the draw scrolls down to it.
                // Positions from before a fold or a reveal in the same key batch
                // do not count.
                let inside = view.row_starts.len() == len && view.row_starts[len - 1] < view.scroll;
                view.hold_view = inside.then(|| view.hold(len - 1));
            }
        }
    }

    /// Scroll the post by `delta` lines.
    pub fn scroll_post(&mut self, delta: isize) {
        if let Some(view) = &mut self.post_view {
            if delta < 0 && view.scroll <= delta.unsigned_abs() {
                // Up to the top: the image there shows again.
                view.go_to_top();
            } else {
                view.scroll = view.scroll.saturating_add_signed(delta);
                view.free_scroll = true;
            }
        }
    }

    /// Scroll by `num/den` of the visible document, keeping one line of context on
    /// a full page so nothing is skipped.
    pub fn page_post(&mut self, num: isize, den: isize) {
        let height = self.post_view.as_ref().map_or(0, |v| v.page_height) as isize;
        let step = if num.abs() == den {
            height - 1
        } else {
            height * num.abs() / den
        };
        self.scroll_post(step.max(1) * num.signum());
    }

    pub fn jump_top_level(&mut self, forward: bool) {
        let Some(view) = &mut self.post_view else {
            return;
        };
        let was_free = view.free_scroll;
        // As with `k`: going back from the post's top stays there.
        if was_free && !forward && view.scroll == 0 {
            view.go_to_top();
            return;
        }
        // As with `j`/`k`: before the post loads and draws, or with positions from
        // before a fold in the same key batch, wait for the draw.
        if was_free && (view.page.is_none() || !view.positions_current()) {
            return;
        }
        // Anchoring commits a selection; with no thread that way it is undone.
        let reading = was_free.then(|| (view.cursor, view.left.clone()));
        let anchor = if forward { Anchor::Down } else { Anchor::Up };
        if was_free && !view.anchor_cursor(anchor) {
            // With the top line in the body, the thread before is the post. With
            // the screen inside the last comment, no thread follows.
            if !forward {
                view.go_to_top();
            }
            return;
        }
        let target = {
            let rows = view.rows();
            let mut indexed = rows.iter().enumerate();
            if forward {
                // Right after anchoring, a top-level comment under the cursor is
                // the "next" one: it was not selected before.
                let stay = was_free && rows.get(view.cursor).is_some_and(|r| r.depth == 0);
                let from = if stay { view.cursor } else { view.cursor + 1 };
                indexed
                    .find(|(i, r)| *i >= from && r.depth == 0)
                    .map(|(i, _)| i)
            } else {
                // Right after anchoring below a comment's header, that header is the
                // "previous" one: the thread itself, or the reply's thread.
                let stay = was_free && view.row_starts.get(view.cursor) < Some(&view.scroll);
                let before = if stay { view.cursor + 1 } else { view.cursor };
                indexed
                    .rev()
                    .find(|(i, r)| *i < before && r.depth == 0)
                    .map(|(i, _)| i)
            }
        };
        match (target, reading) {
            (Some(i), _) => view.cursor = i,
            // No thread that way: keep reading. Going up from the first thread, the
            // thread before is the post, as with `k` on the first comment.
            (None, Some((cursor, left))) => {
                view.cursor = cursor;
                view.left = left;
                view.free_scroll = true;
                if !forward {
                    view.go_to_top();
                }
            }
            (None, None) if !forward => view.go_to_top(),
            (None, None) => {}
        }
    }

    /// Fold/unfold the comment under the cursor, or load "more replies". While
    /// reading the post it only selects a comment (see [`Anchor::Select`]).
    pub fn activate_comment(&mut self) {
        enum Action {
            Toggle(String),
            More { parent: String, path: String },
        }
        let Some(view) = &mut self.post_view else {
            return;
        };
        if view.free_scroll {
            view.anchor_cursor(Anchor::Select);
            return;
        }
        let action = {
            let rows = view.rows();
            match rows.get(view.cursor).map(|r| &r.kind) {
                Some(RowKind::Comment { comment, .. })
                    if !comment.replies.is_empty() || comment.more.is_some() =>
                {
                    Some(Action::Toggle(comment.id.clone()))
                }
                Some(RowKind::More { parent, more }) => Some(Action::More {
                    parent: parent.to_string(),
                    path: more.path.clone(),
                }),
                _ => None,
            }
        };
        match action {
            Some(Action::Toggle(id)) => {
                let was_folded = view.collapsed.remove(&id);
                if !was_folded {
                    view.collapsed.insert(id);
                }
                // Equal row counts can still describe different rows and heights.
                view.row_starts.clear();
                // Show the header with its fold chip: the fold's result is there.
                view.hold_view = None;
            }
            Some(Action::More { parent, path }) => {
                let first_request = view.loading_more.insert(parent.clone());
                let generation = view.generation;
                if first_request {
                    self.pool.submit(Job::MoreReplies {
                        generation,
                        comment_id: parent,
                        path,
                    });
                }
            }
            None => {}
        }
    }

    pub fn toggle_images(&mut self) {
        if self.no_images_possible() {
            self.info(if self.config.image_preview {
                "No images: NO_COLOR is set and this terminal has no image protocol"
            } else {
                "No images: NO_COLOR is set; set image_preview = true and restart for a graphics terminal"
            });
            return;
        }
        self.show_images = !self.show_images;
        if self.inline_images_on()
            && let Some(view) = &mut self.post_view
        {
            // Turned on at the post's top: show the image there now. Until the next
            // draw, changes in the same key pass treat the top as showing it.
            let media = view
                .page
                .as_ref()
                .map_or(&view.post.media, |p| &p.post.media);
            let url = media.preview_image().map(str::to_string);
            if view.scroll == 0 {
                view.image_away = false;
                view.image_shown =
                    media.shows_inline() && view.page.is_some() && !view.spoiler_hidden();
            }
            if let Some(url) = url {
                self.request_image(&url);
            }
        }
        self.info(if self.show_images {
            "Images on"
        } else {
            "Images off"
        });
    }

    pub fn toggle_sidebar(&mut self) {
        self.show_sidebar = !self.show_sidebar;
        self.sidebar_focused = false;
        self.sidebar_peek = false;
    }

    /// Tab indexes in sidebar order: Home, Bookmarks, subscriptions, opened tabs.
    pub fn sidebar_tabs(&self) -> (Vec<usize>, Vec<usize>) {
        let of = |pinned_wanted: bool| {
            (0..self.tabs.len())
                .filter(|&i| {
                    matches!(self.tabs[i].kind, TabKind::Subreddit { pinned, .. } if pinned == pinned_wanted)
                })
                .collect()
        };
        (of(true), of(false))
    }

    /// `h` in a list: move focus to the sidebar, showing it while focused if hidden.
    pub fn focus_sidebar(&mut self) {
        if !self.show_sidebar {
            self.show_sidebar = true;
            self.sidebar_peek = true;
        }
        self.sidebar_focused = true;
    }

    pub fn focus_main(&mut self) {
        self.sidebar_focused = false;
        if self.sidebar_peek {
            self.show_sidebar = false;
            self.sidebar_peek = false;
        }
    }

    /// Switch to the tab `delta` rows away in the sidebar, stopping at the ends.
    pub fn move_sidebar(&mut self, delta: isize) {
        let (pinned, opened) = self.sidebar_tabs();
        let order: Vec<usize> = [0, BOOKMARKS_TAB]
            .into_iter()
            .chain(pinned)
            .chain(opened)
            .collect();
        let at = order.iter().position(|&i| i == self.active).unwrap_or(0);
        self.switch_tab(order[step(at, delta, order.len())]);
    }

    pub fn sidebar_edge(&mut self, last: bool) {
        self.move_sidebar(if last { isize::MAX } else { isize::MIN });
    }

    fn request_image(&mut self, url: &str) {
        if matches!(self.images.get(url), None | Some(ImageSlot::Failed(_))) {
            self.images.insert(url.to_string(), ImageSlot::Loading);
            self.pool.want_image(url);
            self.pool.submit(Job::Image {
                url: url.to_string(),
            });
        }
    }

    /// Thumbnails and icons need a graphics protocol: half-blocks at a few cells are noise.
    pub fn thumbnails_on(&self) -> bool {
        self.show_images && self.image_picker.protocol_type() != ProtocolType::Halfblocks
    }

    /// The post view's image: not under NO_COLOR with half-blocks, which are only colors.
    pub fn inline_images_on(&self) -> bool {
        self.show_images && !self.no_images_possible()
    }

    /// NO_COLOR with half-blocks: half-block images are nothing but colors.
    fn no_images_possible(&self) -> bool {
        self.no_color && self.image_picker.protocol_type() == ProtocolType::Halfblocks
    }

    /// A thumbnail or icon URL to draw: not one that failed to load. (Row slots are
    /// reserved by the list either way; a failed image leaves its slot blank.)
    pub fn usable_thumb<'a>(&self, url: Option<&'a str>) -> Option<&'a str> {
        url.filter(|u| !matches!(self.thumbs.get(*u), Some(ImageSlot::Failed(_))))
    }

    /// The icon to draw for a subreddit: thumbnails on, icon known and not failed.
    pub fn shown_icon(&self, subreddit: &str) -> Option<&str> {
        if !self.thumbnails_on() {
            return None;
        }
        // Names are ASCII; most are already lowercase, so skip the allocation then.
        let icon = if subreddit.bytes().any(|b| b.is_ascii_uppercase()) {
            self.icons.get(&subreddit.to_ascii_lowercase())
        } else {
            self.icons.get(subreddit)
        };
        self.usable_thumb(icon.map(String::as_str))
    }

    /// After a frame: load the thumbnails it drew, let queued off-screen ones be skipped,
    /// and bound the cache (past the limit, keep only what is on screen or loading).
    pub fn thumbs_drawn(&mut self, urls: &[&str]) {
        self.pool.set_on_screen(urls);
        for url in urls {
            if !self.thumbs.contains_key(*url) {
                self.thumbs.insert(url.to_string(), ImageSlot::Loading);
                self.pool.submit(Job::Thumb {
                    url: url.to_string(),
                });
            }
        }
        if self.thumbs.len() > THUMB_CACHE_MAX {
            let drawn: HashSet<&str> = urls.iter().copied().collect();
            self.thumbs.retain(|url, slot| {
                matches!(slot, ImageSlot::Loading) || drawn.contains(url.as_str())
            });
        }
    }

    // ----- actions on the current post ------------------------------------

    pub fn open_in_browser(&mut self) {
        let Some(post) = self.current_post() else {
            return;
        };
        let browser = self.config.commands.browser.clone();
        match media::spawn_detached(&browser, &[post.reddit_url()]) {
            Ok(()) => self.info("Opened on reddit.com"),
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    pub fn copy_link(&mut self) {
        let Some(post) = self.current_post() else {
            return;
        };
        match media::copy(&post.reddit_url()) {
            Ok(tool) => self.info(format!("Copied link ({tool})")),
            Err(e) => self.error(format!("copy failed: {e:#}")),
        }
    }

    pub fn toggle_bookmark(&mut self) {
        let Some(post) = self.current_post() else {
            return;
        };
        let selected = self.selected_bookmark_id();
        let saved = self.state.toggle_bookmark(&post, now());
        self.reselect_bookmark(selected);
        self.info(if saved {
            "Bookmarked"
        } else {
            "Bookmark removed"
        });
        // Bookmarks are deliberate; never lose one to a crash or closed terminal.
        // Saved after the message so a save error replaces it.
        self.persist();
    }

    pub fn open_media(&mut self) {
        let Some(post) = self.current_post() else {
            return;
        };
        let media = post.media.clone();
        self.hand_off_media(&post.permalink, media);
    }

    fn hand_off_media(&mut self, permalink: &str, media: Media) {
        let commands = self.config.commands.clone();
        let result = match media {
            Media::None => {
                self.info("This post has no media");
                return;
            }
            Media::Image { url } => {
                self.download_images(permalink, vec![url]);
                return;
            }
            Media::Gallery { images, .. } if !images.is_empty() => {
                self.download_images(permalink, images);
                return;
            }
            Media::Video { url: Some(url), .. } => {
                let started = media::spawn_detached(&commands.video, &[self.redlib.absolute(&url)]);
                // The window plays it now: the inline copy waits, so two do not sound.
                if started.is_ok()
                    && let Some(player) = &mut self.video
                    && !player.paused()
                {
                    player.toggle_pause(Instant::now());
                }
                started
            }
            Media::Link { url, .. } => media::spawn_detached(&commands.browser, &[url]),
            Media::Gallery { .. } | Media::Video { url: None, .. } => {
                if self.pending_media.insert(permalink.to_string()) {
                    self.pool.submit(Job::ResolveMedia {
                        permalink: permalink.to_string(),
                    });
                }
                self.info("Fetching media…");
                return;
            }
        };
        match result {
            Ok(()) => self.info("Opened media"),
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    /// Download images for the viewer once per post; a second `m` while the first
    /// download runs would open the viewer twice.
    fn download_images(&mut self, permalink: &str, urls: Vec<String>) {
        if !self.pending_media.insert(permalink.to_string()) {
            self.info("Already downloading…");
            return;
        }
        self.info(match urls.len() {
            1 => "Downloading image…".to_string(),
            n => format!("Downloading {n} images…"),
        });
        self.pool.submit(Job::DownloadImages {
            permalink: permalink.to_string(),
            urls,
        });
    }

    pub fn start_redlib(&mut self) {
        if self.command_running {
            self.info("Still starting Redlib…");
            return;
        }
        match self.config.redlib_start_command.clone() {
            Some(argv) => {
                self.command_running = true;
                self.info(format!("Running {}…", argv.join(" ")));
                self.pool.submit(Job::RunCommand { argv });
            }
            None => self.info("Set redlib_start_command in config.toml to start Redlib from here"),
        }
    }

    // ----- picker -----------------------------------------------------------

    /// Picker rows for the typed text: tabs whose name starts with it, then
    /// "open r/<text>" for a new subreddit, then tabs that merely contain it,
    /// then fuzzy matches. Row 0 is the default, so a new name is never
    /// hijacked by a longer subscription that happens to contain it.
    pub fn picker_entries(&self, input: &str) -> Vec<PickerEntry> {
        use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
        use nucleo_matcher::{Config as MatcherConfig, Matcher};
        let tab_entry = |(index, t): (usize, &Tab)| PickerEntry::Tab {
            index,
            title: t.title(),
        };
        let query = normalize_subreddit(input);
        if query.is_empty() {
            return self.tabs.iter().enumerate().map(tab_entry).collect();
        }
        let lower = query.to_lowercase();
        let name_of = |t: &Tab| t.title().trim_start_matches("r/").to_lowercase();
        let mut prefix: Vec<(usize, &Tab)> = Vec::new();
        let mut inside: Vec<(usize, &Tab)> = Vec::new();
        for (i, t) in self.tabs.iter().enumerate() {
            let name = name_of(t);
            if name.starts_with(&lower) {
                prefix.push((i, t));
            } else if name.contains(&lower) {
                inside.push((i, t));
            }
        }
        // Exact name first among prefix matches.
        prefix.sort_by_key(|(_, t)| name_of(t) != lower);
        let mut entries: Vec<PickerEntry> = prefix.into_iter().map(tab_entry).collect();
        // Only a subreddit tab with this exact name makes "open r/<query>" redundant;
        // Home and Bookmarks are not subreddits.
        let exact = self
            .tabs
            .iter()
            .any(|t| matches!(t.kind, TabKind::Subreddit { .. }) && name_of(t) == lower);
        if !exact && is_subreddit_name(query) {
            entries.push(PickerEntry::Open(query.to_string()));
        }
        entries.extend(inside.into_iter().map(tab_entry));
        let mut matcher = Matcher::new(MatcherConfig::DEFAULT);
        let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
        let titles: Vec<String> = self.tabs.iter().map(Tab::title).collect();
        for (title, _) in pattern.match_list(titles.iter().map(String::as_str), &mut matcher) {
            if let Some(index) = titles.iter().position(|t| t == title)
                && !entries
                    .iter()
                    .any(|e| matches!(e, PickerEntry::Tab { index: i, .. } if *i == index))
            {
                entries.push(PickerEntry::Tab {
                    index,
                    title: title.to_string(),
                });
            }
        }
        entries
    }

    pub fn picker_accept(&mut self, state: &PickerState) {
        match self
            .picker_entries(&state.input)
            .into_iter()
            .nth(state.selected)
        {
            Some(PickerEntry::Tab { index, .. }) => self.switch_tab(index),
            Some(PickerEntry::Open(name)) => self.open_subreddit(&name),
            None => {}
        }
    }

    // ----- worker results -----------------------------------------------------

    pub fn on_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Feed {
                tab,
                generation,
                append,
                result,
            } => self.on_feed(tab, generation, append, result),
            Msg::Group { subreddit, result } => self.on_group(&subreddit, result),
            Msg::Post { generation, result } => {
                let Some(view) = &mut self.post_view else {
                    return;
                };
                if view.generation != generation {
                    return;
                }
                view.loading = false;
                match result {
                    Ok(page) => {
                        let mut page = *page;
                        // A link post's page has no thumbnail; keep the listing's.
                        if let (
                            Media::Link {
                                url: page_url,
                                thumb: thumb @ None,
                            },
                            Media::Link {
                                url,
                                thumb: Some(listing_thumb),
                            },
                        ) = (&mut page.post.media, &view.post.media)
                            && page_url == url
                        {
                            *thumb = Some(listing_thumb.clone());
                        }
                        let image = page.post.media.preview_image().map(str::to_string);
                        // A bookmark learns a spoiler tag the post page has and the
                        // listing it was saved from may predate.
                        if page.post.spoiler {
                            self.state.mark_bookmark_spoiler(&page.post.id);
                        }
                        view.page = Some(page);
                        view.row_starts.clear();
                        if self.inline_images_on()
                            && let Some(url) = image
                        {
                            self.request_image(&url);
                        }
                    }
                    Err(e) => view.error = Some(e),
                }
            }
            Msg::MoreReplies {
                generation,
                comment_id,
                result,
            } => {
                let Some(view) = &mut self.post_view else {
                    return;
                };
                if view.generation != generation {
                    return;
                }
                view.loading_more.remove(&comment_id);
                match result {
                    Ok(replies) => {
                        // Rows above the cursor may grow: keep the selected comment
                        // selected, and the text being read where it is.
                        let selected = view.cursor_key();
                        view.pin_view();
                        let more_at = view.row_index(&RowKey::More(comment_id.clone()));
                        if let Some(page) = &mut view.page
                            && let Some(c) = find_comment_mut(&mut page.comments, &comment_id)
                        {
                            for reply in replies {
                                if !c.replies.iter().any(|r| r.id == reply.id) {
                                    c.replies.push(reply);
                                }
                            }
                            c.more = None;
                        }
                        view.reselect(selected);
                        view.row_starts.clear();
                        // Reading at this "more replies" row: it is gone, and its index
                        // now holds its first new reply, as in `reselect`.
                        let reading = view.keep_reading.take();
                        view.keep_reading =
                            reading.map(|(key, offset, scroll)| match (&key, more_at) {
                                (RowKey::More(id), Some(i)) if *id == comment_id => {
                                    (view.rows().get(i).map_or(key, RowKey::of), offset, scroll)
                                }
                                _ => (key, offset, scroll),
                            });
                    }
                    Err(e) => self.error(format!("more replies: {e}")),
                }
            }
            Msg::Image { url, result } => {
                // A post closed since the request cleared its entry: drop the image.
                if self.images.contains_key(&url) {
                    let slot = match result {
                        Ok(img) => {
                            ImageSlot::Ready(Box::new(self.image_picker.new_resize_protocol(img)))
                        }
                        Err(e) => ImageSlot::Failed(e),
                    };
                    self.images.insert(url, slot);
                }
            }
            // Its post closed: nothing waits for it.
            Msg::ImageSkipped => {}
            Msg::Thumb { url, result } => {
                let slot = match result {
                    Ok(img) => {
                        ImageSlot::Ready(Box::new(self.image_picker.new_resize_protocol(img)))
                    }
                    Err(e) => ImageSlot::Failed(e),
                };
                self.thumbs.insert(url, slot);
            }
            // Forget it, so the row loads it again if it scrolls back into view.
            Msg::ThumbSkipped { url } => {
                self.thumbs.remove(&url);
            }
            Msg::MediaResolved { permalink, result } => {
                if !self.pending_media.remove(&permalink) {
                    return;
                }
                match result {
                    Ok(Media::Gallery { images, .. }) if images.is_empty() => {
                        self.error("Gallery has no images Redlib can show")
                    }
                    Ok(Media::Video { url: None, .. }) => {
                        self.error("Redlib has no playable source for this video")
                    }
                    Ok(media) => self.hand_off_media(&permalink, media),
                    Err(e) => self.error(format!("media: {e}")),
                }
            }
            Msg::ImagesDownloaded { permalink, result } => match result {
                Ok(paths) => {
                    self.pending_media.remove(&permalink);
                    let args: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
                    match media::spawn_detached(&self.config.commands.image, &args) {
                        Ok(()) => self.info("Opened media"),
                        Err(e) => self.error(format!("{e:#}")),
                    }
                }
                Err(e) => {
                    self.pending_media.remove(&permalink);
                    self.error(format!("image download: {e}"))
                }
            },
            Msg::Health(up) => {
                let was = self.redlib_up;
                self.redlib_up = Some(up);
                if was == Some(false) && up {
                    self.info("Redlib is back; press r to reload");
                }
            }
            Msg::CommandDone { argv, result } => {
                self.command_running = false;
                match result {
                    Ok(end) => {
                        let done = match end {
                            CommandEnd::Exited => "finished",
                            CommandEnd::Running => "is running",
                        };
                        self.info(format!("{} {done}; reloading what failed", argv.join(" ")));
                        // Unknown until the check answers, so it does not report
                        // "Redlib is back; press r" for what was just reloaded.
                        self.redlib_up = None;
                        self.check_health();
                        self.reload_failed();
                    }
                    Err(e) => self.error(e),
                }
            }
        }
    }

    fn on_feed(
        &mut self,
        tab_id: u64,
        generation: u64,
        append: bool,
        result: Result<Listing, String>,
    ) {
        let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) else {
            return;
        };
        let feed = &mut tab.feed;
        if feed.generation != generation {
            return;
        }
        feed.loading = false;
        match result {
            Ok(listing) => {
                if append {
                    let known: HashSet<String> = feed.posts.iter().map(|p| p.id.clone()).collect();
                    feed.posts
                        .extend(listing.posts.into_iter().filter(|p| !known.contains(&p.id)));
                } else {
                    feed.posts = listing.posts;
                    feed.selected = 0;
                    feed.offset = 0;
                }
                feed.after = listing.after;
                feed.loaded = true;
                feed.shown_query = Some(feed.query());
                let baseline = feed.is_default_view().then(|| feed.posts.clone());
                if let (Some(name), Some(posts)) = (tab.subreddit().map(str::to_string), baseline) {
                    self.state.baseline(&name, &posts, now());
                }
                if let (Some(name), Some(icon)) = (tab.subreddit(), listing.icon) {
                    self.icons.insert(name.to_lowercase(), icon);
                }
            }
            Err(e) => {
                feed.error = Some(e.clone());
                feed.page_failed = append;
                // A failed load of a new sort, time range or search must not leave the
                // previous query's posts and cursor behind: `L` would append the new
                // query's next page to them. A failed reload of the same query keeps them.
                if !append && feed.shown_query.as_ref() != Some(&feed.query()) {
                    feed.posts.clear();
                    feed.after = None;
                    feed.selected = 0;
                    feed.offset = 0;
                    feed.shown_query = None;
                }
                let title = tab.title();
                self.error(format!("{title}: {e}"));
            }
        }
    }

    fn on_group(&mut self, subreddit: &str, result: Result<Listing, String>) {
        let Some(index) = self
            .home
            .groups
            .iter()
            .position(|g| g.subreddit == subreddit)
        else {
            return;
        };
        let group = &mut self.home.groups[index];
        group.loading = false;
        match result {
            Ok(listing) => {
                group.posts = listing.posts.clone();
                group.error = None;
                if let Some(icon) = &listing.icon {
                    self.icons.insert(subreddit.to_lowercase(), icon.clone());
                }
                self.state.baseline(subreddit, &listing.posts, now());
                // A refresh can return fewer posts: keep the selection on one.
                if index == self.home.group {
                    let shown = self.home_group_posts(&self.home.groups[index]).len();
                    self.home.post = self.home.post.min(shown.saturating_sub(1));
                }
                // Keep the subreddit's tab in step (instant to open, current
                // badge), unless you are on it or browsing another view of it.
                let active_id = self.tabs[self.active].id;
                if let Some(tab) = self
                    .tabs
                    .iter_mut()
                    .find(|t| t.subreddit().is_some_and(|s| s == subreddit))
                    && tab.id != active_id
                    && !tab.feed.loading
                    && tab.feed.is_default_view()
                {
                    tab.feed.posts = listing.posts;
                    tab.feed.after = listing.after;
                    tab.feed.loaded = true;
                    tab.feed.shown_query = Some(tab.feed.query());
                    tab.feed.selected = 0;
                    tab.feed.offset = 0;
                    tab.feed.error = None;
                }
            }
            Err(e) => group.error = Some(e),
        }
    }
}

/// Move `current` by `delta` within `0..len`, clamping at the ends.
fn step(current: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    current.saturating_add_signed(delta).min(len - 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Media;

    fn app_with(subscriptions: &[&str]) -> App {
        app_with_messages(subscriptions).0
    }

    /// An app plus the replies its jobs produce. Port 9 is discard: nothing answers,
    /// so every network job fails quickly and no test touches the network.
    fn app_with_messages(subscriptions: &[&str]) -> (App, std::sync::mpsc::Receiver<Msg>) {
        let redlib = Redlib::new("http://127.0.0.1:9", false);
        let (pool, messages) = Pool::start(redlib.clone(), 3);
        let state = std::env::temp_dir().join(format!(
            "redtab-app-test-{}-{:?}.json",
            std::process::id(),
            std::thread::current().id()
        ));
        let app = App::new(
            Config::default(),
            subscriptions.iter().map(|s| s.to_string()).collect(),
            pool,
            redlib,
            state,
            Picker::halfblocks(),
        );
        (app, messages)
    }

    fn post(id: &str, sub: &str) -> Post {
        Post {
            id: id.into(),
            subreddit: sub.into(),
            title: format!("title {id}"),
            author: "a".into(),
            flair: None,
            score: Some(1),
            comments: 0,
            created: None,
            age: String::new(),
            permalink: format!("/r/{sub}/comments/{id}/x/"),
            media: Media::None,
            stickied: false,
            nsfw: false,
            spoiler: false,
        }
    }

    fn listing(sub: &str, ids: &[&str], after: Option<&str>) -> Listing {
        Listing {
            posts: ids.iter().map(|i| post(i, sub)).collect(),
            after: after.map(str::to_string),
            icon: None,
        }
    }

    fn comment(id: &str, replies: Vec<Comment>, more: Option<&str>) -> Comment {
        Comment {
            id: id.into(),
            author: "a".into(),
            flair: None,
            is_op: false,
            is_mod: false,
            score: None,
            age: String::new(),
            body: id.into(),
            replies,
            more: more.map(|p| MoreReplies {
                path: p.into(),
                count: Some(1),
            }),
        }
    }

    /// Remove a test's state file and the lock file saving leaves beside it.
    fn remove_state(path: &std::path::Path) {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(path.with_extension("json.lock"));
    }

    fn tab_index(app: &App, name: &str) -> usize {
        app.tabs
            .iter()
            .position(|t| t.subreddit() == Some(name))
            .unwrap()
    }

    #[test]
    fn opening_a_bookmarked_post_records_its_spoiler_tag_only() {
        let mut app = app_with(&[]);
        let mut listed = post("p", "s");
        listed.media = Media::Gallery {
            images: vec![],
            thumb: Some("/preview/small.jpg".into()),
        };
        app.state.toggle_bookmark(&listed, 1);
        app.open_post_permalink(listed.clone(), "/r/s/comments/p/x/".into());
        let generation = app.post_view.as_ref().unwrap().generation;
        let mut page_post = post("p", "s");
        page_post.spoiler = true;
        page_post.media = Media::Gallery {
            images: vec!["/img/full.jpg".into()],
            thumb: None,
        };

        app.on_msg(Msg::Post {
            generation,
            result: Ok(Box::new(PostPage {
                post: page_post,
                body: String::new(),
                comments: vec![],
            })),
        });

        let stored = &app.state.bookmarks()[0];
        assert!(stored.post.spoiler);
        assert_eq!(
            stored.post.media, listed.media,
            "the small listing thumbnail stays"
        );
        assert_eq!(stored.saved_at, 1);
    }

    fn entry_title(app: &App, entry: &PickerEntry) -> Option<String> {
        match entry {
            PickerEntry::Tab { index, .. } => Some(app.tabs[*index].title()),
            PickerEntry::Open(_) => None,
        }
    }

    #[test]
    fn picker_prefers_prefix_matches_exact_first() {
        let app = app_with(&["neovim", "sidehustle", "vim"]);

        assert_eq!(
            entry_title(&app, &app.picker_entries("neov")[0]).as_deref(),
            Some("r/neovim")
        );
        let vim = app.picker_entries("vim");
        assert_eq!(entry_title(&app, &vim[0]).as_deref(), Some("r/vim"));
        assert!(!vim.contains(&PickerEntry::Open("vim".into())));
    }

    #[test]
    fn picker_offers_a_new_subreddit_before_containing_and_fuzzy_matches() {
        let app = app_with(&["learnrust", "sidehustle"]);

        let rust = app.picker_entries("r/rust");

        assert_eq!(rust[0], PickerEntry::Open("rust".into()));
        assert_eq!(entry_title(&app, &rust[1]).as_deref(), Some("r/learnrust"));
        assert!(
            rust.iter()
                .any(|e| entry_title(&app, e).as_deref() == Some("r/sidehustle"))
        );
    }

    #[test]
    fn picker_never_offers_an_invalid_name() {
        let app = app_with(&[]);

        assert!(
            !app.picker_entries("no spaces")
                .iter()
                .any(|e| matches!(e, PickerEntry::Open(_)))
        );
    }

    #[test]
    fn stale_feed_results_are_dropped_and_pages_append_without_duplicates() {
        let mut app = app_with(&["vim"]);
        let i = tab_index(&app, "vim");
        app.switch_tab(i);
        let id = app.tabs[i].id;
        let stale = app.tabs[i].feed.generation;
        app.load_feed(i, false);
        let current = app.tabs[i].feed.generation;

        app.on_msg(Msg::Feed {
            tab: id,
            generation: stale,
            append: false,
            result: Ok(listing("vim", &["old"], None)),
        });
        assert!(app.tabs[i].feed.posts.is_empty());

        app.on_msg(Msg::Feed {
            tab: id,
            generation: current,
            append: false,
            result: Ok(listing("vim", &["a", "b"], Some("t3_b"))),
        });
        app.load_feed(i, true);
        let next = app.tabs[i].feed.generation;
        app.on_msg(Msg::Feed {
            tab: id,
            generation: next,
            append: true,
            result: Ok(listing("vim", &["b", "c"], None)),
        });

        let ids: Vec<&str> = app.tabs[i]
            .feed
            .posts
            .iter()
            .map(|p| p.id.as_str())
            .collect();
        assert_eq!(ids, ["a", "b", "c"]);
        assert_eq!(app.tabs[i].feed.after, None);
    }

    #[test]
    fn a_failed_load_is_reported_and_stops_automatic_paging() {
        let mut app = app_with(&["vim"]);
        let i = tab_index(&app, "vim");
        app.switch_tab(i);
        let id = app.tabs[i].id;
        let g = app.tabs[i].feed.generation;
        app.on_msg(Msg::Feed {
            tab: id,
            generation: g,
            append: false,
            result: Ok(listing("vim", &["a", "b"], Some("t3_b"))),
        });
        app.load_feed(i, true);
        let g = app.tabs[i].feed.generation;
        app.on_msg(Msg::Feed {
            tab: id,
            generation: g,
            append: true,
            result: Err("timeout".into()),
        });

        assert!(
            app.status
                .as_ref()
                .is_some_and(|s| s.error && s.text.contains("timeout"))
        );
        app.move_selection(1);
        assert!(
            !app.tabs[i].feed.loading,
            "must not retry on every key after a failure"
        );
    }

    #[test]
    fn after_s_a_failed_next_page_is_retried_as_a_next_page() {
        let (mut app, messages) = app_with_messages(&["vim"]);
        let i = tab_index(&app, "vim");
        app.switch_tab(i);
        let id = app.tabs[i].id;
        let g = app.tabs[i].feed.generation;
        app.on_msg(Msg::Feed {
            tab: id,
            generation: g,
            append: false,
            result: Ok(listing("vim", &["a", "b"], Some("t3_b"))),
        });
        app.load_feed(i, true);
        let g = app.tabs[i].feed.generation;
        app.on_msg(Msg::Feed {
            tab: id,
            generation: g,
            append: true,
            result: Err("timeout".into()),
        });

        app.on_msg(Msg::CommandDone {
            argv: vec!["start".into()],
            result: Ok(crate::worker::CommandEnd::Exited),
        });
        let retry = app.tabs[i].feed.generation;
        let deadline = Instant::now() + Duration::from_secs(10);
        let append = loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match messages.recv_timeout(left).expect("the retry's reply") {
                Msg::Feed {
                    generation, append, ..
                } if generation == retry => break append,
                _ => {}
            }
        };

        assert!(append, "the retry asks for the next page, not page 1");
        app.on_msg(Msg::Feed {
            tab: id,
            generation: retry,
            append,
            result: Ok(listing("vim", &["c"], None)),
        });
        let ids: Vec<&str> = app.tabs[i]
            .feed
            .posts
            .iter()
            .map(|p| p.id.as_str())
            .collect();
        assert_eq!(ids, ["a", "b", "c"], "the next page joins the loaded one");
    }

    /// A failed load keeps the posts only when it asked for the same listing:
    /// a reload keeps the reader's place, a new sort shows nothing to page onto.
    #[test]
    fn a_failed_load_keeps_the_posts_only_for_the_same_query() {
        let fail_after = |change: fn(&mut App)| {
            let mut app = app_with(&["vim"]);
            let i = tab_index(&app, "vim");
            app.switch_tab(i);
            let id = app.tabs[i].id;
            let g = app.tabs[i].feed.generation;
            app.on_msg(Msg::Feed {
                tab: id,
                generation: g,
                append: false,
                result: Ok(listing("vim", &["h1", "h2"], Some("t3_h2"))),
            });
            app.move_selection(1);
            change(&mut app);
            let g = app.tabs[i].feed.generation;
            app.on_msg(Msg::Feed {
                tab: id,
                generation: g,
                append: false,
                result: Err("timeout".into()),
            });
            app
        };

        let reloaded = fail_after(App::reload);
        let resorted = fail_after(|app| app.set_sort(Sort::New));
        let (reloaded, resorted) = (
            &reloaded.tabs[reloaded.active].feed,
            &resorted.tabs[resorted.active].feed,
        );

        assert_eq!(reloaded.posts.len(), 2);
        assert_eq!(reloaded.selected, 1, "the reader keeps their place");
        assert_eq!(reloaded.after.as_deref(), Some("t3_h2"));
        assert!(
            resorted.posts.is_empty(),
            "hot posts must not show under New"
        );
        assert_eq!(
            resorted.after, None,
            "L must not page New from a hot cursor"
        );
        assert_eq!(resorted.error.as_deref(), Some("timeout"));
    }

    #[test]
    fn removing_a_bookmark_from_another_tab_keeps_the_bookmarks_selection_usable() {
        let mut app = app_with(&["s"]);
        let _ = std::fs::remove_file(&app.state_path);
        for id in ["a", "b", "c"] {
            app.state.toggle_bookmark(&post(id, "s"), 1);
        }
        // Newest first: c, b, a. Select the last row, "a".
        app.tabs[BOOKMARKS_TAB].feed.selected = 2;
        let i = tab_index(&app, "s");
        app.switch_tab(i);
        let id = app.tabs[i].id;
        let g = app.tabs[i].feed.generation;
        app.on_msg(Msg::Feed {
            tab: id,
            generation: g,
            append: false,
            result: Ok(listing("s", &["b"], None)),
        });

        app.toggle_bookmark(); // removes "b" from the subreddit tab
        app.switch_tab(BOOKMARKS_TAB);
        let current = app.current_post().map(|p| p.id);
        remove_state(&app.state_path);

        assert_eq!(current.as_deref(), Some("a"));
    }

    #[test]
    fn more_replies_arriving_keep_the_selected_comment_selected() {
        let mut app = app_with(&[]);
        app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
        let generation = app.post_view.as_ref().unwrap().generation;
        app.on_msg(Msg::Post {
            generation,
            result: Ok(Box::new(PostPage {
                post: post("p", "s"),
                body: String::new(),
                comments: vec![
                    comment("c1", vec![], Some("/r/s/comments/p/x/c1")),
                    comment("c2", vec![], None),
                    comment("c3", vec![], None),
                ],
            })),
        });
        let view = app.post_view.as_mut().unwrap();
        view.free_scroll = false;
        view.cursor = 3; // rows: c1, more, c2, c3

        app.on_msg(Msg::MoreReplies {
            generation,
            comment_id: "c1".into(),
            result: Ok(vec![
                comment("r1", vec![], None),
                comment("r2", vec![], None),
            ]),
        });

        let view = app.post_view.as_ref().unwrap();
        let rows = view.rows();
        let selected = match rows[view.cursor].kind {
            RowKind::Comment { comment, .. } => comment.id.clone(),
            RowKind::More { .. } => "more".into(),
        };
        assert_eq!(selected, "c3");
    }

    #[test]
    fn more_replies_arriving_above_keep_a_selected_more_row_selected() {
        let mut app = app_with(&[]);
        app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
        let generation = app.post_view.as_ref().unwrap().generation;
        app.on_msg(Msg::Post {
            generation,
            result: Ok(Box::new(PostPage {
                post: post("p", "s"),
                body: String::new(),
                comments: vec![
                    comment("c1", vec![], Some("/r/s/comments/p/x/c1")),
                    comment("c2", vec![], Some("/r/s/comments/p/x/c2")),
                ],
            })),
        });
        let view = app.post_view.as_mut().unwrap();
        view.free_scroll = false;
        view.cursor = 3; // rows: c1, more(c1), c2, more(c2)

        app.on_msg(Msg::MoreReplies {
            generation,
            comment_id: "c1".into(),
            result: Ok(vec![
                comment("r1", vec![], None),
                comment("r2", vec![], None),
            ]),
        });

        let view = app.post_view.as_ref().unwrap();
        let rows = view.rows();
        assert!(matches!(
            rows[view.cursor].kind,
            RowKind::More { parent: "c2", .. }
        ));
    }

    #[test]
    fn a_home_refresh_with_fewer_posts_keeps_a_post_selected() {
        let mut app = app_with(&["a"]);
        app.on_msg(Msg::Group {
            subreddit: "a".into(),
            result: Ok(listing("a", &["a1", "a2", "a3"], None)),
        });
        app.home.post = 2;

        app.on_msg(Msg::Group {
            subreddit: "a".into(),
            result: Ok(listing("a", &["a1"], None)),
        });

        assert_eq!(app.current_post().map(|p| p.id).as_deref(), Some("a1"));
    }

    #[test]
    fn passing_through_a_tab_keeps_its_new_badge_but_engaging_clears_it() {
        let mut app = app_with(&["a", "b", "c"]);
        app.on_msg(Msg::Group {
            subreddit: "b".into(),
            result: Ok(listing("b", &["b1"], None)),
        });
        // New posts appear in r/b after the baseline.
        app.on_msg(Msg::Group {
            subreddit: "b".into(),
            result: Ok(listing("b", &["b1", "b2", "b3"], None)),
        });
        let b = tab_index(&app, "b");
        assert_eq!(app.badge(&app.tabs[b]), 2);

        app.switch_tab(b);
        app.switch_tab(tab_index(&app, "c"));
        assert_eq!(
            app.badge(&app.tabs[b]),
            2,
            "a quick pass must not mark posts seen"
        );

        app.switch_tab(b);
        app.move_selection(1);
        app.switch_tab(tab_index(&app, "c"));
        assert_eq!(app.badge(&app.tabs[b]), 0);
    }

    #[test]
    fn home_refresh_updates_inactive_tabs_but_not_the_one_you_read() {
        let mut app = app_with(&["a", "b"]);
        app.on_msg(Msg::Group {
            subreddit: "a".into(),
            result: Ok(listing("a", &["a1"], None)),
        });
        app.on_msg(Msg::Group {
            subreddit: "b".into(),
            result: Ok(listing("b", &["b1"], None)),
        });
        let a = tab_index(&app, "a");
        app.switch_tab(a);

        app.on_msg(Msg::Group {
            subreddit: "a".into(),
            result: Ok(listing("a", &["a2", "a1"], None)),
        });
        app.on_msg(Msg::Group {
            subreddit: "b".into(),
            result: Ok(listing("b", &["b2", "b1"], None)),
        });

        assert_eq!(app.tabs[a].feed.posts[0].id, "a1");
        assert_eq!(app.tabs[tab_index(&app, "b")].feed.posts[0].id, "b2");
    }

    #[test]
    fn home_navigation_flows_across_groups_and_skips_pinned_posts() {
        let mut app = app_with(&["a", "b"]);
        let mut pinned = listing("a", &["pin", "a1", "a2"], None);
        pinned.posts[0].stickied = true;
        app.on_msg(Msg::Group {
            subreddit: "a".into(),
            result: Ok(pinned),
        });
        app.on_msg(Msg::Group {
            subreddit: "b".into(),
            result: Ok(listing("b", &["b1"], None)),
        });

        assert_eq!(app.current_post().unwrap().id, "a1");
        app.move_selection(2);
        assert_eq!(app.current_post().unwrap().id, "b1");
        app.move_selection(-1);
        assert_eq!(app.current_post().unwrap().id, "a2");
    }

    #[test]
    fn folding_more_replies_and_cursor_anchoring() {
        let mut app = app_with(&[]);
        app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
        let generation = app.post_view.as_ref().unwrap().generation;
        let page = PostPage {
            post: post("p", "s"),
            body: String::new(),
            comments: vec![
                comment(
                    "c1",
                    vec![comment("c1a", vec![], None), comment("c1b", vec![], None)],
                    Some("/r/s/comments/p/x/c1"),
                ),
                comment("c2", vec![], None),
            ],
        };
        app.on_msg(Msg::Post {
            generation,
            result: Ok(Box::new(page)),
        });
        let view = app.post_view.as_mut().unwrap();
        assert_eq!(view.rows().len(), 5); // c1, c1a, c1b, more, c2
        assert!(view.free_scroll, "a post opens at its top");
        view.row_starts = vec![0, 4, 8, 12, 14];
        view.scroll = 9;
        view.page_height = 10;

        // The first comment key selects the first comment on screen.
        app.move_comment_cursor(1);
        assert_eq!(app.post_view.as_ref().unwrap().cursor, 3);
        // That row is "more replies": Enter loads them in place.
        app.activate_comment();
        app.on_msg(Msg::MoreReplies {
            generation,
            comment_id: "c1".into(),
            result: Ok(vec![comment("c1c", vec![], None)]),
        });
        let view = app.post_view.as_mut().unwrap();
        assert_eq!(view.rows().len(), 5); // c1, c1a, c1b, c1c, c2
        view.cursor = 0;
        app.activate_comment();
        let rows = app.post_view.as_ref().unwrap().rows();
        assert_eq!(rows.len(), 2);
        assert!(matches!(rows[0].kind, RowKind::Comment { hidden: 3, .. }));
    }

    #[test]
    fn next_top_level_from_the_top_of_a_post_selects_the_first_comment() {
        let mut app = app_with(&[]);
        app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
        let generation = app.post_view.as_ref().unwrap().generation;
        app.on_msg(Msg::Post {
            generation,
            result: Ok(Box::new(PostPage {
                post: post("p", "s"),
                body: String::new(),
                comments: vec![comment("c1", vec![], None), comment("c2", vec![], None)],
            })),
        });
        app.post_view.as_mut().unwrap().row_starts = vec![4, 8];

        app.jump_top_level(true);
        assert_eq!(app.post_view.as_ref().unwrap().cursor, 0);
        app.jump_top_level(true);
        assert_eq!(app.post_view.as_ref().unwrap().cursor, 1);
    }

    #[test]
    fn upward_keys_at_the_top_of_a_post_stay_there() {
        let mut app = app_with(&[]);
        app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
        let generation = app.post_view.as_ref().unwrap().generation;
        app.on_msg(Msg::Post {
            generation,
            result: Ok(Box::new(PostPage {
                post: post("p", "s"),
                body: String::new(),
                comments: vec![comment("c1", vec![], None), comment("c2", vec![], None)],
            })),
        });
        app.post_view.as_mut().unwrap().row_starts = vec![4, 8];
        let at_top = |app: &App| {
            let view = app.post_view.as_ref().unwrap();
            view.free_scroll && view.scroll == 0
        };

        // On the first comment, `k` goes up to the post; more `k` and `N` stay there
        // instead of flipping back down to the first comment.
        app.move_comment_cursor(1);
        assert!(!at_top(&app));
        let mut states = Vec::new();
        for _ in 0..3 {
            app.move_comment_cursor(-1);
            states.push(at_top(&app));
        }
        app.jump_top_level(false);
        states.push(at_top(&app));
        assert_eq!(states, [true, true, true, true]);

        // `j` from the top still selects the first comment.
        app.move_comment_cursor(1);
        let view = app.post_view.as_ref().unwrap();
        assert!(!view.free_scroll);
        assert_eq!(view.cursor, 0);
        // `N` on the selected first thread goes up to the post, as `k` does.
        app.post_view.as_mut().unwrap().scroll = 4;
        app.jump_top_level(false);
        assert!(at_top(&app));
    }

    #[test]
    fn comment_keys_while_reading_move_the_way_they_point() {
        let mut app = app_with(&[]);
        app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
        let generation = app.post_view.as_ref().unwrap().generation;
        app.on_msg(Msg::Post {
            generation,
            result: Ok(Box::new(PostPage {
                post: post("p", "s"),
                body: String::new(),
                comments: ["c1", "c2", "c3"]
                    .map(|id| comment(id, vec![], None))
                    .into(),
            })),
        });
        // A long body, then comments at lines 100-139 (tall), 140-149 and 150 on
        // (the last, tall); 20 lines show.
        let press_at = |app: &mut App, scroll: usize, key: fn(&mut App)| {
            let view = app.post_view.as_mut().unwrap();
            view.row_starts = vec![100, 140, 150];
            view.page_height = 20;
            view.scroll = scroll;
            view.free_scroll = true;
            key(app);
            let view = app.post_view.as_ref().unwrap();
            (view.free_scroll, view.cursor, view.scroll)
        };
        let k = |app: &mut App| app.move_comment_cursor(-1);
        let j = |app: &mut App| app.move_comment_cursor(1);
        let up = |app: &mut App| app.jump_top_level(false);
        let down = |app: &mut App| app.jump_top_level(true);

        // The first comment's header on the bottom line: up keys keep reading up.
        assert_eq!(press_at(&mut app, 81, k), (true, 0, 80));
        assert!(matches!(press_at(&mut app, 81, up), (true, _, 0)));
        // The first thread's header on the top line: the thread before is the post.
        assert!(matches!(press_at(&mut app, 100, up), (true, _, 0)));
        // Inside the tall first comment: up keys select it, down keys the next one.
        assert_eq!(press_at(&mut app, 110, k), (false, 0, 110));
        assert_eq!(press_at(&mut app, 110, up), (false, 0, 110));
        // A thread header exactly on the top line: `N` goes to the thread before.
        assert_eq!(press_at(&mut app, 140, up), (false, 0, 140));
        assert_eq!(press_at(&mut app, 110, j), (false, 1, 110));
        assert_eq!(press_at(&mut app, 110, down), (false, 1, 110));
        // Inside a later thread, `N` selects that thread, not the one before.
        assert_eq!(press_at(&mut app, 145, up), (false, 1, 145));
        // Inside the last comment: `j` reads on by a line, `n` stays.
        assert!(matches!(press_at(&mut app, 155, j), (true, _, 156)));
        assert!(matches!(press_at(&mut app, 155, down), (true, _, 155)));

        // `l` selects the comment being read: the first starting on screen, else
        // the one holding the top line; after `h`, the comment it left.
        let l = |app: &mut App| app.activate_comment();
        assert_eq!(press_at(&mut app, 135, l), (false, 1, 135));
        assert_eq!(press_at(&mut app, 110, l), (false, 0, 110));
        app.post_view.as_mut().unwrap().cursor = 2;
        app.post_view.as_mut().unwrap().free_scroll = false;
        app.leave_comment();
        assert_eq!(press_at(&mut app, 135, l), (false, 2, 135));
        // Scrolling away without `h` forgets it: `l` selects what is being read.
        assert_eq!(press_at(&mut app, 135, l), (false, 1, 135));
        // `G` selects too, so it also forgets the comment `h` left.
        app.post_view.as_mut().unwrap().cursor = 0;
        app.post_view.as_mut().unwrap().free_scroll = false;
        app.leave_comment();
        app.post_bottom();
        assert_eq!(press_at(&mut app, 135, l), (false, 1, 135));
    }

    #[test]
    fn next_thread_while_reading_the_last_one_keeps_reading() {
        let mut app = app_with(&[]);
        app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
        let generation = app.post_view.as_ref().unwrap().generation;
        app.on_msg(Msg::Post {
            generation,
            result: Ok(Box::new(PostPage {
                post: post("p", "s"),
                body: String::new(),
                comments: vec![comment(
                    "c",
                    vec![comment("r1", vec![], None), comment("r2", vec![], None)],
                    None,
                )],
            })),
        });
        // Rows c, r1, r2 at lines 100 (tall), 125 and 135; the screen shows 120-139.
        let view = app.post_view.as_mut().unwrap();
        view.row_starts = vec![100, 125, 135];
        view.page_height = 20;
        view.cursor = 2;
        view.free_scroll = false;
        app.leave_comment(); // `h` from r2
        app.post_view.as_mut().unwrap().scroll = 120;

        app.jump_top_level(true);
        let view = app.post_view.as_ref().unwrap();
        let after_n = (view.free_scroll, view.scroll);
        app.activate_comment();
        let after_l = app.post_view.as_ref().unwrap().cursor;

        assert_eq!(
            after_n,
            (true, 120),
            "`n` selects no reply: no thread follows"
        );
        assert_eq!(after_l, 2, "and `l` still returns to the comment `h` left");
    }

    #[test]
    fn l_with_the_left_comment_scrolled_off_selects_what_is_read() {
        let mut app = app_with(&[]);
        app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
        let generation = app.post_view.as_ref().unwrap().generation;
        app.on_msg(Msg::Post {
            generation,
            result: Ok(Box::new(PostPage {
                post: post("p", "s"),
                body: String::new(),
                comments: ["c0", "c1", "c2", "c3"]
                    .map(|id| comment(id, vec![], None))
                    .into(),
            })),
        });
        // Rows of 6 lines plus a blank line; `h` left c1 (lines 12-17).
        let view = app.post_view.as_mut().unwrap();
        view.row_starts = vec![5, 12, 19, 26];
        view.page_height = 20;
        view.cursor = 1;
        view.free_scroll = false;
        app.leave_comment();
        // Read on until the top line is the blank line under c1.
        app.post_view.as_mut().unwrap().scroll = 18;

        app.activate_comment();
        let view = app.post_view.as_ref().unwrap();

        assert_eq!(view.cursor, 2, "c1 is off screen: the comment read is c2");
    }

    #[test]
    fn comment_keys_after_a_fold_in_the_same_batch_wait_for_the_draw() {
        let mut app = app_with(&[]);
        app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
        let generation = app.post_view.as_ref().unwrap().generation;
        let replies = ["a1", "a2", "a3"]
            .map(|id| comment(id, vec![], None))
            .into();
        app.on_msg(Msg::Post {
            generation,
            result: Ok(Box::new(PostPage {
                post: post("p", "s"),
                body: String::new(),
                comments: vec![
                    comment("a", replies, None),
                    comment("b", vec![], None),
                    comment("c", vec![], None),
                ],
            })),
        });
        // Positions of the last draw: a, a1, a2, a3, b, c.
        let view = app.post_view.as_mut().unwrap();
        view.row_starts = vec![10, 14, 18, 22, 26, 30];
        view.page_height = 20;
        view.free_scroll = false;
        // Fold `a`, `h`, then `j`, `n`, `N` and `l`, all before the next draw.
        app.activate_comment();
        app.leave_comment();
        app.post_view.as_mut().unwrap().scroll = 23;
        app.move_comment_cursor(1);
        app.jump_top_level(true);
        app.jump_top_level(false);
        app.activate_comment();
        let view = app.post_view.as_ref().unwrap();

        assert!(view.free_scroll, "nothing selected from stale positions");
        assert_eq!(view.scroll, 23);
        assert!(view.cursor < view.rows().len());
    }

    #[test]
    fn opening_a_subreddit_accepts_the_spellings_the_subscriptions_file_does() {
        let mut app = app_with(&[]);
        for name in ["r/tmux/", "R/rust", "/r/vim/"] {
            app.open_subreddit(name);
        }
        let titles: Vec<String> = app.tabs.iter().map(Tab::title).collect();

        assert!(app.status.as_ref().is_none_or(|s| !s.error));
        for name in ["tmux", "rust", "vim"] {
            assert!(titles.iter().any(|t| t.contains(name)), "{titles:?}");
        }
        assert!(
            app.picker_entries("r/tmux/")
                .iter()
                .any(|e| matches!(e, PickerEntry::Tab { title, .. } if title.contains("tmux")))
        );
    }

    #[test]
    fn a_merge_from_another_instance_keeps_the_selected_bookmark() {
        let mut app = app_with(&[]);
        let _ = std::fs::remove_file(&app.state_path);
        app.state.toggle_bookmark(&post("x", "s"), 50);
        app.state.toggle_bookmark(&post("y", "s"), 100);
        app.switch_tab(BOOKMARKS_TAB);
        app.tabs[BOOKMARKS_TAB].feed.selected = 1; // [y, x] → x
        let mut other = State::default();
        other.toggle_bookmark(&post("z", "s"), 200);
        other.save(&app.state_path, 200).unwrap();

        let redraw = app.persist();
        remove_state(&app.state_path);

        assert!(redraw);
        assert_eq!(app.current_post().unwrap().id, "x");
    }

    #[test]
    fn a_failed_write_after_a_merge_keeps_the_selection_and_reports_the_error() {
        use std::os::unix::fs::PermissionsExt;
        let mut app = app_with(&[]);
        let dir = std::env::temp_dir().join(format!("redtab-failwrite-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("state.json");
        app.state_path = path.clone();
        app.state.toggle_bookmark(&post("x", "s"), 50);
        app.state.toggle_bookmark(&post("y", "s"), 100);
        app.switch_tab(BOOKMARKS_TAB);
        app.tabs[BOOKMARKS_TAB].feed.selected = 1; // [y, x] → x
        let mut other = State::default();
        other.toggle_bookmark(&post("z", "s"), 200);
        other.save(&path, 200).unwrap();
        // A read-only directory: the merge reads the file, then writing the
        // temp file fails.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();

        let redraw = app.persist();
        let selected = app.current_post().map(|p| p.id);
        app.toggle_bookmark();
        let status = app.status.as_ref().map(|s| (s.error, s.text.clone()));
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();

        assert!(redraw);
        assert_eq!(selected.as_deref(), Some("x"));
        assert!(
            app.state.is_bookmarked("z"),
            "the merge ran before the write failed"
        );
        let (error, text) = status.unwrap();
        assert!(
            error,
            "a failed bookmark save must not read as success: {text}"
        );
    }

    #[test]
    fn closing_a_tab_starts_a_fresh_visit_on_the_next_one() {
        let mut app = app_with(&["a"]);
        let a = tab_index(&app, "a");
        app.switch_tab(a);
        app.engage();
        app.open_subreddit("b");
        app.on_msg(Msg::Group {
            subreddit: "a".into(),
            result: Ok(listing("a", &["a2", "a1"], None)),
        });

        app.close_active_tab();

        assert!(!app.tabs[app.active].engaged);
    }

    #[test]
    fn picker_offers_subreddits_named_like_builtin_tabs() {
        let app = app_with(&[]);

        assert!(
            app.picker_entries("r/home")
                .contains(&PickerEntry::Open("home".into()))
        );
    }
}
