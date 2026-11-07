use super::*;

impl App {
    /// The document was actually drawn; a request or an obscured page is not a read.
    pub fn post_drawn(&mut self, visible: bool) {
        if !visible || self.popup_open() || self.focus == Focus::Sidebar {
            return;
        }
        let Some(view) = &self.post_view else {
            return;
        };
        let Some(page) = view.page.as_ref() else {
            return;
        };
        if view.spoiler_hidden() {
            return;
        }
        let was_read = self.state.is_read(&page.post.id);
        self.state.mark_read(&page.post, now());
        self.read_redraw |= !was_read;
    }

    pub(super) fn ensure_feed_loaded(&mut self, index: usize) {
        if index == self.active && !self.feed_settle.holding(Instant::now()) {
            if index == SEARCH_TAB {
                self.search_arrived();
            }
            let deferred = std::mem::take(&mut self.tabs[index].feed.deferred);
            for (cursor, force) in deferred {
                let feed = &self.tabs[index].feed;
                if cursor.is_none()
                    || cursor == feed.after
                    || feed.pages.iter().any(|page| page.cursor == cursor)
                {
                    self.fetch_feed(index, cursor, force);
                }
            }
        }
        if index == self.active
            && let Some(name) = self.tabs[index].subreddit().map(str::to_string)
        {
            self.demand_icon(&name);
        }
        if index == self.active
            && let TabKind::User { name, .. } = &self.tabs[index].kind
        {
            let name = name.clone();
            self.request_profile(&name, false);
        }
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
                || tab
                    .feed
                    .update
                    .as_ref()
                    .map(|update| update.freshness)
                    .or(tab.feed.freshness)
                    .is_some_and(|f| {
                        now().saturating_sub(f.fetched_at) >= crate::cache::CONTENT_TTL
                    }))
        {
            self.load_feed(index, false);
        }
        if index == self.active {
            self.prefetch_short_home();
        }
    }

    pub(super) fn prefetch_short_home(&mut self) {
        let tab = self.active_tab();
        let feed = &tab.feed;
        if !matches!(tab.kind, TabKind::Home)
            || self.feed_settle.holding(Instant::now())
            || self.picture_settle.holding(Instant::now())
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

    pub(super) fn fetch_manual(&mut self, after: Option<String>) {
        let before = self.tabs[self.active].feed.generation;
        self.fetch_feed(self.active, after, true);
        let feed = &mut self.tabs[self.active].feed;
        if feed.generation != before {
            feed.manual.insert(feed.generation);
        }
    }

    pub(super) fn fetch_feed(&mut self, index: usize, after: Option<String>, force: bool) {
        if self.reddit.needs_sign_in() {
            return;
        }
        let generation = self.generation();
        // A first-page load ends the wait of a tab switch to this tab: once it fails, that
        // wait would ask again.
        if after.is_none() {
            if index == self.active {
                self.list_prefetch = None;
            }
            self.feed_settle.forget(self.tabs[index].id);
        }
        let tab = &mut self.tabs[index];
        let Some(target) = tab.feed_target() else {
            return;
        };
        let community = tab.subreddit().is_some();
        let feed = &mut tab.feed;
        let query = feed.query();
        if feed.request_query.as_ref() != Some(&query) {
            feed.requests.clear();
            feed.pending.clear();
            feed.refreshing.clear();
            feed.forced.clear();
            feed.manual.clear();
            feed.deferred.clear();
            feed.update = None;
            feed.failures
                .retain(|_, failure| community && failure.error.access().is_some());
            feed.request_query = Some(query.clone());
        }
        feed.generation = generation;
        if feed
            .failures
            .get(&after)
            .is_none_or(|failure| !community || failure.error.access().is_none())
        {
            feed.failures.remove(&after);
        }
        feed.requests.retain(|_, cursor| *cursor != after);
        feed.pending.retain(|id| feed.requests.contains_key(id));
        feed.refreshing.retain(|id| feed.requests.contains_key(id));
        feed.forced.retain(|id| feed.requests.contains_key(id));
        feed.manual.retain(|id| feed.requests.contains_key(id));
        feed.requests.insert(generation, after.clone());
        feed.pending.insert(generation);
        feed.deferred.remove(&after);
        if force {
            feed.forced.insert(generation);
        }
        let job = Job::Feed {
            tab: tab.id,
            generation,
            target,
            query,
            after,
        };
        self.sync_feed_demand();
        if force {
            self.pool.refresh(job);
        } else {
            self.pool.submit(job);
        }
    }

    pub(super) fn sync_feed_demand(&self) {
        let active = self
            .active_tab()
            .feed_target()
            .map(|_| self.active_tab().id);
        let mut requests: HashMap<u64, HashSet<u64>> = self
            .tabs
            .iter()
            .map(|tab| (tab.id, tab.feed.requests.keys().copied().collect()))
            .collect();
        if self.search.preview.loading {
            requests.insert(
                crate::worker::PREVIEW_FEED,
                HashSet::from([self.search.preview.generation]),
            );
        }
        self.pool.want_feeds(active, requests);
    }

    pub(super) fn retry_failed_thumbs(&mut self) {
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
        self.picture_settle.take();
        self.list_prefetch = None;
        if !self.post_focused()
            && matches!(self.active_tab().kind, TabKind::AllSubs)
            && self
                .all_subs
                .groups
                .iter()
                .any(|group| group.arrivals.is_some())
        {
            let new_posts = self.all_subs.has_new_posts(&self.config);
            for group in &mut self.all_subs.groups {
                group.apply_arrivals();
            }
            if new_posts {
                self.all_subs.show_top();
            }
            self.preview_selection();
            return;
        }
        self.icon_demands.clear();
        if let TabKind::User { name, .. } = &self.active_tab().kind
            && !self.post_focused()
        {
            let name = name.clone();
            self.request_profile(&name, true);
        }
        self.retry_failed_thumbs();
        // Only the open post's pictures are kept: its failed ones load anew.
        self.retry_failed_pictures();
        if self.post_focused() && self.post_view.is_some() {
            self.refresh_post();
            if let Some(view) = &mut self.post_view {
                view.reveal = true;
            }
            return;
        }
        match self.active_tab().kind {
            TabKind::AllSubs => {
                self.load_subscriptions(true);
                let generation = self.all_subs.next_group_generation;
                self.all_subs.next_group_generation += 1;
                for g in &mut self.all_subs.groups {
                    g.loading = false;
                    g.needs_refresh = true;
                    g.manual = true;
                    g.generation = generation;
                }
            }
            TabKind::Bookmarks => {}
            TabKind::Search if self.search.shows_rows() => self.reload_search(),
            TabKind::Home | TabKind::Subreddit { .. } | TabKind::User { .. } | TabKind::Search => {
                if let Some(update) = self.tabs[self.active].feed.update.take() {
                    self.apply_update(self.active, update, true);
                    self.prefetch_short_home();
                    self.preview_selection();
                    self.sync_feed_demand();
                    self.follow_ancestor();
                    if self.tabs[self.active].feed.error().is_none() {
                        return;
                    }
                }
                let feed = &self.tabs[self.active].feed;
                if feed.loading_more() {
                    return;
                }
                if feed.page_failed() {
                    let after = feed.after.clone();
                    self.fetch_manual(after);
                    return;
                }
                let failed_pages: Vec<_> = feed
                    .pages
                    .iter()
                    .filter(|page| {
                        page.cursor.is_some() && feed.failures.contains_key(&page.cursor)
                    })
                    .map(|page| page.cursor.clone())
                    .collect();
                self.fetch_manual(None);
                for cursor in failed_pages {
                    self.fetch_manual(cursor);
                }
            }
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
        if feed.search.is_some() {
            feed.search_time = feed.search_time.next();
            feed.selected = 0;
            feed.cards = CardScroll::default();
            self.load_feed(self.active, false);
            return;
        }
        if !feed.sort.takes_time() {
            self.info("Time range applies to Top and Controversial (press SPC f s)");
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
        self.tabs[self.active].feed.set_search(query, false);
        self.load_feed(self.active, false);
    }

    /// Only for a search feed; the `SPC f s` binding checks that.
    pub fn cycle_search_sort(&mut self) {
        let feed = &mut self.tabs[self.active].feed;
        feed.search_sort = feed.search_sort.next();
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

    pub(super) fn on_feed_fallback(
        &mut self,
        tab_id: u64,
        generation: u64,
        listing: Listing,
        cursor: String,
        error: String,
        freshness: Freshness,
    ) {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == tab_id) else {
            return;
        };
        let feed = &self.tabs[index].feed;
        if !(feed.pending.contains(&generation) || feed.refreshing.contains(&generation)) {
            return;
        }
        let retain = feed.shows_current_query()
            && feed.pages.iter().any(|page| {
                page.cursor.is_none() && (page.complete || page.source != freshness.source)
            });
        if retain {
            self.on_feed(
                tab_id,
                generation,
                false,
                Err(error.into()),
                freshness,
                false,
            );
            return;
        }
        if feed.pages.iter().any(|page| {
            page.complete
                && page.cursor.as_deref() == Some(cursor.as_str())
                && page.source.compare_requests(freshness.source)
                    == Some(std::cmp::Ordering::Greater)
        }) {
            self.on_feed(tab_id, generation, false, Ok(listing), freshness, false);
            return;
        }
        self.on_feed(tab_id, generation, false, Ok(listing), freshness, false);
        let feed = &mut self.tabs[index].feed;
        let cursor = Some(cursor);
        let source = freshness.source;
        if !feed.pages.iter().any(|page| {
            page.cursor == cursor
                && page.source.compare_requests(source) == Some(std::cmp::Ordering::Greater)
        }) && !feed.failures.get(&cursor).is_some_and(|failure| {
            failure.source.compare_requests(source) == Some(std::cmp::Ordering::Greater)
        }) {
            feed.failures.insert(
                cursor,
                FeedFailure {
                    error: error.into(),
                    source,
                },
            );
        }
    }
}
