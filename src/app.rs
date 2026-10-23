//! Application state and key actions; rendering is in `ui`, dispatch in `keys`.

pub mod experience;

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
    Comment, CommentSources, Listing, Media, MoreReplies, ParentMembership, Post, PostPage,
    ReplyPage, SPOILER_START, Sort, THREAD_ROOT, TimeRange,
};
use crate::reddit::{Health, Reddit, is_community};
use crate::state::{ListView, State};
use crate::video::{self, Player};
use crate::worker::{FeedQuery, FeedTarget, ImageMetadata, Job, Msg, Pool};

const SAVE_INTERVAL: Duration = Duration::from_secs(60);
const STATUS_TTL: Duration = Duration::from_secs(6);
/// A tab's posts count as seen once you stay this long or interact with it.
const SEEN_DWELL: Duration = Duration::from_secs(2);
/// Load the next page when the selection gets this close to the end.
const PREFETCH_MARGIN: usize = 5;
/// Bound automatic Home fill when filters or missing post details leave pages empty.
const HOME_AUTO_PAGES: usize = 4;
/// Built-in tabs stay before account subscriptions and tabs opened with the picker.
const BOOKMARKS_TAB: usize = 1;
pub const ALL_SUBS_TAB: usize = 2;
/// Past this many cached thumbnails, those not on screen are dropped.
const THUMB_CACHE_MAX: usize = 400;
/// Recent decoded card previews; visible images are not part of this offscreen budget.
const CARD_CACHE_MAX: usize = 8;
const CARD_CACHE_BYTES: usize = 64 * 1024 * 1024;
/// Small retained previews keep revisits visible while full pixels return from disk.
const CARD_PREVIEW_MAX: usize = 256;
/// A video area that changed size restarts the video once the size holds this long.
const VIDEO_RESIZE_SETTLE: Duration = Duration::from_millis(250);
/// Moves through lists, tabs and posts closer together than this count as one quick run.
/// A move after a pause loads at once; later moves in a quick run load only where it stops.
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

/// Holds navigation loads until moves pause. `arrive` permits the first target
/// immediately; `defer` always waits for the pause.
#[derive(Debug, Default)]
struct Settle<T = u64> {
    /// The target held back (a post view's generation, a tab id, or `()` for what is on
    /// screen) and when it loads.
    waiting: Option<(T, Instant)>,
    last_move: Option<Instant>,
}

impl<T: Copy + PartialEq> Settle<T> {
    /// A move to `target`: true when it loads now, else it waits for the moves to pause.
    fn arrive(&mut self, target: T, now: Instant) -> bool {
        let quick = self
            .last_move
            .is_some_and(|last| now.saturating_duration_since(last) < NAV_SETTLE);
        self.last_move = Some(now);
        self.waiting = quick.then(|| (target, now + NAV_SETTLE));
        !quick
    }

    fn defer(&mut self, target: T, now: Instant) {
        self.last_move = Some(now);
        self.waiting = Some((target, now + NAV_SETTLE));
    }

    /// The held-back target whose wait is over by `now`.
    fn due(&mut self, now: Instant) -> Option<T> {
        if self.waiting.is_some_and(|(_, at)| at <= now) {
            self.take()
        } else {
            None
        }
    }

    /// The held-back target, its wait cut short.
    fn take(&mut self) -> Option<T> {
        self.waiting.take().map(|(target, _)| target)
    }

    /// Drop the held-back load of `target`: something else just loaded it.
    fn forget(&mut self, target: T) {
        if self.waiting.is_some_and(|(waiting, _)| waiting == target) {
            self.waiting = None;
        }
    }

    /// Whether a quick run of moves still holds its load back at `now`.
    fn holding(&self, now: Instant) -> bool {
        self.waiting.is_some_and(|(_, at)| at > now)
    }

    fn wake_in(&self, now: Instant) -> Option<Duration> {
        self.waiting
            .map(|(_, at)| at.saturating_duration_since(now))
    }
}

/// A list picture cut to its slot's shape, centred, so it fills the slot without
/// stretching: thumbnails are `THUMB_COLS`×`THUMB_ROWS` cells and icons 2×1, both two
/// cells across per row.
fn fill_slot(img: image::DynamicImage, font: ratatui_image::FontSize) -> image::DynamicImage {
    use crate::ui::{THUMB_COLS, THUMB_ROWS};
    let (w, h) = (u64::from(img.width()), u64::from(img.height()));
    let slot_w = u64::from(THUMB_COLS) * u64::from(font.width);
    let slot_h = u64::from(THUMB_ROWS) * u64::from(font.height);
    if w == 0 || h == 0 || slot_w == 0 || slot_h == 0 {
        return img;
    }
    // The widest centred part of the slot's shape the picture holds.
    let (cw, ch) = if w * slot_h > h * slot_w {
        ((h * slot_w / slot_h).max(1), h)
    } else {
        (w, (w * slot_h / slot_w).max(1))
    };
    img.crop_imm(
        ((w - cw) / 2) as u32,
        ((h - ch) / 2) as u32,
        cw as u32,
        ch as u32,
    )
}

#[derive(Debug, Default, Clone)]
pub struct CardScroll {
    /// First visible card, independent of the selected post.
    pub post: Option<String>,
    pub line: usize,
    pub height: usize,
    pub viewport: usize,
    pub pending: isize,
}

#[derive(Debug, Default)]
pub struct Feed {
    pub posts: Vec<Post>,
    pub after: Option<String>,
    pub sort: Sort,
    pub time: TimeRange,
    pub search: Option<String>,
    failures: HashMap<Option<String>, FeedFailure>,
    pub generation: u64,
    pub selected: usize,
    pub loaded: bool,
    /// First visible list item; kept in sync by the UI.
    pub offset: usize,
    pub cards: CardScroll,
    /// The query `posts` came from: a failed load of another query clears them, of the
    /// same keeps them.
    shown_query: Option<FeedQuery>,
    pub freshness: Option<Freshness>,
    pages: Vec<FeedPage>,
    request_query: Option<FeedQuery>,
    requests: HashMap<u64, Option<String>>,
    pending: HashSet<u64>,
    refreshing: HashSet<u64>,
}

#[derive(Debug)]
struct FeedFailure {
    error: String,
    source: Source,
}

#[derive(Debug)]
struct FeedPage {
    cursor: Option<String>,
    len: usize,
    after: Option<String>,
    source: Source,
}

impl Feed {
    fn query(&self) -> FeedQuery {
        match &self.search {
            Some(q) => FeedQuery::Search { query: q.clone() },
            // Sorts without a time range send none; the kept range must not split them.
            None => FeedQuery::Listing {
                sort: self.sort,
                time: if self.sort.takes_time() {
                    self.time
                } else {
                    TimeRange::Day
                },
            },
        }
    }

    pub fn is_default_view(&self) -> bool {
        self.search.is_none() && self.sort == Sort::Hot
    }

    pub fn refreshing(&self) -> bool {
        !self.refreshing.is_empty()
    }

    /// `posts` answer the current sort, time range or search, not an earlier one still
    /// shown while the new one loads.
    pub fn shows_current_query(&self) -> bool {
        self.shown_query.as_ref() == Some(&self.query())
    }

    pub fn loading(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn loading_more(&self) -> bool {
        self.requests.iter().any(|(id, cursor)| {
            cursor.is_some() && (self.pending.contains(id) || self.refreshing.contains(id))
        })
    }

    pub fn error(&self) -> Option<&str> {
        self.failures
            .get(&self.after)
            .or_else(|| self.failures.get(&None))
            .or_else(|| self.failures.values().next())
            .map(|failure| failure.error.as_str())
    }

    pub fn page_failed(&self) -> bool {
        self.after.is_some()
            && self.failures.contains_key(&self.after)
            && !self.pages.iter().any(|page| page.cursor == self.after)
    }

    fn accepts(&self, generation: u64) -> bool {
        generation == self.generation || self.requests.contains_key(&generation)
    }

    fn remove_post(&mut self, id: &str) {
        let Some(index) = self.posts.iter().position(|post| post.id == id) else {
            return;
        };
        self.posts.remove(index);
        let mut start = 0;
        for page in &mut self.pages {
            if index < start + page.len {
                page.len -= 1;
                break;
            }
            start += page.len;
        }
    }

    /// Replace this cursor's page, not the entire feed or an append-only union.
    fn replace_page(
        &mut self,
        cursor: Option<String>,
        mut posts: Vec<Post>,
        after: Option<String>,
        source: Source,
    ) -> bool {
        let index = self
            .pages
            .iter()
            .position(|page| page.cursor == cursor)
            .unwrap_or(self.pages.len());
        if self
            .pages
            .get(index)
            .is_some_and(|page| !source.may_replace(page.source))
        {
            return false;
        }
        let mut protected = HashSet::new();
        let mut start = 0;
        for (i, page) in self.pages.iter().enumerate() {
            if i != index
                && (!source.may_replace(page.source)
                    || (page.source.compare(source) == Some(std::cmp::Ordering::Equal)
                        && source.cached
                        && i < index))
            {
                protected.extend(
                    self.posts[start..start + page.len]
                        .iter()
                        .map(|post| post.id.as_str()),
                );
            }
            start += page.len;
        }
        posts.retain(|post| !protected.contains(post.id.as_str()));
        let incoming_ids: HashSet<_> = posts.iter().map(|post| post.id.as_str()).collect();
        // Newer refreshed records own their positions; older cached pages cannot move them back.
        let mut pages = self.pages.iter_mut().enumerate();
        let mut current = pages.next();
        let mut remaining = current.as_ref().map_or(0, |(_, page)| page.len);
        self.posts.retain(|post| {
            while remaining == 0 && current.is_some() {
                current = pages.next();
                remaining = current.as_ref().map_or(0, |(_, page)| page.len);
            }
            remaining = remaining.saturating_sub(1);
            let keep = current.as_ref().is_some_and(|(i, _)| *i == index)
                || !incoming_ids.contains(post.id.as_str());
            if !keep && let Some((_, page)) = current.as_mut() {
                page.len -= 1;
            }
            keep
        });
        let start: usize = self.pages[..index].iter().map(|page| page.len).sum();
        let end = start + self.pages.get(index).map_or(0, |page| page.len);
        let page = FeedPage {
            cursor,
            len: posts.len(),
            after,
            source,
        };
        self.posts.splice(start..end, posts);
        if index == self.pages.len() {
            self.pages.push(page);
        } else {
            self.pages[index] = page;
        }
        self.after = self.pages.last().and_then(|page| page.after.clone());
        true
    }
}

#[derive(Debug)]
pub enum TabKind {
    Home,
    Bookmarks,
    AllSubs,
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
    fn feed_target(&self) -> Option<FeedTarget> {
        match &self.kind {
            TabKind::Home => Some(FeedTarget::Home),
            TabKind::Subreddit { name, .. } => Some(FeedTarget::Subreddit(name.clone())),
            _ => None,
        }
    }

    pub fn sorts(&self) -> &[Sort] {
        if matches!(self.kind, TabKind::Home) {
            &Sort::ALL
        } else {
            &Sort::ALL[1..]
        }
    }

    pub fn subreddit(&self) -> Option<&str> {
        match &self.kind {
            TabKind::Subreddit { name, .. } => Some(name),
            _ => None,
        }
    }

    pub fn title(&self) -> String {
        match &self.kind {
            TabKind::Home => "Home".into(),
            TabKind::AllSubs => "All subs".into(),
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
    pub freshness: Option<Freshness>,
    pub generation: u64,
}

#[derive(Debug, Default)]
pub struct AllSubs {
    pub loading: bool,
    pub error: Option<String>,
    pub freshness: Option<Freshness>,
    generation: u64,
    next_group_generation: u64,
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
    pub freshness: Option<Freshness>,
    pub generation: u64,
    pub collapsed: HashSet<String>,
    pub loading_more: HashSet<String>,
    /// Removal evidence outlives a comment so older ancestor snapshots cannot restore it.
    removed_comments: HashMap<String, Source>,
    /// Index into [`PostView::rows`].
    pub cursor: usize,
    pub scroll: usize,
    /// Reading by scrolling; the next comment key selects near the top (see [`Anchor`]).
    pub free_scroll: bool,
    /// The comment `h` left, which `l` returns to while it is on screen.
    left: Option<RowKey>,
    /// A comment `l` selected while reading (or `G` inside the last one): draws keep its
    /// text in place while it stays selected and visible.
    pub hold_view: Option<Hold>,
    /// Row at the top line, offset into it and scroll, taken before the document changed
    /// above it; the next draw restores it (see `take_reading_anchor`).
    keep_reading: Option<(RowKey, isize, usize)>,
    /// The selected row the last draw placed on screen; None while reading.
    pub drawn_cursor: Option<usize>,
    /// `v` while reading: spoiler state and scroll of the last draw, to find the top line
    /// in the new wrap.
    pub rewrapped: Option<(bool, usize)>,
    /// Last rendered document, moved here after drawing.
    pub rendered_document: Vec<ratatui::text::Line<'static>>,
    /// A successful content update arrived since that document was drawn.
    pub content_changed: bool,
    /// More rows in the last displayed geometry, even after spoiler invalidation.
    pub rendered_more: Vec<(String, usize)>,
    /// Pictures in the last displayed document: URL, first line, rows. A picture above
    /// the view that grows when it loads moves the view down by as much.
    pub rendered_pictures: Vec<(String, usize, usize)>,
    /// This update redirects a read More row to its first newly inserted reply.
    pub more_handoff: bool,
    /// The inline image was scrolled away; it stays away (even if a short post clamps the
    /// scroll back to 0) until a key returns to the top.
    pub image_away: bool,
    /// The last draw showed the inline image (so the post was at its top).
    pub image_shown: bool,
    /// Document line where each row starts; kept in sync by the UI.
    pub row_starts: Vec<usize>,
    /// Document lines visible in the last draw; pages are this tall.
    pub page_height: usize,
    /// `v` shows spoilers: a spoiler-tagged post's body and image, and `>!…!<` text.
    pub reveal_spoilers: bool,
    pub gallery_index: usize,
    pub comment_sort: String,
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
    pub fn preview_image(&self) -> Option<&str> {
        match &self.shown_post().media {
            Media::Gallery { images, .. } => images
                .get(self.gallery_index % images.len().max(1))
                .map(String::as_str),
            media => media.preview_image(),
        }
    }
    pub fn shown_post(&self) -> &Post {
        self.page.as_ref().map_or(&self.post, |page| &page.post)
    }

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

    /// Comment tree in display order, honoring folds. Empty for a hidden spoiler post: its
    /// comments discuss what the body hides.
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
        if c.id.starts_with(THREAD_ROOT) {
            for reply in &c.replies {
                self.flatten(reply, depth, rows);
            }
            if let Some(more) = &c.more {
                rows.push(Row {
                    depth,
                    kind: RowKind::More {
                        parent: &c.id,
                        more,
                    },
                });
            }
            return;
        }
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

    /// The comment a key selects while reading (free scroll); None leaves the post. Uses
    /// the last draw's spans; no key moves the view against its direction.
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
                    // A row's blank line belongs to the next row (as in `reading_position`).
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

    /// Leave free scroll by selecting the comment `anchor` picks; true if it did.
    fn anchor_cursor(&mut self, anchor: Anchor) -> bool {
        // No positions before a loaded post's first draw (keep it at its top), and stale
        // ones after a fold in the same batch: wait for the draw.
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

    /// Whether the last draw's `row_starts` still describe [`PostView::rows`] (no fold,
    /// reveal or load since).
    fn positions_current(&self) -> bool {
        self.row_starts.len() == self.rows().len()
    }

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

    /// Before the document changes above what is read (loads, spoiler rewrap), keep the
    /// last-drawn text in place. Several changes before one draw keep the first anchor.
    fn pin_view(&mut self) {
        // At the top with the image shown, stay at the top: anchoring would scroll off 0
        // and dismiss the image, so a hold keeps scroll 0 and a selection stays in view.
        // If a key in this pass already scrolled, the reading position counts instead.
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
                // The selection the last draw placed; if a key moved it since, the draw
                // shows the new one.
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

    /// The row holding the top line and the offset into it, by the last draw. None in the
    /// body (loads land below it) or when positions are stale.
    fn reading_position(&self) -> Option<(RowKey, isize)> {
        let rows = self.rows();
        if !self.free_scroll || self.row_starts.len() != rows.len() {
            return None;
        }
        let mut i = self.row_starts.iter().rposition(|&s| s <= self.scroll)?;
        // A row's blank line belongs to the next row, so a loading More row's blank line
        // can't anchor to it.
        if self.row_starts.get(i + 1) == Some(&(self.scroll + 1)) {
            i += 1;
        }
        Some((
            RowKey::of(&rows[i]),
            self.scroll as isize - self.row_starts[i] as isize,
        ))
    }

    /// For the draw: where to put the top line back after the document changed above it,
    /// once. Keys that scrolled since move on from it; `g` stays at the top.
    pub fn take_reading_anchor(&mut self) -> Option<(usize, isize)> {
        let (key, offset, pinned) = self.keep_reading.take()?;
        if self.scroll == 0 && pinned != 0 {
            return None;
        }
        let moved = self.scroll as isize - pinned as isize;
        Some((self.row_index(&key)?, offset + moved))
    }

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

    /// Keep the cursor on the same row after rows changed above it; a loaded More row's
    /// index now holds its first new reply.
    fn reselect(&mut self, key: Option<RowKey>) {
        let count = self.rows().len();
        if count == 0 {
            self.cursor = 0;
            self.free_scroll = true;
            self.hold_view = None;
            self.keep_reading = None;
            self.drawn_cursor = None;
            self.left = None;
            return;
        }
        let found = key.and_then(|key| self.row_index(&key));
        let held = self.hold_view.is_some_and(|h| h.row == self.cursor);
        self.cursor = found.unwrap_or(self.cursor).min(count.saturating_sub(1));
        // The comment `l` selected moved: its hold on the view moves with it.
        if held && let Some(hold) = &mut self.hold_view {
            hold.row = self.cursor;
        }
    }
}

/// See [`PostView::hold_view`].
#[derive(Debug, Clone, Copy)]
pub struct Hold {
    pub row: usize,
    /// Top line's offset from the comment's first line when the hold began.
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
    /// `l` `c` Enter: the comment `h` left if on screen, else the one being read (first
    /// starting on screen, else holding the top line, else the next below).
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

fn find_comment<'a>(comments: &'a [Comment], id: &str) -> Option<&'a Comment> {
    comments.iter().find_map(|c| {
        if c.id == id {
            Some(c)
        } else {
            find_comment(&c.replies, id)
        }
    })
}

#[derive(Debug, Default)]
pub struct PickerState {
    pub input: String,
    pub selected: usize,
}

pub enum Overlay {
    Experience,
    Auth,
    Membership {
        subreddit: String,
        join: bool,
    },
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

/// Refetch state of a loaded picture; its pixels stay drawn whatever this says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageRefresh {
    Idle,
    Loading,
    Failed,
    /// Skipped work or an explicit retry waits until the URL is drawn.
    Needed,
}

pub enum ImageSlot {
    Loading,
    Ready {
        protocol: Box<StatefulProtocol>,
        refresh: ImageRefresh,
        /// The picture's size in pixels, which decides its shape in the post view.
        pixels: (u32, u32),
    },
    Failed(String),
}

pub enum CardImageSlot {
    Loading,
    Ready {
        image: std::sync::Arc<image::DynamicImage>,
        protocol: Option<CardImageProtocol>,
    },
    /// Usable retained or stale pixels while a full preview is restored.
    Cached {
        image: std::sync::Arc<image::DynamicImage>,
        protocol: Option<CardImageProtocol>,
        refreshing: bool,
    },
    Failed(String),
}

pub struct CardImageProtocol {
    pub size: Size,
    pub pixels: (u32, u32),
    pub tiles: HashMap<(u16, u16, u16), std::sync::Arc<ratatui_image::protocol::Protocol>>,
}

pub struct Status {
    pub text: String,
    pub error: bool,
    pub at: Instant,
    /// A cache warning: once a draw shows it, the same text is not repeated.
    pub cache_warning: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerEntry {
    Tab { index: usize, title: String },
    Open(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    List,
    Post,
}

struct MembershipChange {
    generation: u64,
    subreddit: String,
    join: bool,
    checking: bool,
    error: Option<String>,
}

pub struct App {
    pub experience: experience::Experience,
    pub config: Config,
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
    membership: Option<MembershipChange>,
    pub post_view: Option<PostView>,
    pub overlay: Option<Overlay>,
    pub status: Option<Status>,
    pub health: Health,
    last_save: Instant,
    pub show_images: bool,
    pub show_sidebar: bool,
    /// NO_COLOR is set: half-block images, which are only colors, are not drawn.
    pub no_color: bool,
    pub focus: Focus,
    /// Last drawable terminal width, shared by layout and key dispatch.
    width: u16,
    /// The sidebar was hidden and is shown only while it has focus.
    sidebar_peek: bool,
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
    /// List-row thumbnails and subreddit icons, keyed by image URL.
    pub thumbs: HashMap<String, ImageSlot>,
    /// Subreddit icon URLs, keyed by lowercase subreddit name; `None` when it has none.
    icons: HashMap<String, Option<String>>,
    /// Subreddits whose icon lookup has not answered yet.
    icon_lookups: HashSet<String>,
    pending_media: HashSet<String>,
    /// Cache warnings a draw has shown; a broken or busy cache repeats them per job.
    cache_warnings: HashSet<String>,
    /// The open post's inline video while it plays (or loads, or failed).
    pub video: Option<Player>,
    /// `a` turned the sound on: later videos start with it too.
    video_sound_on: bool,
    /// The box the video decodes for; the drawn picture follows each frame's shape inside.
    pub video_area: Option<Rect>,
    /// The most rows a video frame not wider than tall may take.
    pub video_tall_rows: u16,
    /// Frame number and cells last sent to the terminal (kitty), so a frame goes out once.
    pub video_sent: Option<(u64, Rect)>,
    /// The video area's new size and when it first appeared; the video restarts once it holds.
    video_resize: Option<(ratatui::layout::Size, Instant)>,
    /// Post loads of list moves beside an open post, by post view generation.
    post_settle: Settle,
    /// Feed loads of tab switches, by tab id.
    feed_settle: Settle,
    /// Thumbnails, icons and pictures in post text that moves bring on screen.
    picture_settle: Settle<()>,
    pub quit: bool,
}

impl App {
    pub fn new(
        config: Config,
        pool: Pool,
        reddit: Reddit,
        state_path: PathBuf,
        image_picker: Picker,
    ) -> Self {
        let (state, warning) = State::load(&state_path);
        let show_images = config.image_preview;
        let mut app = Self {
            experience: experience::Experience::default(),
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
            post_view: None,
            overlay: None,
            status: None,
            health: Health::Unknown,
            last_save: Instant::now(),
            show_images,
            show_sidebar: true,
            no_color: false,
            focus: Focus::List,
            width: 0,
            sidebar_peek: false,
            image_picker,
            images: HashMap::new(),
            card_images: HashMap::new(),
            card_image_metadata: HashMap::new(),
            card_image_order: VecDeque::new(),
            card_previews: HashMap::new(),
            card_renderer: crate::card_image::Renderer::default(),
            card_preview_order: VecDeque::new(),
            queued_images: HashMap::new(),
            thumbs: HashMap::new(),
            icons: HashMap::new(),
            icon_lookups: HashSet::new(),
            pending_media: HashSet::new(),
            cache_warnings: HashSet::new(),
            video: None,
            video_sound_on: false,
            video_area: None,
            video_tall_rows: 1,
            video_sent: None,
            video_resize: None,
            post_settle: Settle::default(),
            membership: None,
            feed_settle: Settle::default(),
            picture_settle: Settle::default(),
            quit: false,
        };
        app.push_tab(TabKind::Home);
        app.push_tab(TabKind::Bookmarks);
        app.push_tab(TabKind::AllSubs);
        app.tabs[0].feed.sort = Sort::Best;
        app.tabs[0].entered = Some(Instant::now());
        if let Some(w) = warning {
            app.error(w);
        }
        app
    }

    pub fn start(&mut self) {
        if self.reddit.needs_sign_in() {
            return;
        }
        self.load_feed(0, false);
        self.load_subscriptions(false);
    }

    pub fn show_auth(&mut self) {
        if self.membership.is_some() || self.experience.writing() {
            self.info("Wait for the pending Reddit action before reconnecting");
            return;
        }
        self.auth.active_id = self.reddit.saved_id();
        self.auth.active_name = self.reddit.account().map(|account| account.name);
        self.auth.open(self.reddit.source());
        self.overlay = Some(Overlay::Auth);
    }

    /// Returns whether the account dialog should close.
    pub fn select_auth(&mut self) -> bool {
        if self.auth.phase == crate::auth::Phase::Accounts
            && let Some(account) = self.auth.accounts.get(self.auth.selected)
        {
            if self.auth.active_id.as_ref() == Some(&account.id)
                && self
                    .reddit
                    .account()
                    .is_some_and(|current| current.id == account.id)
                && !self.reddit.needs_sign_in()
            {
                self.auth.cancel();
                return true;
            }
            self.mark_active_seen();
            if let Err(error) = self.persist_checked() {
                self.auth.message = Some(format!(
                    "Could not save the current reading state; connection unchanged: {error}"
                ));
                return false;
            }
        }
        self.auth.select(self.config.show_nsfw);
        false
    }

    pub fn poll_auth(&mut self) -> bool {
        let (changed, connection) = self.auth.poll(self.config.show_nsfw);
        if let Some(connection) = connection {
            self.install_connection(connection);
        }
        changed
    }

    pub fn confirm_auth(&mut self) {
        use crate::auth::Phase;
        if !matches!(self.auth.phase, Phase::Confirm | Phase::DisconnectConfirm) {
            return;
        }
        self.mark_active_seen();
        match self.persist_checked() {
            Ok(_) if self.auth.phase == Phase::DisconnectConfirm => {
                self.auth.disconnect(self.config.show_nsfw)
            }
            Ok(_) => self.auth.confirm(),
            Err(error) => {
                self.auth.message = Some(format!(
                    "Could not save the current reading state; connection unchanged: {error}"
                ))
            }
        }
    }

    pub fn take_replacement_messages(&mut self) -> Option<std::sync::mpsc::Receiver<Msg>> {
        self.replacement_messages.take()
    }

    fn install_connection(&mut self, reddit: Reddit) {
        let disconnected = reddit.needs_sign_in();
        let same_account = !disconnected
            && match self.reddit.account().zip(reddit.account()) {
                Some((old, new)) => old.id == new.id,
                None => self.reddit.session_fingerprint() == reddit.session_fingerprint(),
            };
        self.pool.stop();
        self.reddit.retire();
        let (pool, messages) = self.pool.restart(reddit.clone());
        let root = if self
            .state_path
            .parent()
            .is_some_and(|p| p.ends_with("signed-out"))
        {
            self.state_path
                .parent()
                .and_then(|p| p.parent())
                .unwrap()
                .to_path_buf()
        } else if self
            .state_path
            .parent()
            .and_then(|p| p.parent())
            .is_some_and(|p| p.ends_with("accounts"))
        {
            self.state_path
                .parent()
                .and_then(|p| p.parent())
                .and_then(|p| p.parent())
                .unwrap()
                .to_path_buf()
        } else {
            self.state_path
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .to_path_buf()
        };
        let state_path = if disconnected {
            root.join("signed-out/state.json")
        } else {
            crate::auth::account_state_path(&root, reddit.account().as_ref())
        };
        let mut state_warning = None;
        if let Some(parent) = state_path.parent() {
            use std::os::unix::fs::DirBuilderExt;
            if let Err(error) = std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)
            {
                state_warning = Some(format!(
                    "Could not create account reading-state directory: {error}"
                ));
            }
        }
        if same_account {
            self.pool = pool;
            self.reddit = reddit;
            self.state_path = state_path;
            self.reset_pending_loads();
            self.overlay = None;
            self.start();
            self.ensure_feed_loaded(self.active);
            if self.post_view.is_some() {
                self.refresh_post();
            }
        } else {
            let image_picker = std::mem::replace(&mut self.image_picker, Picker::halfblocks());
            let mut next = Self::new(self.config.clone(), pool, reddit, state_path, image_picker);
            state_warning = state_warning.or_else(|| {
                next.status
                    .as_ref()
                    .filter(|s| s.error)
                    .map(|s| s.text.clone())
            });
            next.no_color = self.no_color;
            next.show_images = self.show_images;
            next.show_sidebar = self.show_sidebar;
            next.width = self.width;
            next.start();
            *self = next;
        }
        self.replacement_messages = Some(messages);
        if disconnected {
            self.health = Health::SignedOut;
            if let Some(warning) = state_warning {
                self.error(format!("Disconnected from Reddit; {warning}"));
            } else {
                self.info("Disconnected from Reddit. Press A to connect.");
            }
            return;
        }
        let name = self.reddit.account().map(|a| a.name).unwrap_or_default();
        if let Some(warning) = state_warning {
            self.error(format!("Connected as u/{name}; {warning}"));
        } else {
            self.info(format!("Connected as u/{name}"));
        }
    }

    fn reset_pending_loads(&mut self) {
        self.experience = experience::Experience::default();
        self.membership = None;
        self.all_subs.loading = false;
        self.all_subs.error = None;
        let generation = self.all_subs.next_group_generation;
        self.all_subs.next_group_generation += 1;
        for group in &mut self.all_subs.groups {
            if group.loading || group.error.is_some() || group.freshness.is_none() {
                Self::refresh_group(&self.pool, group, generation);
            }
        }
        let mut feeds = Vec::new();
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            let cursors: HashSet<_> = tab
                .feed
                .requests
                .iter()
                .filter(|(id, _)| tab.feed.pending.contains(id) || tab.feed.refreshing.contains(id))
                .map(|(_, cursor)| cursor)
                .chain(tab.feed.failures.keys())
                .cloned()
                .collect();
            feeds.extend(cursors.into_iter().map(|cursor| (index, cursor)));
            tab.feed.pending.clear();
            tab.feed.refreshing.clear();
            tab.feed.requests.clear();
            tab.feed.failures.clear();
        }
        for (index, cursor) in feeds {
            // start() reloads Home's first page after the pool handoff.
            if index != 0 || cursor.is_some() {
                self.fetch_feed(index, cursor, false);
            }
        }
        if let Some(view) = &mut self.post_view {
            view.loading = false;
            view.loading_more.clear();
        }
        self.pending_media.clear();
        self.queued_images.clear();
        self.icon_lookups.clear();
        self.images
            .retain(|_, slot| matches!(slot, ImageSlot::Ready { .. }));
        self.thumbs
            .retain(|_, slot| matches!(slot, ImageSlot::Ready { .. }));
        self.card_images.retain(|_, slot| {
            matches!(
                slot,
                CardImageSlot::Ready { .. } | CardImageSlot::Cached { .. }
            )
        });
        for slot in self.card_images.values_mut() {
            if let CardImageSlot::Cached { refreshing, .. } = slot {
                *refreshing = false;
            }
        }
        self.video = None;
        self.post_settle = Settle::default();
        self.feed_settle = Settle::default();
        self.picture_settle = Settle::default();
    }

    fn load_subscriptions(&mut self, force: bool) {
        if self.reddit.needs_sign_in() {
            return;
        }
        if self
            .membership
            .as_ref()
            .is_some_and(|change| !change.checking || self.all_subs.loading)
        {
            return;
        }
        self.all_subs.generation += 1;
        self.all_subs.loading = true;
        self.all_subs.error = None;
        let job = Job::Subscriptions {
            generation: self.all_subs.generation,
        };
        if force {
            self.pool.refresh(job);
        } else {
            self.pool.submit(job);
        }
    }

    fn sync_subscription_tabs(&mut self, names: &[String]) {
        let active = self.active_tab().id;
        let order: HashMap<_, _> = names
            .iter()
            .enumerate()
            .map(|(i, name)| (name.to_ascii_lowercase(), i))
            .collect();
        for tab in &mut self.tabs {
            if let TabKind::Subreddit { name, pinned } = &mut tab.kind {
                *pinned = order.contains_key(&name.to_ascii_lowercase());
            }
        }
        for name in names {
            if !self.tabs.iter().any(|tab| {
                tab.subreddit()
                    .is_some_and(|s| s.eq_ignore_ascii_case(name))
            }) {
                self.push_tab(TabKind::Subreddit {
                    name: name.clone(),
                    pinned: true,
                });
            }
        }
        self.tabs.sort_by_key(|tab| match &tab.kind {
            TabKind::Home => (0, 0),
            TabKind::Bookmarks => (1, 0),
            TabKind::AllSubs => (2, 0),
            TabKind::Subreddit { name, pinned: true } => (3, order[&name.to_ascii_lowercase()]),
            _ => (4, 0),
        });
        self.active = self
            .tabs
            .iter()
            .position(|tab| tab.id == active)
            .unwrap_or(0);
    }

    fn sync_islands(&mut self, names: Vec<String>) {
        let selected = self
            .all_subs
            .groups
            .get(self.all_subs.group)
            .map(|g| g.subreddit.clone());
        let mut groups: HashMap<_, _> = std::mem::take(&mut self.all_subs.groups)
            .into_iter()
            .map(|group| (group.subreddit.to_ascii_lowercase(), group))
            .collect();
        let generation = self.all_subs.next_group_generation;
        self.all_subs.next_group_generation += 1;
        self.all_subs.groups = names
            .into_iter()
            .map(|name| {
                groups
                    .remove(&name.to_ascii_lowercase())
                    .unwrap_or_else(|| {
                        self.pool.submit(Job::Group {
                            subreddit: name.clone(),
                            generation,
                        });
                        Group {
                            subreddit: name,
                            generation,
                            loading: true,
                            ..Group::default()
                        }
                    })
            })
            .collect();
        self.all_subs.group = selected
            .and_then(|name| {
                self.all_subs
                    .groups
                    .iter()
                    .position(|g| g.subreddit == name)
            })
            .unwrap_or(self.all_subs.group)
            .min(self.all_subs.groups.len().saturating_sub(1));
        let count = self
            .all_subs
            .groups
            .get(self.all_subs.group)
            .map_or(0, |g| self.group_posts(g).count());
        self.all_subs.post = self.all_subs.post.min(count.saturating_sub(1));
        self.all_subs.offset = self.all_subs.offset.min(self.all_subs.group);
    }

    fn on_subscriptions(
        &mut self,
        generation: u64,
        result: Result<Vec<String>, String>,
        freshness: Freshness,
        refreshing: bool,
    ) {
        if generation != self.all_subs.generation
            || self
                .membership
                .as_ref()
                .is_some_and(|change| !change.checking || freshness.source.cached)
            || self
                .all_subs
                .freshness
                .is_some_and(|current| !freshness.source.may_replace(current.source))
        {
            return;
        }
        self.all_subs.loading = refreshing;
        let confirmation = result.as_ref().ok().and_then(|names| {
            self.membership
                .as_ref()
                .filter(|change| change.checking)
                .map(|change| {
                    names
                        .iter()
                        .any(|name| name.eq_ignore_ascii_case(&change.subreddit))
                        == change.join
                })
        });
        match result {
            Ok(names) => {
                self.sync_subscription_tabs(&names);
                self.sync_islands(names);
                self.all_subs.error = None;
                self.all_subs.freshness = Some(freshness);
            }
            Err(error) => {
                self.error(format!("Subscriptions: {error}"));
                self.all_subs.error = Some(error);
            }
        }
        if self
            .membership
            .as_ref()
            .is_some_and(|change| change.checking)
        {
            let change = self.membership.take().expect("checked pending membership");
            if confirmation == Some(true) {
                self.info(format!(
                    "{} r/{} on Reddit",
                    if change.join { "Joined" } else { "Left" },
                    change.subreddit
                ));
            } else {
                self.error(change.error.unwrap_or_else(|| format!(
                    "Could not confirm membership for r/{}; r in All subs refreshes the account list",
                    change.subreddit
                )));
            }
        }
    }

    fn on_membership(&mut self, generation: u64, result: Result<(), String>) {
        let Some(change) = &mut self.membership else {
            return;
        };
        if change.generation != generation || change.checking {
            return;
        }
        change.checking = true;
        change.error = result.err();
        self.load_subscriptions(true);
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
            cache_warning: false,
        });
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.status = Some(Status {
            text: text.into(),
            error: true,
            at: Instant::now(),
            cache_warning: false,
        });
    }

    /// After a draw: a cache warning the footer showed is not repeated this session.
    pub fn status_drawn(&mut self) {
        if let Some(status) = self.status.as_mut().filter(|s| s.cache_warning) {
            status.cache_warning = false;
            self.cache_warnings.insert(status.text.clone());
        }
    }

    /// Timers: held-back navigation loads, status expiry, Reddit health, saves, video
    /// frames. True when something visible changed.
    pub fn tick(&mut self) -> bool {
        let now = Instant::now();
        let mut changed = self.load_settled(now);
        changed |= self.card_renderer.poll();
        // Pictures a quick run of moves held back: the redraw fetches what it shows.
        changed |= self.picture_settle.due(now).is_some();
        changed |= self.sync_video(now);
        if self
            .status
            .as_ref()
            .is_some_and(|s| s.at.elapsed() > STATUS_TTL)
        {
            self.status = None;
            changed = true;
        }
        let health = self.reddit.health();
        if health != self.health {
            self.health = health;
            changed = true;
        }
        if self.last_save.elapsed() > SAVE_INTERVAL && self.persist() {
            changed = true;
        }
        changed
    }

    /// Start the load a quick run of moves held back, once the moves paused and only if
    /// its post or tab is still the one shown.
    fn load_settled(&mut self, now: Instant) -> bool {
        let mut changed = false;
        if let Some(generation) = self.post_settle.due(now)
            && let Some(view) = self.post_view.as_ref()
            && view.generation == generation
        {
            let permalink = view.post.permalink.clone();
            self.load_post(permalink);
            changed = true;
        }
        if let Some(tab) = self.feed_settle.due(now)
            && self.active_tab().id == tab
        {
            self.ensure_feed_loaded(self.active);
            changed = true;
        }
        changed
    }

    /// How long the main loop may wait for input: next video frame, resize settling, a
    /// held-back load, or the timers.
    pub fn wake_in(&self) -> Duration {
        const IDLE: Duration = Duration::from_millis(100);
        let media = (!self.queued_images.is_empty() || self.card_renderer.pending())
            .then_some(Duration::from_millis(16));
        let now = Instant::now();
        let settle = self.video_resize.map(|(_, since)| {
            VIDEO_RESIZE_SETTLE.saturating_sub(now.saturating_duration_since(since))
        });
        self.video
            .as_ref()
            .and_then(|p| p.wake_in(now))
            .into_iter()
            .chain(settle)
            .chain(media)
            .chain(self.post_settle.wake_in(now))
            .chain(self.feed_settle.wake_in(now))
            .chain(self.picture_settle.wake_in(now))
            .fold(IDLE, Duration::min)
    }

    /// The stream to play inline: page loaded, images on, no spoiler hiding it.
    fn wanted_video(&self) -> Option<String> {
        let view = self.post_view.as_ref()?;
        let page = view.page.as_ref()?;
        if !self.inline_images_on() || view.spoiler_hidden() {
            return None;
        }
        page.post
            .media
            .video_url()
            .and_then(|url| self.reddit.media_url(url).ok())
    }

    /// Starts, stops, pauses and advances the inline video to match the post view. It
    /// starts once a draw placed its area (restarting in place on a size change) and
    /// pauses while off screen or under an overlay. True when something visible changed.
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
                // A new size: wait until it holds still (a window drag passes many).
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
                    video::Output::Image(self.image_picker.clone(), area, self.video_tall_rows)
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

    pub fn toggle_video_pause(&mut self) {
        match &mut self.video {
            Some(player) => player.toggle_pause(Instant::now()),
            None => self.info("No video plays here"),
        }
    }

    /// `a`: toggles sound for this and later videos; a video without sound stays silent.
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

    /// Whether videos start with sound (`a`).
    pub fn video_sound_wanted(&self) -> bool {
        self.video_sound_on
    }

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

    /// Save state. True when something visible changed: a merge changed the bookmark
    /// list (the selection follows its post), or saving failed. The merge happens even
    /// when the write fails.
    fn persist(&mut self) -> bool {
        match self.persist_checked() {
            Ok(changed) => changed,
            Err(error) => {
                self.error(format!("cannot save state: {error:#}"));
                true
            }
        }
    }

    fn persist_checked(&mut self) -> anyhow::Result<bool> {
        self.last_save = Instant::now();
        let before = self.bookmark_ids();
        let before_view = self.state.list_view();
        let selected = self.selected_bookmark_id();
        let result = self.state.save(&self.state_path, now());
        let changed = self.bookmark_ids() != before;
        if changed {
            self.reselect_bookmark(selected);
        }
        result?;
        Ok(changed || before_view != self.state.list_view())
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

    /// Keep the Bookmarks selection on its post after the list changed, else clamp it.
    fn reselect_bookmark(&mut self, id: Option<String>) {
        let bookmarks = self.state.bookmarks();
        let feed = &mut self.tabs[BOOKMARKS_TAB].feed;
        feed.selected = id
            .and_then(|id| bookmarks.iter().position(|b| b.post.id == id))
            .unwrap_or(feed.selected)
            .min(bookmarks.len().saturating_sub(1));
        self.preview_selection();
    }

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

    fn enter_tab(&mut self, index: usize) {
        self.active = index;
        let tab = &mut self.tabs[index];
        tab.entered = Some(Instant::now());
        tab.engaged = false;
        self.close_post();
        let now = Instant::now();
        if self.focus == Focus::Sidebar {
            self.feed_settle.defer(self.tabs[index].id, now);
            self.picture_settle.defer((), now);
        } else {
            self.moving();
            if self.feed_settle.arrive(self.tabs[index].id, now) {
                self.ensure_feed_loaded(index);
            }
        }
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

    /// Mark listed posts seen when leaving a subreddit tab you looked at (moved, opened,
    /// or stayed a moment); passing through keeps "new" badges.
    fn mark_active_seen(&mut self) {
        let tab = &self.tabs[self.active];
        let looked = tab.engaged || tab.entered.is_some_and(|t| t.elapsed() >= SEEN_DWELL);
        if !looked || !tab.feed.loaded || !tab.feed.is_default_view() {
            return;
        }
        if let Some(name) = tab.subreddit() {
            self.state
                .mark_seen(name, tab.feed.posts.iter().map(|p| p.id.as_str()), now());
        }
    }

    pub fn badge(&self, tab: &Tab) -> usize {
        let Some(name) = tab.subreddit() else {
            return 0;
        };
        if tab.feed.loaded && tab.feed.is_default_view() {
            return self
                .state
                .unseen(name, self.config.visible(&tab.feed.posts));
        }
        self.all_subs
            .groups
            .iter()
            .find(|g| g.subreddit.eq_ignore_ascii_case(name))
            .map_or(0, |g| {
                self.state.unseen(name, self.config.visible(&g.posts))
            })
    }

    // ----- feeds ------------------------------------------------------------

    fn ensure_feed_loaded(&mut self, index: usize) {
        let tab = &self.tabs[index];
        if tab.feed_target().is_some()
            && !tab.feed.loading()
            && !(tab.feed.request_query.as_ref() == Some(&tab.feed.query())
                && tab
                    .feed
                    .requests
                    .iter()
                    .any(|(id, cursor)| cursor.is_none() && tab.feed.refreshing.contains(id)))
            && (!tab.feed.loaded
                || tab.feed.freshness.is_some_and(|f| {
                    now().saturating_sub(f.fetched_at) >= crate::cache::CONTENT_TTL
                }))
        {
            self.load_feed(index, false);
        }
        if index == self.active {
            self.prefetch_short_home();
        }
    }

    fn prefetch_short_home(&mut self) {
        let tab = self.active_tab();
        let feed = &tab.feed;
        if !matches!(tab.kind, TabKind::Home)
            || self.feed_settle.holding(Instant::now())
            || !feed.loaded
            || !feed.shows_current_query()
            || feed.loading()
            || feed.page_failed()
            || feed.after.is_none()
            || feed.pages.len() >= HOME_AUTO_PAGES
            || feed.pages.iter().any(|page| page.cursor == feed.after)
            || self.config.visible(&feed.posts).count() > PREFETCH_MARGIN
        {
            return;
        }
        self.load_feed(self.active, true);
    }

    pub fn load_feed(&mut self, index: usize, more: bool) {
        let feed = &self.tabs[index].feed;
        if more && (feed.loading() || feed.after.is_none()) {
            return;
        }
        let after = if more { feed.after.clone() } else { None };
        self.fetch_feed(index, after, false);
    }

    fn fetch_feed(&mut self, index: usize, after: Option<String>, force: bool) {
        if self.reddit.needs_sign_in() {
            return;
        }
        let generation = self.generation();
        // A first-page load ends the wait of a tab switch to this tab: once it fails, that
        // wait would ask again.
        if after.is_none() {
            self.feed_settle.forget(self.tabs[index].id);
        }
        let tab = &mut self.tabs[index];
        let Some(target) = tab.feed_target() else {
            return;
        };
        let feed = &mut tab.feed;
        let query = feed.query();
        if feed.request_query.as_ref() != Some(&query) {
            feed.requests.clear();
            feed.pending.clear();
            feed.refreshing.clear();
            feed.failures.clear();
            feed.request_query = Some(query.clone());
        }
        feed.generation = generation;
        feed.failures.remove(&after);
        feed.requests.retain(|_, cursor| *cursor != after);
        feed.pending.retain(|id| feed.requests.contains_key(id));
        feed.refreshing.retain(|id| feed.requests.contains_key(id));
        feed.requests.insert(generation, after.clone());
        feed.pending.insert(generation);
        let job = Job::Feed {
            tab: tab.id,
            generation,
            target,
            query,
            after,
        };
        if force {
            self.pool.refresh(job);
        } else {
            self.pool.submit(job);
        }
    }

    fn retry_failed_thumbs(&mut self) {
        self.thumbs.retain(|_, slot| match slot {
            ImageSlot::Failed(_) => false,
            ImageSlot::Ready { refresh, .. } if *refresh == ImageRefresh::Failed => {
                *refresh = ImageRefresh::Needed;
                true
            }
            _ => true,
        });
    }

    pub fn reload(&mut self) {
        if self.reddit.needs_sign_in() {
            self.show_auth();
            return;
        }
        self.retry_failed_thumbs();
        // Only the open post's pictures are kept: its failed ones load anew.
        self.retry_failed_pictures();
        if self.post_focused() && self.post_view.is_some() {
            self.refresh_post();
            return;
        }
        match self.active_tab().kind {
            TabKind::AllSubs => {
                self.load_subscriptions(true);
                let generation = self.all_subs.next_group_generation;
                self.all_subs.next_group_generation += 1;
                for g in &mut self.all_subs.groups {
                    Self::refresh_group(&self.pool, g, generation);
                }
            }
            TabKind::Bookmarks => {}
            TabKind::Home | TabKind::Subreddit { .. } => {
                let feed = &self.tabs[self.active].feed;
                let failed_pages: Vec<_> = feed
                    .pages
                    .iter()
                    .filter(|page| {
                        page.cursor.is_some() && feed.failures.contains_key(&page.cursor)
                    })
                    .map(|page| page.cursor.clone())
                    .collect();
                self.fetch_feed(self.active, None, true);
                for cursor in failed_pages {
                    self.fetch_feed(self.active, cursor, true);
                }
            }
        }
    }

    fn refresh_group(pool: &Pool, group: &mut Group, generation: u64) {
        group.loading = true;
        group.error = None;
        group.generation = generation;
        pool.refresh(Job::Group {
            subreddit: group.subreddit.clone(),
            generation: group.generation,
        });
    }

    fn refresh_post(&mut self) {
        let generation = self.generation();
        let Some(view) = &mut self.post_view else {
            return;
        };
        // A preview still held back loads here first: it now counts as read.
        if self.post_settle.take() == Some(view.generation) {
            self.state.mark_read(&view.post, now());
        }
        view.generation = generation;
        view.loading = true;
        view.error = None;
        view.loading_more.clear();
        self.pool.want_post(Some(generation));
        if view.comment_sort.is_empty() {
            self.pool.refresh(Job::Post {
                generation,
                permalink: view.post.permalink.clone(),
            });
        } else {
            self.pool.refresh(Job::SortedPost {
                generation,
                permalink: view.post.permalink.clone(),
                sort: view.comment_sort.clone(),
            });
        }
    }

    /// The open post's pictures that failed load again: its own image now, pictures in
    /// its text when next drawn.
    fn retry_failed_pictures(&mut self) {
        self.images.retain(|_, slot| match slot {
            ImageSlot::Failed(_) => false,
            ImageSlot::Ready { refresh, .. } if *refresh == ImageRefresh::Failed => {
                *refresh = ImageRefresh::Needed;
                true
            }
            _ => true,
        });
        self.card_images.retain(|_, slot| {
            !matches!(
                slot,
                CardImageSlot::Failed(_)
                    | CardImageSlot::Cached {
                        refreshing: false,
                        ..
                    }
            )
        });
        let image = self
            .post_view
            .as_ref()
            .and_then(|view| view.page.as_ref())
            .and_then(|page| page.post.media.preview_image())
            .map(str::to_string);
        if self.inline_images_on()
            && let Some(url) = image
        {
            self.request_image(&url);
        }
    }

    pub fn set_sort(&mut self, sort: Sort) {
        if !self.active_tab().sorts().contains(&sort) || self.active_tab().feed_target().is_none() {
            return;
        }
        let feed = &mut self.tabs[self.active].feed;
        feed.sort = sort;
        feed.search = None;
        feed.selected = 0;
        feed.cards = CardScroll::default();
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
        feed.cards = CardScroll::default();
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
        feed.cards = CardScroll::default();
        self.load_feed(self.active, false);
    }

    pub fn clear_search(&mut self) -> bool {
        let feed = &mut self.tabs[self.active].feed;
        if feed.search.take().is_none() {
            return false;
        }
        feed.selected = 0;
        feed.cards = CardScroll::default();
        self.load_feed(self.active, false);
        true
    }

    pub fn move_selection(&mut self, delta: isize) {
        self.engage();
        let before = self.active_tab().feed.selected;
        match self.active_tab().kind {
            TabKind::AllSubs => self.move_all_subs(delta),
            TabKind::Bookmarks => {
                let len = self.state.bookmarks().len();
                let feed = &mut self.tabs[self.active].feed;
                feed.selected = step(feed.selected, delta, len);
            }
            TabKind::Home | TabKind::Subreddit { .. } => {
                let len = self
                    .config
                    .visible(&self.tabs[self.active].feed.posts)
                    .count();
                let feed = &mut self.tabs[self.active].feed;
                feed.selected = step(feed.selected, delta, len);
            }
        }
        self.prefetch_list(self.active_tab().feed.selected);
        if self.active_tab().feed.selected != before {
            self.tabs[self.active].feed.cards = CardScroll::default();
        }
        self.moving();
        self.preview_selection();
    }

    pub fn page_list(&mut self, direction: isize, divisor: usize) {
        if self.state.list_view() != ListView::Card
            || matches!(self.active_tab().kind, TabKind::AllSubs)
            || self.active_tab().feed.cards.viewport < 3
        {
            self.move_selection(direction * 10);
            return;
        }
        let rows = (self.active_tab().feed.cards.viewport / divisor).max(1);
        self.scroll_card(direction * rows as isize);
    }

    pub fn scroll_card(&mut self, delta: isize) {
        if self.state.list_view() != ListView::Card
            || matches!(self.active_tab().kind, TabKind::AllSubs)
            || self.current_post().is_none()
            || self.active_tab().feed.cards.post.is_none()
        {
            return;
        }
        self.engage();
        let cards = &mut self.tabs[self.active].feed.cards;
        cards.pending = cards.pending.saturating_add(delta);
    }

    fn prefetch_list(&mut self, last_visible: usize) {
        if !matches!(
            self.active_tab().kind,
            TabKind::Home | TabKind::Subreddit { .. }
        ) {
            return;
        }
        let feed = &self.active_tab().feed;
        let len = self.config.visible(&feed.posts).count();
        // Failed pages wait for L or r.
        if last_visible.saturating_add(PREFETCH_MARGIN) >= len
            && feed.after.is_some()
            && !feed.loading()
            && !feed.page_failed()
        {
            self.load_feed(self.active, true);
        }
    }

    pub fn set_list_viewport(
        &mut self,
        selected: usize,
        offset: usize,
        scroll: CardScroll,
        scrolled_end: Option<usize>,
    ) {
        let before = self.active_tab().feed.selected;
        if selected != before {
            self.move_selection(selected as isize - before as isize);
        }
        let feed = &mut self.tabs[self.active].feed;
        feed.offset = offset;
        feed.cards = scroll;
        if let Some(last) = scrolled_end {
            self.prefetch_list(last);
        }
    }

    pub fn select_edge(&mut self, end: bool) {
        match self.active_tab().kind {
            TabKind::AllSubs => {
                self.all_subs.group = if end {
                    self.all_subs.groups.len().saturating_sub(1)
                } else {
                    0
                };
                self.all_subs.post = 0;
                self.moving();
                self.preview_selection();
            }
            _ => {
                self.move_selection(if end { isize::MAX / 2 } else { isize::MIN / 2 });
                self.tabs[self.active].feed.cards = CardScroll::default();
            }
        }
    }

    /// Posts shown in an All subs island: the first `all_subs_posts_per_group` of the hot listing
    /// that the config does not hide, pinned posts left out.
    pub fn group_posts<'a>(&self, group: &'a Group) -> impl Iterator<Item = &'a Post> {
        self.config
            .visible(&group.posts)
            .filter(|p| !p.stickied)
            .take(self.config.all_subs_posts_per_group)
    }

    fn move_all_subs(&mut self, delta: isize) {
        if self.all_subs.groups.is_empty() {
            return;
        }
        let mut remaining = delta;
        while remaining != 0 {
            let count = self
                .group_posts(&self.all_subs.groups[self.all_subs.group])
                .count();
            if remaining > 0 {
                if self.all_subs.post + 1 < count {
                    self.all_subs.post += 1;
                } else if self.all_subs.group + 1 < self.all_subs.groups.len() {
                    self.all_subs.group += 1;
                    self.all_subs.post = 0;
                } else {
                    break;
                }
                remaining -= 1;
            } else {
                if self.all_subs.post > 0 {
                    self.all_subs.post -= 1;
                } else if self.all_subs.group > 0 {
                    self.all_subs.group -= 1;
                    let count = self
                        .group_posts(&self.all_subs.groups[self.all_subs.group])
                        .count();
                    self.all_subs.post = count.saturating_sub(1);
                } else {
                    break;
                }
                remaining += 1;
            }
        }
    }

    pub fn move_all_subs_group(&mut self, delta: isize) {
        self.all_subs.group = step(self.all_subs.group, delta, self.all_subs.groups.len());
        self.all_subs.post = 0;
        self.moving();
        self.preview_selection();
    }

    pub fn community(&self) -> Option<String> {
        if self.focus == Focus::Sidebar {
            return self.active_tab().subreddit().map(str::to_string);
        }
        if self.post_focused() {
            return self
                .post_view
                .as_ref()
                .map(|view| view.shown_post().subreddit.clone());
        }
        match &self.active_tab().kind {
            TabKind::AllSubs => self
                .all_subs
                .groups
                .get(self.all_subs.group)
                .map(|group| group.subreddit.clone()),
            TabKind::Subreddit { name, .. } if is_community(name) => Some(name.clone()),
            _ => self.current_post().map(|post| post.subreddit),
        }
    }

    pub fn open_current_community(&mut self) {
        if let Some(name) = self.community().filter(|name| is_community(name)) {
            self.open_subreddit(&name);
        }
    }

    pub fn joined(&self, name: &str) -> Option<bool> {
        (self.all_subs.freshness.is_some() && self.all_subs.error.is_none()).then(|| {
            self.all_subs
                .groups
                .iter()
                .any(|group| group.subreddit.eq_ignore_ascii_case(name))
        })
    }

    pub fn membership_progress(&self) -> Option<String> {
        self.membership.as_ref().map(|change| {
            format!(
                "{} r/{}…",
                if change.checking {
                    "Checking membership for"
                } else if change.join {
                    "Joining"
                } else {
                    "Leaving"
                },
                change.subreddit
            )
        })
    }

    pub fn show_membership(&mut self) {
        if let Some(progress) = self.membership_progress() {
            self.info(progress);
            return;
        }
        let Some(subreddit) = self.community().filter(|name| is_community(name)) else {
            self.info("Select an individual subreddit first");
            return;
        };
        let Some(joined) = self.joined(&subreddit) else {
            self.load_subscriptions(true);
            self.info("Loading account subscriptions; press S again when ready");
            return;
        };
        self.overlay = Some(Overlay::Membership {
            subreddit,
            join: !joined,
        });
    }

    pub fn set_membership(&mut self, subreddit: String, join: bool) {
        if self.membership.is_some() || !is_community(&subreddit) {
            return;
        }
        let generation = self.generation();
        // Older membership reads must not settle this write or undo its confirmation.
        self.all_subs.generation += 1;
        self.all_subs.loading = false;
        self.membership = Some(MembershipChange {
            generation,
            subreddit: subreddit.clone(),
            join,
            checking: false,
            error: None,
        });
        self.status = None;
        self.pool.submit(Job::Membership {
            generation,
            subreddit,
            join,
        });
    }

    /// The post under the cursor in whatever view is active.
    pub fn current_post(&self) -> Option<Post> {
        if self.post_focused()
            && let Some(view) = &self.post_view
        {
            return Some(view.shown_post().clone());
        }
        let tab = self.active_tab();
        match tab.kind {
            TabKind::AllSubs => {
                let group = self.all_subs.groups.get(self.all_subs.group)?;
                self.group_posts(group).nth(self.all_subs.post).cloned()
            }
            TabKind::Bookmarks => self
                .state
                .bookmarks()
                .get(tab.feed.selected)
                .map(|b| b.post.clone()),
            TabKind::Home | TabKind::Subreddit { .. } => self
                .config
                .visible(&tab.feed.posts)
                .nth(tab.feed.selected)
                .cloned(),
        }
    }

    // ----- post view --------------------------------------------------------

    pub fn open_current_post(&mut self) {
        self.engage();
        if let Some(post) = self.current_post() {
            let permalink = post.permalink.clone();
            if let Some(generation) = self
                .post_view
                .as_ref()
                .filter(|view| view.post.permalink == permalink)
                .map(|view| view.generation)
            {
                self.focus = Focus::Post;
                // A post previewed in a quick run of list moves loads now.
                if self.post_settle.take() == Some(generation) {
                    self.load_post(permalink);
                }
                return;
            }
            self.open_post_permalink(post, permalink);
        } else if self.post_view.is_some() {
            self.focus = Focus::Post;
        }
    }

    fn open_post_permalink(&mut self, post: Post, permalink: String) {
        if self.show_post(post, &permalink).is_some() {
            self.focus = Focus::Post;
            self.load_post(permalink);
        }
    }

    /// A move through a list, tab or post: pictures it brings on screen are not fetched
    /// while moves keep coming quickly; the redraw once they stop fetches them.
    fn moving(&mut self) {
        self.picture_settle.arrive((), Instant::now());
    }

    /// Beside an open post, the list selection takes its place as the list moves.
    fn preview_selection(&mut self) {
        if self.focus != Focus::List || !self.detail_fits() {
            return;
        }
        let (Some(view), Some(post)) = (&self.post_view, self.current_post()) else {
            return;
        };
        if view.post.permalink == post.permalink {
            return;
        }
        let permalink = post.permalink.clone();
        if let Some(generation) = self.show_post(post, &permalink)
            && self.post_settle.arrive(generation, Instant::now())
        {
            self.load_post(permalink);
        }
    }

    /// Show `post` in the detail pane, its page not asked for yet; its view's generation,
    /// or None when it has no page.
    fn show_post(&mut self, post: Post, permalink: &str) -> Option<u64> {
        if permalink.is_empty() {
            self.error("This post has no comments link");
            return None;
        }
        self.close_post();
        let generation = self.generation();
        self.post_view = Some(PostView {
            post,
            page: None,
            loading: true,
            error: None,
            freshness: None,
            generation,
            collapsed: HashSet::new(),
            loading_more: HashSet::new(),
            removed_comments: HashMap::new(),
            cursor: 0,
            scroll: 0,
            // Start at the top of the post; the first comment key selects a comment.
            free_scroll: true,
            left: None,
            hold_view: None,
            keep_reading: None,
            drawn_cursor: None,
            rewrapped: None,
            rendered_document: Vec::new(),
            content_changed: false,
            rendered_more: Vec::new(),
            rendered_pictures: Vec::new(),
            more_handoff: false,
            image_away: false,
            image_shown: false,
            row_starts: Vec::new(),
            page_height: 0,
            reveal_spoilers: false,
            gallery_index: 0,
            comment_sort: String::new(),
        });
        Some(generation)
    }

    /// Ask for the shown post's page; from now on the post counts as read.
    fn load_post(&mut self, permalink: String) {
        let Some(view) = &self.post_view else {
            return;
        };
        self.state.mark_read(&view.post, now());
        self.pool.want_post(Some(view.generation));
        self.pool.submit(Job::Post {
            generation: view.generation,
            permalink,
        });
    }

    pub fn toggle_spoilers(&mut self) {
        let Some(view) = &mut self.post_view else {
            return;
        };
        if !view.has_spoilers() {
            self.info("No spoilers in this post");
            return;
        }
        // Masking rewraps lines: keep the text being read in place (reading: the draw
        // finds the top line again; a selection holds its text).
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
            // The comments went away with the post: read from the top; positions are stale
            // until the next draw.
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
        if self.focus == Focus::Post {
            self.focus = Focus::List;
        }
        // Full-size previews are megabytes each: keep them only while their post is open.
        self.images.clear();
        self.pool.forget_images();
        for url in self.card_images.keys() {
            self.pool.want_image(url);
        }
        self.pool.want_post(None);
    }

    /// `h` in a post: leave a comment selection for reading; false if there is none.
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
        self.moving();
        let Some(view) = &mut self.post_view else {
            return;
        };
        if view.free_scroll {
            // At the top `k` stays: anchoring would select the first comment, so held `k`
            // would flip.
            if delta < 0 && view.scroll == 0 {
                view.go_to_top();
                return;
            }
            // Before the post draws, or with positions stale after a fold in this batch, stay
            // put. A post without comments reads by line.
            if view.page.is_none() || !view.positions_current() {
                return;
            }
            let anchor = if delta < 0 { Anchor::Up } else { Anchor::Down };
            // With no comment that way, read on by a line, like C-y / C-e.
            if !view.anchor_cursor(anchor) {
                if delta < 0 && view.scroll <= 1 {
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
        self.moving();
        if let Some(view) = &mut self.post_view {
            view.cursor = 0;
            view.go_to_top();
        }
    }

    pub fn post_bottom(&mut self) {
        self.moving();
        if let Some(view) = &mut self.post_view {
            let len = view.rows().len();
            // A hidden spoiler post has no comments to select.
            if len > 0 {
                view.cursor = len - 1;
                view.free_scroll = false;
                view.left = None;
                // Already reading inside the last comment: stay rather than jump to its header;
                // otherwise the draw scrolls to it. Positions from before a fold or reveal in
                // this batch don't count.
                let inside = view.row_starts.len() == len && view.row_starts[len - 1] < view.scroll;
                view.hold_view = inside.then(|| view.hold(len - 1));
            }
        }
    }

    pub fn scroll_post(&mut self, delta: isize) {
        self.moving();
        if let Some(view) = &mut self.post_view {
            if delta < 0 && view.scroll <= delta.unsigned_abs() {
                view.go_to_top();
            } else {
                view.scroll = view.scroll.saturating_add_signed(delta);
                view.free_scroll = true;
            }
        }
    }

    /// Scroll by `num/den` of the visible document; a full page keeps one line of context.
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
        self.moving();
        let Some(view) = &mut self.post_view else {
            return;
        };
        let was_free = view.free_scroll;
        // As with `k`: going back from the post's top stays there.
        if was_free && !forward && view.scroll == 0 {
            view.go_to_top();
            return;
        }
        // As with `j`/`k`: wait for the draw before the post loads or after a fold in this batch.
        if was_free && (view.page.is_none() || !view.positions_current()) {
            return;
        }
        // Anchoring commits a selection; with no thread that way it is undone.
        let reading = was_free.then(|| (view.cursor, view.left.clone()));
        let anchor = if forward { Anchor::Down } else { Anchor::Up };
        if was_free && !view.anchor_cursor(anchor) {
            // From the body the previous thread is the post; inside the last comment none follows.
            if !forward {
                view.go_to_top();
            }
            return;
        }
        let target = {
            let rows = view.rows();
            let mut indexed = rows.iter().enumerate();
            if forward {
                // Right after anchoring, a top-level comment under the cursor counts as next.
                let stay = was_free && rows.get(view.cursor).is_some_and(|r| r.depth == 0);
                let from = if stay { view.cursor } else { view.cursor + 1 };
                indexed
                    .find(|(i, r)| *i >= from && r.depth == 0)
                    .map(|(i, _)| i)
            } else {
                // Right after anchoring below a header, that header's thread counts as previous.
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
            // No thread that way: keep reading. Up from the first thread, the previous one
            // is the post.
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

    /// Fold or unfold the comment under the cursor, or load more replies; while reading
    /// it only selects (see [`Anchor::Select`]).
    pub fn activate_comment(&mut self) {
        enum Action {
            Toggle(String),
            More { parent: String, cursor: String },
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
                    cursor: more.cursor.clone(),
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
            Some(Action::More { parent, cursor }) => {
                let first_request = view.loading_more.insert(parent.clone());
                let generation = view.generation;
                if first_request {
                    let minimum = view
                        .page
                        .as_mut()
                        .and_then(|page| find_comment_mut(&mut page.comments, &parent))
                        .and_then(|comment| comment.sources.map(|sources| sources.more))
                        .or_else(|| view.freshness.map(|freshness| freshness.source));
                    self.pool.submit(Job::MoreReplies {
                        generation,
                        comment_id: parent,
                        cursor,
                        minimum,
                    });
                }
            }
            None => {}
        }
    }

    pub fn toggle_list_view(&mut self) {
        for tab in &mut self.tabs {
            tab.feed.cards = CardScroll::default();
        }
        self.state.toggle_list_view();
        self.persist();
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
        // Icons are looked up only while thumbnails show: fetch the ones skipped meanwhile.
        let names: Vec<String> = (self.tabs.iter().filter_map(Tab::subreddit))
            .chain(self.all_subs.groups.iter().map(|g| g.subreddit.as_str()))
            .map(str::to_string)
            .collect();
        for name in names {
            self.request_icon(&name);
        }
        if self.inline_images_on()
            && let Some(view) = &mut self.post_view
            && let Some(page) = &view.page
        {
            // Turned on at the post's top: show the image now; same-pass changes treat the
            // top as showing it.
            let media = &page.post.media;
            let url = media.preview_image().map(str::to_string);
            if view.scroll == 0 {
                view.image_away = false;
                view.image_shown = media.shows_inline() && !view.spoiler_hidden();
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

    // ----- focus, layout and sidebar ----------------------------------------

    pub fn post_focused(&self) -> bool {
        self.focus == Focus::Post && self.post_view.is_some()
    }

    pub fn sidebar_width(&self) -> u16 {
        if !self.show_sidebar {
            0
        } else if self.width >= 110 {
            SIDEBAR_WIDE
        } else {
            SIDEBAR_NARROW
        }
    }

    pub fn detail_fits(&self) -> bool {
        self.width.saturating_sub(self.sidebar_width()) >= MAIN_MIN_WIDTH + DETAIL_MIN_WIDTH
    }

    pub fn resize(&mut self, width: u16) {
        self.width = width;
        if self.focus == Focus::List && self.post_view.is_some() && !self.detail_fits() {
            self.focus = Focus::Post;
        }
    }

    pub fn toggle_sidebar(&mut self) {
        self.show_sidebar = !self.show_sidebar;
        if self.focus == Focus::Sidebar {
            self.focus_main();
        }
        self.sidebar_peek = false;
        self.resize(self.width);
    }

    /// Tab indexes in sidebar order after Home, Bookmarks and All subs.
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
        self.focus = Focus::Sidebar;
    }

    pub fn focus_main(&mut self) {
        self.focus = Focus::List;
        if self.sidebar_peek {
            self.show_sidebar = false;
            self.sidebar_peek = false;
        }
        self.resize(self.width);
    }

    /// Switch to the tab `delta` rows away in the sidebar, stopping at the ends.
    pub fn move_sidebar(&mut self, delta: isize) {
        let (pinned, opened) = self.sidebar_tabs();
        let order: Vec<usize> = [0, BOOKMARKS_TAB, ALL_SUBS_TAB]
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

    // ----- pictures ---------------------------------------------------------

    pub fn card_preparation_allowed(&self) -> bool {
        self.focus != Focus::Sidebar || !self.picture_settle.holding(Instant::now())
    }

    /// The open post's image or video poster, fetched ahead of everything else queued.
    fn request_image(&mut self, url: &str) {
        self.fetch_image(url, false);
    }

    /// A picture in post or comment text, fetched in the background once: a failed one
    /// waits for `r`. Held during a quick run of moves.
    pub fn request_text_picture(&mut self, url: &str) {
        if matches!(
            self.images.get(url),
            None | Some(ImageSlot::Ready {
                refresh: ImageRefresh::Needed,
                ..
            })
        ) && !self.picture_settle.holding(Instant::now())
        {
            self.fetch_image(url, true);
        }
    }

    /// A picture in post or comment text now far from view: let it go, loaded or still
    /// queued (the fetch is skipped), so the ones in view come first. Asked for again
    /// when near.
    pub fn drop_text_picture(&mut self, url: &str) {
        if !matches!(self.images.get(url), Some(ImageSlot::Failed(_))) {
            self.images.remove(url);
            if !self.card_images.contains_key(url) {
                self.pool.unwant_image(url);
            }
        }
    }

    pub fn card_pictures_drawn(&mut self, urls: &[&str]) {
        let keep_pixels = self.inline_images_on();
        if !keep_pixels {
            self.card_renderer.clear();
            self.card_previews.clear();
            self.card_preview_order.clear();
        }
        let offscreen: Vec<String> = self
            .card_images
            .keys()
            .filter(|url| !urls.contains(&url.as_str()))
            .cloned()
            .collect();
        for url in offscreen {
            match self.card_images.get_mut(&url) {
                Some(CardImageSlot::Ready { protocol, .. }) if keep_pixels => {
                    *protocol = None;
                }
                _ => {
                    self.card_images.remove(&url);
                }
            }
            if !self.images.contains_key(&url) {
                self.pool.unwant_image(&url);
            }
        }
        self.card_image_order
            .retain(|url| self.card_images.contains_key(url));
        for &url in urls {
            self.card_image_order.retain(|cached| cached != url);
            self.card_image_order.push_back(url.to_string());
            if self.card_previews.contains_key(url) {
                self.card_preview_order.retain(|cached| cached != url);
                self.card_preview_order.push_back(url.to_string());
            }
        }
        let cached: Vec<_> = self
            .card_image_order
            .iter()
            .filter(|url| !urls.contains(&url.as_str()))
            .filter_map(|url| match self.card_images.get(url) {
                Some(CardImageSlot::Ready { image, .. }) => {
                    Some((url.clone(), image.as_bytes().len()))
                }
                _ => None,
            })
            .collect();
        let mut count = cached.len();
        let mut bytes: usize = cached.iter().map(|(_, bytes)| bytes).sum();
        for (url, size) in cached {
            if count <= CARD_CACHE_MAX && bytes <= CARD_CACHE_BYTES {
                break;
            }
            self.card_images.remove(&url);
            self.card_image_order.retain(|cached| cached != &url);
            count -= 1;
            bytes -= size;
        }
        let settled = !self.picture_settle.holding(Instant::now());
        for &url in urls {
            if !self.card_images.contains_key(url) {
                let slot = match self.card_previews.get(url) {
                    Some(image) => CardImageSlot::Cached {
                        image: image.clone(),
                        protocol: None,
                        refreshing: true,
                    },
                    None if settled => CardImageSlot::Loading,
                    None => continue,
                };
                self.card_images.insert(url.to_string(), slot);
            }
            if settled
                && matches!(
                    self.card_images.get(url),
                    Some(
                        CardImageSlot::Loading
                            | CardImageSlot::Cached {
                                refreshing: true,
                                ..
                            }
                    )
                )
            {
                self.pool.want_image(url);
                if let std::collections::hash_map::Entry::Vacant(entry) =
                    self.queued_images.entry(url.to_string())
                {
                    entry.insert(true);
                    self.pool.submit(Job::Image {
                        url: url.to_string(),
                        background: true,
                    });
                }
            }
        }
    }

    fn fetch_image(&mut self, url: &str, background: bool) {
        let fetch = match self.images.get_mut(url) {
            None | Some(ImageSlot::Failed(_)) => {
                self.images.insert(url.to_string(), ImageSlot::Loading);
                true
            }
            Some(ImageSlot::Ready { refresh, .. }) if *refresh == ImageRefresh::Needed => {
                *refresh = ImageRefresh::Loading;
                true
            }
            _ => false,
        };
        if fetch {
            self.pool.want_image(url);
            // A fetch still queued for it runs now that it is wanted again.
            let queued = self.queued_images.insert(url.to_string(), background);
            if queued.is_none() || (!background && queued == Some(true)) {
                self.pool.submit(Job::Image {
                    url: url.to_string(),
                    background,
                });
            }
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

    fn no_images_possible(&self) -> bool {
        self.no_color && self.image_picker.protocol_type() == ProtocolType::Halfblocks
    }

    /// A thumbnail or icon URL to draw, unless it failed; its row slot is reserved regardless.
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
        self.usable_thumb(icon.and_then(Option::as_deref))
    }

    /// Look a subreddit's icon up once per run, again only after a failure; the worker keeps
    /// it on disk for days, so reloading a feed does not spend a request on it. Multireddits
    /// (`a+b`) have no icon. Only while thumbnails are on: nothing draws it otherwise.
    fn request_icon(&mut self, subreddit: &str) {
        let key = subreddit.to_ascii_lowercase();
        if !self.thumbnails_on()
            || matches!(key.as_str(), "all" | "popular")
            || key.contains('+')
            || self.icons.contains_key(&key)
            || self.icon_lookups.contains(&key)
        {
            return;
        }
        self.icon_lookups.insert(key.clone());
        self.pool.submit(Job::Icon { subreddit: key });
    }

    /// After a frame: load drawn thumbnails, let off-screen queued ones be skipped, and past
    /// `THUMB_CACHE_MAX` keep only what is on screen or in flight. Rows a quick run of moves
    /// only passes are not fetched; the redraw once it stops fetches what it shows.
    pub fn thumbs_drawn(&mut self, urls: &[&str]) {
        self.pool.set_on_screen(urls);
        if !self.picture_settle.holding(Instant::now()) {
            for url in urls {
                let submit = match self.thumbs.get_mut(*url) {
                    None => {
                        self.thumbs.insert(url.to_string(), ImageSlot::Loading);
                        true
                    }
                    Some(ImageSlot::Ready { refresh, .. }) if *refresh == ImageRefresh::Needed => {
                        *refresh = ImageRefresh::Loading;
                        true
                    }
                    _ => false,
                };
                if submit {
                    self.pool.submit(Job::Thumb {
                        url: url.to_string(),
                    });
                }
            }
        }
        if self.thumbs.len() > THUMB_CACHE_MAX {
            let drawn: HashSet<&str> = urls.iter().copied().collect();
            self.thumbs.retain(|url, slot| {
                matches!(
                    slot,
                    ImageSlot::Loading
                        | ImageSlot::Ready {
                            refresh: ImageRefresh::Loading,
                            ..
                        }
                ) || drawn.contains(url.as_str())
            });
        }
    }

    // ----- actions: current post, Reddit ------------------------------------

    pub fn open_in_browser(&mut self) {
        if let Some(target) = self.comment_target() {
            self.open_reddit_link(&target.permalink);
            return;
        }
        let Some(post) = self.current_post() else {
            return;
        };
        match media::spawn_detached(&self.config.commands.browser, &[post.reddit_url()]) {
            Ok(()) => self.info("Opened on reddit.com"),
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    pub fn copy_link(&mut self) {
        if let Some(target) = self.comment_target() {
            match target.url().and_then(|url| media::copy(&url)) {
                Ok(tool) => self.info(format!("Copied comment link ({tool})")),
                Err(e) => self.error(format!("copy failed: {e:#}")),
            }
            return;
        }
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
        // Persist bookmarks at once; after the message so a save error replaces it.
        self.persist();
    }

    pub fn open_media(&mut self) {
        let Some(post) = self.current_post() else {
            return;
        };
        self.hand_off_media(&post.permalink, post.media);
    }

    fn hand_off_media(&mut self, permalink: &str, media: Media) {
        let commands = &self.config.commands;
        let result = if let Some(url) = media.video_url() {
            let started = self
                .reddit
                .media_url(url)
                .and_then(|url| media::spawn_media(&commands.video, &[url]));
            // The window plays it now: the inline copy waits, so two do not sound.
            if started.is_ok()
                && let Some(player) = &mut self.video
                && !player.paused()
            {
                player.toggle_pause(Instant::now());
            }
            started
        } else {
            match media {
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
                Media::Link { url, .. } => media::spawn_detached(&commands.browser, &[url]),
                Media::Gallery { .. } | Media::Video { .. } => {
                    if self.pending_media.insert(permalink.to_string()) {
                        self.pool.submit(Job::ResolveMedia {
                            permalink: permalink.to_string(),
                        });
                    }
                    self.info("Fetching media…");
                    return;
                }
            }
        };
        match result {
            Ok(()) => self.info("Opened media"),
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    /// One image download per post at a time, so a second `m` while it runs can't open
    /// the viewer twice.
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

    pub fn media_loading(&self) -> bool {
        !self.pending_media.is_empty()
    }

    // ----- picker -----------------------------------------------------------

    /// Picker rows: tabs starting with the input, then "open r/<input>", then tabs
    /// containing it, then fuzzy matches. Row 0 is the default, so a new name isn't
    /// hijacked by a longer subscription containing it.
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
        prefix.sort_by_key(|(_, t)| name_of(t) != lower);
        let mut entries: Vec<PickerEntry> = prefix.into_iter().map(tab_entry).collect();
        // Only an exact subreddit tab makes "open r/<query>" redundant; Home and Bookmarks
        // aren't subreddits.
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
        // New list data can put another post under the cursor: the side pane follows it.
        let list_data = match &msg {
            Msg::Snapshot { message, .. } => {
                matches!(
                    **message,
                    Msg::Feed { .. } | Msg::Group { .. } | Msg::Subscriptions { .. }
                )
            }
            message => matches!(
                message,
                Msg::Feed { .. } | Msg::Group { .. } | Msg::Subscriptions { .. }
            ),
        };
        self.on_result(msg, Source::default(), false);
        if list_data {
            self.prefetch_short_home();
            self.preview_selection();
        }
    }

    fn on_result(&mut self, msg: Msg, source: Source, refreshing: bool) {
        match msg {
            Msg::SortedPost {
                generation,
                sort,
                result,
            } => {
                if self
                    .post_view
                    .as_ref()
                    .is_some_and(|view| view.generation == generation && view.comment_sort == sort)
                {
                    self.on_result(Msg::Post { generation, result }, source, refreshing);
                }
            }
            Msg::Experience { generation, result } => self.on_experience(generation, result),
            Msg::Membership { generation, result } => self.on_membership(generation, result),
            Msg::Subscriptions { generation, result } => self.on_subscriptions(
                generation,
                result,
                Freshness {
                    fetched_at: now(),
                    source,
                },
                refreshing,
            ),
            Msg::Snapshot {
                message,
                fetched_at,
                refreshing,
                source,
            } => {
                self.on_snapshot(*message, Freshness { fetched_at, source }, refreshing);
            }
            Msg::CacheWarning(warning) => {
                if !self.cache_warnings.contains(&warning) {
                    self.error(warning);
                    if let Some(status) = &mut self.status {
                        status.cache_warning = true;
                    }
                }
            }
            Msg::Feed {
                tab,
                generation,
                append,
                result,
            } => self.on_feed(
                tab,
                generation,
                append,
                result,
                Freshness {
                    fetched_at: now(),
                    source,
                },
            ),
            Msg::Group {
                subreddit,
                generation,
                result,
            } => self.on_group(
                &subreddit,
                generation,
                result,
                Freshness {
                    fetched_at: now(),
                    source,
                },
            ),
            Msg::Post { generation, result } => {
                let Some(view) = &mut self.post_view else {
                    return;
                };
                if view.generation != generation {
                    // An earlier load of this same post, still on its way when you came
                    // back to it: take its network answer rather than wait for the newer
                    // load, while this view has nothing, or only a cached copy refreshing
                    // that the answer is provably no older than (`r` keeps an up-to-date
                    // copy loading). Never a cached answer: one still refreshing may rely
                    // on a network load the worker has since dropped.
                    let waiting = view.page.is_none()
                        || (view.loading
                            && view
                                .freshness
                                .is_some_and(|f| f.source.cached && source.may_replace(f.source)));
                    let same_post = !source.cached
                        && view.comment_sort.is_empty()
                        && generation > self.content_generation_floor
                        && waiting
                        && result.as_ref().is_ok_and(|page| {
                            !page.post.id.is_empty() && page.post.id == view.post.id
                        });
                    if !same_post {
                        return;
                    }
                    view.generation = generation;
                    // Reply loads sent under the newer generation are dropped on arrival.
                    view.loading_more.clear();
                    self.pool.want_post(Some(generation));
                }
                view.loading = false;
                match result {
                    Ok(page) => {
                        let mut page = *page;
                        filter_removed_comments(&mut page.comments, source, &view.removed_comments);
                        let selected = view.cursor_key();
                        view.content_changed = true;
                        view.pin_view();
                        if let Some(old) = &mut view.page {
                            merge_comments(
                                &mut page.comments,
                                &mut old.comments,
                                source,
                                false,
                                &mut view.removed_comments,
                            );
                        } else {
                            merge_comments(
                                &mut page.comments,
                                &mut Vec::new(),
                                source,
                                false,
                                &mut view.removed_comments,
                            );
                        }
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
                        // The bookmark learns a spoiler tag its saved listing may lack.
                        if page.post.spoiler {
                            self.state.mark_bookmark_spoiler(&page.post.id);
                        }
                        view.page = Some(page);
                        view.error = None;
                        view.reselect(selected);
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
                    Ok(ReplyPage {
                        comments: mut replies,
                        more,
                        refreshed,
                        order,
                    }) => {
                        filter_removed_comments(&mut replies, source, &view.removed_comments);
                        view.content_changed = true;
                        // Rows above may grow: keep the selection and the text being read in place.
                        let mut selected = view.cursor_key();
                        if !view.more_handoff
                            && view.free_scroll
                            && let Some((_, start)) =
                                view.rendered_more.iter().find(|(id, start)| {
                                    id == &comment_id
                                        && (*start == view.scroll
                                            || *start == view.scroll.saturating_add(1))
                                })
                        {
                            let offset = view.scroll as isize - *start as isize;
                            view.keep_reading =
                                Some((RowKey::More(comment_id.clone()), offset, view.scroll));
                            view.more_handoff = true;
                        }
                        view.pin_view();
                        let more_at = view.row_index(&RowKey::More(comment_id.clone()));
                        let selected_more =
                            matches!(&selected, Some(RowKey::More(id)) if *id == comment_id);
                        let reading_more = matches!(&view.keep_reading, Some((RowKey::More(id), _, _)) if *id == comment_id);
                        let mut first_added = None;
                        if let Some(page) = &mut view.page
                            && let Some(c) = find_comment_mut(&mut page.comments, &comment_id)
                        {
                            let allow_new = !replies_superseded(c, source);
                            if selected_more || reading_more {
                                first_added = first_new_reply(
                                    &replies,
                                    &c.replies,
                                    source,
                                    allow_new,
                                    &view.collapsed,
                                )
                                .map(|id| RowKey::Comment(id.to_owned()));
                            }
                            merge_replies(
                                &mut c.replies,
                                replies,
                                source,
                                allow_new,
                                ReplyMerge::Expansion {
                                    reorder: c
                                        .sources
                                        .is_none_or(|old| source.may_replace(old.more)),
                                    refreshed: refreshed.as_deref(),
                                    order: &order,
                                },
                                &mut view.removed_comments,
                            );
                            let mut sources = c.sources.unwrap_or(CommentSources {
                                content: source,
                                more: source,
                                parent: None,
                            });
                            if source.may_replace(sources.more) {
                                // An empty preview has no replacement row: keep the More
                                // target until terminal delivery.
                                if !refreshing {
                                    c.more = more;
                                }
                                sources.more = source;
                                c.sources = Some(sources);
                            }
                        }
                        if selected_more {
                            selected = if reading_more {
                                first_added.clone().or(selected)
                            } else {
                                first_added.take().or(selected)
                            };
                        }
                        view.reselect(selected);
                        view.row_starts.clear();
                        // A selected or read More row follows its first inserted reply,
                        // even with a newer continuation pending.
                        let reading = view.keep_reading.take();
                        view.keep_reading =
                            reading.map(|(key, offset, scroll)| match (&key, more_at) {
                                (RowKey::More(id), index) if *id == comment_id => {
                                    let next = first_added.take().unwrap_or_else(|| {
                                        if view.row_index(&key).is_some() {
                                            key
                                        } else {
                                            index
                                                .and_then(|i| view.rows().get(i).map(RowKey::of))
                                                .unwrap_or(key)
                                        }
                                    });
                                    (next, offset, scroll)
                                }
                                _ => (key, offset, scroll),
                            });
                    }
                    Err(e) => self.error(format!("more replies: {e}")),
                }
            }
            Msg::Image { url, result } => {
                if !refreshing {
                    self.queued_images.remove(&url);
                }
                // A cached snapshot stays usable until its live refresh finishes.
                let updating = matches!(
                    self.images.get(&url),
                    Some(ImageSlot::Ready {
                        refresh: ImageRefresh::Loading,
                        ..
                    })
                );
                if updating && result.is_err() {
                    if let Some(ImageSlot::Ready { refresh, .. }) = self.images.get_mut(&url) {
                        *refresh = ImageRefresh::Failed;
                    }
                } else if matches!(self.images.get(&url), Some(ImageSlot::Loading)) || updating {
                    let slot = match &result {
                        Ok(img) => ImageSlot::Ready {
                            pixels: (img.image.width(), img.image.height()),
                            protocol: Box::new(
                                self.image_picker.new_resize_protocol((*img.image).clone()),
                            ),
                            refresh: if refreshing {
                                ImageRefresh::Loading
                            } else {
                                ImageRefresh::Idle
                            },
                        },
                        Err(e) => ImageSlot::Failed(e.clone()),
                    };
                    self.images.insert(url.clone(), slot);
                }
                if matches!(
                    self.card_images.get(&url),
                    Some(
                        CardImageSlot::Loading
                            | CardImageSlot::Cached {
                                refreshing: true,
                                ..
                            }
                    )
                ) {
                    if result.is_err()
                        && let Some(CardImageSlot::Cached { refreshing, .. }) =
                            self.card_images.get_mut(&url)
                    {
                        *refreshing = false;
                        return;
                    }
                    let slot = match result {
                        Ok(image) => {
                            self.card_image_metadata.insert(url.clone(), image.metadata);
                            self.card_previews.insert(url.clone(), image.preview);
                            let image = image.image;
                            self.card_preview_order.retain(|cached| cached != &url);
                            self.card_preview_order.push_back(url.clone());
                            while self.card_preview_order.len() > CARD_PREVIEW_MAX {
                                if let Some(old) = self.card_preview_order.pop_front() {
                                    self.card_previews.remove(&old);
                                }
                            }
                            if refreshing {
                                CardImageSlot::Cached {
                                    image,
                                    protocol: None,
                                    refreshing: true,
                                }
                            } else {
                                CardImageSlot::Ready {
                                    image,
                                    protocol: None,
                                }
                            }
                        }
                        Err(error) => CardImageSlot::Failed(error),
                    };
                    self.card_images.insert(url, slot);
                }
            }
            // Skipped as unwanted; wanted again since (the slot waits), it goes back in.
            Msg::ImageSkipped { url } => {
                if let Some(background) = self.queued_images.remove(&url)
                    && (matches!(self.images.get(&url), Some(ImageSlot::Loading))
                        || (!self.picture_settle.holding(Instant::now())
                            && matches!(
                                self.card_images.get(&url),
                                Some(
                                    CardImageSlot::Loading
                                        | CardImageSlot::Cached {
                                            refreshing: true,
                                            ..
                                        }
                                )
                            )))
                {
                    self.queued_images.insert(url.clone(), background);
                    self.pool.submit(Job::Image { url, background });
                }
            }
            Msg::Thumb { url, result } => {
                let slot = match result {
                    Ok(img) => {
                        let img = fill_slot(img, self.image_picker.font_size());
                        ImageSlot::Ready {
                            pixels: (img.width(), img.height()),
                            protocol: Box::new(self.image_picker.new_resize_protocol(img)),
                            refresh: if refreshing {
                                ImageRefresh::Loading
                            } else {
                                ImageRefresh::Idle
                            },
                        }
                    }
                    Err(e) => {
                        if let Some(ImageSlot::Ready { refresh, .. }) = self.thumbs.get_mut(&url) {
                            *refresh = if refreshing {
                                ImageRefresh::Loading
                            } else {
                                ImageRefresh::Failed
                            };
                            return;
                        }
                        ImageSlot::Failed(e)
                    }
                };
                self.thumbs.insert(url, slot);
            }
            Msg::Icon { subreddit, result } => {
                let key = subreddit.to_ascii_lowercase();
                self.icon_lookups.remove(&key);
                // A failure keeps a stale disk answer; with none, the next load asks again.
                if let Ok(icon) = result {
                    self.icons.insert(key, icon);
                }
            }
            // Forget it so it reloads if it scrolls back; a skipped refresh of a shown
            // image keeps it.
            Msg::ThumbSkipped { url } => {
                if let Some(ImageSlot::Ready { refresh, .. }) = self.thumbs.get_mut(&url) {
                    *refresh = ImageRefresh::Needed;
                } else {
                    self.thumbs.remove(&url);
                }
            }
            Msg::MediaResolved { permalink, result } => {
                if !self.pending_media.remove(&permalink) {
                    return;
                }
                match result {
                    Ok(Media::Gallery { images, .. }) if images.is_empty() => {
                        self.error("Gallery has no readable images")
                    }
                    Ok(Media::Video { url: None, .. }) => {
                        self.error("This video has no playable source")
                    }
                    Ok(media) => self.hand_off_media(&permalink, media),
                    Err(e) => self.error(format!("media: {e}")),
                }
            }
            Msg::ImagesDownloaded { permalink, result } => {
                self.pending_media.remove(&permalink);
                match result {
                    Ok(paths) => {
                        let args: Vec<String> =
                            paths.iter().map(|p| p.display().to_string()).collect();
                        match media::spawn_detached(&self.config.commands.image, &args) {
                            Ok(()) => self.info("Opened media"),
                            Err(e) => self.error(format!("{e:#}")),
                        }
                    }
                    Err(e) => self.error(format!("image download: {e}")),
                }
            }
        }
    }

    fn on_snapshot(&mut self, message: Msg, freshness: Freshness, refreshing: bool) {
        match message {
            Msg::Subscriptions { generation, result } => {
                self.on_subscriptions(generation, result, freshness, refreshing)
            }
            Msg::Feed {
                tab,
                generation,
                append,
                result,
            } => {
                self.on_feed(tab, generation, append, result, freshness);
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab)
                    && tab.feed.accepts(generation)
                    && refreshing
                {
                    tab.feed.refreshing.insert(generation);
                }
            }
            Msg::Group {
                subreddit,
                generation,
                result,
            } => {
                if !self
                    .all_subs
                    .groups
                    .iter()
                    .any(|g| g.subreddit == subreddit && g.generation == generation)
                {
                    return;
                }
                self.on_group(&subreddit, generation, result, freshness);
                if let Some(group) = self
                    .all_subs
                    .groups
                    .iter_mut()
                    .find(|g| g.subreddit == subreddit)
                {
                    group.freshness = Some(freshness);
                    group.loading = refreshing;
                }
            }
            Msg::Post { generation, result } => {
                self.on_result(
                    Msg::Post { generation, result },
                    freshness.source,
                    refreshing,
                );
                if let Some(view) = &mut self.post_view
                    && view.generation == generation
                {
                    view.freshness = Some(freshness);
                    view.loading = refreshing;
                }
            }
            Msg::MoreReplies {
                generation,
                comment_id,
                result,
            } => {
                // The post may have refreshed after this expansion was submitted.
                let revalidate = if freshness.source.cached && !refreshing {
                    self.post_view
                        .as_mut()
                        .filter(|view| view.generation == generation)
                        .and_then(|view| view.page.as_mut())
                        .and_then(|page| find_comment_mut(&mut page.comments, &comment_id))
                        .and_then(|comment| {
                            let minimum = comment.sources?.more;
                            let more = comment.more.as_ref()?;
                            (!freshness.source.may_replace(minimum))
                                .then(|| (more.cursor.clone(), minimum))
                        })
                } else {
                    None
                };
                let refreshing = refreshing || revalidate.is_some();
                if let Some((cursor, minimum)) = revalidate {
                    self.pool.refresh(Job::MoreReplies {
                        generation,
                        comment_id: comment_id.clone(),
                        cursor,
                        minimum: Some(minimum),
                    });
                }
                self.on_result(
                    Msg::MoreReplies {
                        generation,
                        comment_id: comment_id.clone(),
                        result,
                    },
                    freshness.source,
                    refreshing,
                );
                if refreshing
                    && let Some(view) = &mut self.post_view
                    && view.generation == generation
                {
                    view.loading_more.insert(comment_id);
                }
            }
            other => self.on_result(other, freshness.source, refreshing),
        }
    }

    fn on_feed(
        &mut self,
        tab_id: u64,
        generation: u64,
        append: bool,
        result: Result<Listing, String>,
        freshness: Freshness,
    ) {
        let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) else {
            return;
        };
        let feed = &mut tab.feed;
        if !feed.accepts(generation) {
            return;
        }
        feed.pending.remove(&generation);
        feed.refreshing.remove(&generation);
        let cursor = if append {
            feed.requests
                .get(&generation)
                .cloned()
                .flatten()
                .or_else(|| feed.after.clone())
        } else {
            None
        };
        match result {
            Ok(listing) => {
                let failed_next = feed.page_failed().then(|| feed.after.clone()).flatten();
                let same_query = feed.shown_query.as_ref() == Some(&feed.query());
                let selected = same_query
                    .then(|| {
                        self.config
                            .visible(&feed.posts)
                            .nth(feed.selected)
                            .map(|p| p.id.clone())
                    })
                    .flatten();
                let screen_row = feed.selected.saturating_sub(feed.offset);
                if !same_query {
                    feed.posts.clear();
                    feed.pages.clear();
                }
                if !feed.replace_page(
                    cursor.clone(),
                    listing.posts,
                    listing.after,
                    freshness.source,
                ) {
                    return;
                }
                if !feed.failures.get(&cursor).is_some_and(|failure| {
                    failure.source.compare_requests(freshness.source)
                        == Some(std::cmp::Ordering::Greater)
                }) {
                    feed.failures.remove(&cursor);
                }
                if let Some(failed) = failed_next
                    && feed.after.as_ref() != Some(&failed)
                {
                    feed.failures.remove(&Some(failed));
                }
                if !append {
                    feed.freshness = Some(freshness);
                }
                let visible: Vec<_> = self.config.visible(&feed.posts).collect();
                feed.selected = selected
                    .and_then(|id| visible.iter().position(|p| p.id == id))
                    .unwrap_or(if same_query { feed.selected } else { 0 })
                    .min(visible.len().saturating_sub(1));
                feed.offset = feed.selected.saturating_sub(screen_row);
                feed.loaded = true;
                feed.shown_query = Some(feed.query());
                let feed = &tab.feed;
                if feed.is_default_view()
                    && let Some(name) = tab.subreddit()
                {
                    self.state.baseline(name, &feed.posts, now());
                }
                if !append && let Some(name) = tab.subreddit().map(str::to_string) {
                    self.request_icon(&name);
                }
            }
            Err(e) => {
                // Suppress only when reservations or observed starts prove this failure
                // predates the shown page.
                if feed
                    .pages
                    .iter()
                    .find(|page| page.cursor == cursor)
                    .is_some_and(|page| {
                        page.source.compare_requests(freshness.source)
                            == Some(std::cmp::Ordering::Greater)
                    })
                {
                    return;
                }
                feed.failures.insert(
                    cursor,
                    FeedFailure {
                        error: e.clone(),
                        source: freshness.source,
                    },
                );
                // A failed load of a new query must not leave the old query's posts and cursor
                // (`L` would append to them); a failed reload of the same query keeps them.
                if !append && feed.shown_query.as_ref() != Some(&feed.query()) {
                    feed.posts.clear();
                    feed.after = None;
                    feed.selected = 0;
                    feed.offset = 0;
                    feed.shown_query = None;
                    feed.freshness = None;
                    feed.pages.clear();
                }
                let title = tab.title();
                self.error(format!("{title}: {e}"));
            }
        }
    }

    fn on_group(
        &mut self,
        subreddit: &str,
        generation: u64,
        result: Result<Listing, String>,
        freshness: Freshness,
    ) {
        let Some(index) = self
            .all_subs
            .groups
            .iter()
            .position(|g| g.subreddit == subreddit && g.generation == generation)
        else {
            return;
        };
        let selected = (index == self.all_subs.group)
            .then(|| {
                self.group_posts(&self.all_subs.groups[index])
                    .nth(self.all_subs.post)
                    .map(|p| p.id.clone())
            })
            .flatten();
        let group = &mut self.all_subs.groups[index];
        group.loading = false;
        match result {
            Ok(mut listing) => {
                // All subs and the default first page share a cache key and response ordering.
                let query = FeedQuery::DEFAULT_LISTING;
                let seeded_tab = self.tabs.iter().position(|tab| {
                    let feed = &tab.feed;
                    tab.subreddit() == Some(subreddit)
                        && feed.query() == query
                        && (feed.shown_query.as_ref() != Some(&query)
                            || feed
                                .pages
                                .first()
                                .is_none_or(|page| freshness.source.may_replace(page.source)))
                });
                // Only a seeded tab needs its own copy of the first page.
                group.posts = if seeded_tab.is_some() {
                    listing.posts.clone()
                } else {
                    std::mem::take(&mut listing.posts)
                };
                group.error = None;
                self.state.baseline(subreddit, &group.posts, now());
                self.request_icon(subreddit);
                // A refresh can return fewer posts: keep the selection on one.
                if index == self.all_subs.group {
                    let shown: Vec<_> = self.group_posts(&self.all_subs.groups[index]).collect();
                    self.all_subs.post = selected
                        .and_then(|id| shown.iter().position(|p| p.id == id))
                        .unwrap_or(self.all_subs.post)
                        .min(shown.len().saturating_sub(1));
                }
                if let Some(i) = seeded_tab {
                    let generation = self.generation();
                    let tab = &mut self.tabs[i];
                    let feed = &mut tab.feed;
                    // Pending first pages stay: a newer one replaces this response; an
                    // older one is rejected by its source.
                    feed.requests.retain(|id, cursor| {
                        cursor.is_some()
                            || feed.pending.contains(id)
                            || feed.refreshing.contains(id)
                    });
                    feed.pending.retain(|id| match feed.requests.get(id) {
                        Some(None) => {
                            feed.refreshing.insert(*id);
                            false
                        }
                        Some(Some(_)) => true,
                        None => false,
                    });
                    feed.generation = generation;
                    feed.requests.insert(generation, None);
                    feed.request_query = Some(query);
                    let tab_id = tab.id;
                    self.on_feed(tab_id, generation, false, Ok(listing), freshness);
                }
            }
            Err(e) => group.error = Some(e),
        }
    }
}

fn filter_removed_comments(
    comments: &mut Vec<Comment>,
    source: Source,
    removed: &HashMap<String, Source>,
) {
    comments.retain_mut(|comment| {
        if removed
            .get(&comment.id)
            .is_some_and(|old| !source.may_replace(*old))
        {
            return false;
        }
        filter_removed_comments(&mut comment.replies, source, removed);
        true
    });
}

fn record_removal(removed: &mut HashMap<String, Source>, id: &str, source: Source) {
    if removed.get(id).is_none_or(|old| source.may_replace(*old)) {
        removed.insert(id.to_owned(), source);
    }
}

/// Merge partial trees without letting older snapshots replace newer fields or continuations.
fn merge_comments(
    incoming: &mut Vec<Comment>,
    previous: &mut Vec<Comment>,
    source: Source,
    keep_missing: bool,
    removed: &mut HashMap<String, Source>,
) {
    for comment in incoming.iter_mut() {
        let old = previous.iter_mut().find(|old| old.id == comment.id);
        merge_comment(comment, old, source, removed);
    }
    if previous.is_empty() {
        return;
    }
    let known: HashSet<_> = incoming.iter().map(|comment| comment.id.as_str()).collect();
    previous.retain_mut(|comment| {
        let retained = !known.contains(comment.id.as_str())
            && (keep_missing || comment_newer_than(comment, source));
        if retained && let Some(sources) = &mut comment.sources {
            sources.parent = Some(parent_membership(sources.parent, source, false));
        }
        if !retained && !known.contains(comment.id.as_str()) {
            record_removal(removed, &comment.id, source);
        }
        retained
    });
    drop(known);
    incoming.append(previous);
}

fn parent_membership(
    previous: Option<ParentMembership>,
    source: Source,
    present: bool,
) -> ParentMembership {
    previous
        .filter(|old| !source.may_replace(old.source))
        .unwrap_or(ParentMembership { source, present })
}

fn merge_comment(
    incoming: &mut Comment,
    previous: Option<&mut Comment>,
    source: Source,
    removed: &mut HashMap<String, Source>,
) {
    let Some(previous) = previous else {
        incoming.sources = Some(CommentSources {
            content: source,
            more: source,
            parent: Some(ParentMembership {
                source,
                present: true,
            }),
        });
        for child in &mut incoming.replies {
            merge_comment(child, None, source, removed);
        }
        return;
    };
    let old = previous.sources.unwrap_or_default();
    let keep_content = !source.may_replace(old.content);
    let keep_more = !source.may_replace(old.more);
    let keep_children = replies_superseded(previous, source);
    let incoming_children = std::mem::take(&mut incoming.replies);
    let mut previous_children = std::mem::take(&mut previous.replies);
    let incoming_more = incoming.more.take();
    let previous_more = previous.more.take();
    let more = if keep_more {
        previous_more
    } else {
        incoming_more
    };
    if keep_content {
        std::mem::swap(incoming, previous);
    }
    if keep_children || keep_content {
        incoming.replies = previous_children;
        merge_replies(
            &mut incoming.replies,
            incoming_children,
            source,
            !keep_children,
            ReplyMerge::ParentSnapshot,
            removed,
        );
    } else {
        incoming.replies = incoming_children;
        merge_comments(
            &mut incoming.replies,
            &mut previous_children,
            source,
            more.is_some(),
            removed,
        );
    }
    incoming.more = more;
    incoming.sources = Some(CommentSources {
        content: if keep_content { old.content } else { source },
        more: if keep_more { old.more } else { source },
        parent: Some(parent_membership(old.parent, source, true)),
    });
}

enum ReplyMerge<'a> {
    ParentSnapshot,
    Expansion {
        reorder: bool,
        refreshed: Option<&'a [String]>,
        order: &'a [String],
    },
}

fn merge_replies(
    existing: &mut Vec<Comment>,
    replies: Vec<Comment>,
    source: Source,
    allow_new: bool,
    kind: ReplyMerge<'_>,
    removed: &mut HashMap<String, Source>,
) {
    let expansion = matches!(&kind, ReplyMerge::Expansion { .. });
    let order = match &kind {
        ReplyMerge::Expansion {
            reorder: true,
            order,
            ..
        } if allow_new => Some(*order),
        _ => None,
    };
    let covered = |id: &str| match &kind {
        ReplyMerge::Expansion {
            refreshed: Some(ids),
            ..
        } => ids.iter().any(|candidate| candidate == id),
        _ => true,
    };
    // A More response replaces unconfirmed children within its coverage; current
    // parent membership and independently newer content still win.
    if expansion {
        let protected = |comment: &Comment| {
            comment
                .sources
                .is_none_or(|s| s.parent.is_some_and(|p| p.present))
                || comment_newer_than(comment, source)
        };
        if let ReplyMerge::Expansion {
            refreshed: Some(ids),
            ..
        } = &kind
        {
            for id in *ids {
                if find_comment(&replies, id).is_none()
                    && find_comment(existing, id).is_none_or(|c| !protected(c))
                {
                    record_removal(removed, id, source);
                }
            }
        }
        existing.retain(|old| {
            let retained = protected(old)
                || !covered(&old.id)
                || replies.iter().any(|reply| reply.id == old.id);
            if !retained {
                record_removal(removed, &old.id, source);
            }
            retained
        });
    }
    for mut reply in replies {
        if let Some(at) = existing.iter().position(|old| old.id == reply.id) {
            let old = &mut existing[at];
            let parent = old.sources.and_then(|sources| sources.parent);
            merge_comment(&mut reply, Some(old), source, removed);
            if expansion {
                reply
                    .sources
                    .as_mut()
                    .expect("merged comment provenance")
                    .parent = parent;
            }
            *old = reply;
        } else if allow_new {
            merge_comment(&mut reply, None, source, removed);
            if expansion {
                reply
                    .sources
                    .as_mut()
                    .expect("merged comment provenance")
                    .parent = None;
            }
            existing.push(reply);
        }
    }
    if let Some(order) = order {
        // A batch ranks unconfirmed siblings before its continuation, including
        // retained later batches. Parent-confirmed and unranked positions stay put.
        let mut ranks = HashMap::new();
        for (rank, id) in order.iter().enumerate() {
            ranks.entry(id.as_str()).or_insert(rank);
        }
        let slots: Vec<_> = existing
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                ranks.contains_key(c.id.as_str())
                    && c.sources
                        .is_some_and(|s| s.parent.is_none_or(|p| !p.present))
            })
            .map(|(i, _)| i)
            .collect();
        let mut ids: Vec<_> = slots.iter().map(|&i| existing[i].id.clone()).collect();
        ids.sort_by_key(|id| ranks[id.as_str()]);
        for (slot, id) in slots.into_iter().zip(ids) {
            let at = existing
                .iter()
                .position(|c| c.id == id)
                .expect("ranked reply");
            existing.swap(slot, at);
        }
    }
}

/// True when `parent`'s reply list came complete (no More link) from a source that
/// `source` cannot replace: `source` may then neither replace nor add to it. Deeper
/// lists are judged on their own.
fn replies_superseded(parent: &Comment, source: Source) -> bool {
    parent.more.is_none()
        && parent
            .sources
            .is_some_and(|old| !source.may_replace(old.more))
}

/// The first reply in `replies` that `previous` lacks, depth first, skipping folded
/// threads and lists `source` may not add to (`allow_new` at this level).
fn first_new_reply<'a>(
    replies: &'a [Comment],
    previous: &[Comment],
    source: Source,
    allow_new: bool,
    collapsed: &HashSet<String>,
) -> Option<&'a str> {
    for reply in replies {
        let Some(old) = previous.iter().find(|old| old.id == reply.id) else {
            if allow_new {
                return Some(&reply.id);
            }
            continue;
        };
        if !collapsed.contains(&old.id)
            && let Some(id) = first_new_reply(
                &reply.replies,
                &old.replies,
                source,
                !replies_superseded(old, source),
                collapsed,
            )
        {
            return Some(id);
        }
    }
    None
}

fn comment_newer_than(comment: &Comment, source: Source) -> bool {
    comment.sources.is_some_and(|old| {
        !source.may_replace(old.content)
            || !source.may_replace(old.more)
            || old.parent.is_some_and(|p| !source.may_replace(p.source))
    }) || comment
        .replies
        .iter()
        .any(|child| comment_newer_than(child, source))
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

    /// The ids of the replies under comment `parent` in the open post.
    fn reply_ids(app: &App, parent: &str) -> Vec<String> {
        let page = app.post_view.as_ref().unwrap().page.as_ref().unwrap();
        find_comment(&page.comments, parent)
            .unwrap()
            .replies
            .iter()
            .map(|c| c.id.clone())
            .collect()
    }

    #[test]
    fn cached_ancestor_cannot_resurrect_a_removed_reply_but_newer_data_can() {
        let mut app = app_with(&[]);
        app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
        let generation = app.post_view.as_ref().unwrap().generation;
        let parent = |id| comment("parent", vec![comment(id, vec![], None)], Some("/parent"));
        let page = |children| {
            thread_message(
                generation,
                vec![comment("ancestor", children, Some("/ancestor"))],
            )
        };
        let ancestor = || Msg::MoreReplies {
            generation,
            comment_id: "ancestor".into(),
            result: Ok(ReplyPage::complete(vec![parent("b")])),
        };
        app.on_msg(ordered_snapshot(page(vec![]), 1, false, false));
        app.on_msg(ordered_snapshot(ancestor(), 2, true, false));
        app.on_msg(ordered_snapshot(page(vec![parent("a")]), 3, false, false));
        app.on_msg(ordered_snapshot(page(vec![]), 4, false, false));
        app.on_msg(ordered_snapshot(
            Msg::MoreReplies {
                generation,
                comment_id: "parent".into(),
                result: Ok(ReplyPage {
                    comments: vec![],
                    refreshed: Some(vec!["b".into()]),
                    order: vec![],
                    more: Some(MoreReplies {
                        cursor: "/parent-next".into(),
                        count: None,
                    }),
                }),
            },
            5,
            false,
            false,
        ));
        let has_b = |app: &App| reply_ids(app, "parent").iter().any(|id| id == "b");
        let removed = has_b(&app);
        app.on_msg(ordered_snapshot(ancestor(), 2, true, true));
        let stale = has_b(&app);
        app.on_msg(ordered_snapshot(ancestor(), 6, false, false));
        assert_eq!([removed, stale, has_b(&app)], [false, false, true]);
    }

    #[test]
    fn explicit_omissions_remember_unseen_ids_and_advance_without_rolling_back() {
        let mut outcomes = Vec::new();
        for loaded in [false, true] {
            let mut app = app_with(&[]);
            app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
            let generation = app.post_view.as_ref().unwrap().generation;
            let parent = |ids: &[&str]| {
                comment(
                    "parent",
                    ids.iter().map(|id| comment(id, vec![], None)).collect(),
                    Some("/parent"),
                )
            };
            let page = |children| {
                thread_message(
                    generation,
                    vec![comment("ancestor", children, Some("/ancestor"))],
                )
            };
            app.on_msg(ordered_snapshot(
                page(vec![parent(if loaded { &["a", "b"] } else { &["a"] })]),
                1,
                false,
                false,
            ));
            app.on_msg(ordered_snapshot(
                page(vec![parent(&["a"])]),
                3,
                false,
                false,
            ));
            let omission = || Msg::MoreReplies {
                generation,
                comment_id: "parent".into(),
                result: Ok(ReplyPage {
                    comments: vec![],
                    refreshed: Some(vec!["b".into()]),
                    order: vec![],
                    more: Some(MoreReplies {
                        cursor: "/next".into(),
                        count: None,
                    }),
                }),
            };
            for sequence in [5, 7, 5] {
                app.on_msg(ordered_snapshot(omission(), sequence, false, false));
            }
            let ancestor = || Msg::MoreReplies {
                generation,
                comment_id: "ancestor".into(),
                result: Ok(ReplyPage::complete(vec![parent(&["b"])])),
            };
            let has_b = |app: &App| reply_ids(app, "parent").iter().any(|id| id == "b");
            app.on_msg(ordered_snapshot(ancestor(), 6, true, true));
            let stale = has_b(&app);
            app.on_msg(ordered_snapshot(ancestor(), 8, false, false));
            outcomes.push([stale, has_b(&app)]);
        }
        assert_eq!(outcomes, [[false, true], [false, true]]);
    }

    #[test]
    fn parent_page_demotion_allows_later_batch_order_and_removal() {
        let mut app = app_with(&[]);
        app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
        let generation = app.post_view.as_ref().unwrap().generation;
        let page = |ids: &[&str]| {
            thread_message(
                generation,
                vec![comment(
                    "parent",
                    ids.iter().map(|id| comment(id, vec![], None)).collect(),
                    Some("/more"),
                )],
            )
        };
        let batch = |ids: &[&str], covered: &[&str], remaining: &[&str]| Msg::MoreReplies {
            generation,
            comment_id: "parent".into(),
            result: Ok(ReplyPage {
                comments: ids.iter().map(|id| comment(id, vec![], None)).collect(),
                refreshed: Some(covered.iter().map(|s| s.to_string()).collect()),
                order: ids.iter().chain(remaining).map(|s| s.to_string()).collect(),
                more: (!remaining.is_empty()).then(|| MoreReplies {
                    cursor: "/next".into(),
                    count: None,
                }),
            }),
        };
        app.on_msg(ordered_snapshot(page(&["a", "b"]), 1, false, false));
        app.on_msg(ordered_snapshot(
            batch(&["c", "d"], &["c", "d"], &[]),
            2,
            false,
            false,
        ));
        app.on_msg(ordered_snapshot(page(&["a"]), 3, false, false));
        // An older cached parent cannot confirm b again after the newer demotion.
        app.on_msg(ordered_snapshot(page(&["a", "b"]), 1, true, false));
        app.on_msg(ordered_snapshot(
            batch(&["x", "b", "c"], &["x", "b", "c"], &["d"]),
            4,
            false,
            false,
        ));
        let ordered = reply_ids(&app, "parent");
        // A covered omission removes b, and a newer response may add it back.
        app.on_msg(ordered_snapshot(
            batch(&[], &["b"], &["d"]),
            5,
            false,
            false,
        ));
        let removed = reply_ids(&app, "parent");
        app.on_msg(ordered_snapshot(
            batch(&["b"], &["b"], &["c", "d"]),
            6,
            false,
            false,
        ));
        let readded = reply_ids(&app, "parent");
        let expected = [
            vec!["a", "x", "b", "c", "d"],
            vec!["a", "x", "c", "d"],
            vec!["a", "x", "b", "c", "d"],
        ]
        .map(|ids| ids.into_iter().map(str::to_owned).collect::<Vec<_>>());
        assert_eq!([ordered, removed, readded], expected);
    }

    #[test]
    fn growing_reply_batches_stay_before_retained_later_comments() {
        let mut outcomes = Vec::new();
        for returned in [&["x", "b", "c"][..], &["x", "y"][..]] {
            let mut app = app_with(&[]);
            app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
            let generation = app.post_view.as_ref().unwrap().generation;
            let parent_page = || {
                thread_message(
                    generation,
                    vec![comment(
                        "parent",
                        vec![comment("a", vec![], None)],
                        Some("/more"),
                    )],
                )
            };
            app.on_msg(parent_page());
            let batch =
                |comments: Vec<Comment>, covered: &[&str], remaining: &[&str]| Msg::MoreReplies {
                    generation,
                    comment_id: "parent".into(),
                    result: Ok(ReplyPage {
                        order: comments
                            .iter()
                            .map(|c| c.id.clone())
                            .chain(remaining.iter().map(|id| id.to_string()))
                            .collect(),
                        comments,
                        refreshed: Some(covered.iter().map(|id| id.to_string()).collect()),
                        more: (!remaining.is_empty()).then(|| MoreReplies {
                            cursor: "/next".into(),
                            count: None,
                        }),
                    }),
                };
            app.on_msg(batch(
                vec![
                    comment("b", vec![], None),
                    comment("c", vec![], None),
                    comment("d", vec![comment("child", vec![], None)], Some("/child")),
                ],
                &["b", "c", "d"],
                &[],
            ));
            let view = app.post_view.as_mut().unwrap();
            view.free_scroll = false;
            view.reselect(Some(RowKey::Comment("child".into())));
            app.on_msg(parent_page());
            let remaining = if returned.contains(&"b") {
                &[][..]
            } else {
                &["b", "c"][..]
            };
            let remaining: Vec<_> = remaining.iter().copied().chain(["d"]).collect();
            app.on_msg(batch(
                returned
                    .iter()
                    .map(|id| comment(id, vec![], None))
                    .collect(),
                returned,
                &remaining,
            ));
            let mut expected: Vec<_> = ["a"]
                .into_iter()
                .chain(returned.iter().copied())
                .chain(remaining.iter().copied())
                .map(str::to_string)
                .collect();
            outcomes.push((reply_ids(&app, "parent"), expected.clone()));
            app.on_msg(batch(
                remaining
                    .iter()
                    .map(|id| comment(id, vec![], (*id == "d").then_some("/child")))
                    .collect(),
                &remaining,
                &[],
            ));
            outcomes.push((reply_ids(&app, "parent"), std::mem::take(&mut expected)));
            let view = app.post_view.as_ref().unwrap();
            assert!(matches!(view.cursor_key(), Some(RowKey::Comment(id)) if id == "child"));
            assert_eq!(
                view.page.as_ref().unwrap().comments[0]
                    .replies
                    .iter()
                    .find(|c| c.id == "d")
                    .unwrap()
                    .replies[0]
                    .id,
                "child"
            );
        }
        assert!(
            outcomes.iter().all(|(actual, expected)| actual == expected),
            "{outcomes:?}"
        );
    }

    #[test]
    fn top_level_refresh_and_reexpansion_keep_later_batches_and_selection() {
        use serde_json::json;
        let page = |ids: &[&str], more: &[&str]| {
            let mut value: serde_json::Value =
                serde_json::from_str(include_str!("../tests/fixtures/post.json")).unwrap();
            let more_of = |children: &[&str]| {
                let count = children.len();
                json!({"kind": "more", "data": {"children": children, "count": count}})
            };
            let hidden =
                |child| json!({"kind": "Listing", "data": {"children": [more_of(&[child])]}});
            let mut children: Vec<_> = ids
                .iter()
                .map(|id| {
                    let replies = match *id {
                        "c" => hidden("child"),
                        "d" => hidden("dchild"),
                        _ => json!(""),
                    };
                    json!({"kind": "t1", "data": {"id": id, "body": id, "replies": replies}})
                })
                .collect();
            if !more.is_empty() {
                children.push(more_of(more));
            }
            value[1]["data"]["children"] = json!(children);
            crate::reddit::parse::parse_post_page(&value.to_string()).unwrap()
        };
        let batch = |comments: Vec<Comment>, refreshed: &[&str], next: Option<&str>| ReplyPage {
            order: comments.iter().map(|c| c.id.clone()).collect(),
            comments,
            refreshed: Some(refreshed.iter().map(|s| s.to_string()).collect()),
            more: next.map(|cursor| MoreReplies {
                cursor: cursor.into(),
                count: None,
            }),
        };
        let mut app = app_with(&[]);
        let initial = page(&["a"], &["b", "c", "d"]);
        app.open_post_permalink(initial.post.clone(), initial.post.permalink.clone());
        let generation = app.post_view.as_ref().unwrap().generation;
        let root = initial.comments[0].id.clone();
        app.on_msg(Msg::Post {
            generation,
            result: Ok(Box::new(initial)),
        });
        for result in [
            batch(
                vec![
                    comment("b", vec![], None),
                    comment("c", vec![comment("child", vec![], None)], Some("/child")),
                ],
                &["b", "c"],
                Some("/second"),
            ),
            batch(
                vec![comment(
                    "d",
                    vec![comment("dchild", vec![], None)],
                    Some("/dchild"),
                )],
                &["d"],
                None,
            ),
        ] {
            app.on_msg(Msg::MoreReplies {
                generation,
                comment_id: root.clone(),
                result: Ok(result),
            });
        }
        let view = app.post_view.as_mut().unwrap();
        view.free_scroll = false;
        view.reselect(Some(RowKey::Comment("dchild".into())));
        let check = |app: &App| {
            let view = app.post_view.as_ref().unwrap();
            let ids: Vec<_> = view
                .rows()
                .iter()
                .filter_map(|r| match r.kind {
                    RowKind::Comment { comment, .. } => Some(comment.id.as_str()),
                    _ => None,
                })
                .collect();
            assert_eq!(ids, ["a", "b", "c", "child", "d", "dchild"]);
            assert!(matches!(view.cursor_key(), Some(RowKey::Comment(id)) if id == "dchild"));
        };
        check(&app);
        app.on_msg(Msg::Post {
            generation,
            result: Ok(Box::new(page(&["a", "b"], &["c", "d"]))),
        });
        check(&app);
        for result in [
            batch(
                vec![comment("c", vec![], Some("/child"))],
                &["c"],
                Some("/second"),
            ),
            batch(vec![comment("d", vec![], Some("/dchild"))], &["d"], None),
        ] {
            app.on_msg(Msg::MoreReplies {
                generation,
                comment_id: root.clone(),
                result: Ok(result),
            });
            check(&app);
        }
        app.on_msg(Msg::Post {
            generation,
            result: Ok(Box::new(page(&["a", "b", "c", "d"], &[]))),
        });
        check(&app);
        let loaded = app.post_view.as_ref().unwrap().page.as_ref().unwrap();
        assert!(
            find_comment(&loaded.comments, &root)
                .unwrap()
                .more
                .is_none()
        );
    }

    #[test]
    fn top_level_more_expands_without_a_synthetic_author_or_extra_indent() {
        let mut app = app_with(&[]);
        let mut value: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/post.json")).unwrap();
        value[1]["data"]["children"] =
            serde_json::json!([{"kind":"more","data":{"children":["c1"],"count":1}}]);
        let page = crate::reddit::parse::parse_post_page(&value.to_string()).unwrap();
        app.open_post_permalink(page.post.clone(), page.post.permalink.clone());
        let generation = app.post_view.as_ref().unwrap().generation;
        let parent = page.comments[0].id.clone();
        app.on_msg(Msg::Post {
            generation,
            result: Ok(Box::new(page)),
        });
        let rows = app.post_view.as_ref().unwrap().rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].depth, 0);
        assert!(matches!(rows[0].kind, RowKind::More { .. }));
        app.on_msg(Msg::MoreReplies {
            generation,
            comment_id: parent,
            result: Ok(ReplyPage::complete(vec![comment("c1", vec![], None)])),
        });
        let rows = app.post_view.as_ref().unwrap().rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].depth, 0);
        assert!(matches!(&rows[0].kind, RowKind::Comment { comment, .. } if comment.id == "c1"));
    }

    #[test]
    fn when_a_list_picture_arrives_it_should_be_cut_to_the_slots_shape_around_its_middle() {
        // 8×16 px cells: a 4×2-cell slot is 32×32 px, square.
        let font = ratatui_image::FontSize::new(8, 16);
        // A wide picture whose middle third is red: the crop keeps exactly that.
        let wide = image::RgbImage::from_fn(300, 100, |x, _| {
            if (100..200).contains(&x) {
                image::Rgb([255, 0, 0])
            } else {
                image::Rgb([0, 0, 255])
            }
        });

        let cut = fill_slot(image::DynamicImage::ImageRgb8(wide), font).to_rgb8();
        let tall = fill_slot(image::DynamicImage::new_rgb8(100, 300), font);

        assert_eq!(cut.dimensions(), (100, 100));
        assert!(
            cut.pixels().all(|p| p.0 == [255, 0, 0]),
            "the middle, not an edge"
        );
        assert_eq!((tall.width(), tall.height()), (100, 100));
    }

    #[test]
    fn when_card_and_detail_wait_for_one_image_both_should_receive_independent_previews() {
        let mut app = app_with(&[]);
        let url = "https://i.redd.it/shared.png";
        app.card_pictures_drawn(&[url]);
        app.request_text_picture(url);

        app.on_msg(Msg::Image {
            url: url.into(),
            result: Ok(image::DynamicImage::new_rgb8(640, 320).into()),
        });

        assert!(matches!(
            app.images.get(url),
            Some(ImageSlot::Ready {
                pixels: (640, 320),
                ..
            })
        ));
        assert!(matches!(
            app.card_images.get(url),
            Some(CardImageSlot::Ready { image, .. }) if (image.width(), image.height()) == (640, 320)
        ));
        assert!(!app.queued_images.contains_key(url));
    }

    #[test]
    fn when_detail_closes_a_visible_card_should_keep_its_loaded_preview() {
        let mut app = app_with(&[]);
        let url = "https://i.redd.it/shared.png";
        app.card_pictures_drawn(&[url]);
        app.request_text_picture(url);
        app.on_msg(Msg::Image {
            url: url.into(),
            result: Ok(image::DynamicImage::new_rgb8(640, 320).into()),
        });

        app.close_post();

        assert!(app.images.is_empty());
        assert!(matches!(
            app.card_images.get(url),
            Some(CardImageSlot::Ready { .. })
        ));
    }

    #[test]
    fn when_a_card_returns_after_a_skipped_image_it_should_resume_the_fetch() {
        let mut app = app_with(&[]);
        let url = "https://i.redd.it/revisit.png";
        app.card_pictures_drawn(&[url]);
        app.card_pictures_drawn(&[]);
        app.card_pictures_drawn(&[url]);

        app.on_msg(Msg::ImageSkipped { url: url.into() });

        assert!(matches!(
            app.card_images.get(url),
            Some(CardImageSlot::Loading)
        ));
        assert!(app.queued_images.contains_key(url));
    }

    #[test]
    fn when_revisiting_a_card_its_recent_decoded_image_should_be_reused() {
        let mut app = app_with(&[]);
        let url = "https://i.redd.it/revisit.png";
        app.card_pictures_drawn(&[url]);
        app.on_msg(Msg::Image {
            url: url.into(),
            result: Ok(image::DynamicImage::new_rgb8(320, 640).into()),
        });
        app.card_pictures_drawn(&[]);

        app.card_pictures_drawn(&[url]);

        assert!(matches!(
            app.card_images.get(url),
            Some(CardImageSlot::Ready { .. })
        ));
        assert!(!app.queued_images.contains_key(url));
    }

    #[test]
    fn when_recent_card_images_exceed_the_count_limit_the_oldest_pixels_should_be_evicted() {
        let mut app = app_with(&[]);
        for number in 0..10 {
            let url = format!("https://i.redd.it/{number}.png");
            app.card_pictures_drawn(&[&url]);
            app.on_msg(Msg::Image {
                url,
                result: Ok(image::DynamicImage::new_rgb8(1, 1).into()),
            });
        }

        app.card_pictures_drawn(&[]);

        assert_eq!(app.card_images.len(), 8);
        assert!(!app.card_images.contains_key("https://i.redd.it/0.png"));
        assert!(!app.card_images.contains_key("https://i.redd.it/1.png"));
        assert!(app.card_images.contains_key("https://i.redd.it/2.png"));
        assert_eq!(
            app.card_image_metadata
                .get("https://i.redd.it/0.png")
                .map(|metadata| metadata.size),
            Some((1, 1))
        );
    }

    #[test]
    fn when_recent_card_pixels_exceed_the_byte_limit_the_oldest_image_should_be_evicted() {
        let mut app = app_with(&[]);
        let large = "https://i.redd.it/large.png";
        let small = "https://i.redd.it/small.png";
        app.card_pictures_drawn(&[large]);
        app.on_msg(Msg::Image {
            url: large.into(),
            result: Ok(image::DynamicImage::new_rgba8(4096, 4096).into()),
        });
        app.card_pictures_drawn(&[]);
        assert!(
            app.card_images.contains_key(large),
            "exactly 64 MiB may stay cached"
        );
        app.card_pictures_drawn(&[small]);
        app.on_msg(Msg::Image {
            url: small.into(),
            result: Ok(image::DynamicImage::new_rgb8(1, 1).into()),
        });

        app.card_pictures_drawn(&[]);

        assert!(!app.card_images.contains_key(large));
        assert!(app.card_images.contains_key(small));
        assert_eq!(
            app.card_image_metadata
                .get(large)
                .map(|metadata| metadata.size),
            Some((4096, 4096))
        );
    }

    #[test]
    fn when_a_retained_preview_upgrade_is_skipped_it_should_resume_without_losing_pixels() {
        let mut app = app_with(&[]);
        let url = "https://i.redd.it/retained.png";
        app.card_pictures_drawn(&[url]);
        app.on_msg(Msg::Image {
            url: url.into(),
            result: Ok(image::DynamicImage::new_rgb8(640, 320).into()),
        });
        app.card_images.clear();
        app.card_pictures_drawn(&[url]);
        app.card_pictures_drawn(&[]);
        app.card_pictures_drawn(&[url]);

        app.on_msg(Msg::ImageSkipped { url: url.into() });

        assert!(matches!(
            app.card_images.get(url),
            Some(CardImageSlot::Cached {
                refreshing: true,
                ..
            })
        ));
        assert!(app.queued_images.contains_key(url));
        app.on_msg(Msg::Image {
            url: url.into(),
            result: Err("offline".into()),
        });
        assert!(matches!(
            app.card_images.get(url),
            Some(CardImageSlot::Cached {
                refreshing: false,
                ..
            })
        ));
        assert!(!app.queued_images.contains_key(url));
    }

    #[test]
    fn when_retained_previews_exceed_the_limit_the_oldest_should_be_released() {
        let mut app = app_with(&[]);
        for number in 0..257 {
            let url = format!("https://i.redd.it/{number}.png");
            app.card_pictures_drawn(&[&url]);
            app.on_msg(Msg::Image {
                url,
                result: Ok(image::DynamicImage::new_rgb8(1, 1).into()),
            });
        }

        app.card_pictures_drawn(&["https://i.redd.it/0.png", "https://i.redd.it/1.png"]);

        assert_eq!(app.card_previews.len(), 256);
        assert_eq!(app.card_previews["https://i.redd.it/1.png"].width(), 1);
        assert_eq!(app.card_previews["https://i.redd.it/1.png"].height(), 1);
        assert!(matches!(
            app.card_images.get("https://i.redd.it/0.png"),
            Some(CardImageSlot::Loading)
        ));
        assert!(matches!(
            app.card_images.get("https://i.redd.it/1.png"),
            Some(CardImageSlot::Cached { .. })
        ));
    }

    #[test]
    fn when_a_stale_disk_preview_refreshes_it_should_keep_pixels_and_one_pending_fetch() {
        let mut app = app_with(&[]);
        let url = "https://i.redd.it/stale.png";
        app.request_image(url);
        app.card_pictures_drawn(&[url]);
        assert_eq!(app.queued_images.get(url), Some(&false));
        app.on_msg(Msg::Snapshot {
            message: Box::new(Msg::Image {
                url: url.into(),
                result: Ok(image::DynamicImage::new_rgb8(10, 5).into()),
            }),
            fetched_at: now() - 8 * 86_400,
            refreshing: true,
            source: Source {
                cached: true,
                ..Source::default()
            },
        });
        assert!(matches!(
            app.card_images.get(url),
            Some(CardImageSlot::Cached {
                refreshing: true,
                ..
            })
        ));
        assert!(app.queued_images.contains_key(url));
        assert!(matches!(
            app.images.get(url),
            Some(ImageSlot::Ready {
                refresh: ImageRefresh::Loading,
                ..
            })
        ));

        app.on_msg(Msg::Image {
            url: url.into(),
            result: Ok(image::DynamicImage::new_rgb8(20, 10).into()),
        });

        assert!(
            matches!(app.card_images.get(url), Some(CardImageSlot::Ready { image, .. }) if image.width() == 20)
        );
        assert!(!app.queued_images.contains_key(url));
        assert!(matches!(
            app.images.get(url),
            Some(ImageSlot::Ready {
                pixels: (20, 10),
                refresh: ImageRefresh::Idle,
                ..
            })
        ));
    }

    #[test]
    fn when_images_are_disabled_retained_card_previews_should_be_released() {
        let mut app = app_with(&[]);
        let url = "https://i.redd.it/off.png";
        app.card_pictures_drawn(&[url]);
        app.on_msg(Msg::Image {
            url: url.into(),
            result: Ok(image::DynamicImage::new_rgb8(10, 5).into()),
        });
        assert_eq!(app.card_previews.len(), 1);
        app.show_images = false;

        app.card_pictures_drawn(&[]);

        assert!(app.card_previews.is_empty());
        assert!(app.card_images.is_empty());
        assert!(app.card_preview_order.is_empty());
    }

    #[test]
    fn when_a_shared_disk_preview_refresh_fails_both_consumers_should_keep_cached_pixels() {
        for background in [false, true] {
            let mut app = app_with(&[]);
            let url = "https://i.redd.it/stale-failure.png";
            app.request_image(url);
            app.card_pictures_drawn(&[url]);
            app.on_msg(Msg::Snapshot {
                message: Box::new(Msg::Image {
                    url: url.into(),
                    result: Ok(image::DynamicImage::new_rgb8(10, 5).into()),
                }),
                fetched_at: now() - 8 * 86_400,
                refreshing: true,
                source: Source {
                    cached: true,
                    ..Source::default()
                },
            });

            app.on_msg(Msg::Image {
                url: url.into(),
                result: Err("offline".into()),
            });

            assert!(matches!(
                app.images.get(url),
                Some(ImageSlot::Ready {
                    pixels: (10, 5),
                    refresh: ImageRefresh::Failed,
                    ..
                })
            ));
            assert!(matches!(
                app.card_images.get(url),
                Some(CardImageSlot::Cached {
                    refreshing: false,
                    ..
                })
            ));
            assert!(!app.queued_images.contains_key(url));

            app.request_text_picture(url);
            assert!(
                !app.queued_images.contains_key(url),
                "wait for explicit retry"
            );
            app.retry_failed_pictures();
            if background {
                app.request_text_picture(url);
            } else {
                app.request_image(url);
            }
            app.card_pictures_drawn(&[url]);
            assert_eq!(app.queued_images.get(url), Some(&background));
            assert!(matches!(
                app.images.get(url),
                Some(ImageSlot::Ready {
                    pixels: (10, 5),
                    refresh: ImageRefresh::Loading,
                    ..
                })
            ));
            app.on_msg(Msg::Image {
                url: url.into(),
                result: Ok(image::DynamicImage::new_rgb8(20, 10).into()),
            });
            assert!(matches!(
                app.images.get(url),
                Some(ImageSlot::Ready {
                    pixels: (20, 10),
                    refresh: ImageRefresh::Idle,
                    ..
                })
            ));
            assert!(matches!(
                app.card_images.get(url),
                Some(CardImageSlot::Ready { image, .. }) if image.width() == 20
            ));
            assert!(!app.queued_images.contains_key(url));
        }
    }

    #[test]
    fn a_second_fetch_of_a_loaded_picture_should_not_replace_it() {
        let mut app = app_with(&[]);
        let url = "https://preview.redd.it/twice.png";
        app.request_text_picture(url);
        let image = |side| Ok(image::DynamicImage::new_rgb8(side, side).into());

        app.on_msg(Msg::Image {
            url: url.into(),
            result: image(10),
        });
        app.on_msg(Msg::Image {
            url: url.into(),
            result: image(20),
        });

        assert!(matches!(
            app.images.get(url),
            Some(ImageSlot::Ready {
                pixels: (10, 10),
                ..
            })
        ));
    }

    #[test]
    fn a_picture_asked_for_again_while_still_queued_should_be_fetched_once() {
        let (mut app, messages) = app_with_messages(&[]);
        let url = "https://preview.redd.it/once.png";
        app.request_text_picture(url);
        app.drop_text_picture(url);
        app.request_text_picture(url);

        let mut fetched = 0;
        while let Ok(msg) = messages.recv_timeout(std::time::Duration::from_secs(1)) {
            if matches!(&msg, Msg::Image { url: u, .. } if u == url) {
                fetched += 1;
            }
            app.on_msg(msg);
        }

        assert_eq!(fetched, 1);
    }

    #[test]
    fn a_subreddit_icon_is_looked_up_once_and_again_after_a_failure() {
        let (mut app, messages) = app_with_messages(&[]);
        show_thumbnails(&mut app);
        app.all_subs.groups = vec![Group {
            subreddit: "S".into(),
            ..Group::default()
        }];
        let load = |app: &mut App| {
            app.on_msg(Msg::Group {
                generation: 0,
                subreddit: "S".into(),
                result: Ok(listing("S", &["a"], None)),
            });
            let mut asked = 0;
            while let Ok(msg) = messages.recv_timeout(std::time::Duration::from_millis(300)) {
                if matches!(&msg, Msg::Icon { subreddit, result: Err(_) } if subreddit == "s") {
                    asked += 1;
                }
                app.on_msg(msg);
            }
            asked
        };

        let first = load(&mut app);
        let after_failure = load(&mut app);
        app.icons.insert("s".into(), Some("/icon/s.png".into()));
        let known = load(&mut app);
        app.icons.remove("s");
        app.icon_lookups.insert("s".into());
        let pending = load(&mut app);
        // A stale disk answer, then its refresh fails: the answer stands.
        app.icon_lookups.clear();
        app.on_msg(Msg::Icon {
            subreddit: "s".into(),
            result: Ok(None),
        });
        app.on_msg(Msg::Icon {
            subreddit: "s".into(),
            result: Err("offline".into()),
        });
        let stale = load(&mut app);

        assert_eq!(
            (first, after_failure, known, pending, stale),
            (1, 1, 0, 0, 0)
        );
    }

    #[test]
    fn a_multireddit_asks_for_no_icon() {
        let (mut app, messages) = app_with_messages(&[]);
        show_thumbnails(&mut app);

        app.request_icon("a+B");

        assert!(app.icon_lookups.is_empty());
        assert!(
            messages
                .recv_timeout(std::time::Duration::from_millis(300))
                .is_err()
        );
    }

    #[test]
    fn a_feed_looks_its_icon_up_on_its_first_page_only() {
        let (mut app, messages) = app_with_messages(&["s"]);
        show_thumbnails(&mut app);
        let (tab, generation) = (app.tabs[3].id, app.tabs[3].feed.generation);
        let load = |app: &mut App, append: bool| {
            app.icons.remove("s");
            app.on_msg(Msg::Feed {
                tab,
                generation,
                append,
                result: Ok(listing("s", &["a"], Some("t3_a"))),
            });
            let mut asked = 0;
            while let Ok(msg) = messages.recv_timeout(std::time::Duration::from_millis(300)) {
                if matches!(&msg, Msg::Icon { subreddit, .. } if subreddit == "s") {
                    asked += 1;
                }
            }
            asked
        };

        let next_page = load(&mut app, true);
        let first_page = load(&mut app, false);

        assert_eq!((next_page, first_page), (0, 1));
    }

    #[test]
    fn icons_are_looked_up_only_while_thumbnails_show() {
        let (mut app, messages) = app_with_messages(&[]);
        app.all_subs.groups = vec![Group {
            subreddit: "s".into(),
            ..Group::default()
        }];
        let asked = || {
            let mut asked = 0;
            while let Ok(msg) = messages.recv_timeout(std::time::Duration::from_millis(300)) {
                asked += usize::from(matches!(msg, Msg::Icon { .. }));
            }
            asked
        };
        show_thumbnails(&mut app);
        app.show_images = false;

        app.on_msg(Msg::Group {
            generation: 0,
            subreddit: "s".into(),
            result: Ok(listing("s", &["a"], None)),
        });
        let while_off = asked();
        app.toggle_images();
        let turned_on = asked();

        assert_eq!((while_off, turned_on), (0, 1));
    }

    /// A graphics protocol with images on: icons and thumbnails draw.
    fn show_thumbnails(app: &mut App) {
        app.image_picker = crate::ui::tests::kitty();
        app.show_images = true;
    }

    fn app_with(subscriptions: &[&str]) -> App {
        app_with_messages(subscriptions).0
    }

    #[test]
    fn when_disconnecting_it_should_clear_account_payloads_and_preserve_saved_history() {
        use crate::reddit::tests::{Fixture, Reply};
        let fixture = Fixture::new(|_| {
            Reply::json(serde_json::json!({"data":{"id":"readerid", "name":"reader"}}))
        });
        let (mut app, _old_messages) = app_at(&fixture.base, &[]);
        let directory = tempfile::tempdir().unwrap();
        let account_path = directory.path().join("accounts/readerid/state.json");
        std::fs::create_dir_all(account_path.parent().unwrap()).unwrap();
        app.state_path = account_path.clone();
        app.reddit.verify_account().unwrap();
        let retired = app.reddit.clone();
        app.tabs[0].feed.posts = vec![post("private", "rust")];
        app.state.toggle_bookmark(&post("saved", "rust"), 1000);
        app.pending_media.insert("/private".into());
        app.images
            .insert("private-image".into(), ImageSlot::Loading);
        app.save_state();

        app.install_connection(Reddit::disconnected(false).unwrap());

        assert_eq!(app.health, Health::SignedOut);
        assert!(app.tabs.iter().all(|tab| tab.feed.posts.is_empty()));
        assert!(app.all_subs.groups.is_empty());
        assert!(app.state.bookmarks().is_empty());
        assert!(app.images.is_empty());
        assert!(app.pending_media.is_empty());
        assert!(app.video.is_none());
        assert!(app.post_view.is_none());
        assert!(!app.auth.busy());
        assert!(app.overlay.is_none());
        assert_eq!(
            app.state_path,
            directory.path().join("signed-out/state.json")
        );
        assert!(app.take_replacement_messages().is_some());
        assert!(
            retired
                .verify_account()
                .unwrap_err()
                .to_string()
                .contains("replaced")
        );
        assert!(State::load(&account_path).0.is_bookmarked("saved"));
    }

    #[test]
    fn when_reconnecting_after_disconnect_it_should_restore_that_accounts_bookmarks() {
        use crate::reddit::tests::{Fixture, Reply};
        let fixture = Fixture::new(|_| {
            Reply::json(serde_json::json!({"data":{"id":"readerid", "name":"reader"}}))
        });
        let (mut app, _messages) = app_at(&fixture.base, &[]);
        let directory = tempfile::tempdir().unwrap();
        app.state_path = directory.path().join("accounts/readerid/state.json");
        std::fs::create_dir_all(app.state_path.parent().unwrap()).unwrap();
        app.reddit.verify_account().unwrap();
        app.state.toggle_bookmark(&post("saved", "rust"), 1000);
        app.save_state();
        app.install_connection(Reddit::disconnected(false).unwrap());
        let connection = fixture.reddit();
        connection.verify_account().unwrap();

        app.install_connection(connection);

        assert!(app.state.is_bookmarked("saved"));
        assert_eq!(
            app.state_path,
            directory.path().join("accounts/readerid/state.json")
        );
        assert_eq!(app.reddit.account().unwrap().id, "readerid");
    }

    #[test]
    fn when_reading_state_cannot_be_saved_disconnect_should_not_start() {
        let mut app = app_with(&[]);
        let directory = tempfile::tempdir().unwrap();
        app.state_path = directory.path().join("blocked.json");
        std::fs::create_dir(&app.state_path).unwrap();
        app.auth.active_id = app.reddit.saved_id();
        app.auth.request_disconnect();

        app.confirm_auth();

        assert_eq!(app.auth.phase, crate::auth::Phase::DisconnectConfirm);
        assert!(!app.auth.busy());
        assert!(
            app.auth
                .message
                .as_deref()
                .unwrap()
                .contains("connection unchanged")
        );
        assert!(!app.reddit.needs_sign_in());
        assert!(app.take_replacement_messages().is_none());
    }

    #[test]
    fn when_reading_state_cannot_be_saved_a_saved_account_switch_should_not_start() {
        let mut app = app_with(&[]);
        let directory = tempfile::tempdir().unwrap();
        app.state_path = directory.path().join("blocked.json");
        std::fs::create_dir(&app.state_path).unwrap();
        app.auth.phase = crate::auth::Phase::Accounts;
        app.auth.accounts = vec![crate::reddit::SavedAccount {
            id: "second".into(),
            name: "u/second".into(),
            expired: false,
            source: None,
        }];

        app.select_auth();

        assert_eq!(app.auth.phase, crate::auth::Phase::Accounts);
        assert!(!app.auth.busy());
        assert!(
            app.auth
                .message
                .as_deref()
                .unwrap()
                .contains("connection unchanged")
        );
        assert!(!app.reddit.needs_sign_in());
        assert!(app.take_replacement_messages().is_none());
    }

    #[test]
    fn when_selecting_the_current_healthy_account_it_should_close_without_loading_or_saving() {
        let mut app = app_with(&[]);
        app.reddit = Reddit::from_saved(
            &crate::reddit::tests::saved_export("reader", "reader"),
            false,
        )
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        app.state_path = directory.path().join("state.json");
        let id = app.reddit.saved_id().unwrap();
        app.auth.phase = crate::auth::Phase::Accounts;
        app.auth.active_id = Some(id.clone());
        app.auth.accounts = vec![crate::reddit::SavedAccount {
            id,
            name: "u/reader".into(),
            expired: false,
            source: None,
        }];
        app.overlay = Some(Overlay::Auth);

        crate::keys::handle(
            &mut app,
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Enter,
                crossterm::event::KeyModifiers::NONE,
            ),
        );

        assert!(app.overlay.is_none());
        assert!(!app.auth.busy());
        assert!(app.take_replacement_messages().is_none());
        assert!(!app.state_path.exists());
    }

    #[test]
    fn when_the_active_saved_login_is_unnamed_selection_should_reach_the_state_save_guard() {
        use crate::reddit::tests::{Fixture, Reply};
        let fixture = Fixture::new(|_| {
            Reply::json(serde_json::json!({"data":{"id":"reader","name":"reader"}}))
        });
        for transient_identity in [false, true] {
            let (mut app, _messages) = app_at(&fixture.base, &[]);
            if transient_identity {
                app.reddit.verify_account().unwrap();
            }
            let directory = tempfile::tempdir().unwrap();
            app.state_path = directory.path().join("blocked.json");
            std::fs::create_dir(&app.state_path).unwrap();
            let id = app.reddit.saved_id().unwrap();
            app.auth.phase = crate::auth::Phase::Accounts;
            app.auth.active_id = Some(id.clone());
            app.auth.accounts = vec![crate::reddit::SavedAccount {
                id,
                name: "Existing connection".into(),
                expired: false,
                source: None,
            }];
            app.overlay = Some(Overlay::Auth);

            crate::keys::handle(
                &mut app,
                crossterm::event::KeyEvent::new(
                    crossterm::event::KeyCode::Enter,
                    crossterm::event::KeyModifiers::NONE,
                ),
            );

            assert!(matches!(app.overlay, Some(Overlay::Auth)));
            assert!(
                app.auth
                    .message
                    .as_deref()
                    .unwrap()
                    .contains("connection unchanged")
            );
            assert!(!app.auth.busy());
            assert!(app.take_replacement_messages().is_none());
        }
    }

    #[test]
    fn when_a_saved_switch_reports_expiry_it_should_have_persisted_the_current_feed_seen_state() {
        let mut app = app_with(&["rust"]);
        let directory = tempfile::tempdir().unwrap();
        app.state_path = directory.path().join("state.json");
        app.active = tab_index(&app, "rust");
        let feed = &mut app.tabs[app.active].feed;
        feed.posts = vec![post("viewed", "rust")];
        feed.loaded = true;
        app.engage();
        app.auth.phase = crate::auth::Phase::Accounts;
        app.auth.accounts = vec![crate::reddit::SavedAccount {
            id: "second".into(),
            name: "u/second".into(),
            expired: true,
            source: None,
        }];
        assert!(!app.state.is_seen("rust", "viewed"));

        app.select_auth();

        let saved = State::load(&app.state_path).0;
        assert!(saved.is_seen("rust", "viewed"));
        assert_eq!(app.auth.phase, crate::auth::Phase::Accounts);
        assert!(app.auth.message.as_deref().unwrap().contains("expired"));
        assert!(!app.auth.busy());
    }

    #[test]
    fn when_switching_between_saved_accounts_and_back_it_should_restore_each_accounts_own_bookmarks()
     {
        use crate::reddit::tests::{Fixture, Reply, saved_export};
        let first = Fixture::new(|_| {
            Reply::json(serde_json::json!({"data":{"id":"first","name":"first_reader"}}))
        });
        let second = Fixture::new(|_| {
            Reply::json(serde_json::json!({"data":{"id":"second","name":"second_reader"}}))
        });
        let (mut app, _messages) = app_at(&first.base, &[]);
        let directory = tempfile::tempdir().unwrap();
        app.state_path = directory.path().join("accounts/first/state.json");
        app.reddit.verify_account().unwrap();
        app.state
            .toggle_bookmark(&post("first_saved", "rust"), 1000);
        app.save_state();
        app.install_connection(second.saved_reddit(&saved_export("second", "second_reader")));
        app.state
            .toggle_bookmark(&post("second_saved", "python"), 1000);
        app.save_state();

        app.install_connection(first.saved_reddit(&saved_export("first", "first_reader")));

        assert!(app.state.is_bookmarked("first_saved"));
        assert!(!app.state.is_bookmarked("second_saved"));
        assert_eq!(
            app.state_path,
            directory.path().join("accounts/first/state.json")
        );
        let other = State::load(&directory.path().join("accounts/second/state.json")).0;
        assert!(other.is_bookmarked("second_saved"));
        assert!(!other.is_bookmarked("first_saved"));
    }

    #[test]
    fn when_no_account_is_connected_start_should_not_queue_feed_or_subscription_loads() {
        let reddit = Reddit::disconnected(false).unwrap();
        let (pool, _messages) = Pool::start(reddit.clone(), 1, None);
        let directory = tempfile::tempdir().unwrap();
        let mut app = App::new(
            Config::default(),
            pool,
            reddit,
            directory.path().join("state.json"),
            Picker::halfblocks(),
        );
        app.start();
        assert!(!app.tabs[0].feed.loading());
        assert!(!app.all_subs.loading);
        assert_eq!(app.reddit.health(), Health::SignedOut);
    }

    #[test]
    fn when_reconnecting_the_same_verified_account_it_should_keep_the_selected_post_and_reading_state()
     {
        use crate::reddit::tests::{Fixture, Reply};
        let fixture = Fixture::new(|_| {
            Reply::json(serde_json::json!({"data":{"id":"sameaccount", "name":"reader"}}))
        });
        let (mut app, _messages) = app_at(&fixture.base, &[]);
        let directory = tempfile::tempdir().unwrap();
        app.state_path = directory.path().join("state.json");
        app.reddit.verify_account().unwrap();
        app.active = 0;
        app.tabs[0].feed.posts = vec![post("one", "rust"), post("two", "rust")];
        app.tabs[0].feed.selected = 1;
        app.tabs[0].feed.offset = 1;
        app.tabs[0].feed.loaded = true;
        app.state.toggle_bookmark(&post("saved", "rust"), 1000);
        let connection = fixture.reddit();
        connection.verify_account().unwrap();
        app.save_state();
        app.install_connection(connection);
        assert_eq!(app.tabs[0].feed.selected, 1);
        assert_eq!(app.tabs[0].feed.offset, 1);
        assert_eq!(app.tabs[0].feed.posts[1].id, "two");
        assert!(app.state.is_bookmarked("saved"));
        assert_eq!(
            app.state_path,
            directory.path().join("accounts/sameaccount/state.json")
        );
        assert!(app.take_replacement_messages().is_some());
    }

    #[test]
    fn when_reconnecting_it_should_not_replay_completed_subreddit_pages() {
        use crate::reddit::tests::{Fixture, Reply};
        let fixture = Fixture::new(|_| {
            Reply::json(serde_json::json!({"data":{"id":"sameaccount", "name":"reader"}}))
        });
        let (mut app, _messages) = app_at(&fixture.base, &[]);
        let directory = tempfile::tempdir().unwrap();
        app.state_path = directory.path().join("state.json");
        app.reddit.verify_account().unwrap();
        app.open_subreddit("rust");
        let index = app.active;
        app.on_msg(Msg::Feed {
            tab: app.tabs[index].id,
            generation: app.tabs[index].feed.generation,
            append: false,
            result: Ok(listing("rust", &["first"], Some("completed-cursor"))),
        });
        app.load_feed(index, true);
        app.on_msg(Msg::Feed {
            tab: app.tabs[index].id,
            generation: app.tabs[index].feed.generation,
            append: true,
            result: Ok(listing("rust", &["second"], None)),
        });
        assert_eq!(app.tabs[index].feed.requests.len(), 2);
        assert!(!app.tabs[index].feed.loading());
        let connection = fixture.reddit();
        connection.verify_account().unwrap();
        app.install_connection(connection);
        assert!(
            !app.tabs[index].feed.loading(),
            "Completed pages must stay idle"
        );
        assert!(app.tabs[index].feed.requests.is_empty());
        assert_eq!(app.tabs[index].feed.posts.len(), 2);
    }

    #[test]
    fn when_reconnecting_it_should_resume_interrupted_search_pages_and_islands() {
        use crate::reddit::tests::{Fixture, Reply};
        let identity = serde_json::json!({"data":{"id":"sameaccount", "name":"reader"}});
        let old_identity = identity.clone();
        let old = Fixture::new(move |_| Reply::json(old_identity.clone()));
        let requested = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let requests = requested.clone();
        let new = Fixture::new(move |request| {
            requests.lock().unwrap().push(request.url.clone());
            if request.url == "/api/me.json" {
                Reply::json(identity.clone())
            } else {
                Reply::json(serde_json::json!({"data":{"children":[], "after":null}}))
            }
        });
        let (mut app, _messages) = app_at(&old.base, &[]);
        let directory = tempfile::tempdir().unwrap();
        app.state_path = directory.path().join("state.json");
        app.reddit.verify_account().unwrap();
        app.open_subreddit("rust");
        let search = app.active;
        app.tabs[search].feed.search = Some("needle".into());
        app.fetch_feed(search, None, false);
        app.open_subreddit("python");
        let paged = app.active;
        app.fetch_feed(paged, Some("next-page".into()), false);
        app.active = search;
        app.all_subs.groups = vec![
            Group {
                subreddit: "unfinished".into(),
                loading: true,
                ..Group::default()
            },
            Group {
                subreddit: "failed".into(),
                error: Some("Sign in".into()),
                ..Group::default()
            },
        ];
        let connection = new.reddit();
        connection.verify_account().unwrap();
        app.install_connection(connection);

        assert!(app.tabs[search].feed.loading());
        assert!(app.tabs[paged].feed.loading_more());
        assert!(app.all_subs.groups.iter().all(|group| group.loading));
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let urls = requested.lock().unwrap().clone();
            if urls
                .iter()
                .any(|url| url.contains("/r/rust/search.json") && url.contains("needle"))
                && urls
                    .iter()
                    .any(|url| url.contains("/r/python/") && url.contains("next-page"))
                && urls.iter().any(|url| url.contains("/r/unfinished/"))
                && urls.iter().any(|url| url.contains("/r/failed/"))
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Replacement requests missing: {urls:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn when_switching_verified_accounts_it_should_drop_old_account_payloads_and_use_a_separate_state_file()
     {
        use crate::reddit::tests::{Fixture, Reply};
        let old = Fixture::new(|_| {
            Reply::json(serde_json::json!({"data":{"id":"oldaccount", "name":"oldreader"}}))
        });
        let new = Fixture::new(|_| {
            Reply::json(serde_json::json!({"data":{"id":"newaccount", "name":"newreader"}}))
        });
        let (mut app, _messages) = app_at(&old.base, &["privatecommunity"]);
        let directory = tempfile::tempdir().unwrap();
        app.state_path = directory.path().join("state.json");
        app.reddit.verify_account().unwrap();
        app.tabs[0].feed.posts = vec![post("privatepost", "privatecommunity")];
        app.all_subs.groups[0].posts = vec![post("islandpost", "privatecommunity")];
        app.pending_media.insert("/old-account".into());
        app.state
            .toggle_bookmark(&post("privatebookmark", "privatecommunity"), 1000);
        let connection = new.reddit();
        connection.verify_account().unwrap();
        app.save_state();
        app.install_connection(connection);
        assert!(app.tabs.iter().all(|t| t.feed.posts.is_empty()));
        assert!(app.all_subs.groups.is_empty());
        assert!(app.pending_media.is_empty());
        assert!(app.state.bookmarks().is_empty());
        assert_eq!(
            app.state_path,
            directory.path().join("accounts/newaccount/state.json")
        );
        assert!(
            directory.path().join("state.json").exists(),
            "the prior state stays intact"
        );
    }

    #[test]
    fn when_the_app_starts_it_should_load_home_and_account_subscriptions_in_background() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.server_addr());
        let handler = std::thread::spawn(move || {
            let mut completed = 0;
            while completed < 4 {
                let request = server
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap()
                    .unwrap();
                if request.method() == &tiny_http::Method::Head {
                    request.respond(tiny_http::Response::empty(200)).unwrap();
                    continue;
                }
                let path = request.url().split('?').next().unwrap();
                let body = match path {
                    "/svc/shreddit/feeds/home-feed" => {
                        "<shreddit-post id='t3_1x4v4sn'></shreddit-post>"
                    }
                    "/by_id/t3_1x4v4sn.json" | "/r/rust/hot.json" => {
                        include_str!("../tests/fixtures/sub.json")
                    }
                    "/subreddits/mine/subscriber.json" => {
                        r#"{"kind":"Listing","data":{"children":[{"kind":"t5","data":{"display_name":"rust"}}],"after":null}}"#
                    }
                    _ => panic!("unexpected API path: {path}"),
                };
                request
                    .respond(tiny_http::Response::from_string(body))
                    .unwrap();
                completed += 1;
            }
        });
        let (mut app, messages) = app_at(&base, &[]);
        app.active = 0;
        app.start();
        while !app.tabs[0].feed.loaded || app.all_subs.groups.first().is_none_or(|g| g.loading) {
            app.on_msg(
                messages
                    .recv_timeout(Duration::from_secs(5))
                    .expect("startup results"),
            );
        }
        handler.join().unwrap();

        assert_eq!(app.active_tab().title(), "Home");
        assert!(!app.tabs[0].feed.posts.is_empty());
        assert_eq!(app.all_subs.groups[0].subreddit, "rust");
        assert!(app.tabs.iter().any(
            |t| matches!(&t.kind, TabKind::Subreddit { name, pinned: true } if name == "rust")
        ));
    }

    #[test]
    fn when_home_starts_with_three_posts_it_should_fill_without_a_tab_switch() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.server_addr());
        let handler = std::thread::spawn(move || {
            let mut completed = 0;
            while completed < 4 {
                let request = server
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap()
                    .unwrap();
                if request.method() == &tiny_http::Method::Head {
                    request.respond(tiny_http::Response::empty(200)).unwrap();
                    continue;
                }
                let url = url::Url::parse(&format!("http://fixture{}", request.url())).unwrap();
                let body = match url.path() {
                    "/svc/shreddit/feeds/home-feed" => {
                        if url
                            .query_pairs()
                            .any(|(key, value)| key == "cursor" && value == "next&batch")
                        {
                            (0..25)
                                .map(|id| {
                                    format!(
                                        "<shreddit-post id='t3_recommended{id}'></shreddit-post>"
                                    )
                                })
                                .collect()
                        } else {
                            "<shreddit-post id='t3_a'></shreddit-post><shreddit-post id='t3_b'></shreddit-post><shreddit-post id='t3_c'></shreddit-post><faceplate-partial id='feed-next-page-partial' src='/svc/shreddit/feeds/home-feed?sort=BEST&amp;cursor=next%26batch'></faceplate-partial>".into()
                        }
                    }
                    path if path.starts_with("/by_id/") => {
                        let ids = path
                            .strip_prefix("/by_id/")
                            .unwrap()
                            .strip_suffix(".json")
                            .unwrap();
                        let children: Vec<_> = ids.split(',').map(|id| {
                            let id = id.strip_prefix("t3_").unwrap();
                            serde_json::json!({"kind":"t3","data":{"id":id,"title":id,"subreddit":"unjoined","author":"reader","permalink":format!("/r/unjoined/comments/{id}/title/")}})
                        }).collect();
                        serde_json::json!({"kind":"Listing","data":{"children":children,"after":null}}).to_string()
                    }
                    path => panic!("unexpected path: {path}"),
                };
                request
                    .respond(tiny_http::Response::from_string(body))
                    .unwrap();
                completed += 1;
            }
        });
        let (mut app, messages) = app_at(&base, &[]);

        app.switch_tab(0);
        while app.tabs[0].feed.posts.len() < 28 {
            app.on_msg(
                messages
                    .recv_timeout(Duration::from_secs(2))
                    .expect("Home should continue automatically"),
            );
        }
        handler.join().unwrap();
        let generation = app.tabs[0].feed.generation;
        app.switch_tab(BOOKMARKS_TAB);
        app.switch_tab(0);
        app.load_settled(Instant::now() + NAV_SETTLE);

        assert_eq!(app.tabs[0].feed.posts.len(), 28);
        assert_eq!(app.current_post().unwrap().id, "a");
        assert_eq!(app.tabs[0].feed.posts[3].id, "recommended0");
        assert_eq!(app.tabs[0].feed.generation, generation);
        assert!(!app.tabs[0].feed.loading());
        assert!(app.tabs[0].feed.after.is_none());
    }

    #[test]
    fn when_home_has_a_short_cached_head_it_should_keep_the_continuation_during_refresh() {
        let mut app = app_with(&[]);
        app.switch_tab(0);
        let tab = app.tabs[0].id;
        let first = app.tabs[0].feed.generation;

        app.on_msg(Msg::Snapshot {
            message: Box::new(Msg::Feed {
                tab,
                generation: first,
                append: false,
                result: Ok(listing("joined", &["a", "b", "c"], Some("next"))),
            }),
            fetched_at: now() - crate::cache::CONTENT_TTL,
            source: ordered_source(1, true),
            refreshing: true,
        });
        assert!(
            app.tabs[0].feed.loading(),
            "the cached primer should continue automatically"
        );
        let next = app.tabs[0].feed.generation;
        app.tabs[0].feed.selected = 1;
        app.on_msg(Msg::Snapshot {
            message: Box::new(Msg::Feed {
                tab,
                generation: next,
                append: true,
                result: Ok(listing(
                    "unjoined",
                    &["d", "e", "f", "g", "h", "i"],
                    Some("later"),
                )),
            }),
            fetched_at: now(),
            source: ordered_source(3, false),
            refreshing: false,
        });
        app.on_msg(Msg::Snapshot {
            message: Box::new(Msg::Feed {
                tab,
                generation: first,
                append: false,
                result: Ok(listing("joined", &["a", "b", "c"], Some("different-next"))),
            }),
            fetched_at: now(),
            source: ordered_source(2, false),
            refreshing: false,
        });
        app.switch_tab(BOOKMARKS_TAB);
        app.switch_tab(0);
        app.load_settled(Instant::now() + NAV_SETTLE);

        assert_eq!(app.tabs[0].feed.posts.len(), 9);
        assert_eq!(app.tabs[0].feed.after.as_deref(), Some("later"));
        assert_eq!(app.current_post().unwrap().id, "b");
        assert!(!app.tabs[0].feed.loading());
    }

    #[test]
    fn when_home_finishes_offscreen_it_should_continue_only_after_returning() {
        let mut app = app_with(&[]);
        app.switch_tab(0);
        let tab = app.tabs[0].id;
        let generation = app.tabs[0].feed.generation;
        app.switch_tab(BOOKMARKS_TAB);
        app.on_msg(Msg::Feed {
            tab,
            generation,
            append: false,
            result: Ok(listing("joined", &["a", "b", "c"], Some("next"))),
        });
        assert!(
            !app.tabs[0].feed.loading(),
            "inactive Home must not fetch more"
        );

        app.switch_tab(0);
        app.load_settled(Instant::now() + NAV_SETTLE);

        assert!(
            app.tabs[0].feed.loading(),
            "returning should resume the short list"
        );
        assert_eq!(app.tabs[0].feed.after.as_deref(), Some("next"));
    }

    #[test]
    fn when_home_continuation_fails_it_should_wait_for_an_explicit_retry() {
        let mut app = app_with(&[]);
        app.switch_tab(0);
        let tab = app.tabs[0].id;
        app.on_msg(Msg::Feed {
            tab,
            generation: app.tabs[0].feed.generation,
            append: false,
            result: Ok(listing("joined", &["a", "b", "c"], Some("next"))),
        });
        app.on_msg(Msg::Feed {
            tab,
            generation: app.tabs[0].feed.generation,
            append: true,
            result: Err("offline".into()),
        });
        app.switch_tab(BOOKMARKS_TAB);
        app.switch_tab(0);
        app.load_settled(Instant::now() + NAV_SETTLE);
        assert!(app.tabs[0].feed.page_failed());
        assert!(!app.tabs[0].feed.loading());

        app.load_feed(0, true);

        assert!(app.tabs[0].feed.loading());
        assert!(app.tabs[0].feed.error().is_none());
    }

    #[test]
    fn when_home_repeats_a_cursor_it_should_stop_automatic_paging() {
        let mut app = app_with(&[]);
        app.switch_tab(0);
        let tab = app.tabs[0].id;
        app.on_msg(Msg::Feed {
            tab,
            generation: app.tabs[0].feed.generation,
            append: false,
            result: Ok(listing("joined", &["a"], Some("loop"))),
        });
        app.on_msg(Msg::Feed {
            tab,
            generation: app.tabs[0].feed.generation,
            append: true,
            result: Ok(listing("unjoined", &["b"], Some("loop"))),
        });

        assert!(!app.tabs[0].feed.loading());
        assert_eq!(app.tabs[0].feed.posts.len(), 2);
    }

    #[test]
    fn when_home_filters_hide_every_page_it_should_bound_automatic_fetches() {
        let mut app = app_with(&[]);
        app.config.filters.keywords = vec!["title".into()];
        app.switch_tab(0);
        let tab = app.tabs[0].id;

        for page in 0..4 {
            app.on_msg(Msg::Feed {
                tab,
                generation: app.tabs[0].feed.generation,
                append: page != 0,
                result: Ok(listing(
                    "unjoined",
                    &["hidden"],
                    Some(&format!("next{page}")),
                )),
            });
            assert_eq!(app.tabs[0].feed.loading(), page < 3, "page {page}");
        }

        assert_eq!(app.config.visible(&app.tabs[0].feed.posts).count(), 0);
        app.load_feed(0, true);
        assert!(
            app.tabs[0].feed.loading(),
            "L can continue after the automatic limit"
        );
    }

    #[test]
    fn when_community_space_is_pressed_on_home_it_should_open_the_selected_subreddit() {
        let mut app = app_with(&[]);
        app.switch_tab(0);
        app.on_msg(Msg::Feed {
            tab: app.active_tab().id,
            generation: app.active_tab().feed.generation,
            append: false,
            result: Ok(listing("armenia", &["a"], None)),
        });

        crate::keys::handle(
            &mut app,
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char(' '),
                crossterm::event::KeyModifiers::NONE,
            ),
        );

        assert_eq!(app.active_tab().subreddit(), Some("armenia"));
        assert!(matches!(
            &app.active_tab().kind,
            TabKind::Subreddit { pinned: false, .. }
        ));
    }

    #[test]
    fn when_home_pages_it_should_keep_a_list_separate_from_all_subs() {
        let mut app = app_with(&["rust"]);
        app.switch_tab(0);
        let id = app.active_tab().id;
        let generation = app.active_tab().feed.generation;
        app.on_msg(Msg::Feed {
            tab: id,
            generation,
            append: false,
            result: Ok(listing("rust", &["a", "b"], Some("t3_b"))),
        });
        app.load_feed(0, true);
        let generation = app.active_tab().feed.generation;
        app.on_msg(Msg::Feed {
            tab: id,
            generation,
            append: true,
            result: Ok(listing("neovim", &["b", "c"], None)),
        });

        assert_eq!(
            app.tabs[0]
                .feed
                .posts
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b", "c"]
        );
        assert_eq!(app.tabs[0].feed.sort, Sort::Best);
        assert_eq!(app.current_post().unwrap().id, "a");
        assert!(app.all_subs.groups[0].posts.is_empty());
        app.toggle_bookmark();
        assert!(app.state.is_bookmarked("a"));
        remove_state(&app.state_path);
    }

    #[test]
    fn when_membership_changes_it_should_preserve_tabs_and_reject_old_island_results() {
        let mut app = app_with(&["rust"]);
        let index = tab_index(&app, "rust");
        app.switch_tab(index);
        app.tabs[index].feed.selected = 3;
        let tab_id = app.active_tab().id;
        let old_generation = app.all_subs.groups[0].generation;
        app.on_msg(Msg::Subscriptions {
            generation: 0,
            result: Ok(vec![]),
        });
        assert_eq!(app.active_tab().id, tab_id);
        assert_eq!(app.active_tab().feed.selected, 3);
        assert!(matches!(
            app.active_tab().kind,
            TabKind::Subreddit { pinned: false, .. }
        ));
        assert!(app.all_subs.groups.is_empty());

        app.on_msg(Msg::Subscriptions {
            generation: 0,
            result: Ok(vec!["rust".into()]),
        });
        app.on_msg(Msg::Group {
            generation: old_generation,
            subreddit: "rust".into(),
            result: Ok(listing("rust", &["obsolete"], None)),
        });
        assert_eq!(
            app.tabs
                .iter()
                .filter(|t| t.subreddit() == Some("rust"))
                .count(),
            1
        );
        assert_eq!(app.active_tab().id, tab_id);
        assert!(matches!(
            app.active_tab().kind,
            TabKind::Subreddit { pinned: true, .. }
        ));
        assert!(app.all_subs.groups[0].posts.is_empty());
    }

    fn press(app: &mut App, code: crossterm::event::KeyCode) {
        crate::keys::handle(
            app,
            crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE),
        );
    }

    #[test]
    fn when_join_leave_and_rejoin_are_confirmed_it_should_move_the_same_reading_tab() {
        use crate::reddit::tests::{Fixture, Reply};
        use crossterm::event::KeyCode;
        use serde_json::json;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let joined = Arc::new(AtomicBool::new(false));
        let writes = Arc::new(AtomicUsize::new(0));
        let actual = joined.clone();
        let count = writes.clone();
        let fixture = Fixture::new(move |request| {
            let url = url::Url::parse(&format!("http://fixture{}", request.url)).unwrap();
            match url.path() {
                "/api/me.json" => {
                    Reply::json(json!({"data":{"name":"reader","modhash":"fixture-csrf"}}))
                }
                "/api/subscribe" => {
                    let form: HashMap<_, _> = url::form_urlencoded::parse(request.body.as_bytes())
                        .into_owned()
                        .collect();
                    assert_eq!(form["sr_name"], "armenia");
                    count.fetch_add(1, Ordering::SeqCst);
                    actual.store(form["action"] == "sub", Ordering::SeqCst);
                    Reply::json(json!({}))
                }
                "/subreddits/mine/subscriber.json" => {
                    let mut children = vec![json!({"kind":"t5","data":{"display_name":"rust"}})];
                    if actual.load(Ordering::SeqCst) {
                        children.push(json!({"kind":"t5","data":{"display_name":"armenia"}}));
                    }
                    Reply::json(json!({"kind":"Listing","data":{"children":children,"after":null}}))
                }
                path if path.ends_with("/hot.json") => {
                    let sub = path.split('/').nth(2).unwrap();
                    let children: Vec<_> = ["a","b","c","d"].into_iter().map(|id|
                        json!({"kind":"t3","data":{"id":id,"title":id,"subreddit":sub,"permalink":format!("/r/{sub}/comments/{id}/title/"),"author":"reader"}})).collect();
                    Reply::json(json!({"kind":"Listing","data":{"children":children,"after":null}}))
                }
                _ => Reply::status(404),
            }
        });
        let (mut app, messages) = app_at(&fixture.base, &["rust"]);
        app.open_subreddit("armenia");
        while app.active_tab().feed.loading() {
            app.on_msg(
                messages
                    .recv_timeout(Duration::from_secs(5))
                    .expect("feed response"),
            );
        }
        app.tabs[app.active].feed.selected = 3;
        app.tabs[app.active].feed.offset = 2;
        let tab = app.active_tab().id;
        for (expected_count, join) in [(1, true), (2, false), (3, true)] {
            press(&mut app, KeyCode::Char('S'));
            assert!(
                matches!(&app.overlay, Some(Overlay::Membership { subreddit, join: action }) if subreddit == "armenia" && *action == join)
            );
            press(&mut app, KeyCode::Enter);
            assert!(app.overlay.is_none());
            while app.membership.is_some() {
                app.on_msg(
                    messages
                        .recv_timeout(Duration::from_secs(5))
                        .expect("membership response"),
                );
            }
            assert_eq!(joined.load(Ordering::SeqCst), join);
            assert_eq!(writes.load(Ordering::SeqCst), expected_count);
            assert_eq!(app.active_tab().id, tab);
            assert_eq!(app.active_tab().feed.selected, 3);
            assert_eq!(app.active_tab().feed.offset, 2);
            assert_eq!(app.joined("armenia"), Some(join));
            assert!(
                matches!(app.active_tab().kind, TabKind::Subreddit { pinned, .. } if pinned == join)
            );
            assert_eq!(
                app.tabs
                    .iter()
                    .filter(|tab| tab.subreddit() == Some("armenia"))
                    .count(),
                1
            );
        }
    }

    #[test]
    fn when_membership_is_pending_it_should_ignore_duplicate_writes_and_old_or_cached_lists() {
        let mut app = app_with(&["rust"]);
        let old = app.all_subs.generation;
        app.set_membership("armenia".into(), true);
        let generation = app.membership.as_ref().unwrap().generation;
        app.set_membership("neovim".into(), true);
        assert_eq!(app.membership.as_ref().unwrap().generation, generation);
        app.on_msg(Msg::Subscriptions {
            generation: old,
            result: Ok(vec![]),
        });
        assert_eq!(app.joined("rust"), Some(true));
        app.on_msg(Msg::Membership {
            generation: generation + 1,
            result: Ok(()),
        });
        assert!(!app.membership.as_ref().unwrap().checking);
        app.on_msg(Msg::Membership {
            generation,
            result: Ok(()),
        });
        let read = app.all_subs.generation;
        app.on_msg(Msg::Snapshot {
            message: Box::new(Msg::Subscriptions {
                generation: read,
                result: Ok(vec!["armenia".into()]),
            }),
            fetched_at: now(),
            refreshing: false,
            source: Source {
                cached: true,
                ..Source::default()
            },
        });
        assert!(app.membership.is_some());
        assert_eq!(app.joined("armenia"), Some(false));
        app.on_msg(Msg::Subscriptions {
            generation: read,
            result: Ok(vec!["armenia".into(), "rust".into()]),
        });
        assert!(app.membership.is_none());
        app.on_msg(Msg::Subscriptions {
            generation: old,
            result: Ok(vec![]),
        });
        assert_eq!(app.joined("armenia"), Some(true));
    }

    #[test]
    fn when_membership_fails_or_is_ambiguous_it_should_follow_the_fresh_account_list() {
        for (confirmed, read_fails) in [(false, false), (true, false), (false, true)] {
            let mut app = app_with(&["rust"]);
            app.open_subreddit("armenia");
            app.set_membership("armenia".into(), true);
            let generation = app.membership.as_ref().unwrap().generation;
            app.on_msg(Msg::Membership {
                generation,
                result: Err("write unavailable".into()),
            });
            let result = if read_fails {
                Err("read unavailable".into())
            } else if confirmed {
                Ok(vec!["armenia".into(), "rust".into()])
            } else {
                Ok(vec!["rust".into()])
            };
            app.on_msg(Msg::Subscriptions {
                generation: app.all_subs.generation,
                result,
            });
            assert!(app.membership.is_none());
            assert_eq!(
                app.joined("armenia"),
                if read_fails { None } else { Some(confirmed) }
            );
            assert_eq!(
                app.joined("rust"),
                if read_fails { None } else { Some(true) }
            );
            assert!(
                matches!(app.active_tab().kind, TabKind::Subreddit { pinned, .. } if pinned == confirmed)
            );
        }
    }

    #[test]
    fn when_membership_controls_are_used_it_should_resolve_empty_views_and_preserve_text_input() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = app_with(&["rust"]);
        assert_eq!(
            app.community().as_deref(),
            Some("rust"),
            "empty All subs island"
        );
        press(&mut app, KeyCode::Char('S'));
        assert!(
            matches!(&app.overlay, Some(Overlay::Membership { subreddit, join: false }) if subreddit == "rust")
        );
        press(&mut app, KeyCode::Esc);
        assert!(app.membership.is_none());
        app.open_subreddit("armenia");
        assert_eq!(app.community().as_deref(), Some("armenia"));
        app.focus = Focus::Sidebar;
        press(&mut app, KeyCode::Char('S'));
        assert!(
            matches!(&app.overlay, Some(Overlay::Membership { subreddit, join: true }) if subreddit == "armenia")
        );
        press(&mut app, KeyCode::Char(' '));
        assert!(app.membership.is_none());
        assert!(app.overlay.is_none());
        crate::keys::handle(
            &mut app,
            KeyEvent::new(KeyCode::Char('S'), KeyModifiers::CONTROL),
        );
        assert!(app.overlay.is_none());
        app.overlay = Some(Overlay::Search {
            input: String::new(),
        });
        press(&mut app, KeyCode::Char('S'));
        assert!(matches!(&app.overlay, Some(Overlay::Search { input }) if input == "S"));
    }

    #[test]
    fn when_subscription_refresh_fails_it_should_retain_membership_and_expose_retry() {
        let mut app = app_with(&["rust"]);
        app.reload();
        let generation = app.all_subs.generation;
        app.on_msg(Msg::Subscriptions {
            generation,
            result: Err("offline".into()),
        });

        assert!(!app.all_subs.loading);
        assert_eq!(app.all_subs.error.as_deref(), Some("offline"));
        assert_eq!(app.all_subs.groups[0].subreddit, "rust");
        app.on_msg(Msg::Subscriptions {
            generation: generation - 1,
            result: Ok(vec![]),
        });
        assert_eq!(app.all_subs.groups.len(), 1);
    }

    #[test]
    fn when_subscriptions_reorder_it_should_keep_the_selected_island() {
        let mut app = app_with(&["rust", "vim"]);
        app.all_subs.group = 1;
        app.on_msg(Msg::Subscriptions {
            generation: 0,
            result: Ok(vec!["neovim".into(), "rust".into(), "vim".into()]),
        });
        assert_eq!(app.all_subs.groups[app.all_subs.group].subreddit, "vim");
        assert_eq!(app.active, ALL_SUBS_TAB);
    }

    /// An app plus the replies its jobs produce. Port 9 is discard: nothing answers,
    /// so every network job fails quickly and no test touches the network.
    fn app_with_messages(subscriptions: &[&str]) -> (App, std::sync::mpsc::Receiver<Msg>) {
        app_at("http://127.0.0.1:9", subscriptions)
    }

    /// An app whose Reddit API is the server at `url`, without a disk cache, plus the replies
    /// its jobs produce.
    fn app_at(url: &str, subscriptions: &[&str]) -> (App, std::sync::mpsc::Receiver<Msg>) {
        let reddit = Reddit::test_at(url, false);
        let (pool, messages) = Pool::start(reddit.clone(), 3, None);
        let state = std::env::temp_dir().join(format!(
            "redtab-app-test-{}-{:?}.json",
            std::process::id(),
            std::thread::current().id()
        ));
        let mut app = App::new(Config::default(), pool, reddit, state, Picker::halfblocks());
        app.on_msg(Msg::Subscriptions {
            generation: 0,
            result: Ok(subscriptions.iter().map(|s| s.to_string()).collect()),
        });
        app.active = ALL_SUBS_TAB;
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
            permalink: format!("/r/{sub}/comments/{id}/x/"),
            media: Media::None,
            preview_size: None,
            stickied: false,
            nsfw: false,
            spoiler: false,
        }
    }

    fn listing(sub: &str, ids: &[&str], after: Option<&str>) -> Listing {
        Listing {
            posts: ids.iter().map(|i| post(i, sub)).collect(),
            after: after.map(str::to_string),
        }
    }

    fn comment(id: &str, replies: Vec<Comment>, more: Option<&str>) -> Comment {
        Comment {
            sources: None,
            created: None,
            id: id.into(),
            author: "a".into(),
            flair: None,
            is_op: false,
            is_mod: false,
            score: None,
            body: id.into(),
            replies,
            more: more.map(|p| MoreReplies {
                cursor: p.into(),
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

    /// r/s listing `ids`, its first post open beside the list and the list focused.
    fn split_feed(ids: &[&str]) -> (App, std::sync::mpsc::Receiver<Msg>) {
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

    /// A fixture Reddit API that answers each post page `delay` late, as `post.json`
    /// retitled with the requested id, and records the post ids asked for. Other paths
    /// answer at once, empty.
    struct SlowReddit {
        url: String,
        asked: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl SlowReddit {
        fn start(delay: Duration) -> Self {
            use std::io::{BufRead, BufReader, Write};
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let log = std::sync::Arc::clone(&asked);
            std::thread::spawn(move || {
                for mut stream in listener.incoming().flatten() {
                    let log = std::sync::Arc::clone(&log);
                    std::thread::spawn(move || {
                        let mut reader = BufReader::new(stream.try_clone().unwrap());
                        let mut request = String::new();
                        let _ = reader.read_line(&mut request);
                        let mut line = String::new();
                        while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                            line.clear();
                        }
                        let path = request.split(' ').nth(1).unwrap_or_default();
                        let body = match path
                            .split('/')
                            .nth(2)
                            .filter(|_| path.starts_with("/comments/"))
                            .map(|id| id.split('.').next().unwrap())
                        {
                            Some(id) => {
                                log.lock().unwrap().push(id.to_string());
                                std::thread::sleep(delay);
                                include_str!("../tests/fixtures/post.json").replace("1x4v4sn", id)
                            }
                            None => String::new(),
                        };
                        let _ = write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                    });
                }
            });
            Self { url, asked }
        }

        /// Home with one card per post id, the first post open beside the cards and the
        /// cards focused. No subscriptions: nothing else asks this server for pages.
        fn home(&self, ids: &[&str]) -> (App, std::sync::mpsc::Receiver<Msg>) {
            let (mut app, messages) = app_at(&self.url, &[]);
            app.all_subs.groups = ids
                .iter()
                .enumerate()
                .map(|(i, id)| Group {
                    subreddit: format!("s{i}"),
                    posts: vec![post(id, &format!("s{i}"))],
                    ..Group::default()
                })
                .collect();
            app.resize(200);
            app.open_current_post();
            app.focus_main();
            (app, messages)
        }

        fn asked(&self) -> Vec<String> {
            self.asked.lock().unwrap().clone()
        }
    }

    #[test]
    fn a_rate_limit_shows_paused_requests_not_an_unanswering_reddit() {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                    line.clear();
                }
                let _ = write!(
                    stream,
                    "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 60\r\n\
                     Content-Length: 0\r\nConnection: close\r\n\r\n"
                );
            }
        });
        let (mut app, messages) = app_at(&url, &[]);
        app.status = None;
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(110, 30)).unwrap();
        let mut footer = |app: &mut App| {
            let buffer = terminal
                .draw(|frame| crate::ui::draw(frame, app))
                .unwrap()
                .buffer
                .clone();
            (0..110)
                .map(|x| buffer[(x, 29)].symbol())
                .collect::<String>()
        };

        assert!(app.reddit.search("rust", "x", None).is_err());
        let started = Instant::now();
        let mut shown = footer(&mut app);
        while !shown.contains("paused")
            && !shown.contains("not answering")
            && started.elapsed() < Duration::from_secs(5)
        {
            if let Ok(msg) = messages.recv_timeout(Duration::from_millis(20)) {
                app.on_msg(msg);
            }
            app.tick();
            shown = footer(&mut app);
        }

        assert!(shown.contains("paused"), "{shown}");
    }

    /// Handle replies and timers until the shown post's page is in; how long that took.
    fn wait_for_page(app: &mut App, messages: &std::sync::mpsc::Receiver<Msg>) -> Duration {
        let started = Instant::now();
        while app.post_view.as_ref().is_some_and(|v| v.page.is_none())
            && started.elapsed() < Duration::from_secs(10)
        {
            if let Ok(msg) = messages.recv_timeout(Duration::from_millis(20)) {
                app.on_msg(msg);
            }
            app.tick();
        }
        started.elapsed()
    }

    #[test]
    fn coming_back_to_a_post_still_loading_shows_its_first_answer_without_another_wait() {
        let delay = Duration::from_secs(1);
        let server = SlowReddit::start(delay);
        let (mut app, messages) = server.home(&["warm", "fa", "fb"]);
        wait_for_page(&mut app, &messages);
        let pause = NAV_SETTLE + Duration::from_millis(50);

        app.move_selection(1);
        std::thread::sleep(pause);
        app.move_selection(1);
        std::thread::sleep(pause);
        app.move_selection(-1);
        let waited = wait_for_page(&mut app, &messages);

        assert_eq!(app.post_view.as_ref().unwrap().post.id, "fa");
        assert!(
            waited < delay,
            "waited {waited:?}: fa's first load, still running, answers it"
        );
    }

    #[test]
    fn post_loads_moved_past_while_queued_never_reach_reddit() {
        let server = SlowReddit::start(Duration::from_millis(1500));
        let (mut app, messages) = server.home(&["warm", "fa", "fb", "fc", "fd"]);
        wait_for_page(&mut app, &messages);
        let pause = NAV_SETTLE + Duration::from_millis(50);

        for _ in 0..4 {
            app.move_selection(1);
            std::thread::sleep(pause);
        }
        wait_for_page(&mut app, &messages);

        assert_eq!(app.post_view.as_ref().unwrap().post.id, "fd");
        assert_eq!(
            server.asked(),
            ["warm", "fa", "fb", "fd"],
            "fc waited behind fa and fb and was moved past"
        );
    }

    #[test]
    fn beside_an_open_post_a_quick_run_of_list_moves_previews_each_and_loads_only_its_end() {
        let (mut app, _) = split_feed(&["run1", "run2", "run3", "run4"]);

        app.move_selection(1);
        app.move_selection(1);
        assert_eq!(app.post_view.as_ref().unwrap().post.id, "run3");
        app.move_selection(1);

        assert_eq!(app.post_view.as_ref().unwrap().post.id, "run4");
        assert_eq!(app.focus, Focus::List, "the list keeps focus");
        // A post counts as read exactly when its page is asked for.
        assert!(
            app.state.is_read("run2"),
            "a move after a pause loads at once"
        );
        assert!(!app.state.is_read("run4"), "mid-run nothing loads");
        assert!(!app.load_settled(Instant::now()));
        assert!(app.load_settled(Instant::now() + NAV_SETTLE));
        assert!(
            !app.state.is_read("run3"),
            "a post passed over stays unread"
        );
        assert!(app.state.is_read("run4"));
    }

    #[test]
    fn opening_a_held_back_preview_loads_it_at_once_and_only_once() {
        let (mut app, _) = split_feed(&["hold1", "hold2", "hold3"]);
        app.move_selection(1);
        app.move_selection(1);
        assert!(!app.state.is_read("hold3"), "held back");

        app.open_current_post();

        assert_eq!(app.focus, Focus::Post);
        assert!(app.state.is_read("hold3"));
        assert!(
            !app.load_settled(Instant::now() + NAV_SETTLE),
            "opening ended the wait"
        );
    }

    #[test]
    fn a_quick_run_of_tab_switches_loads_only_the_tab_it_stops_on() {
        let mut app = app_with(&["a", "b", "c"]);
        let [a, b, c] = ["a", "b", "c"].map(|name| tab_index(&app, name));

        app.switch_tab(a);
        app.switch_tab(b);
        app.switch_tab(c);

        assert!(
            app.tabs[a].feed.loading(),
            "a switch after a pause loads at once"
        );
        assert!(!app.tabs[c].feed.loading(), "mid-run nothing loads");
        assert!(app.load_settled(Instant::now() + NAV_SETTLE));
        assert!(
            !app.tabs[b].feed.loading(),
            "a tab passed over is not loaded"
        );
        assert!(app.tabs[c].feed.loading());
    }

    #[test]
    fn when_sidebar_navigation_starts_its_first_feed_and_images_should_wait_for_settling() {
        let mut app = app_with(&["a", "b"]);
        app.focus_sidebar();

        app.move_sidebar(1);

        let a = tab_index(&app, "a");
        assert_eq!(app.active, a);
        assert!(!app.tabs[a].feed.loading());
        app.card_pictures_drawn(&["/image.png"]);
        assert!(app.card_images.is_empty());
        assert!(!app.load_settled(Instant::now()));
        assert!(app.load_settled(Instant::now() + NAV_SETTLE));
        assert!(app.tabs[a].feed.loading());
    }

    #[test]
    fn when_sidebar_navigation_crosses_several_tabs_only_the_settled_tab_should_load() {
        let mut app = app_with(&["a", "b", "c"]);
        app.focus_sidebar();

        app.move_sidebar(1);
        app.move_sidebar(1);
        app.move_sidebar(1);

        let [a, b, c] = ["a", "b", "c"].map(|name| tab_index(&app, name));
        assert_eq!(app.active, c);
        assert!(!app.tabs[a].feed.loading());
        assert!(!app.tabs[b].feed.loading());
        assert!(!app.tabs[c].feed.loading());
        app.load_settled(Instant::now() + NAV_SETTLE);
        assert!(!app.tabs[a].feed.loading());
        assert!(!app.tabs[b].feed.loading());
        assert!(app.tabs[c].feed.loading());
    }

    #[test]
    fn when_sidebar_navigation_is_waiting_an_explicit_reload_should_load_immediately_once() {
        let mut app = app_with(&["a"]);
        app.focus_sidebar();
        app.move_sidebar(1);

        app.reload();

        assert!(app.active_tab().feed.loading());
        assert!(!app.load_settled(Instant::now() + NAV_SETTLE));
    }

    #[test]
    fn reloading_a_newer_cached_copy_keeps_it_over_an_older_loads_late_answer() {
        let (mut app, _) = split_feed(&["ord1", "ord2"]);
        let first = app.post_view.as_ref().unwrap().generation;
        app.move_selection(1);
        app.move_selection(-1);
        let back = app.post_view.as_ref().unwrap().generation;
        let answer = |generation, body: &str, order, cached| {
            let page = PostPage {
                post: post("ord1", "s"),
                body: body.into(),
                comments: vec![],
            };
            ordered_snapshot(
                Msg::Post {
                    generation,
                    result: Ok(Box::new(page)),
                },
                order,
                cached,
                false,
            )
        };
        app.on_msg(answer(back, "newer", 12, true));
        app.open_current_post();
        app.reload();
        let refresh = app.post_view.as_ref().unwrap().generation;

        app.on_msg(answer(first, "older", 10, false));

        let view = app.post_view.as_ref().unwrap();
        assert_eq!(view.generation, refresh, "the refresh still owns the view");
        assert_eq!(view.page.as_ref().unwrap().body, "newer");
    }

    #[test]
    fn coming_back_over_a_cached_copy_takes_only_the_earlier_loads_network_answer() {
        let (mut app, _) = split_feed(&["back1", "back2"]);
        let first = app.post_view.as_ref().unwrap().generation;
        app.move_selection(1);
        app.move_selection(-1);
        let back = app.post_view.as_ref().unwrap().generation;
        let answer = |generation, order, cached, refreshing| {
            let page = PostPage {
                post: post("back1", "s"),
                body: String::new(),
                comments: vec![],
            };
            ordered_snapshot(
                Msg::Post {
                    generation,
                    result: Ok(Box::new(page)),
                },
                order,
                cached,
                refreshing,
            )
        };

        app.on_msg(answer(first, 1, true, true));
        assert_eq!(
            app.post_view.as_ref().unwrap().generation,
            back,
            "a cached answer still refreshing is not taken"
        );
        app.on_msg(answer(back, 1, true, true));
        // Replies asked for under the newer load, whose answer will be dropped.
        let view = app.post_view.as_mut().unwrap();
        view.loading_more.insert("parent".into());
        app.on_msg(answer(first, 2, false, false));

        let view = app.post_view.as_ref().unwrap();
        assert_eq!(
            view.generation, first,
            "the earlier load's network answer replaces the cached copy"
        );
        assert!(!view.loading);
        assert!(
            view.loading_more.is_empty(),
            "no reply row stays loading for a dropped answer"
        );
    }

    #[test]
    fn beside_an_open_post_a_card_that_loads_under_the_cursor_shows_its_post() {
        let mut app = app_with(&["ca", "cb"]);
        let group = |subreddit: &str, id: &str| Msg::Group {
            subreddit: subreddit.into(),
            generation: 0,
            result: Ok(listing(subreddit, &[id], None)),
        };
        app.on_msg(group("ca", "card1"));
        app.resize(200);
        app.open_current_post();
        app.focus_main();
        app.move_all_subs_group(1);
        assert_eq!(
            app.post_view.as_ref().unwrap().post.id,
            "card1",
            "the next card has nothing yet"
        );

        app.on_msg(group("cb", "card2"));

        assert_eq!(app.post_view.as_ref().unwrap().post.id, "card2");
    }

    #[test]
    fn reloading_a_held_back_preview_marks_it_read() {
        let (mut app, _) = split_feed(&["rel1", "rel2", "rel3"]);
        app.move_selection(1);
        app.move_selection(1);
        app.resize(100);

        app.reload();

        assert!(app.state.is_read("rel3"));
    }

    #[test]
    fn pictures_a_quick_run_of_moves_passes_wait_until_it_stops() {
        let mut app = app_with(&[]);
        let (thumb, picture) = (
            "https://i.redd.it/row.png",
            "https://preview.redd.it/text.png",
        );
        app.move_selection(1);
        app.thumbs_drawn(&[thumb]);
        assert!(app.thumbs.contains_key(thumb), "a lone move loads at once");
        app.thumbs.clear();

        app.move_selection(1);
        app.thumbs_drawn(&[thumb]);
        app.request_text_picture(picture);

        assert!(!app.thumbs.contains_key(thumb), "mid-run rows wait");
        assert!(!app.images.contains_key(picture), "mid-run pictures wait");
        std::thread::sleep(NAV_SETTLE);
        assert!(app.tick(), "the pause redraws");
        app.thumbs_drawn(&[thumb]);
        app.request_text_picture(picture);
        assert!(app.thumbs.contains_key(thumb));
        assert!(app.images.contains_key(picture));
    }

    #[test]
    fn a_reload_during_a_tab_switch_wait_is_not_asked_again_once_it_fails() {
        let mut app = app_with(&["a", "b"]);
        let [a, b] = ["a", "b"].map(|name| tab_index(&app, name));
        app.switch_tab(a);
        app.switch_tab(b);

        app.reload();
        let (tab, generation) = (app.tabs[b].id, app.tabs[b].feed.generation);
        app.on_msg(Msg::Feed {
            tab,
            generation,
            append: false,
            result: Err("connection refused".into()),
        });

        assert!(!app.load_settled(Instant::now() + NAV_SETTLE));
        assert!(
            !app.tabs[b].feed.loading(),
            "the failed reload is not repeated"
        );
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
            images: vec!["https://i.redd.it/full.jpg".into()],
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
            !app.tabs[i].feed.loading(),
            "must not retry on every key after a failure"
        );
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
        assert_eq!(resorted.error(), Some("timeout"));
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
        app.on_msg(thread_message(
            generation,
            vec![
                comment("c1", vec![], Some("/r/s/comments/p/x/c1")),
                comment("c2", vec![], None),
                comment("c3", vec![], None),
            ],
        ));
        let view = app.post_view.as_mut().unwrap();
        view.free_scroll = false;
        view.cursor = 3; // rows: c1, more, c2, c3

        app.on_msg(Msg::MoreReplies {
            generation,
            comment_id: "c1".into(),
            result: Ok(ReplyPage::complete(vec![
                comment("r1", vec![], None),
                comment("r2", vec![], None),
            ])),
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
        app.on_msg(thread_message(
            generation,
            vec![
                comment("c1", vec![], Some("/r/s/comments/p/x/c1")),
                comment("c2", vec![], Some("/r/s/comments/p/x/c2")),
            ],
        ));
        let view = app.post_view.as_mut().unwrap();
        view.free_scroll = false;
        view.cursor = 3; // rows: c1, more(c1), c2, more(c2)

        app.on_msg(Msg::MoreReplies {
            generation,
            comment_id: "c1".into(),
            result: Ok(ReplyPage::complete(vec![
                comment("r1", vec![], None),
                comment("r2", vec![], None),
            ])),
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
            generation: 0,
            subreddit: "a".into(),
            result: Ok(listing("a", &["a1", "a2", "a3"], None)),
        });
        app.all_subs.post = 2;

        app.on_msg(Msg::Group {
            generation: 0,
            subreddit: "a".into(),
            result: Ok(listing("a", &["a1"], None)),
        });

        assert_eq!(app.current_post().map(|p| p.id).as_deref(), Some("a1"));
    }

    #[test]
    fn passing_through_a_tab_keeps_its_new_badge_but_engaging_clears_it() {
        let mut app = app_with(&["a", "b", "c"]);
        app.on_msg(Msg::Group {
            generation: 0,
            subreddit: "b".into(),
            result: Ok(listing("b", &["b1"], None)),
        });
        // New posts appear in r/b after the baseline.
        app.on_msg(Msg::Group {
            generation: 0,
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
    fn home_navigation_flows_across_groups_and_skips_pinned_posts() {
        let mut app = app_with(&["a", "b"]);
        let mut pinned = listing("a", &["pin", "a1", "a2"], None);
        pinned.posts[0].stickied = true;
        app.on_msg(Msg::Group {
            generation: 0,
            subreddit: "a".into(),
            result: Ok(pinned),
        });
        app.on_msg(Msg::Group {
            generation: 0,
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
            result: Ok(ReplyPage::complete(vec![comment("c1c", vec![], None)])),
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
        app.on_msg(thread_message(
            generation,
            vec![comment("c1", vec![], None), comment("c2", vec![], None)],
        ));
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
        app.on_msg(thread_message(
            generation,
            vec![comment("c1", vec![], None), comment("c2", vec![], None)],
        ));
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
        app.on_msg(thread_message(
            generation,
            ["c1", "c2", "c3"]
                .map(|id| comment(id, vec![], None))
                .into(),
        ));
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
        app.on_msg(thread_message(
            generation,
            vec![comment(
                "c",
                vec![comment("r1", vec![], None), comment("r2", vec![], None)],
                None,
            )],
        ));
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
        app.on_msg(thread_message(
            generation,
            ["c0", "c1", "c2", "c3"]
                .map(|id| comment(id, vec![], None))
                .into(),
        ));
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
        app.on_msg(thread_message(
            generation,
            vec![
                comment("a", replies, None),
                comment("b", vec![], None),
                comment("c", vec![], None),
            ],
        ));
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
    fn opening_a_subreddit_accepts_common_reddit_spellings() {
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
            generation: 0,
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

    #[test]
    fn cache_refresh_keeps_the_selected_post_and_screen_row() {
        let mut app = app_with(&["vim"]);
        let i = tab_index(&app, "vim");
        app.switch_tab(i);
        let tab = app.tabs[i].id;
        app.on_msg(Msg::Feed {
            tab,
            generation: app.tabs[i].feed.generation,
            append: false,
            result: Ok(listing("vim", &["a", "b", "c"], None)),
        });
        app.tabs[i].feed.selected = 1;
        app.tabs[i].feed.offset = 1;
        app.reload();

        app.on_msg(Msg::Feed {
            tab,
            generation: app.tabs[i].feed.generation,
            append: false,
            result: Ok(listing("vim", &["new", "a", "b", "c"], None)),
        });

        assert_eq!(app.current_post().unwrap().id, "b");
        assert_eq!(app.tabs[i].feed.selected - app.tabs[i].feed.offset, 0);
    }

    #[test]
    fn cache_refresh_keeps_post_content_visible_while_loading() {
        let mut app = app_with(&[]);
        app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
        app.on_msg(Msg::Post {
            generation: app.post_view.as_ref().unwrap().generation,
            result: Ok(Box::new(PostPage {
                post: post("p", "s"),
                body: "saved body".into(),
                comments: vec![comment("c1", vec![], None), comment("c2", vec![], None)],
            })),
        });
        let view = app.post_view.as_mut().unwrap();
        view.scroll = 12;
        view.cursor = 1;
        view.free_scroll = false;
        view.collapsed.insert("c1".into());

        app.reload();

        let view = app.post_view.as_ref().unwrap();
        assert_eq!(view.page.as_ref().unwrap().body, "saved body");
        assert_eq!((view.scroll, view.cursor, view.free_scroll), (12, 1, false));
        assert!(view.collapsed.contains("c1"));
        assert!(view.loading);
    }

    #[test]
    fn cached_post_refresh_failure_still_renders_the_saved_body() {
        let mut app = app_with(&[]);
        app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
        let generation = app.post_view.as_ref().unwrap().generation;
        app.on_msg(Msg::Post {
            generation,
            result: Ok(Box::new(PostPage {
                post: post("p", "s"),
                body: "offline reading works".into(),
                comments: vec![],
            })),
        });
        app.on_msg(Msg::Post {
            generation,
            result: Err("connection refused".into()),
        });
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(110, 30)).unwrap();

        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();

        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("offline reading works"), "{text}");
    }

    #[test]
    fn cached_refresh_keeps_loaded_later_pages_and_their_cursor() {
        let mut app = app_with(&["vim"]);
        let i = tab_index(&app, "vim");
        app.switch_tab(i);
        let tab = app.tabs[i].id;
        let generation = app.tabs[i].feed.generation;
        app.on_msg(Msg::Feed {
            tab,
            generation,
            append: false,
            result: Ok(listing("vim", &["a", "b"], Some("t3_b"))),
        });
        app.on_msg(Msg::Feed {
            tab,
            generation,
            append: true,
            result: Ok(listing("vim", &["c", "d"], Some("t3_d"))),
        });
        app.tabs[i].feed.selected = 3;
        app.reload();

        app.on_msg(Msg::Feed {
            tab,
            generation: app.tabs[i].feed.generation,
            append: false,
            result: Ok(listing("vim", &["new", "a"], Some("t3_a"))),
        });

        assert_eq!(app.current_post().unwrap().id, "d");
        assert_eq!(app.tabs[i].feed.after.as_deref(), Some("t3_d"));
        assert_eq!(
            app.tabs[i]
                .feed
                .posts
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>(),
            ["new", "a", "c", "d"]
        );
    }

    #[test]
    fn cached_post_refresh_preserves_expanded_replies_and_comment_anchor() {
        let mut app = app_with(&[]);
        app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
        let generation = app.post_view.as_ref().unwrap().generation;
        app.on_msg(thread_message(
            generation,
            vec![
                comment("c1", vec![comment("reply", vec![], None)], None),
                comment("c2", vec![], None),
            ],
        ));
        let view = app.post_view.as_mut().unwrap();
        view.cursor = 2;
        view.free_scroll = false;
        view.row_starts = vec![5, 10, 20];
        view.drawn_cursor = Some(2);
        view.scroll = 21;

        app.on_msg(thread_message(
            generation,
            vec![
                comment("new", vec![], None),
                comment("c1", vec![], Some("/more")),
                comment("c2", vec![], None),
            ],
        ));

        let view = app.post_view.as_mut().unwrap();
        assert!(matches!(view.cursor_key(), Some(RowKey::Comment(id)) if id == "c2"));
        assert_eq!(view.take_reading_anchor(), Some((4, 1)));
        assert_eq!(
            view.page.as_ref().unwrap().comments[1].replies[0].id,
            "reply"
        );
    }

    #[test]
    fn a_late_home_snapshot_cannot_replace_a_forced_refresh() {
        let mut app = app_with(&["vim"]);
        app.reload();
        app.on_msg(Msg::Group {
            subreddit: "vim".into(),
            generation: 1,
            result: Ok(listing("vim", &["new"], None)),
        });

        app.on_msg(Msg::Snapshot {
            message: Box::new(Msg::Group {
                subreddit: "vim".into(),
                generation: 0,
                result: Ok(listing("vim", &["old"], None)),
            }),
            fetched_at: 1,
            refreshing: true,
            source: Source {
                cached: true,
                ..Source::default()
            },
        });

        assert_eq!(app.all_subs.groups[0].posts[0].id, "new");
        assert!(!app.all_subs.groups[0].loading);
    }

    #[test]
    fn refreshed_cached_replies_update_text_without_losing_expanded_children() {
        let mut app = app_with(&[]);
        app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
        let generation = app.post_view.as_ref().unwrap().generation;
        app.on_msg(thread_message(
            generation,
            vec![comment(
                "parent",
                vec![comment("reply", vec![comment("child", vec![], None)], None)],
                None,
            )],
        ));
        let mut updated = comment("reply", vec![], Some("/more"));
        updated.body = "edited reply".into();

        app.on_msg(Msg::MoreReplies {
            generation,
            comment_id: "parent".into(),
            result: Ok(ReplyPage::complete(vec![updated])),
        });

        let reply = &app
            .post_view
            .as_ref()
            .unwrap()
            .page
            .as_ref()
            .unwrap()
            .comments[0]
            .replies[0];
        assert_eq!(reply.body, "edited reply");
        assert_eq!(reply.replies[0].id, "child");
    }

    #[test]
    fn a_refreshed_spoiler_post_can_be_revealed_without_a_stale_selection() {
        let mut app = app_with(&[]);
        app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
        let generation = app.post_view.as_ref().unwrap().generation;
        let mut page = PostPage {
            post: post("p", "s"),
            body: "secret body".into(),
            comments: vec![comment("secret reply", vec![], None)],
        };
        app.on_msg(Msg::Post {
            generation,
            result: Ok(Box::new(page.clone())),
        });
        app.post_view.as_mut().unwrap().free_scroll = false;
        page.post.spoiler = true;
        app.on_msg(Msg::Post {
            generation,
            result: Ok(Box::new(page)),
        });
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(110, 30)).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();

        app.toggle_spoilers();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();

        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("secret reply"), "{text}");
        assert!(app.post_view.as_ref().unwrap().free_scroll);
    }

    #[test]
    fn a_leaf_gaining_replies_keeps_the_new_continuation_on_refresh() {
        let mut app = app_with(&[]);
        app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
        let generation = app.post_view.as_ref().unwrap().generation;
        app.on_msg(thread_message(
            generation,
            vec![comment("leaf", vec![], None)],
        ));

        app.on_msg(thread_message(
            generation,
            vec![comment("leaf", vec![], Some("/new-replies"))],
        ));

        assert!(matches!(app.post_view.as_ref().unwrap().rows()[1].kind,
            RowKind::More { more, .. } if more.cursor == "/new-replies"));
    }

    #[test]
    fn revalidating_a_cached_next_page_replaces_that_page() {
        let mut app = app_with(&["vim"]);
        let i = tab_index(&app, "vim");
        app.switch_tab(i);
        let tab = app.tabs[i].id;
        app.on_msg(Msg::Feed {
            tab,
            generation: app.tabs[i].feed.generation,
            append: false,
            result: Ok(listing("vim", &["a", "b"], Some("t3_b"))),
        });
        app.load_feed(i, true);
        let generation = app.tabs[i].feed.generation;
        app.on_msg(Msg::Snapshot {
            message: Box::new(Msg::Feed {
                tab,
                generation,
                append: true,
                result: Ok(listing("vim", &["c", "d"], Some("t3_d"))),
            }),
            fetched_at: 1,
            refreshing: true,
            source: Source {
                cached: true,
                ..Source::default()
            },
        });
        app.tabs[i].feed.selected = 2;

        app.on_msg(Msg::Feed {
            tab,
            generation,
            append: true,
            result: Ok(listing("vim", &["x", "c"], Some("t3_c"))),
        });

        assert_eq!(
            app.tabs[i]
                .feed
                .posts
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b", "x", "c"]
        );
        assert_eq!(app.current_post().unwrap().id, "c");
        assert_eq!(app.tabs[i].feed.after.as_deref(), Some("t3_c"));
    }

    #[test]
    fn stale_first_page_refresh_does_not_block_loading_the_next_page() {
        let mut app = app_with(&["vim"]);
        let i = tab_index(&app, "vim");
        app.switch_tab(i);
        let tab = app.tabs[i].id;
        let first = app.tabs[i].feed.generation;
        app.on_msg(Msg::Snapshot {
            message: Box::new(Msg::Feed {
                tab,
                generation: first,
                append: false,
                result: Ok(listing("vim", &["a", "b"], Some("t3_b"))),
            }),
            fetched_at: 1,
            refreshing: true,
            source: Source {
                cached: true,
                ..Source::default()
            },
        });

        app.load_feed(i, true);
        let next = app.tabs[i].feed.generation;
        app.on_msg(Msg::Feed {
            tab,
            generation: next,
            append: true,
            result: Ok(listing("vim", &["c"], None)),
        });
        app.on_msg(Msg::Feed {
            tab,
            generation: first,
            append: false,
            result: Ok(listing("vim", &["new", "a"], Some("t3_a"))),
        });

        assert_ne!(next, first);
        assert_eq!(
            app.tabs[i]
                .feed
                .posts
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>(),
            ["new", "a", "c"]
        );
    }

    #[test]
    fn cache_refresh_keeps_updated_posts_that_move_across_pages() {
        let mut feed = Feed::default();
        feed.replace_page(
            None,
            listing("vim", &["a", "b"], None).posts,
            Some("b".into()),
            ordered_source(1, false),
        );
        feed.replace_page(
            Some("b".into()),
            listing("vim", &["c", "d"], None).posts,
            None,
            ordered_source(2, false),
        );
        let mut updated = post("c", "vim");
        updated.title = "updated title".into();

        feed.replace_page(
            None,
            vec![updated, post("a", "vim")],
            Some("a".into()),
            ordered_source(3, false),
        );

        assert_eq!(
            feed.posts.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            ["c", "a", "d"]
        );
        assert_eq!(feed.posts[0].title, "updated title");
        // The moved item no longer belongs to the second page's replacement range.
        feed.replace_page(
            Some("b".into()),
            listing("vim", &["x", "d"], None).posts,
            None,
            ordered_source(4, false),
        );
        assert_eq!(
            feed.posts.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            ["c", "a", "x", "d"]
        );
    }

    #[test]
    fn cached_paging_cannot_undo_a_newer_live_refresh() {
        let mut app = app_with(&["vim"]);
        let i = tab_index(&app, "vim");
        app.switch_tab(i);
        let tab = app.tabs[i].id;
        let mut fresh = listing("vim", &["b", "x"], Some("x"));
        fresh.posts[0].title = "updated title".into();
        app.on_msg(Msg::Snapshot {
            message: Box::new(Msg::Feed {
                tab,
                generation: app.tabs[i].feed.generation,
                append: false,
                result: Ok(fresh),
            }),
            fetched_at: now(),
            refreshing: false,
            source: Source {
                cached: false,
                ..Source::default()
            },
        });
        app.load_feed(i, true);
        let generation = app.tabs[i].feed.generation;

        app.on_msg(Msg::Snapshot {
            message: Box::new(Msg::Feed {
                tab,
                generation,
                append: true,
                result: Ok(listing("vim", &["b", "c"], None)),
            }),
            fetched_at: now() - 1,
            refreshing: false,
            source: Source {
                cached: true,
                ..Source::default()
            },
        });

        assert_eq!(
            app.tabs[i]
                .feed
                .posts
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>(),
            ["b", "x", "c"]
        );
        assert_eq!(app.tabs[i].feed.posts[0].title, "updated title");
        let mut newer = listing("vim", &["b", "c"], None);
        newer.posts[0].title = "newest title".into();
        app.on_msg(Msg::Snapshot {
            message: Box::new(Msg::Feed {
                tab,
                generation,
                append: true,
                result: Ok(newer),
            }),
            fetched_at: now() + 1,
            refreshing: false,
            source: Source {
                cached: false,
                ..Source::default()
            },
        });
        assert_eq!(
            app.tabs[i]
                .feed
                .posts
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>(),
            ["x", "b", "c"]
        );
        assert_eq!(app.tabs[i].feed.posts[1].title, "newest title");
    }

    #[test]
    fn clock_rollback_does_not_suppress_a_live_page_update() {
        let mut app = app_with(&["vim"]);
        let i = tab_index(&app, "vim");
        app.switch_tab(i);
        let tab = app.tabs[i].id;
        app.on_msg(Msg::Snapshot {
            message: Box::new(Msg::Feed {
                tab,
                generation: app.tabs[i].feed.generation,
                append: false,
                result: Ok(listing("vim", &["a", "b"], Some("b"))),
            }),
            fetched_at: 1000,
            refreshing: false,
            source: Source {
                cached: false,
                ..Source::default()
            },
        });
        app.load_feed(i, true);
        app.on_msg(Msg::Snapshot {
            message: Box::new(Msg::Feed {
                tab,
                generation: app.tabs[i].feed.generation,
                append: true,
                result: Ok(listing("vim", &["c", "d"], None)),
            }),
            fetched_at: 1000,
            refreshing: false,
            source: Source {
                cached: false,
                ..Source::default()
            },
        });
        app.reload();
        let mut updated = listing("vim", &["c", "a"], Some("a"));
        updated.posts[0].title = "updated after clock correction".into();

        app.on_msg(Msg::Snapshot {
            message: Box::new(Msg::Feed {
                tab,
                generation: app.tabs[i].feed.generation,
                append: false,
                result: Ok(updated),
            }),
            fetched_at: 900,
            refreshing: false,
            source: Source {
                cached: false,
                ..Source::default()
            },
        });

        assert_eq!(
            app.tabs[i].feed.posts[0].title,
            "updated after clock correction"
        );
        assert_eq!(
            app.tabs[i]
                .feed
                .posts
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>(),
            ["c", "a", "d"]
        );
    }

    fn ordered_source(order: u64, cached: bool) -> Source {
        Source {
            order: Some(crate::cache::RequestOrder {
                generation: [1; 16],
                sequence: order,
            }),
            observed: Some((order, order)),
            cached,
        }
    }

    fn ordered_snapshot(message: Msg, order: u64, cached: bool, refreshing: bool) -> Msg {
        Msg::Snapshot {
            message: Box::new(message),
            fetched_at: 1000,
            source: ordered_source(order, cached),
            refreshing,
        }
    }

    #[test]
    fn newer_cached_page_updates_an_overlapping_post_after_restart() {
        let mut app = app_with(&["vim"]);
        let i = tab_index(&app, "vim");
        app.switch_tab(i);
        let tab = app.tabs[i].id;
        app.on_msg(ordered_snapshot(
            Msg::Feed {
                tab,
                generation: app.tabs[i].feed.generation,
                append: false,
                result: Ok(listing("vim", &["a", "b"], Some("b"))),
            },
            1,
            true,
            false,
        ));
        app.load_feed(i, true);
        let mut later = listing("vim", &["a", "c"], None);
        later.posts[0].title = "newer cached title".into();

        app.on_msg(ordered_snapshot(
            Msg::Feed {
                tab,
                generation: app.tabs[i].feed.generation,
                append: true,
                result: Ok(later),
            },
            2,
            true,
            false,
        ));

        assert_eq!(app.current_post().unwrap().title, "newer cached title");
        assert_eq!(
            app.tabs[i]
                .feed
                .posts
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>(),
            ["b", "a", "c"]
        );
    }

    #[test]
    fn older_cached_replies_add_missing_children_without_undoing_edits() {
        let mut app = app_with(&[]);
        app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
        let generation = app.post_view.as_ref().unwrap().generation;
        let mut edited = comment("child", vec![], None);
        edited.body = "edited live reply".into();
        app.on_msg(ordered_snapshot(
            Msg::Post {
                generation,
                result: Ok(Box::new(PostPage {
                    post: post("p", "s"),
                    body: String::new(),
                    comments: vec![comment("parent", vec![edited], Some("/more"))],
                })),
            },
            2,
            false,
            false,
        ));

        app.on_msg(ordered_snapshot(
            Msg::MoreReplies {
                generation,
                comment_id: "parent".into(),
                result: Ok(ReplyPage::complete(vec![
                    comment("child", vec![comment("removed", vec![], None)], None),
                    comment("missing", vec![], None),
                ])),
            },
            1,
            true,
            false,
        ));

        let parent = &app
            .post_view
            .as_ref()
            .unwrap()
            .page
            .as_ref()
            .unwrap()
            .comments[0];
        assert_eq!(parent.replies[0].body, "edited live reply");
        assert!(
            parent.replies[0].replies.is_empty(),
            "an older branch must not restore removed descendants"
        );
        assert_eq!(parent.replies[1].id, "missing");
        assert!(parent.more.is_some());
    }

    #[test]
    fn a_home_refresh_supersedes_an_older_feed_revalidation() {
        let mut app = app_with(&["vim"]);
        let i = tab_index(&app, "vim");
        app.switch_tab(i);
        let tab = app.tabs[i].id;
        let first = app.tabs[i].feed.generation;
        app.on_msg(ordered_snapshot(
            Msg::Feed {
                tab,
                generation: first,
                append: false,
                result: Ok(listing("vim", &["cached"], None)),
            },
            1,
            true,
            true,
        ));
        app.switch_tab(ALL_SUBS_TAB);
        app.reload();
        app.on_msg(ordered_snapshot(
            Msg::Group {
                subreddit: "vim".into(),
                generation: 1,
                result: Ok(listing("vim", &["new"], None)),
            },
            3,
            false,
            false,
        ));

        app.on_msg(ordered_snapshot(
            Msg::Feed {
                tab,
                generation: first,
                append: false,
                result: Ok(listing("vim", &["old"], None)),
            },
            2,
            false,
            false,
        ));

        assert_eq!(app.tabs[i].feed.posts[0].id, "new");
    }

    #[test]
    fn a_first_page_refresh_does_not_retry_a_failed_next_page() {
        let mut app = app_with(&["vim"]);
        let i = tab_index(&app, "vim");
        app.switch_tab(i);
        let tab = app.tabs[i].id;
        let first = app.tabs[i].feed.generation;
        app.on_msg(ordered_snapshot(
            Msg::Feed {
                tab,
                generation: first,
                append: false,
                result: Ok(listing("vim", &["a"], Some("a"))),
            },
            1,
            true,
            true,
        ));
        app.load_feed(i, true);
        app.on_msg(Msg::Feed {
            tab,
            generation: app.tabs[i].feed.generation,
            append: true,
            result: Err("offline".into()),
        });
        app.on_msg(ordered_snapshot(
            Msg::Feed {
                tab,
                generation: first,
                append: false,
                result: Ok(listing("vim", &["a"], Some("a"))),
            },
            2,
            false,
            false,
        ));

        app.move_selection(1);

        assert!(
            !app.tabs[i].feed.loading(),
            "navigation must not retry the failed cursor"
        );
        app.load_feed(i, true);
        assert!(app.tabs[i].feed.loading(), "L still explicitly retries");
    }

    fn thread_message(generation: u64, comments: Vec<Comment>) -> Msg {
        Msg::Post {
            generation,
            result: Ok(Box::new(PostPage {
                post: post("p", "s"),
                body: String::new(),
                comments,
            })),
        }
    }

    #[test]
    fn reload_recovers_a_failed_revalidation_of_a_loaded_page() {
        let mut app = app_with(&["vim"]);
        let i = tab_index(&app, "vim");
        app.switch_tab(i);
        let tab = app.tabs[i].id;
        app.on_msg(ordered_snapshot(
            Msg::Feed {
                tab,
                generation: app.tabs[i].feed.generation,
                append: false,
                result: Ok(listing("vim", &["a"], Some("a"))),
            },
            1,
            false,
            false,
        ));
        app.load_feed(i, true);
        let page_generation = app.tabs[i].feed.generation;
        app.on_msg(ordered_snapshot(
            Msg::Feed {
                tab,
                generation: page_generation,
                append: true,
                result: Ok(listing("vim", &["b"], Some("b"))),
            },
            2,
            true,
            true,
        ));
        app.on_msg(Msg::Feed {
            tab,
            generation: page_generation,
            append: true,
            result: Err("offline".into()),
        });

        app.reload();
        let requests: Vec<_> = app.tabs[i]
            .feed
            .requests
            .iter()
            .filter(|(id, _)| app.tabs[i].feed.pending.contains(id))
            .map(|(id, cursor)| (*id, cursor.clone()))
            .collect();
        for (generation, cursor) in requests {
            let response = if cursor.is_none() {
                listing("vim", &["a"], Some("a"))
            } else {
                let mut response = listing("vim", &["b"], Some("b"));
                response.posts[0].title = "recovered page".into();
                response
            };
            app.on_msg(ordered_snapshot(
                Msg::Feed {
                    tab,
                    generation,
                    append: cursor.is_some(),
                    result: Ok(response),
                },
                generation + 10,
                false,
                false,
            ));
        }

        let feed = &app.tabs[i].feed;
        assert_eq!(feed.error(), None);
        assert_eq!(
            feed.posts
                .iter()
                .map(|post| post.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(feed.posts[1].title, "recovered page");
    }

    #[test]
    fn cached_replies_revalidate_when_the_parent_changes_after_submission() {
        let mut app = app_with(&[]);
        app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
        let generation = app.post_view.as_ref().unwrap().generation;
        app.on_msg(ordered_snapshot(
            thread_message(generation, vec![comment("parent", vec![], Some("/more"))]),
            10,
            true,
            true,
        ));
        let view = app.post_view.as_mut().unwrap();
        view.cursor = 1;
        view.free_scroll = false;
        app.activate_comment();
        app.on_msg(ordered_snapshot(
            thread_message(
                generation,
                vec![comment(
                    "parent",
                    vec![comment("edited", vec![], None)],
                    Some("/more"),
                )],
            ),
            21,
            false,
            false,
        ));

        app.on_msg(ordered_snapshot(
            Msg::MoreReplies {
                generation,
                comment_id: "parent".into(),
                result: Ok(ReplyPage::complete(vec![comment("missing", vec![], None)])),
            },
            20,
            true,
            false,
        ));

        let view = app.post_view.as_ref().unwrap();
        assert!(
            view.loading_more.contains("parent"),
            "the newly stale expansion must keep refreshing"
        );
        assert_eq!(
            view.page.as_ref().unwrap().comments[0]
                .replies
                .iter()
                .map(|reply| reply.id.as_str())
                .collect::<Vec<_>>(),
            ["edited", "missing"]
        );
    }

    #[test]
    fn cached_reply_expansion_keeps_selection_on_new_replies_through_revalidation() {
        let mut app = app_with(&[]);
        app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
        let generation = app.post_view.as_ref().unwrap().generation;
        app.on_msg(ordered_snapshot(
            thread_message(
                generation,
                vec![
                    comment(
                        "parent",
                        vec![comment("child", vec![], None)],
                        Some("/more"),
                    ),
                    comment("sibling", vec![], None),
                ],
            ),
            2,
            true,
            false,
        ));
        let view = app.post_view.as_mut().unwrap();
        view.cursor = 2;
        view.free_scroll = false;
        app.activate_comment();
        app.on_msg(ordered_snapshot(
            Msg::MoreReplies {
                generation,
                comment_id: "parent".into(),
                result: Ok(ReplyPage::complete(vec![
                    comment("child", vec![], None),
                    comment("missing", vec![], None),
                ])),
            },
            1,
            true,
            true,
        ));
        app.on_msg(ordered_snapshot(
            Msg::MoreReplies {
                generation,
                comment_id: "parent".into(),
                result: Ok(ReplyPage::complete(vec![
                    comment("child", vec![], None),
                    comment("missing", vec![], None),
                ])),
            },
            3,
            false,
            false,
        ));

        assert!(matches!(app.post_view.as_ref().unwrap().cursor_key(),
            Some(RowKey::Comment(id)) if id == "missing"));
    }

    #[test]
    fn live_expansion_reorders_cached_replies_without_losing_selection() {
        for nested in [false, true] {
            let mut app = app_with(&[]);
            app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
            let generation = app.post_view.as_ref().unwrap().generation;
            app.on_msg(ordered_snapshot(
                thread_message(
                    generation,
                    vec![comment(
                        "parent",
                        vec![comment("confirmed", vec![], None)],
                        Some("/more"),
                    )],
                ),
                2,
                true,
                false,
            ));
            let more = |ids: &[&str]| Msg::MoreReplies {
                generation,
                comment_id: "parent".into(),
                result: Ok(ReplyPage::complete(
                    ids.iter()
                        .map(|id| comment(id, vec![], (*id == "a").then_some("/nested")))
                        .collect(),
                )),
            };
            app.on_msg(ordered_snapshot(more(&["a", "b"]), 1, true, true));
            let view = app.post_view.as_mut().unwrap();
            view.cursor = view.row_index(&RowKey::Comment("b".into())).unwrap();
            view.free_scroll = false;
            if nested {
                app.on_msg(ordered_snapshot(
                    Msg::MoreReplies {
                        generation,
                        comment_id: "a".into(),
                        result: Ok(ReplyPage::complete(vec![comment("nested", vec![], None)])),
                    },
                    4,
                    false,
                    false,
                ));
            }
            app.on_msg(ordered_snapshot(more(&["b", "c", "a"]), 3, false, false));
            let view = app.post_view.as_ref().unwrap();
            assert_eq!(
                view.page.as_ref().unwrap().comments[0]
                    .replies
                    .iter()
                    .map(|c| c.id.as_str())
                    .collect::<Vec<_>>(),
                ["confirmed", "b", "c", "a"]
            );
            assert!(matches!(view.cursor_key(), Some(RowKey::Comment(id)) if id == "b"));
            // An older response arriving afterward cannot roll the refreshed order back.
            app.on_msg(ordered_snapshot(more(&["a", "b"]), 1, true, false));
            let view = app.post_view.as_ref().unwrap();
            assert_eq!(
                view.page.as_ref().unwrap().comments[0]
                    .replies
                    .iter()
                    .map(|c| c.id.as_str())
                    .collect::<Vec<_>>(),
                ["confirmed", "b", "c", "a"]
            );
            assert!(matches!(view.cursor_key(), Some(RowKey::Comment(id)) if id == "b"));
            if nested {
                let a = view.page.as_ref().unwrap().comments[0]
                    .replies
                    .iter()
                    .find(|c| c.id == "a")
                    .unwrap();
                assert_eq!(a.replies[0].id, "nested");
                assert!(a.more.is_none());
            }
        }
    }

    #[test]
    fn a_newly_complete_parent_rejects_an_older_cached_expansion() {
        let mut app = app_with(&[]);
        app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
        let generation = app.post_view.as_ref().unwrap().generation;
        app.on_msg(ordered_snapshot(
            thread_message(generation, vec![comment("parent", vec![], Some("/more"))]),
            10,
            true,
            true,
        ));
        let view = app.post_view.as_mut().unwrap();
        view.cursor = 1;
        view.free_scroll = false;
        app.activate_comment();
        app.on_msg(ordered_snapshot(
            thread_message(
                generation,
                vec![comment(
                    "parent",
                    vec![comment("current", vec![], Some("/child-more"))],
                    None,
                )],
            ),
            21,
            false,
            false,
        ));

        app.on_msg(ordered_snapshot(
            Msg::MoreReplies {
                generation,
                comment_id: "parent".into(),
                result: Ok(ReplyPage::complete(vec![
                    comment("removed", vec![], None),
                    comment("current", vec![comment("recovered", vec![], None)], None),
                ])),
            },
            20,
            true,
            false,
        ));

        let view = app.post_view.as_ref().unwrap();
        assert_eq!(
            view.page.as_ref().unwrap().comments[0]
                .replies
                .iter()
                .map(|reply| reply.id.as_str())
                .collect::<Vec<_>>(),
            ["current"]
        );
        assert_eq!(
            view.page.as_ref().unwrap().comments[0].replies[0]
                .replies
                .iter()
                .map(|reply| reply.id.as_str())
                .collect::<Vec<_>>(),
            ["recovered"],
        );
        assert!(!view.loading_more.contains("parent"));
    }

    #[test]
    fn home_seed_preserves_a_newer_pending_request_without_discarding_newer_data() {
        let mut app = app_with(&["vim"]);
        let i = tab_index(&app, "vim");
        app.switch_tab(i);
        let tab = app.tabs[i].id;
        app.on_msg(ordered_snapshot(
            Msg::Feed {
                tab,
                generation: app.tabs[i].feed.generation,
                append: false,
                result: Ok(listing("vim", &["old"], None)),
            },
            1,
            false,
            false,
        ));
        app.load_feed(i, false);
        let pending = app.tabs[i].feed.generation;
        app.on_msg(Msg::Snapshot {
            message: Box::new(Msg::Group {
                subreddit: "vim".into(),
                generation: 0,
                result: Ok(listing("vim", &["home-newer"], Some("A"))),
            }),
            fetched_at: now(),
            refreshing: false,
            source: ordered_source(2, false),
        });
        assert_eq!(app.tabs[i].feed.posts[0].id, "home-newer");
        let before = app.tabs[i].feed.generation;
        app.load_feed(i, true);
        let next = app.tabs[i].feed.generation;
        assert_ne!(
            next, before,
            "Home supplies the first page while HTTP remains pending"
        );
        app.on_msg(ordered_snapshot(
            Msg::Feed {
                tab,
                generation: next,
                append: true,
                result: Ok(listing("vim", &["tail"], None)),
            },
            2,
            true,
            false,
        ));
        assert!(app.tabs[i].feed.posts.iter().any(|post| post.id == "tail"));

        app.on_msg(ordered_snapshot(
            Msg::Feed {
                tab,
                generation: pending,
                append: false,
                result: Ok(listing("vim", &["old"], None)),
            },
            1,
            true,
            true,
        ));
        assert_eq!(app.tabs[i].feed.posts[0].id, "home-newer");
        app.on_msg(ordered_snapshot(
            Msg::Feed {
                tab,
                generation: pending,
                append: false,
                result: Ok(listing("vim", &["feed-newest"], None)),
            },
            3,
            false,
            false,
        ));
        assert_eq!(app.tabs[i].feed.posts[0].id, "feed-newest");
    }

    #[test]
    fn home_seed_compares_only_the_query_currently_being_requested() {
        let mut app = app_with(&["vim"]);
        let i = tab_index(&app, "vim");
        app.switch_tab(i);
        app.set_sort(Sort::New);
        let tab = app.tabs[i].id;
        app.on_msg(ordered_snapshot(
            Msg::Feed {
                tab,
                generation: app.tabs[i].feed.generation,
                append: false,
                result: Ok(listing("vim", &["new-sort"], None)),
            },
            9,
            false,
            false,
        ));
        app.set_sort(Sort::Hot);
        let pending = app.tabs[i].feed.generation;

        app.on_msg(Msg::Snapshot {
            message: Box::new(Msg::Group {
                subreddit: "vim".into(),
                generation: 0,
                result: Ok(listing("vim", &["home-hot"], None)),
            }),
            fetched_at: now(),
            refreshing: false,
            source: ordered_source(4, false),
        });
        assert_eq!(app.tabs[i].feed.posts[0].id, "home-hot");
        app.on_msg(ordered_snapshot(
            Msg::Feed {
                tab,
                generation: pending,
                append: false,
                result: Ok(listing("vim", &["old-hot"], None)),
            },
            3,
            true,
            false,
        ));
        assert_eq!(app.tabs[i].feed.posts[0].id, "home-hot");
    }

    #[test]
    fn older_expansion_recovers_incomplete_descendants_beneath_complete_children() {
        let mut app = app_with(&[]);
        app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
        let generation = app.post_view.as_ref().unwrap().generation;
        app.on_msg(ordered_snapshot(
            thread_message(
                generation,
                vec![comment(
                    "parent",
                    vec![comment(
                        "complete",
                        vec![comment("incomplete", vec![], Some("/nested-more"))],
                        None,
                    )],
                    Some("/more"),
                )],
            ),
            2,
            true,
            false,
        ));
        let view = app.post_view.as_mut().unwrap();
        view.cursor = 4;
        view.free_scroll = false;
        app.activate_comment();
        app.on_msg(ordered_snapshot(
            Msg::MoreReplies {
                generation,
                comment_id: "parent".into(),
                result: Ok(ReplyPage::complete(vec![comment(
                    "complete",
                    vec![
                        comment("removed", vec![], None),
                        comment("incomplete", vec![comment("recovered", vec![], None)], None),
                    ],
                    None,
                )])),
            },
            1,
            true,
            true,
        ));
        let view = app.post_view.as_ref().unwrap();
        let complete = &view.page.as_ref().unwrap().comments[0].replies[0];
        assert_eq!(
            complete
                .replies
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            ["incomplete"]
        );
        assert_eq!(
            complete.replies[0]
                .replies
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            ["recovered"]
        );
        assert!(matches!(view.cursor_key(), Some(RowKey::Comment(id)) if id == "recovered"));
    }

    #[test]
    fn completing_reply_revalidation_removes_disappeared_direct_replies() {
        for keep_b in [true, false] {
            let mut app = app_with(&[]);
            app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
            let generation = app.post_view.as_ref().unwrap().generation;
            app.on_msg(ordered_snapshot(
                thread_message(generation, vec![comment("parent", vec![], Some("/more"))]),
                2,
                true,
                false,
            ));
            let view = app.post_view.as_mut().unwrap();
            view.cursor = 1;
            view.free_scroll = false;
            app.activate_comment();
            app.on_msg(ordered_snapshot(
                Msg::MoreReplies {
                    generation,
                    comment_id: "parent".into(),
                    result: Ok(ReplyPage::complete(vec![
                        comment("a", vec![], None),
                        comment("b", vec![comment("expanded", vec![], None)], None),
                    ])),
                },
                1,
                true,
                true,
            ));
            let replies = if keep_b {
                vec![comment("b", vec![], Some("/b-more"))]
            } else {
                vec![]
            };
            app.on_msg(ordered_snapshot(
                Msg::MoreReplies {
                    generation,
                    comment_id: "parent".into(),
                    result: Ok(ReplyPage::complete(replies)),
                },
                3,
                false,
                false,
            ));
            let parent = &app
                .post_view
                .as_ref()
                .unwrap()
                .page
                .as_ref()
                .unwrap()
                .comments[0];
            assert_eq!(
                parent
                    .replies
                    .iter()
                    .map(|c| c.id.as_str())
                    .collect::<Vec<_>>(),
                if keep_b { vec!["b"] } else { vec![] }
            );
            assert!(parent.more.is_none());
            if keep_b {
                assert_eq!(parent.replies[0].replies[0].id, "expanded");
            }
        }
    }

    #[test]
    fn reply_revalidation_keeps_children_confirmed_by_a_parent_refresh() {
        let mut app = app_with(&[]);
        app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
        let generation = app.post_view.as_ref().unwrap().generation;
        app.on_msg(ordered_snapshot(
            thread_message(generation, vec![comment("parent", vec![], Some("/more"))]),
            1,
            true,
            false,
        ));
        app.on_msg(ordered_snapshot(
            Msg::MoreReplies {
                generation,
                comment_id: "parent".into(),
                result: Ok(ReplyPage::complete(vec![
                    comment("confirmed", vec![], None),
                    comment("removed", vec![], None),
                ])),
            },
            2,
            true,
            false,
        ));
        app.on_msg(ordered_snapshot(
            thread_message(
                generation,
                vec![comment(
                    "parent",
                    vec![comment("confirmed", vec![], None)],
                    Some("/more"),
                )],
            ),
            3,
            false,
            false,
        ));
        app.on_msg(ordered_snapshot(
            Msg::MoreReplies {
                generation,
                comment_id: "parent".into(),
                result: Ok(ReplyPage::complete(vec![
                    comment("confirmed", vec![], None),
                    comment("removed", vec![], None),
                ])),
            },
            2,
            true,
            true,
        ));
        app.on_msg(ordered_snapshot(
            Msg::MoreReplies {
                generation,
                comment_id: "parent".into(),
                result: Ok(ReplyPage::complete(vec![comment("new", vec![], None)])),
            },
            4,
            false,
            false,
        ));
        let parent = &app
            .post_view
            .as_ref()
            .unwrap()
            .page
            .as_ref()
            .unwrap()
            .comments[0];
        assert_eq!(
            parent
                .replies
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            ["confirmed", "new"]
        );
    }

    #[test]
    fn an_older_parent_response_confirms_children_without_undoing_newer_content() {
        for already_expanded in [false, true] {
            let mut app = app_with(&[]);
            app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
            let generation = app.post_view.as_ref().unwrap().generation;
            app.on_msg(ordered_snapshot(
                thread_message(
                    generation,
                    vec![comment(
                        "ancestor",
                        vec![comment("parent", vec![], Some("/parent-more"))],
                        Some("/ancestor-more"),
                    )],
                ),
                1,
                true,
                false,
            ));
            app.reload();
            let generation = app.post_view.as_ref().unwrap().generation;
            app.on_msg(ordered_snapshot(
                Msg::MoreReplies {
                    generation,
                    comment_id: "ancestor".into(),
                    result: Ok(ReplyPage::complete(vec![comment(
                        "parent",
                        vec![],
                        Some("/parent-more"),
                    )])),
                },
                3,
                false,
                false,
            ));
            if already_expanded {
                let mut child = comment("confirmed", vec![], None);
                child.body = "newer expansion text".into();
                app.on_msg(ordered_snapshot(
                    Msg::MoreReplies {
                        generation,
                        comment_id: "parent".into(),
                        result: Ok(ReplyPage::complete(vec![child])),
                    },
                    4,
                    true,
                    true,
                ));
            }
            let mut child = comment("confirmed", vec![], None);
            child.body = "older parent text".into();
            app.on_msg(ordered_snapshot(
                thread_message(
                    generation,
                    vec![comment(
                        "ancestor",
                        vec![comment("parent", vec![child], Some("/parent-more"))],
                        Some("/ancestor-more"),
                    )],
                ),
                2,
                false,
                false,
            ));
            app.on_msg(ordered_snapshot(
                Msg::MoreReplies {
                    generation,
                    comment_id: "parent".into(),
                    result: Ok(ReplyPage::complete(vec![comment("new", vec![], None)])),
                },
                5,
                false,
                false,
            ));
            let parent = &app
                .post_view
                .as_ref()
                .unwrap()
                .page
                .as_ref()
                .unwrap()
                .comments[0]
                .replies[0];
            assert_eq!(
                parent
                    .replies
                    .iter()
                    .map(|c| c.id.as_str())
                    .collect::<Vec<_>>(),
                ["confirmed", "new"]
            );
            assert_eq!(
                parent.replies[0].body,
                if already_expanded {
                    "newer expansion text"
                } else {
                    "older parent text"
                }
            );
        }
    }

    #[test]
    fn home_seeding_keeps_a_feed_request_that_started_later() {
        let mut app = app_with(&["vim"]);
        let i = tab_index(&app, "vim");
        app.switch_tab(i);
        let tab = app.tabs[i].id;
        app.load_feed(i, false);
        let feed_request = app.tabs[i].feed.generation;
        // Home's request was submitted later but started first (lower sequence).
        app.on_msg(Msg::Snapshot {
            message: Box::new(Msg::Group {
                subreddit: "vim".into(),
                generation: 0,
                result: Ok(listing("vim", &["home-earlier-start"], None)),
            }),
            fetched_at: now(),
            refreshing: false,
            source: ordered_source(2, false),
        });
        assert_eq!(app.tabs[i].feed.posts[0].id, "home-earlier-start");

        app.on_msg(Msg::Snapshot {
            message: Box::new(Msg::Feed {
                tab,
                generation: feed_request,
                append: false,
                result: Ok(listing("vim", &["feed-later-start"], None)),
            }),
            fetched_at: now(),
            refreshing: false,
            source: ordered_source(3, false),
        });

        assert_eq!(app.tabs[i].feed.posts[0].id, "feed-later-start");
    }

    #[test]
    fn orderless_home_completion_cannot_undo_a_later_disk_snapshot() {
        for (retired, overlap) in [(false, false), (true, false), (true, true)] {
            let mut app = app_with(&["vim"]);
            let i = tab_index(&app, "vim");
            app.switch_tab(i);
            let tab = app.tabs[i].id;
            app.load_feed(i, false);
            let generation = app.tabs[i].feed.generation;
            let mut disk = ordered_source(5, true);
            disk.observed = Some(if overlap { (0, 3) } else { (2, 3) });
            if retired {
                disk.order.as_mut().unwrap().generation = [2; 16];
            }
            app.on_msg(Msg::Snapshot {
                message: Box::new(Msg::Feed {
                    tab,
                    generation,
                    append: false,
                    result: Ok(listing("vim", &["newer-disk"], None)),
                }),
                fetched_at: now(),
                refreshing: false,
                source: disk,
            });
            app.on_msg(Msg::Snapshot {
                message: Box::new(Msg::Group {
                    subreddit: "vim".into(),
                    generation: 0,
                    result: Ok(listing("vim", &["older-orderless-live"], None)),
                }),
                fetched_at: now(),
                refreshing: false,
                source: Source {
                    observed: Some((1, 1)),
                    ..Source::default()
                },
            });
            assert_eq!(
                app.tabs[i].feed.posts[0].id, "newer-disk",
                "retired={retired}"
            );
            app.fetch_feed(i, None, true);
            app.on_msg(Msg::Snapshot {
                message: Box::new(Msg::Feed {
                    tab,
                    generation: app.tabs[i].feed.generation,
                    append: false,
                    result: Ok(listing("vim", &["later-unreserved-refresh"], None)),
                }),
                fetched_at: now(),
                refreshing: false,
                source: Source {
                    observed: Some((4, 4)),
                    ..Source::default()
                },
            });
            assert_eq!(app.tabs[i].feed.posts[0].id, "later-unreserved-refresh");
        }
    }

    #[test]
    fn a_kept_request_that_started_earlier_cannot_mark_a_newer_page_failed() {
        let mut app = app_with(&["vim"]);
        let i = tab_index(&app, "vim");
        app.switch_tab(i);
        let tab = app.tabs[i].id;
        app.load_feed(i, false);
        let feed_request = app.tabs[i].feed.generation;
        app.on_msg(Msg::Snapshot {
            message: Box::new(Msg::Group {
                subreddit: "vim".into(),
                generation: 0,
                result: Ok(listing("vim", &["home-later-start"], None)),
            }),
            fetched_at: now(),
            refreshing: false,
            source: ordered_source(3, false),
        });

        app.on_msg(Msg::Snapshot {
            message: Box::new(Msg::Feed {
                tab,
                generation: feed_request,
                append: false,
                result: Err("timeout".into()),
            }),
            fetched_at: now(),
            refreshing: false,
            source: ordered_source(2, false),
        });

        let feed = &app.tabs[i].feed;
        assert_eq!(feed.posts[0].id, "home-later-start");
        assert_eq!(feed.error(), None);
        assert!(app.status.as_ref().is_none_or(|s| !s.error));
    }

    #[test]
    fn home_reaches_a_hot_tab_that_kept_an_earlier_time_range() {
        let mut app = app_with(&["vim"]);
        let i = tab_index(&app, "vim");
        app.switch_tab(i);
        app.set_sort(Sort::Top);
        app.cycle_time();
        app.set_sort(Sort::Hot);

        app.on_msg(Msg::Snapshot {
            message: Box::new(Msg::Group {
                subreddit: "vim".into(),
                generation: 0,
                result: Ok(listing("vim", &["home"], None)),
            }),
            fetched_at: now(),
            refreshing: false,
            source: ordered_source(4, false),
        });

        assert_eq!(app.tabs[i].feed.posts[0].id, "home");
    }

    #[test]
    fn unorderable_feed_failures_remain_visible_after_home_seeding() {
        let variants = [
            (None, None),
            (ordered_source(3, false).order, None),
            (None, ordered_source(2, false).order),
            (
                ordered_source(3, false).order,
                Some(crate::cache::RequestOrder {
                    generation: [99; 16],
                    sequence: 2,
                }),
            ),
        ];
        for (home_order, failure_order) in variants {
            let mut app = app_with(&["vim"]);
            let i = tab_index(&app, "vim");
            app.switch_tab(i);
            app.load_feed(i, false);
            let generation = app.tabs[i].feed.generation;
            app.on_msg(Msg::Snapshot {
                message: Box::new(Msg::Group {
                    subreddit: "vim".into(),
                    generation: 0,
                    result: Ok(listing("vim", &["home"], None)),
                }),
                fetched_at: now(),
                refreshing: false,
                source: Source {
                    order: home_order,
                    observed: None,
                    cached: false,
                },
            });
            app.on_msg(Msg::Snapshot {
                message: Box::new(Msg::Feed {
                    tab: app.tabs[i].id,
                    generation,
                    append: false,
                    result: Err("later-started request failed".into()),
                }),
                fetched_at: now(),
                refreshing: false,
                source: Source {
                    order: failure_order,
                    observed: None,
                    cached: false,
                },
            });
            assert_eq!(
                app.tabs[i].feed.error(),
                Some("later-started request failed")
            );
            assert_eq!(app.tabs[i].feed.posts[0].id, "home");
            assert!(!app.tabs[i].feed.refreshing());
        }
    }

    #[test]
    fn later_started_live_feed_wins_when_cache_ordering_is_unavailable() {
        let mut app = app_with(&["vim"]);
        let i = tab_index(&app, "vim");
        app.switch_tab(i);
        app.load_feed(i, false);
        let generation = app.tabs[i].feed.generation;
        app.on_msg(Msg::Snapshot {
            message: Box::new(Msg::Group {
                subreddit: "vim".into(),
                generation: 0,
                result: Ok(listing("vim", &["home"], None)),
            }),
            fetched_at: now(),
            refreshing: false,
            source: Source {
                order: None,
                observed: Some((1, 1)),
                cached: false,
            },
        });
        app.on_msg(Msg::Snapshot {
            message: Box::new(Msg::Feed {
                tab: app.tabs[i].id,
                generation,
                append: false,
                result: Ok(listing("vim", &["later-started-feed"], None)),
            }),
            fetched_at: now(),
            refreshing: false,
            source: Source {
                order: None,
                observed: Some((2, 2)),
                cached: false,
            },
        });
        assert_eq!(app.tabs[i].feed.posts[0].id, "later-started-feed");
    }

    #[test]
    fn revisiting_a_stale_tab_does_not_duplicate_its_head_revalidation() {
        let mut app = app_with(&["vim"]);
        let i = tab_index(&app, "vim");
        app.switch_tab(i);
        let generation = app.tabs[i].feed.generation;
        app.on_msg(ordered_snapshot(
            Msg::Feed {
                tab: app.tabs[i].id,
                generation,
                append: false,
                result: Ok(listing("vim", &["cached"], Some("next"))),
            },
            1,
            true,
            true,
        ));
        for _ in 0..3 {
            app.switch_tab(0);
            app.switch_tab(i);
        }
        assert_eq!(app.tabs[i].feed.generation, generation);
        app.load_feed(i, true);
        assert_ne!(
            app.tabs[i].feed.generation, generation,
            "paging stays independent"
        );
    }

    #[test]
    fn an_older_home_success_keeps_a_newer_feed_failure_visible() {
        let mut app = app_with(&["vim"]);
        let i = tab_index(&app, "vim");
        app.switch_tab(i);
        app.on_msg(ordered_snapshot(
            Msg::Feed {
                tab: app.tabs[i].id,
                generation: app.tabs[i].feed.generation,
                append: false,
                result: Ok(listing("vim", &["initial"], None)),
            },
            1,
            false,
            false,
        ));
        app.reload();
        app.on_msg(ordered_snapshot(
            Msg::Feed {
                tab: app.tabs[i].id,
                generation: app.tabs[i].feed.generation,
                append: false,
                result: Err("newer request failed".into()),
            },
            3,
            false,
            false,
        ));
        app.on_msg(ordered_snapshot(
            Msg::Group {
                subreddit: "vim".into(),
                generation: 0,
                result: Ok(listing("vim", &["useful-older-success"], None)),
            },
            2,
            false,
            false,
        ));
        assert_eq!(app.tabs[i].feed.posts[0].id, "useful-older-success");
        assert_eq!(app.tabs[i].feed.error(), Some("newer request failed"));
    }
    #[test]
    fn an_interim_expansion_without_new_replies_keeps_its_target() {
        for (empty, navigate) in [(true, false), (false, false), (true, true)] {
            let mut app = app_with(&[]);
            app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
            let generation = app.post_view.as_ref().unwrap().generation;
            app.on_msg(ordered_snapshot(
                thread_message(
                    generation,
                    vec![
                        comment(
                            "parent",
                            vec![comment("child", vec![], None)],
                            Some("/more"),
                        ),
                        comment("sibling", vec![], None),
                    ],
                ),
                1,
                true,
                false,
            ));
            let view = app.post_view.as_mut().unwrap();
            view.cursor = 2;
            view.free_scroll = false;
            app.activate_comment();
            app.on_msg(ordered_snapshot(
                Msg::MoreReplies {
                    generation,
                    comment_id: "parent".into(),
                    result: Ok(ReplyPage::complete(if empty {
                        vec![]
                    } else {
                        vec![comment("child", vec![], None)]
                    })),
                },
                2,
                true,
                true,
            ));
            assert!(matches!(app.post_view.as_ref().unwrap().cursor_key(),
                Some(RowKey::More(id)) if id == "parent"));
            if navigate {
                app.move_comment_cursor(1);
            }
            app.on_msg(ordered_snapshot(
                Msg::MoreReplies {
                    generation,
                    comment_id: "parent".into(),
                    result: Ok(ReplyPage::complete(vec![
                        comment("child", vec![], None),
                        comment("new", vec![], None),
                    ])),
                },
                4,
                false,
                false,
            ));
            let view = app.post_view.as_ref().unwrap();
            assert!(matches!(view.cursor_key(), Some(RowKey::Comment(id))
                if id == if navigate { "sibling" } else { "new" }));
            assert!(!view.loading_more.contains("parent"));
        }
    }

    #[test]
    fn incomparable_live_fields_survive_a_later_cached_expansion() {
        for retired in [false, true] {
            let mut app = app_with(&[]);
            app.open_post_permalink(post("p", "s"), "/r/s/comments/p/x/".into());
            let generation = app.post_view.as_ref().unwrap().generation;
            let mut child = comment("child", vec![], None);
            child.body = "new live text".into();
            app.on_msg(Msg::Snapshot {
                message: Box::new(thread_message(
                    generation,
                    vec![comment("parent", vec![child], Some("/more"))],
                )),
                fetched_at: now(),
                refreshing: false,
                source: Source {
                    observed: Some((4, 4)),
                    order: if retired {
                        ordered_source(1, false).order
                    } else {
                        None
                    },
                    ..Source::default()
                },
            });
            let mut old = comment("child", vec![], Some("/obsolete-continuation"));
            old.body = "old cached text".into();
            app.on_msg(Msg::Snapshot {
                message: Box::new(Msg::MoreReplies {
                    generation,
                    comment_id: "parent".into(),
                    result: Ok(ReplyPage::complete(vec![old])),
                }),
                fetched_at: now(),
                refreshing: false,
                source: Source {
                    observed: Some((5, 6)),
                    order: Some(crate::cache::RequestOrder {
                        generation: [2; 16],
                        sequence: 2,
                    }),
                    ..ordered_source(2, true)
                },
            });
            let view = app.post_view.as_ref().unwrap();
            let child = &view.page.as_ref().unwrap().comments[0].replies[0];
            assert_eq!(child.body, "new live text");
            assert!(child.more.is_none());
            assert!(
                view.loading_more.contains("parent"),
                "uncertain cached replies must revalidate"
            );
            let mut new = comment("child", vec![], None);
            new.body = "revalidated text".into();
            app.on_msg(Msg::Snapshot {
                message: Box::new(Msg::MoreReplies {
                    generation,
                    comment_id: "parent".into(),
                    result: Ok(ReplyPage::complete(vec![new])),
                }),
                fetched_at: now(),
                refreshing: false,
                source: Source {
                    observed: Some((7, 7)),
                    order: Some(crate::cache::RequestOrder {
                        generation: [2; 16],
                        sequence: 3,
                    }),
                    ..ordered_source(3, false)
                },
            });
            assert_eq!(
                app.post_view
                    .as_ref()
                    .unwrap()
                    .page
                    .as_ref()
                    .unwrap()
                    .comments[0]
                    .replies[0]
                    .body,
                "revalidated text"
            );
        }
    }
    #[test]
    fn stale_thumbnail_refresh_survives_eviction_and_revisit_until_terminal_delivery() {
        for terminal in 0..3 {
            let mut app = app_with(&[]);
            let url = "/image.png";
            app.thumbs_drawn(&[url]);
            app.on_msg(Msg::Snapshot {
                message: Box::new(Msg::Thumb {
                    url: url.into(),
                    result: Ok(image::DynamicImage::ImageRgba8(
                        image::ImageBuffer::from_pixel(2, 2, image::Rgba([255, 0, 0, 255])),
                    )),
                }),
                fetched_at: now(),
                refreshing: true,
                source: Source {
                    cached: true,
                    ..Source::default()
                },
            });
            for i in 0..THUMB_CACHE_MAX {
                app.thumbs
                    .insert(format!("/old/{i}"), ImageSlot::Failed("unused".into()));
            }
            app.thumbs_drawn(&[]);
            assert!(
                matches!(app.thumbs.get(url), Some(ImageSlot::Ready { .. })),
                "the offscreen cached preview must survive while HTTP is outstanding"
            );
            app.thumbs_drawn(&[url]);
            assert!(
                matches!(app.thumbs.get(url), Some(ImageSlot::Ready { .. })),
                "revisiting must reuse the preview, not replace it with another Loading request"
            );
            app.on_msg(match terminal {
                0 => Msg::Thumb {
                    url: url.into(),
                    result: Ok(image::DynamicImage::new_rgb8(2, 2)),
                },
                1 => Msg::Thumb {
                    url: url.into(),
                    result: Err("offline".into()),
                },
                _ => Msg::ThumbSkipped { url: url.into() },
            });
            assert!(matches!(app.thumbs.get(url), Some(ImageSlot::Ready { .. })));
            for i in 0..THUMB_CACHE_MAX {
                app.thumbs
                    .insert(format!("/old/{i}"), ImageSlot::Failed("unused".into()));
            }
            app.thumbs_drawn(&[]);
            assert!(
                !app.thumbs.contains_key(url),
                "terminal delivery must release the eviction pin"
            );
        }
    }

    #[test]
    fn cached_thumbnail_retries_keep_pixels_after_failure_or_skipped_work() {
        for retry in ["reload", "revisit"] {
            let mut app = app_with(&[]);
            let url = "/image.png";
            app.thumbs_drawn(&[url]);
            app.on_msg(Msg::Snapshot {
                message: Box::new(Msg::Thumb {
                    url: url.into(),
                    result: Ok(image::DynamicImage::ImageRgba8(
                        image::ImageBuffer::from_pixel(2, 2, image::Rgba([255, 0, 0, 255])),
                    )),
                }),
                fetched_at: now(),
                refreshing: true,
                source: Source {
                    cached: true,
                    ..Source::default()
                },
            });
            let render = |app: &mut App| {
                let Some(ImageSlot::Ready { protocol, .. }) = app.thumbs.get_mut(url) else {
                    panic!("cached pixels must remain usable");
                };
                let area = ratatui::layout::Rect::new(0, 0, 2, 1);
                let mut buffer = ratatui::buffer::Buffer::empty(area);
                ratatui::widgets::StatefulWidget::render(
                    ratatui_image::StatefulImage::default(),
                    area,
                    &mut buffer,
                    protocol.as_mut(),
                );
                buffer
            };
            let cached_pixels = render(&mut app);
            app.on_msg(if retry == "revisit" {
                Msg::ThumbSkipped { url: url.into() }
            } else {
                Msg::Thumb {
                    url: url.into(),
                    result: Err("offline".into()),
                }
            });
            app.thumbs_drawn(&[]);
            if retry == "reload" {
                app.reload();
            }
            app.thumbs_drawn(&[url]);
            assert!(
                matches!(
                    app.thumbs.get(url),
                    Some(ImageSlot::Ready {
                        refresh: ImageRefresh::Loading,
                        ..
                    })
                ),
                "{retry} must restart the refresh without losing usable pixels"
            );
            assert_eq!(
                render(&mut app),
                cached_pixels,
                "{retry} must keep rendering the same cached image"
            );
        }
    }
}
