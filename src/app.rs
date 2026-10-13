//! Application state and the actions keys trigger. Rendering lives in `ui`,
//! key dispatch in `keys`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;

use crate::config::{Config, is_subreddit_name};
use crate::media;
use crate::model::{Comment, Listing, Media, MoreReplies, Post, PostPage, Sort, TimeRange};
use crate::redlib::Redlib;
use crate::state::State;
use crate::worker::{FeedQuery, Job, Msg, Pool};

const HEALTH_INTERVAL: Duration = Duration::from_secs(30);
const SAVE_INTERVAL: Duration = Duration::from_secs(60);
const STATUS_TTL: Duration = Duration::from_secs(6);
/// A tab's posts count as seen once you stay this long or interact with it.
const SEEN_DWELL: Duration = Duration::from_secs(2);
/// Load the next page when the selection gets this close to the end.
const PREFETCH_MARGIN: usize = 5;
/// Tabs 0 and 1 are always Home and Bookmarks.
const BOOKMARKS_TAB: usize = 1;

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
    /// True while you read by scrolling; the cursor then re-anchors to the
    /// first visible comment on the next comment key.
    pub free_scroll: bool,
    /// Document line where each row starts; kept in sync by the UI.
    pub row_starts: Vec<usize>,
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
    /// The comment tree flattened in display order, honoring folded threads.
    pub fn rows(&self) -> Vec<Row<'_>> {
        let mut rows = Vec::new();
        if let Some(page) = &self.page {
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

    /// After scrolling, pick the first comment that is on screen. Returns true
    /// when it moved the cursor this way.
    fn anchor_cursor(&mut self) -> bool {
        // Before the first draw of a loaded post there are no row positions yet:
        // keep the post in free scroll so it still opens at its top.
        if !self.free_scroll || self.row_starts.is_empty() {
            return false;
        }
        self.free_scroll = false;
        if let Some(i) = self
            .row_starts
            .iter()
            .position(|&start| start >= self.scroll)
        {
            self.cursor = i;
        } else {
            self.cursor = self.row_starts.len() - 1;
        }
        true
    }
}

fn descendants(c: &Comment) -> usize {
    c.replies.iter().map(|r| 1 + descendants(r)).sum()
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
    Help,
    Picker(PickerState),
    Sort { selected: usize },
    Search { input: String },
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

fn picker_query(input: &str) -> &str {
    input
        .trim()
        .trim_start_matches("/r/")
        .trim_start_matches("r/")
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
    pub image_picker: Picker,
    pub images: HashMap<String, ImageSlot>,
    pending_media: HashSet<String>,
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
            image_picker,
            images: HashMap::new(),
            pending_media: HashSet::new(),
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

    /// Timers: status expiry, health checks, periodic saves. Returns true when
    /// something visible changed.
    pub fn tick(&mut self) -> bool {
        let mut changed = false;
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
        let ids = |state: &State| {
            state
                .bookmarks()
                .iter()
                .map(|b| b.post.id.clone())
                .collect::<Vec<_>>()
        };
        let before = ids(&self.state);
        let selected = before.get(self.tabs[BOOKMARKS_TAB].feed.selected).cloned();
        let failed = match self.state.save(&self.state_path, now()) {
            Ok(_) => false,
            Err(e) => {
                self.error(format!("cannot save state: {e:#}"));
                true
            }
        };
        let after = ids(&self.state);
        let changed = after != before;
        if changed {
            let feed = &mut self.tabs[BOOKMARKS_TAB].feed;
            feed.selected = selected
                .and_then(|id| after.iter().position(|a| *a == id))
                .unwrap_or(feed.selected)
                .min(after.len().saturating_sub(1));
        }
        changed || failed
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
        self.post_view = None;
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
        let name = picker_query(name);
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
                self.tabs[self.active].feed.selected = 0;
                self.load_feed(self.active, false);
            }
        }
        self.check_health();
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
        if self.show_images
            && let Some(url) = post.media.preview_image()
        {
            let url = url.to_string();
            self.request_image(&url);
        }
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
            row_starts: Vec::new(),
        });
        self.pool.submit(Job::Post {
            generation,
            permalink,
        });
    }

    pub fn close_post(&mut self) {
        self.post_view = None;
    }

    pub fn move_comment_cursor(&mut self, delta: isize) {
        let Some(view) = &mut self.post_view else {
            return;
        };
        if view.free_scroll {
            view.anchor_cursor();
            return;
        }
        let len = view.rows().len();
        if delta < 0 && view.cursor == 0 {
            view.scroll = 0;
            view.free_scroll = true;
            return;
        }
        view.cursor = step(view.cursor, delta, len);
    }

    pub fn post_top(&mut self) {
        if let Some(view) = &mut self.post_view {
            view.cursor = 0;
            view.scroll = 0;
            view.free_scroll = true;
        }
    }

    pub fn post_bottom(&mut self) {
        if let Some(view) = &mut self.post_view {
            view.cursor = view.rows().len().saturating_sub(1);
            view.free_scroll = false;
        }
    }

    pub fn scroll_post(&mut self, delta: isize) {
        if let Some(view) = &mut self.post_view {
            view.scroll = view.scroll.saturating_add_signed(delta);
            view.free_scroll = true;
        }
    }

    pub fn jump_top_level(&mut self, forward: bool) {
        let Some(view) = &mut self.post_view else {
            return;
        };
        let was_free = view.free_scroll;
        if was_free && !view.anchor_cursor() {
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
                indexed
                    .rev()
                    .find(|(i, r)| *i < view.cursor && r.depth == 0)
                    .map(|(i, _)| i)
            }
        };
        if let Some(i) = target {
            view.cursor = i;
        }
    }

    /// Fold/unfold the comment under the cursor, or load "more replies".
    pub fn activate_comment(&mut self) {
        enum Action {
            Toggle(String),
            More { parent: String, path: String },
        }
        let Some(view) = &mut self.post_view else {
            return;
        };
        if view.free_scroll && !view.anchor_cursor() {
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
        self.show_images = !self.show_images;
        if self.show_images
            && let Some(view) = &self.post_view
        {
            let media = view
                .page
                .as_ref()
                .map_or(&view.post.media, |p| &p.post.media);
            if let Some(url) = media.preview_image().map(str::to_string) {
                self.request_image(&url);
            }
        }
        self.info(if self.show_images {
            "Inline images on"
        } else {
            "Inline images off"
        });
    }

    pub fn toggle_sidebar(&mut self) {
        self.show_sidebar = !self.show_sidebar;
    }

    fn request_image(&mut self, url: &str) {
        if matches!(self.images.get(url), None | Some(ImageSlot::Failed(_))) {
            self.images.insert(url.to_string(), ImageSlot::Loading);
            self.pool.submit(Job::Image {
                url: url.to_string(),
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
        let saved = self.state.toggle_bookmark(&post, now());
        if !saved && matches!(self.active_tab().kind, TabKind::Bookmarks) {
            let len = self.state.bookmarks().len();
            let feed = &mut self.tabs[self.active].feed;
            feed.selected = feed.selected.min(len.saturating_sub(1));
        }
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
                self.pool.submit(Job::DownloadImages { urls: vec![url] });
                self.info("Downloading image…");
                return;
            }
            Media::Gallery { images } if !images.is_empty() => {
                let count = images.len();
                self.pool.submit(Job::DownloadImages { urls: images });
                self.info(format!("Downloading {count} images…"));
                return;
            }
            Media::Video { url: Some(url), .. } => {
                media::spawn_detached(&commands.video, &[self.redlib.absolute(&url)])
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

    pub fn start_redlib(&mut self) {
        if self.command_running {
            self.info("Still starting Redlib…");
            return;
        }
        match self.config.redlib_start_command.clone() {
            Some(argv) if !argv.is_empty() => {
                self.command_running = true;
                self.info(format!("Running {}…", argv.join(" ")));
                self.pool.submit(Job::RunCommand { argv });
            }
            _ => self.info("Set redlib_start_command in config.toml to start Redlib from here"),
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
        let query = picker_query(input);
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
                        let image = page.post.media.preview_image().map(str::to_string);
                        view.page = Some(*page);
                        if self.show_images
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
                    }
                    Err(e) => self.error(format!("more replies: {e}")),
                }
            }
            Msg::Image { url, result } => {
                let slot = match result {
                    Ok(img) => {
                        ImageSlot::Ready(Box::new(self.image_picker.new_resize_protocol(img)))
                    }
                    Err(e) => ImageSlot::Failed(e),
                };
                self.images.insert(url, slot);
            }
            Msg::MediaResolved { permalink, result } => {
                if !self.pending_media.remove(&permalink) {
                    return;
                }
                match result {
                    Ok(Media::Gallery { images }) if images.is_empty() => {
                        self.error("Gallery has no images Redlib can show")
                    }
                    Ok(Media::Video { url: None, .. }) => {
                        self.error("Redlib has no playable source for this video")
                    }
                    Ok(media) => self.hand_off_media(&permalink, media),
                    Err(e) => self.error(format!("media: {e}")),
                }
            }
            Msg::ImagesDownloaded(result) => match result {
                Ok(paths) => {
                    let args: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
                    match media::spawn_detached(&self.config.commands.image, &args) {
                        Ok(()) => self.info("Opened media"),
                        Err(e) => self.error(format!("{e:#}")),
                    }
                }
                Err(e) => self.error(format!("image download: {e}")),
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
                    Ok(()) => {
                        self.info(format!("{} finished; reloading", argv.join(" ")));
                        self.check_health();
                        self.reload();
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
                let baseline = feed.is_default_view().then(|| feed.posts.clone());
                if let (Some(name), Some(posts)) = (tab.subreddit().map(str::to_string), baseline) {
                    self.state.baseline(&name, &posts, now());
                }
            }
            Err(e) => {
                feed.error = Some(e.clone());
                let title = tab.title();
                self.error(format!("{title}: {e}"));
            }
        }
    }

    fn on_group(&mut self, subreddit: &str, result: Result<Listing, String>) {
        let Some(group) = self
            .home
            .groups
            .iter_mut()
            .find(|g| g.subreddit == subreddit)
        else {
            return;
        };
        group.loading = false;
        match result {
            Ok(listing) => {
                group.posts = listing.posts.clone();
                group.error = None;
                self.state.baseline(subreddit, &listing.posts, now());
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
        // Port 9 is discard; nothing answers, so no test touches the network.
        let redlib = Redlib::new("http://127.0.0.1:9", false);
        let (pool, _messages) = Pool::start(redlib.clone(), 3);
        let state = std::env::temp_dir().join(format!(
            "redtab-app-test-{}-{:?}.json",
            std::process::id(),
            std::thread::current().id()
        ));
        App::new(
            Config::default(),
            subscriptions.iter().map(|s| s.to_string()).collect(),
            pool,
            redlib,
            state,
            Picker::halfblocks(),
        )
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

    fn tab_index(app: &App, name: &str) -> usize {
        app.tabs
            .iter()
            .position(|t| t.subreddit() == Some(name))
            .unwrap()
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
        let _ = std::fs::remove_file(&app.state_path);

        assert!(redraw);
        assert_eq!(app.current_post().unwrap().id, "x");
    }

    #[test]
    fn a_failed_write_after_a_merge_keeps_the_selection_and_reports_the_error() {
        let mut app = app_with(&[]);
        let path = app.state_path.clone();
        // `save` writes to this temp file; a directory there makes the write fail
        // after the merge already happened.
        let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&tmp);
        app.state.toggle_bookmark(&post("x", "s"), 50);
        app.state.toggle_bookmark(&post("y", "s"), 100);
        app.switch_tab(BOOKMARKS_TAB);
        app.tabs[BOOKMARKS_TAB].feed.selected = 1; // [y, x] → x
        let mut other = State::default();
        other.toggle_bookmark(&post("z", "s"), 200);
        other.save(&path, 200).unwrap();
        std::fs::create_dir(&tmp).unwrap();

        let redraw = app.persist();
        let selected = app.current_post().map(|p| p.id);
        app.toggle_bookmark();
        let status = app.status.as_ref().map(|s| (s.error, s.text.clone()));
        std::fs::remove_dir(&tmp).unwrap();
        let _ = std::fs::remove_file(&path);

        assert!(redraw);
        assert_eq!(selected.as_deref(), Some("x"));
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
