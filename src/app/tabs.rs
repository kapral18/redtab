use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnreadBadge {
    Unknown,
    Count(usize),
    Unavailable,
}

#[derive(Debug)]
pub enum TabKind {
    User {
        name: String,
        followed: bool,
        icon: Option<String>,
    },
    Home,
    Bookmarks,
    AllSubs,
    /// Live search over posts, communities, comments, media and people; Explore when the query is empty.
    Search,
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
}

impl Tab {
    pub(super) fn feed_target(&self) -> Option<FeedTarget> {
        match &self.kind {
            TabKind::Home => Some(FeedTarget::Home),
            TabKind::User { name, .. } => Some(FeedTarget::User(name.clone())),
            TabKind::Subreddit { name, .. } => Some(FeedTarget::Subreddit(name.clone())),
            // Post results search all of Reddit; no query, no feed.
            TabKind::Search => self
                .feed
                .search
                .as_ref()
                .map(|_| FeedTarget::Subreddit("all".into())),
            _ => None,
        }
    }

    pub fn sorts(&self) -> &[Sort] {
        if matches!(self.kind, TabKind::User { .. }) {
            &[Sort::New, Sort::Hot, Sort::Top, Sort::Controversial]
        } else if matches!(self.kind, TabKind::Home) {
            &Sort::ALL
        } else if self
            .subreddit()
            .is_some_and(|name| name.eq_ignore_ascii_case("all"))
        {
            // Reddit refuses r/all New and Rising on every host.
            &[Sort::Hot, Sort::Top, Sort::Controversial]
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
            TabKind::User { name, .. } => format!("u/{name}"),
            TabKind::Home => "Home".into(),
            TabKind::AllSubs => "All subs".into(),
            TabKind::Bookmarks => "Bookmarks".into(),
            TabKind::Search => "Search".into(),
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
    pub error: Option<FeedError>,
    pub freshness: Option<Freshness>,
    pub generation: u64,
    pub needs_refresh: bool,
    pub(crate) stream: Option<Source>,
    /// The latest first page while it ranks unseen posts above shown ones; `r` applies it.
    pub arrivals: Option<Vec<Post>>,
    /// `r` asked for this group: its next live answer applies whole.
    pub manual: bool,
}

impl Group {
    /// Shows the waiting first page, if any.
    pub(super) fn apply_arrivals(&mut self) -> bool {
        let Some(posts) = self.arrivals.take() else {
            return false;
        };
        self.posts = posts;
        true
    }
}

/// What counts as new everywhere: an unseen item ranked before every seen item, pinned
/// items aside. Unseen items lower down entered by ranking, like a reorder.
pub(super) fn unseen_leads<'a>(
    items: impl IntoIterator<Item = (&'a str, bool)>,
    seen: impl Fn(&str) -> bool,
) -> bool {
    for (id, pinned) in items {
        if !seen(id) {
            return true;
        }
        if !pinned {
            return false;
        }
    }
    false
}

pub(super) fn feed_has_new(config: &Config, shown: &[Post], incoming: &[Post]) -> bool {
    if config.visible(shown).next().is_none() {
        return false;
    }
    let known: HashSet<_> = shown.iter().map(|post| post.id.as_str()).collect();
    unseen_leads(
        config
            .visible(incoming)
            .map(|post| (post.id.as_str(), post.stickied)),
        |id| known.contains(id),
    )
}

/// A thread's top-level comments, which the parser keeps under one logical root.
pub(super) fn thread_roots(comments: &[Comment]) -> &[Comment] {
    match comments {
        [root] if root.id.starts_with(THREAD_ROOT) => &root.replies,
        _ => comments,
    }
}

/// Islands never show pinned posts, so they lead with their first unpinned one.
pub(super) fn island_has_new(config: &Config, shown: &[Post], incoming: &[Post]) -> bool {
    let known: HashSet<_> = shown.iter().map(|post| post.id.as_str()).collect();
    unseen_leads(
        config
            .visible(incoming)
            .filter(|post| !post.stickied)
            .map(|post| (post.id.as_str(), false)),
        |id| known.contains(id),
    )
}

#[derive(Debug, Default)]
pub struct AllSubs {
    pub loading: bool,
    pub error: Option<String>,
    pub freshness: Option<Freshness>,
    pub(super) generation: u64,
    pub(super) next_group_generation: u64,
    pub groups: Vec<Group>,
    pub group: usize,
    pub post: usize,
    /// First visible row of groups; kept in sync by the UI.
    pub offset: usize,
    /// Selected island and how many grid rows were drawn above it: data changes keep it there.
    pub drawn: Option<(String, usize)>,
    pub(super) waiting_for_rows: bool,
    /// Islands the last draw showed.
    pub(super) shown: Vec<usize>,
}

impl AllSubs {
    pub fn has_new_posts(&self, config: &Config) -> bool {
        self.groups.iter().any(|group| {
            group
                .arrivals
                .as_ref()
                .is_some_and(|incoming| island_has_new(config, &group.posts, incoming))
        })
    }

    pub(super) fn show_top(&mut self) {
        self.group = 0;
        self.post = 0;
        self.offset = 0;
        self.drawn = None;
    }
}

#[derive(Clone)]
pub(super) struct AncestorFollow {
    pub(super) tab: u64,
    pub(super) group: Option<String>,
    pub(super) waiting: bool,
}

impl App {
    pub fn active_tab(&self) -> &Tab {
        &self.tabs[self.active]
    }

    pub fn switch_tab(&mut self, index: usize) {
        if index >= self.tabs.len() || index == self.active {
            return;
        }
        self.enter_tab(index, false);
    }

    pub(super) fn enter_tab(&mut self, index: usize, keep_post: bool) {
        if self.active == SEARCH_TAB && index != SEARCH_TAB {
            self.search_departed();
        }
        self.search.opened_feed = None;
        self.cancel_reveal();
        self.list_prefetch = None;
        let open = self
            .post_view
            .as_ref()
            .filter(|_| keep_post)
            .map(|view| view.shown_post().id.clone());
        let open_here = open.is_some() && open == self.list_post(index).map(|post| post.id);
        let arriving = !open_here && !matches!(self.tabs[index].kind, TabKind::Bookmarks);
        self.active = index;
        self.sync_feed_demand();
        if !keep_post {
            self.close_post();
        }
        if arriving && let Some(update) = self.tabs[index].feed.update.take() {
            self.apply_update(index, update, true);
        }
        let now = Instant::now();
        if self.focus == Focus::Sidebar {
            self.arrive(index, arriving);
            self.feed_settle.defer(self.tabs[index].id, now);
            self.picture_settle.defer((), now);
        } else {
            self.picture_settle.arrive((), now);
            self.arrive(index, arriving);
            if self.feed_settle.arrive(self.tabs[index].id, now) {
                self.ensure_feed_loaded(index);
            }
        }
    }

    pub(super) fn arrive(&mut self, index: usize, arriving: bool) {
        if arriving && matches!(self.tabs[index].kind, TabKind::AllSubs) {
            let mut applied = false;
            for group in &mut self.all_subs.groups {
                applied |= group.apply_arrivals();
            }
            if applied {
                self.all_subs.show_top();
            }
        }
    }

    pub fn cycle_tab(&mut self, forward: bool, preserve_depth: bool) {
        let follow = preserve_depth && (self.post_focused() || self.ancestor_follow.is_some());
        let n = self.tabs.len();
        let next = if forward {
            (self.active + 1) % n
        } else {
            (self.active + n - 1) % n
        };
        if next == self.active {
            return;
        }
        if follow {
            self.switch_ancestor_tab(next);
        } else {
            self.switch_tab(next);
        }
    }

    pub fn open_user(&mut self, name: &str) {
        if !crate::model::is_username(name) {
            self.error("Invalid Reddit username");
            return;
        }
        let from_search = matches!(self.search_row_target(), Some(FeedTarget::User(row)) if row.eq_ignore_ascii_case(name));
        let index = self.tabs.iter().position(|tab| matches!(&tab.kind, TabKind::User { name: existing, .. } if existing.eq_ignore_ascii_case(name)))
            .unwrap_or_else(|| self.push_tab(TabKind::User { name: name.to_string(), followed: false, icon: None }));
        if from_search {
            self.apply_search_preview(index);
        }
        self.switch_tab(index);
        if from_search {
            self.search.editing = false;
            self.search.opened_feed = Some(self.active_tab().id);
        }
        self.ensure_feed_loaded(index);
    }

    pub fn profile_context(&self) -> Option<String> {
        if !self.post_focused() {
            if let TabKind::User { name, .. } = &self.active_tab().kind {
                return Some(name.clone());
            }
            if self.focus == Focus::Sidebar {
                return None;
            }
            if let Some(target) = self.search_row_target() {
                return match target {
                    FeedTarget::User(name) => Some(name),
                    _ => None,
                };
            }
        }
        self.current_post()
            .and_then(|post| post.profile_user().map(str::to_string))
    }

    /// Switch to the tab for `name`, opening a new tab when needed.
    pub fn open_subreddit(&mut self, name: &str) {
        let name = normalize_subreddit(name);
        if !is_subreddit_name(name) {
            self.error(format!("{name:?} is not a subreddit name"));
            return;
        }
        let from_search = matches!(self.search_row_target(), Some(FeedTarget::Subreddit(row)) if normalize_subreddit(&row).eq_ignore_ascii_case(name));
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
        if from_search {
            self.apply_search_preview(index);
        }
        self.switch_tab(index);
        if from_search {
            self.search.editing = false;
            self.search.opened_feed = Some(self.active_tab().id);
        }
        self.ensure_feed_loaded(index);
    }

    pub fn close_active_tab(&mut self) {
        match &self.active_tab().kind {
            TabKind::Subreddit { pinned: false, .. }
            | TabKind::User {
                followed: false, ..
            } => {
                // A feed opened beside the Search results closes back to them.
                let from_search = self.search.opened_feed == Some(self.active_tab().id);
                self.tabs.remove(self.active);
                let next = if from_search {
                    SEARCH_TAB
                } else {
                    self.active.min(self.tabs.len() - 1)
                };
                self.enter_tab(next, false);
            }
            TabKind::User { followed: true, .. } => {
                self.info("Following this user — press SPC m to unfollow")
            }
            TabKind::Subreddit { pinned: true, .. } => {
                self.info("Joined this community — press SPC m to leave")
            }
            _ => self.info("Home, Bookmarks, All subs and Search stay open"),
        }
    }

    pub fn community_badge(&self, name: &str) -> UnreadBadge {
        if !is_community(name)
            || self
                .community_access
                .get(&name.to_lowercase())
                .is_some_and(|access| access.denied.is_some())
        {
            return UnreadBadge::Unavailable;
        }
        match self.state.unread(name, &self.config) {
            Some(count) => UnreadBadge::Count(count),
            None => UnreadBadge::Unknown,
        }
    }

    pub fn badge(&self, tab: &Tab) -> UnreadBadge {
        tab.subreddit()
            .map_or(UnreadBadge::Unavailable, |name| self.community_badge(name))
    }
}
