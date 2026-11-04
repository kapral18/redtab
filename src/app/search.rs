//! The Search tab: live search with a debounce, result rows and a feed preview.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::experience::Field;
use super::{App, Feed, FeedUpdate, SEARCH_TAB, TabKind};
use crate::cache::{Freshness, Source};
use crate::config::Config;
use crate::model::{Listing, Post, SubscriptionIcon, UserProfile};
use crate::reddit::experience::{ListSearch, Query, Response, SearchKind};
use crate::worker::{FeedError, FeedQuery, FeedTarget, Job};

/// Typing pauses this long before the query is sent.
pub(crate) const DEBOUNCE: Duration = Duration::from_millis(450);
/// The selected row rests this long before its feed preview loads.
pub(crate) const PREVIEW_SETTLE: Duration = Duration::from_millis(300);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub label: String,
    pub detail: String,
    pub target: FeedTarget,
    /// A People result's avatar and profile summary; communities use the shared icon map.
    pub profile: Option<UserProfile>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowsStatus {
    Discovering,
    LoadingMore,
    MoreFailed,
    DiscoveryFailed,
}

#[derive(Default)]
pub struct Rows {
    pub entries: Vec<Row>,
    pub loading: bool,
    pub error: Option<String>,
    generation: u64,
    /// The request these rows answer, and Reddit's cursor for its next page.
    query: Option<Query>,
    after: Option<String>,
    /// The loading request adds a page below the shown entries.
    appending: bool,
    suspended: bool,
    visited: HashSet<String>,
}

#[derive(Default)]
pub struct Preview {
    pub target: Option<FeedTarget>,
    pub posts: Vec<Post>,
    pub loading: bool,
    pub error: Option<String>,
    pub update: Option<Vec<Post>>,
    after: Option<String>,
    freshness: Option<Freshness>,
    first_page_len: usize,
    error_source: Option<Source>,
    pub(super) generation: u64,
    due: Option<Instant>,
}

impl Preview {
    pub fn has_new_posts(&self, config: &Config) -> bool {
        self.update
            .as_ref()
            .is_some_and(|posts| super::feed_has_new(config, &self.posts, posts))
    }

    fn accepts_snapshot(&self, freshness: Freshness) -> bool {
        self.freshness.is_none_or(|shown| {
            freshness.source == shown.source || freshness.source.may_replace(shown.source)
        })
    }

    fn clear_error(&mut self, source: Source) {
        if self
            .error_source
            .is_none_or(|shown| source == shown || source.may_replace(shown))
        {
            self.error = None;
            self.error_source = None;
        }
    }

    fn record_error(&mut self, error: String, source: Source) {
        if self
            .freshness
            .map(|shown| shown.source)
            .into_iter()
            .chain(self.error_source)
            .any(|shown| shown.compare_requests(source) == Some(std::cmp::Ordering::Greater))
        {
            return;
        }
        self.error = Some(error);
        self.error_source = Some(source);
    }
}

pub struct Search {
    pub query: Field,
    pub editing: bool,
    pub kind: SearchKind,
    /// The selected row of the shown rows, results or Explore.
    pub selected: usize,
    pub offset: usize,
    due: Option<Instant>,
    /// The query and kind the shown results answer.
    searched: Option<(String, SearchKind)>,
    /// The text of the last Posts search; the same text keeps its sort and time range.
    posts_text: Option<String>,
    pub results: Rows,
    /// Communities from Reddit's discovery list, shown under Popular and All Reddit.
    pub discovered: Rows,
    pub preview: Preview,
    pub(super) opened_feed: Option<u64>,
    /// The last frame drew the preview pane; a hidden pane loads nothing.
    preview_shown: bool,
    /// `r` asked for the next preview from Reddit rather than the cache.
    preview_forced: bool,
}

impl Default for Search {
    fn default() -> Self {
        Self {
            query: Field {
                label: "Search Reddit".into(),
                value: String::new(),
                multiline: false,
                cursor: 0,
                required: false,
            },
            editing: false,
            kind: SearchKind::Posts,
            selected: 0,
            offset: 0,
            due: None,
            searched: None,
            posts_text: None,
            results: Rows::default(),
            discovered: Rows::default(),
            preview: Preview::default(),
            opened_feed: None,
            preview_shown: false,
            preview_forced: false,
        }
    }
}

impl Search {
    pub fn text(&self) -> &str {
        self.query.value.trim()
    }

    /// An empty query explores; communities and people list as rows; posts are feed cards.
    pub fn shows_rows(&self) -> bool {
        self.text().is_empty() || self.kind != SearchKind::Posts
    }

    pub fn exploring(&self) -> bool {
        self.text().is_empty()
    }

    pub fn rows(&self) -> Vec<Row> {
        if self.exploring() {
            [
                ("popular", "Popular", "Trending posts across Reddit"),
                ("all", "All Reddit", "Every community's posts, unfiltered"),
            ]
            .into_iter()
            .map(|(name, label, detail)| Row {
                label: label.into(),
                detail: detail.into(),
                target: FeedTarget::Subreddit(name.into()),
                profile: None,
            })
            .chain(self.discovered.entries.iter().cloned())
            .collect()
        } else {
            self.results.entries.clone()
        }
    }

    pub fn selected_row(&self) -> Option<Row> {
        self.rows().into_iter().nth(self.selected)
    }

    /// Whether the rows the user sees are still on their way.
    pub fn rows_loading(&self) -> bool {
        if self.exploring() {
            self.discovered.loading
        } else {
            self.results.loading || self.due.is_some()
        }
    }

    /// What the line under the shown rows says, if anything.
    pub fn rows_status(&self) -> Option<RowsStatus> {
        let rows = if self.exploring() {
            &self.discovered
        } else {
            &self.results
        };
        if rows.loading && rows.appending {
            Some(RowsStatus::LoadingMore)
        } else if self.exploring() && rows.loading {
            Some(RowsStatus::Discovering)
        } else if rows.error.is_some() && rows.after.is_some() {
            Some(RowsStatus::MoreFailed)
        } else if self.exploring() && rows.error.is_some() {
            Some(RowsStatus::DiscoveryFailed)
        } else {
            None
        }
    }

    #[cfg(test)]
    pub(crate) fn discovered_generation(&self) -> u64 {
        self.discovered.generation
    }

    #[cfg(test)]
    pub(crate) fn results_generation(&self) -> u64 {
        self.results.generation
    }

    #[cfg(test)]
    pub(crate) fn results_query(&self) -> Option<&Query> {
        self.results.query.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn preview_forced(&self) -> bool {
        self.preview_forced
    }

    pub fn rows_error(&self) -> Option<&str> {
        if self.exploring() {
            self.discovered.error.as_deref()
        } else {
            self.results.error.as_deref()
        }
    }
}

fn row(entry: &crate::reddit::experience::Entry) -> Option<Row> {
    let summary = UserProfile::new(
        match &entry.icon {
            SubscriptionIcon::Url(url) => Some(url.clone()),
            _ => None,
        },
        &entry.body,
    );
    let detail = summary.lines.first().cloned().unwrap_or_default();
    let (target, profile) = match entry.query.as_ref()? {
        Query::Community(name) => (FeedTarget::Subreddit(name.clone()), None),
        Query::Profile(name) => (FeedTarget::User(name.clone()), Some(summary)),
        _ => return None,
    };
    Some(Row {
        label: entry.title.clone(),
        detail,
        target,
        profile,
    })
}

impl App {
    /// The tab key and command for F3, `SPC s e`, `SPC s p/c/u` and `/` outside communities.
    pub fn open_search(&mut self, kind: Option<SearchKind>) {
        if self.experience.writing() {
            return;
        }
        self.clear_experience();
        self.overlay = None;
        if SEARCH_TAB != self.active {
            self.switch_tab(SEARCH_TAB);
        }
        self.edit_search();
        if let Some(kind) = kind
            && kind != self.search.kind
        {
            self.search.kind = kind;
            self.search.selected = 0;
            self.search.offset = 0;
            self.schedule_search(Instant::now());
        }
        self.search_arrived();
    }

    pub(super) fn search_arrived(&mut self) {
        if self.search.exploring() {
            self.ensure_discovered();
        } else if self.search.searched.as_ref()
            != Some(&(self.search.text().to_owned(), self.search.kind))
        {
            self.schedule_search(Instant::now());
        } else {
            self.resume_search_rows(false);
        }
        self.schedule_preview(Instant::now());
    }

    pub(super) fn search_departed(&mut self) {
        self.retire_search_rows(false);
        self.retire_search_rows(true);
        self.search.due = None;
        self.search.preview.due = None;
        self.search.preview_shown = false;
        if self.search.preview.loading {
            self.search.preview = Preview::default();
        }
    }

    fn retire_search_rows(&mut self, discovery: bool) {
        let rows = if discovery {
            &mut self.search.discovered
        } else {
            &mut self.search.results
        };
        if let Some(query) = &rows.query {
            self.pool.want_search(query, None);
        }
        if rows.loading {
            rows.loading = false;
            rows.suspended = true;
        }
    }

    fn resume_search_rows(&mut self, discovery: bool) -> bool {
        let rows = if discovery {
            &self.search.discovered
        } else {
            &self.search.results
        };
        if !rows.suspended {
            return false;
        }
        let Some(query) = rows.query.clone() else {
            return false;
        };
        let after = rows.appending.then(|| rows.after.clone()).flatten();
        self.request_rows(discovery, query, after);
        true
    }

    pub fn search_editing(&self) -> bool {
        self.search.editing
            && self.overlay.is_none()
            && self.active == SEARCH_TAB
            && !self.post_focused()
    }

    pub fn search_shows_rows(&self) -> bool {
        self.active == SEARCH_TAB && self.search.shows_rows()
    }

    /// `/` on the Search tab: back to typing, with the rows in view.
    pub fn edit_search(&mut self) {
        self.close_post();
        self.focus_main();
        self.search.editing = true;
    }

    pub fn search_paste(&mut self, text: &str) {
        let text: String = text
            .chars()
            .map(|c| {
                if matches!(c, '\n' | '\r' | '\t') {
                    ' '
                } else {
                    c
                }
            })
            .filter(|c| !c.is_control())
            .collect();
        if text.is_empty() {
            return;
        }
        self.search.query.insert(&text);
        self.search.selected = 0;
        self.search.offset = 0;
        self.schedule_search(Instant::now());
    }

    pub fn search_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers == KeyModifiers::CONTROL;
        let alt = key.modifiers == KeyModifiers::ALT;
        let field = &mut self.search.query;
        let before = field.value.clone();
        match key.code {
            KeyCode::Esc => {
                self.search.editing = false;
                return;
            }
            KeyCode::Down | KeyCode::Up => {
                self.move_search_result(if key.code == KeyCode::Down { 1 } else { -1 });
                return;
            }
            KeyCode::Char('n' | 'j') if ctrl => {
                self.move_search_result(1);
                return;
            }
            KeyCode::Char('p' | 'k') if ctrl => {
                self.move_search_result(-1);
                return;
            }
            // Enter commits the query: a pending search runs now and the results take the keys.
            KeyCode::Enter => {
                if self.search.due.is_some() {
                    self.run_search();
                }
                self.search.editing = false;
                return;
            }
            KeyCode::Tab | KeyCode::BackTab => {
                self.cycle_search_kind(key.code == KeyCode::Tab);
                return;
            }
            KeyCode::Backspace if alt => field.delete_word(),
            KeyCode::Char('w') if ctrl => field.delete_word(),
            KeyCode::Backspace => {
                let start = field.previous();
                field.value.replace_range(start..field.cursor, "");
                field.cursor = start;
            }
            KeyCode::Delete => {
                let end = field.next();
                field.value.replace_range(field.cursor..end, "");
            }
            KeyCode::Left => field.cursor = field.previous(),
            KeyCode::Right => field.cursor = field.next(),
            KeyCode::Home => field.cursor = 0,
            KeyCode::Char('a') if ctrl => field.cursor = 0,
            KeyCode::End => field.cursor = field.value.len(),
            KeyCode::Char('e') if ctrl => field.cursor = field.value.len(),
            KeyCode::Char('u') if ctrl => {
                field.value.clear();
                field.cursor = 0;
            }
            KeyCode::Char(c) if key.modifiers.difference(KeyModifiers::SHIFT).is_empty() => {
                field.insert(&c.to_string())
            }
            _ => return,
        }
        if self.search.query.value != before {
            self.search.selected = 0;
            self.search.offset = 0;
            self.schedule_search(Instant::now());
        }
    }

    /// Tab / S-Tab from the query or the result list; a typed query is searched again at once.
    pub fn cycle_search_kind(&mut self, forward: bool) {
        let kinds = [
            SearchKind::Posts,
            SearchKind::Communities,
            SearchKind::People,
        ];
        let at = kinds
            .iter()
            .position(|kind| *kind == self.search.kind)
            .unwrap_or(0);
        let step = if forward { 1 } else { kinds.len() - 1 };
        self.search.kind = kinds[(at + step) % kinds.len()];
        self.search.selected = 0;
        self.search.offset = 0;
        // Another kind's results replace the post opened from these ones.
        self.close_post();
        self.schedule_search(Instant::now() - DEBOUNCE);
    }

    /// Up/Down while typing move through the shown results without leaving the query.
    fn move_search_result(&mut self, delta: isize) {
        if self.search.shows_rows() {
            self.move_search(delta);
        } else {
            self.move_selection(delta);
        }
    }

    fn schedule_search(&mut self, now: Instant) {
        self.search.due = Some(now + DEBOUNCE);
    }

    fn schedule_preview(&mut self, now: Instant) {
        if self.search.shows_rows() {
            self.search.preview.due = Some(now + PREVIEW_SETTLE);
        }
    }

    /// Fires a debounced search or preview whose pause has passed.
    pub(super) fn search_tick(&mut self, now: Instant) -> bool {
        let mut changed = false;
        if self.search.due.is_some_and(|due| now >= due) {
            self.run_search();
            changed = true;
        }
        if self.search.preview.due.is_some_and(|due| now >= due) {
            self.search.preview.due = None;
            self.request_preview();
            changed = true;
        }
        changed
    }

    pub(super) fn search_wake_in(&self, now: Instant) -> Option<Duration> {
        [self.search.due, self.search.preview.due]
            .into_iter()
            .flatten()
            .map(|due| due.saturating_duration_since(now))
            .min()
    }

    /// Returns false when the pending text and kind are what the shown results already answer.
    fn run_search(&mut self) -> bool {
        self.search.due = None;
        let text = self.search.text().to_string();
        let kind = self.search.kind;
        if text.is_empty() {
            self.retire_search_rows(false);
            self.search.searched = None;
            self.search.results = Rows::default();
            self.clear_search_feed();
            self.ensure_discovered();
            self.schedule_preview(Instant::now());
            return true;
        }
        self.retire_search_rows(true);
        if self.search.searched.as_ref() == Some(&(text.clone(), kind)) {
            return self.resume_search_rows(false);
        }
        self.retire_search_rows(false);
        self.search.searched = Some((text.clone(), kind));
        let list = match kind {
            SearchKind::Posts => {
                self.search.results = Rows::default();
                let feed = &mut self.tabs[SEARCH_TAB].feed;
                let kept = (feed.search_sort, feed.search_time);
                feed.set_search(text.clone());
                if self.search.posts_text.as_deref() == Some(text.as_str()) {
                    (feed.search_sort, feed.search_time) = kept;
                }
                self.search.posts_text = Some(text);
                self.load_feed(SEARCH_TAB, false);
                return true;
            }
            SearchKind::Communities => ListSearch::Communities,
            SearchKind::People => ListSearch::People,
        };
        self.clear_search_feed();
        self.request_rows(
            false,
            Query::Search {
                text,
                kind: list,
                time: crate::model::TimeRange::All,
            },
            None,
        );
        true
    }

    /// Asks for result or discovery rows; a cursor adds the next page below the shown ones.
    fn request_rows(&mut self, discovery: bool, query: Query, after: Option<String>) {
        let generation = self.generation();
        let rows = if discovery {
            &mut self.search.discovered
        } else {
            &mut self.search.results
        };
        if let Some(after) = &after {
            rows.visited.insert(after.clone());
            rows.loading = true;
            rows.suspended = false;
            rows.appending = true;
            rows.error = None;
            rows.generation = generation;
        } else {
            *rows = Rows {
                loading: true,
                generation,
                query: Some(query.clone()),
                ..Rows::default()
            };
        }
        self.pool.submit(Job::Search {
            generation,
            query,
            after,
        });
    }

    fn load_more_rows(&mut self) {
        self.fill_search_rows(self.search.selected + 5 >= self.search.rows().len());
    }

    pub fn fill_search_rows(&mut self, needed: bool) {
        if !needed
            || self.active != SEARCH_TAB
            || !self.search.shows_rows()
            || self.popup_open()
            || self.search.due.is_some()
        {
            return;
        }
        let discovery = self.search.exploring();
        let rows = if discovery {
            &self.search.discovered
        } else {
            &self.search.results
        };
        if rows.loading || rows.suspended || rows.error.is_some() {
            return;
        }
        let (Some(query), Some(after)) = (rows.query.clone(), rows.after.clone()) else {
            return;
        };
        self.request_rows(discovery, query, Some(after));
    }

    /// Rows replace Posts results: a fresh feed also drops a Posts request still in flight.
    fn clear_search_feed(&mut self) {
        let feed = &mut self.tabs[SEARCH_TAB].feed;
        if feed.search.is_none() && feed.posts.is_empty() {
            return;
        }
        *feed = Feed {
            sort: feed.sort,
            time: feed.time,
            search_sort: feed.search_sort,
            search_time: feed.search_time,
            ..Feed::default()
        };
        self.sync_feed_demand();
    }

    fn ensure_discovered(&mut self) {
        if self.resume_search_rows(true) {
            return;
        }
        // Asked once; a failed or empty answer waits for `r`, which resets the rows.
        if self.search.discovered.generation != 0 {
            return;
        }
        self.request_rows(true, Query::Communities, None);
    }

    pub(super) fn on_search(&mut self, generation: u64, result: Result<Response, String>) {
        let rows = if self.search.results.generation == generation && self.search.results.loading {
            &mut self.search.results
        } else if self.search.discovered.generation == generation && self.search.discovered.loading
        {
            &mut self.search.discovered
        } else {
            return;
        };
        rows.loading = false;
        let appending = std::mem::take(&mut rows.appending);
        let mut icons = Vec::new();
        let mut profiles = Vec::new();
        match result {
            Ok(Response::Page(page)) => {
                icons = page
                    .entries
                    .iter()
                    .filter_map(|entry| match (&entry.query, &entry.icon) {
                        (Some(Query::Community(name)), SubscriptionIcon::Url(url)) => {
                            Some((name.clone(), Some(url.clone())))
                        }
                        (Some(Query::Community(name)), SubscriptionIcon::Missing) => {
                            Some((name.clone(), None))
                        }
                        _ => None,
                    })
                    .collect();
                let entries: Vec<Row> = page.entries.iter().filter_map(row).collect();
                profiles = entries
                    .iter()
                    .filter_map(|row| match (&row.target, &row.profile) {
                        (FeedTarget::User(name), Some(profile)) => {
                            Some((name.clone(), profile.clone()))
                        }
                        _ => None,
                    })
                    .collect();
                let entries = entries.into_iter();
                if appending {
                    for entry in entries {
                        if !rows
                            .entries
                            .iter()
                            .any(|known| known.target == entry.target)
                        {
                            rows.entries.push(entry);
                        }
                    }
                } else {
                    rows.entries = entries.collect();
                }
                rows.after = page
                    .after
                    .clone()
                    .filter(|after| !rows.visited.contains(after));
                rows.error = None;
            }
            Ok(_) => rows.error = Some("Reddit returned an unexpected answer".into()),
            Err(error) => rows.error = Some(error),
        }
        // People results spare their tabs an `about.json` lookup.
        for (name, profile) in profiles {
            self.observe_profile(&name, profile);
        }
        // Listing icons spare an `about.json` lookup per row; known icons stay.
        for (name, url) in icons {
            if !self.icons.contains_key(&name.to_ascii_lowercase()) {
                self.observe_icon(&name, url, crate::cache::Source::default());
            }
        }
        let len = self.search.rows().len();
        self.search.selected = self.search.selected.min(len.saturating_sub(1));
        self.schedule_preview(Instant::now());
    }

    /// `r` on rows: ask again for the shown results, discovery list and preview.
    pub(super) fn reload_search(&mut self) {
        let current = self
            .search
            .selected_row()
            .is_some_and(|row| self.search.preview.target.as_ref() == Some(&row.target));
        if current && let Some(posts) = self.search.preview.update.take() {
            self.search.preview.posts = posts;
            self.search.preview.first_page_len = self.search.preview.posts.len();
            return;
        }
        if self.search.exploring() {
            if !self.search.discovered.loading {
                self.search.discovered = Rows::default();
            }
            self.ensure_discovered();
        } else {
            self.search.searched = None;
            self.run_search();
        }
        self.search.preview = Preview::default();
        self.search.preview_forced = true;
        self.request_preview();
    }

    pub fn move_search(&mut self, delta: isize) {
        let len = self.search.rows().len();
        if len == 0 {
            return;
        }
        let selected = self
            .search
            .selected
            .saturating_add_signed(delta)
            .min(len - 1);
        if selected != self.search.selected {
            self.search.selected = selected;
            self.schedule_preview(Instant::now());
        }
        self.load_more_rows();
    }

    /// The community or person a selected row stands for, for `SPC c` and `SPC m`.
    pub(super) fn search_row_target(&self) -> Option<FeedTarget> {
        (self.active == SEARCH_TAB
            && self.search.shows_rows()
            && self.focus == super::Focus::List
            && self.post_view.is_none())
        .then(|| self.search.selected_row().map(|row| row.target))
        .flatten()
    }

    pub(crate) fn search_row_path(&self) -> Option<String> {
        if self.overlay.is_some() {
            return None;
        }
        match self.search_row_target()? {
            FeedTarget::Subreddit(name) => {
                let name = crate::config::normalize_subreddit(&name);
                crate::config::is_subreddit_name(name).then(|| format!("/r/{name}/"))
            }
            FeedTarget::User(name) if crate::model::is_username(&name) => {
                Some(format!("/user/{name}/"))
            }
            _ => None,
        }
    }

    /// Each Search tab frame reports whether it drew the preview pane.
    pub fn search_preview_drawn(&mut self, shown: bool) {
        self.search.preview_shown = shown;
        let stale = self
            .search
            .selected_row()
            .is_some_and(|row| self.search.preview.target.as_ref() != Some(&row.target));
        if shown && stale && self.search.preview.due.is_none() {
            self.schedule_preview(Instant::now());
        }
    }

    /// A restarted worker pool drops in-flight answers; ask again for what the tab shows.
    pub(super) fn reset_search_loads(&mut self) {
        self.search.opened_feed = None;
        self.search.results = Rows::default();
        self.search.discovered = Rows::default();
        self.search.preview = Preview::default();
        self.search.searched = None;
        self.search.due = (self.active == SEARCH_TAB
            && !self.search.exploring()
            && self.search.kind != SearchKind::Posts)
            .then(Instant::now);
    }

    /// After `r` the next request skips the cached copy, even when the rows reload first.
    fn request_preview(&mut self) {
        let Some(row) = self
            .search
            .selected_row()
            .filter(|_| self.search.shows_rows())
            .filter(|_| self.active == SEARCH_TAB && self.search.preview_shown)
        else {
            return;
        };
        let preview = &self.search.preview;
        // A failed preview waits for `r`, which resets it.
        if preview.target.as_ref() == Some(&row.target) {
            return;
        }
        let generation = self.generation();
        self.search.preview = Preview {
            target: Some(row.target.clone()),
            loading: true,
            generation,
            ..Preview::default()
        };
        let forced = std::mem::take(&mut self.search.preview_forced);
        if !forced
            && let Some(index) =
                (0..self.tabs.len()).find(|&index| self.preview_matches_feed(index))
        {
            self.sync_search_preview(index);
            self.search.preview.loading = true;
        }
        self.sync_feed_demand();
        let job = Job::Feed {
            tab: crate::worker::PREVIEW_FEED,
            generation,
            target: row.target,
            query: FeedQuery::DEFAULT_LISTING,
            after: None,
        };
        if forced {
            self.pool.refresh(job);
        } else {
            self.pool.submit(job);
        }
    }

    /// A cached copy still refreshing shows at once and keeps waiting for Reddit's answer.
    pub(super) fn on_preview(
        &mut self,
        generation: u64,
        result: Result<Listing, FeedError>,
        freshness: Freshness,
        refreshing: bool,
    ) {
        let preview = &mut self.search.preview;
        if preview.generation != generation || !preview.loading {
            return;
        }
        if result.is_ok() && !preview.accepts_snapshot(freshness) {
            if !refreshing {
                preview.loading = false;
                self.sync_feed_demand();
            }
            return;
        }
        match result {
            Ok(mut listing) => {
                listing
                    .posts
                    .retain(|post| !self.removed_posts.contains(&post.id));
                preview.after = listing.after;
                preview.freshness = Some(freshness);
                preview.first_page_len = listing.posts.len();
                if self.config.visible(&preview.posts).next().is_none() {
                    preview.posts = listing.posts;
                    preview.update = None;
                } else {
                    let fresh: std::collections::HashMap<_, _> = listing
                        .posts
                        .iter()
                        .map(|post| (post.id.as_str(), post))
                        .collect();
                    for post in &mut preview.posts {
                        if let Some(fresh) = fresh.get(post.id.as_str()) {
                            *post = (*fresh).clone();
                        }
                    }
                    preview.update = (!preview
                        .posts
                        .iter()
                        .map(|post| &post.id)
                        .eq(listing.posts.iter().map(|post| &post.id)))
                    .then_some(listing.posts);
                }
                preview.clear_error(freshness.source);
                if refreshing {
                    return;
                }
            }
            Err(error) => preview.record_error(error.to_string(), freshness.source),
        }
        preview.loading = false;
        self.sync_feed_demand();
    }

    pub fn search_feed_open(&self) -> bool {
        self.search.opened_feed == Some(self.active_tab().id) && self.search.shows_rows()
    }

    pub(crate) fn search_source_context(&self) -> bool {
        self.search_shows_rows() || self.search_feed_open()
    }

    pub fn return_to_search(&mut self) -> bool {
        if self.search.opened_feed != Some(self.active_tab().id) {
            return false;
        }
        self.sync_search_preview(self.active);
        self.switch_tab(SEARCH_TAB);
        self.focus_main();
        self.search.editing = false;
        true
    }

    fn preview_matches_feed(&self, index: usize) -> bool {
        let tab = &self.tabs[index];
        let target = match (&self.search.preview.target, &tab.kind) {
            (Some(FeedTarget::Subreddit(row)), TabKind::Subreddit { name, .. })
            | (Some(FeedTarget::User(row)), TabKind::User { name, .. }) => {
                row.eq_ignore_ascii_case(name)
            }
            _ => false,
        };
        target && tab.feed.query() == FeedQuery::DEFAULT_LISTING
    }

    pub(super) fn apply_search_preview(&mut self, index: usize) {
        if !self.preview_matches_feed(index) || self.tabs[index].feed.access_denied().is_some() {
            return;
        }
        let preview = &self.search.preview;
        if let Some(freshness) = preview.freshness {
            let feed = &self.tabs[index].feed;
            if !feed.shows_current_query()
                || feed
                    .freshness
                    .is_none_or(|shown| freshness.source.may_replace(shown.source))
            {
                self.apply_update(
                    index,
                    FeedUpdate {
                        listing: Listing {
                            posts: preview.update.clone().unwrap_or_else(|| {
                                preview.posts[..preview.first_page_len].to_vec()
                            }),
                            after: preview.after.clone(),
                        },
                        freshness,
                        stable: false,
                    },
                    true,
                );
            }
        }
        self.sync_search_preview(index);
    }

    fn sync_search_preview(&mut self, index: usize) {
        if !self.preview_matches_feed(index) {
            return;
        }
        let feed = &self.tabs[index].feed;
        let Some(first) = feed
            .pages
            .first()
            .filter(|page| page.cursor.is_none() && page.complete)
        else {
            return;
        };
        let Some(freshness) = feed
            .freshness
            .filter(|_| feed.shows_current_query() && feed.access_denied().is_none())
        else {
            return;
        };
        let preview = &mut self.search.preview;
        if preview.accepts_snapshot(freshness) {
            preview.posts = feed.posts.clone();
            preview.first_page_len = first.len;
            preview.update = feed
                .update
                .as_ref()
                .map(|update| update.listing.posts.clone());
            preview.after = feed.update.as_ref().map_or_else(
                || first.after.clone(),
                |update| update.listing.after.clone(),
            );
            preview.freshness = Some(freshness);
        }
        if let Some(failure) = feed.failures.get(&None) {
            preview.record_error(failure.error.to_string(), failure.source);
        } else {
            preview.clear_error(freshness.source);
        }
        preview.loading = false;
    }

    /// Enter or `l` on a row opens its feed as a tab.
    pub fn open_search_row(&mut self) {
        let Some(row) = self.search.selected_row() else {
            return;
        };
        self.search.editing = false;
        match row.target {
            FeedTarget::Subreddit(name) => self.open_subreddit(&name),
            FeedTarget::User(name) => self.open_user(&name),
            FeedTarget::Home => {}
        }
    }
}
