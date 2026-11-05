use super::*;

impl App {
    /// The post under the cursor in whatever view is active.
    pub fn current_post(&self) -> Option<Post> {
        if self.post_focused()
            && let Some(view) = &self.post_view
        {
            return Some(view.shown_post().clone());
        }
        self.list_post(self.active)
    }

    pub(super) fn list_post(&self, index: usize) -> Option<Post> {
        let tab = self.tabs.get(index)?;
        if matches!(tab.kind, TabKind::Subreddit { .. }) && tab.feed.access_denied().is_some() {
            return None;
        }
        match tab.kind {
            TabKind::AllSubs => {
                let group = self.all_subs.groups.get(self.all_subs.group)?;
                self.group_posts(group).nth(self.all_subs.post).cloned()
            }
            TabKind::Bookmarks => self
                .visible_bookmarks()
                .nth(tab.feed.selected)
                .map(|b| b.post.clone()),
            TabKind::Search if self.search.shows_rows() => None,
            TabKind::Home | TabKind::Subreddit { .. } | TabKind::User { .. } | TabKind::Search => {
                self.config
                    .visible(&tab.feed.posts)
                    .nth(tab.feed.selected)
                    .cloned()
            }
        }
    }

    pub(crate) fn post_parent_position(&self) -> Option<(usize, usize)> {
        let view = self.post_view.as_ref()?;
        let tab = self.active_tab();
        if tab.subreddit().is_some() && tab.feed.access_denied().is_some() {
            return None;
        }
        match tab.kind {
            TabKind::AllSubs => {
                self.all_subs
                    .groups
                    .iter()
                    .enumerate()
                    .find_map(|(group, entry)| {
                        self.group_posts(entry)
                            .position(|post| post.id == view.post.id)
                            .map(|index| (group, index))
                    })
            }
            TabKind::Bookmarks => self
                .visible_bookmarks()
                .position(|bookmark| bookmark.post.id == view.post.id)
                .map(|index| (0, index)),
            TabKind::Search if self.search.shows_rows() => None,
            _ => self
                .config
                .visible(&tab.feed.posts)
                .position(|post| post.id == view.post.id)
                .map(|index| (0, index)),
        }
    }

    pub fn move_post(&mut self, forward: bool) {
        if self.focus != Focus::Post {
            return;
        }
        let Some(anchor) = self.post_parent_position() else {
            return;
        };
        let posts: Vec<_> = match self.active_tab().kind {
            TabKind::AllSubs => self
                .all_subs
                .groups
                .iter()
                .enumerate()
                .flat_map(|(group, entry)| {
                    self.group_posts(entry)
                        .enumerate()
                        .map(move |(index, post)| ((group, index), post))
                })
                .collect(),
            TabKind::Bookmarks => self
                .visible_bookmarks()
                .enumerate()
                .map(|(index, bookmark)| ((0, index), &bookmark.post))
                .collect(),
            _ => self
                .config
                .visible(&self.active_tab().feed.posts)
                .enumerate()
                .map(|(index, post)| ((0, index), post))
                .collect(),
        };
        let Some(at) = posts.iter().position(|(position, _)| *position == anchor) else {
            return;
        };
        let next = step(at, if forward { 1 } else { -1 }, posts.len());
        let ((group, index), post) = posts[next];
        if next == at {
            self.moving();
            self.prefetch_list(index);
            return;
        }
        let post = post.clone();
        if matches!(self.active_tab().kind, TabKind::AllSubs) {
            self.all_subs.group = group;
            self.all_subs.post = index;
        } else {
            let feed = &mut self.tabs[self.active].feed;
            feed.selected = index;
            feed.cards = CardScroll::default();
        }
        self.moving();
        self.prefetch_list(index);
        let permalink = post.permalink.clone();
        if let Some(generation) = self.show_post(post, &permalink) {
            self.focus = Focus::Post;
            self.post_settle.defer(generation, Instant::now());
        }
    }

    pub fn open_current_post(&mut self) {
        // The list must not jump to `r`'s answer away from the post being opened.
        self.cancel_reveal();
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

    pub(super) fn open_post_permalink(&mut self, post: Post, permalink: String) {
        self.cancel_reveal();
        if self.show_post(post, &permalink).is_some() {
            self.focus = Focus::Post;
            self.load_post(permalink);
        }
    }

    pub(super) fn moving(&mut self) {
        let now = Instant::now();
        self.picture_settle.defer((), now);
        self.list_prefetch = None;
        if let Some((generation, _)) = self.post_settle.waiting {
            self.post_settle.defer(generation, now);
        }
        self.cancel_reveal();
    }

    /// Applies a held first-page answer as a fresh one; a manual one shows the list top.
    pub(super) fn apply_update(&mut self, index: usize, update: FeedUpdate, manual: bool) {
        let generation = self.generation();
        let feed = &mut self.tabs[index].feed;
        feed.requests.insert(generation, None);
        if manual {
            feed.manual.insert(generation);
        }
        let tab_id = self.tabs[index].id;
        self.on_feed(
            tab_id,
            generation,
            false,
            Ok(update.listing),
            update.freshness,
            false,
        );
        self.tabs[index].feed.requests.remove(&generation);
    }

    /// Settles held answers; settling pins the island post at the selected index, so the
    /// post being read is first found by id under the current filters.
    pub(super) fn settle_keeping_island_post(&mut self, id: Option<&str>) {
        self.select_island_post(id);
        let at_top = self.island_at_top(id);
        self.settle_arrivals();
        self.select_island_post(id.filter(|_| !at_top));
    }

    /// An untouched island top, not open while All subs is shown, stays the island's top
    /// whatever an answer ranks.
    pub(super) fn island_at_top(&self, selected: Option<&str>) -> bool {
        let open = self
            .post_view
            .as_ref()
            .map(|view| view.shown_post().id.as_str());
        self.all_subs.post == 0
            && !(matches!(self.active_tab().kind, TabKind::AllSubs) && open == selected)
    }

    /// Held answers whose posts no longer lead the list apply as any other update; runs
    /// after every change to what a list shows.
    pub(super) fn settle_arrivals(&mut self) {
        for index in 0..self.tabs.len() {
            let feed = &mut self.tabs[index].feed;
            if feed.update.as_ref().is_some_and(|update| {
                !feed.leads_with_new(&update.listing.posts, &self.config)
                    && (!update.stable
                        || !feed.first_page_changed(&update.listing.posts, &self.config))
            }) && let Some(update) = feed.update.take()
            {
                self.apply_update(index, update, false);
            }
        }
        for index in 0..self.all_subs.groups.len() {
            // An island showing nothing has nothing to hold its posts behind.
            let displayed = self
                .group_posts(&self.all_subs.groups[index])
                .next()
                .is_some();
            let read = (index == self.all_subs.group)
                .then(|| {
                    self.group_posts(&self.all_subs.groups[index])
                        .nth(self.all_subs.post)
                        .cloned()
                })
                .flatten();
            let group = &mut self.all_subs.groups[index];
            let settled = group.arrivals.as_ref().is_some_and(|arrivals| {
                !displayed
                    || (!island_has_new(&self.config, &group.posts, arrivals)
                        && (!group.stable_arrivals
                            || !island_changed(&self.config, &group.posts, arrivals)))
            });
            if settled {
                group.posts = group.arrivals.take().unwrap_or_default();
                self.keep_read_post_in_slot(index, read);
            }
        }
    }

    /// Selects the post with this id in the selected island, else keeps the index
    /// within the island's shown posts.
    pub(super) fn select_island_post(&mut self, id: Option<&str>) {
        let Some(group) = self.all_subs.groups.get(self.all_subs.group) else {
            return;
        };
        let shown: Vec<_> = self.group_posts(group).collect();
        self.all_subs.post = id
            .and_then(|id| shown.iter().position(|post| post.id == id))
            .unwrap_or(self.all_subs.post)
            .min(shown.len().saturating_sub(1));
    }

    /// The post being read keeps its island slot until you move, as a list keeps the
    /// selected row; the others update around it.
    pub(super) fn keep_read_post_in_slot(&mut self, index: usize, read: Option<Post>) {
        let Some(read) = read.filter(|read| {
            matches!(self.active_tab().kind, TabKind::AllSubs)
                && !self.island_at_top(Some(&read.id))
        }) else {
            return;
        };
        let group = &mut self.all_subs.groups[index];
        let read = group
            .posts
            .iter()
            .position(|post| post.id == read.id)
            .map_or(read, |at| group.posts.remove(at));
        let at = group
            .posts
            .iter()
            .enumerate()
            .filter(|(_, post)| !self.config.hides(post) && !post.stickied)
            .map(|(at, _)| at)
            .nth(self.all_subs.post)
            .unwrap_or(group.posts.len());
        group.posts.insert(at, read);
    }

    /// Moving ends what `r` was waiting to show: later answers keep the new position.
    pub(super) fn cancel_reveal(&mut self) {
        for tab in &mut self.tabs {
            tab.feed.manual.clear();
        }
        if let Some(view) = &mut self.post_view {
            view.reveal = false;
        }
        for group in &mut self.all_subs.groups {
            group.manual = false;
        }
    }

    /// Beside an open post, the list selection takes its place as the list moves.
    pub(super) fn preview_selection(&mut self) {
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
        if let Some(generation) = self.show_post(post, &permalink) {
            let now = Instant::now();
            if self.picture_settle.holding(now) {
                self.post_settle.defer(generation, now);
            } else if self.post_settle.arrive(generation, now) {
                self.load_post(permalink);
            }
        }
    }

    /// Show `post` in the detail pane, its page not asked for yet; its view's generation,
    /// or None when it has no page.
    pub(super) fn show_post(&mut self, post: Post, permalink: &str) -> Option<u64> {
        if permalink.is_empty() {
            self.error("This post has no comments link");
            return None;
        }
        self.close_post();
        let generation = self.generation();
        self.post_view = Some(PostView {
            post,
            page: None.into(),
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
            drawn_anchor: None,
            reveal: false,
            drawn_cursor: None,
            rewrapped: None,
            rendered_document: Vec::new(),
            title_in_document: false,
            content_changed: false,
            rendered_more: Vec::new(),
            rendered_pictures: Vec::new(),
            more_handoff: false,
            image_shown: false,
            row_starts: Vec::new(),
            page_height: 0,
            reveal_spoilers: false,
            gallery_index: 0,
            comment_sort: String::new(),
        });
        Some(generation)
    }

    /// Ask for the shown post's page; it counts as read when its document is drawn.
    pub(super) fn load_post(&mut self, permalink: String) {
        let Some(view) = &mut self.post_view else {
            return;
        };
        if let Some(&generation) = self.pending_posts.get(&permalink)
            && generation > self.content_generation_floor
        {
            view.generation = generation;
            self.pool.want_post(Some(generation));
            return;
        }
        self.pending_posts
            .insert(permalink.clone(), view.generation);
        self.pool.want_post(Some(view.generation));
        self.pool.submit(Job::Post {
            generation: view.generation,
            permalink,
        });
    }
}
