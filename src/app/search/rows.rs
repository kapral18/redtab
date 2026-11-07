use super::*;

impl App {
    pub(super) fn schedule_search(&mut self, now: Instant) {
        self.search.due = Some(now + DEBOUNCE);
        // The query or kind changed: the shown query's empty answer is not asked again,
        // and the same text, typed back, is searched afresh.
        if self.search.empty_retry.take().is_some() {
            self.search.searched = None;
        }
        self.search.preview_forced = false;
    }

    pub(super) fn schedule_preview(&mut self, now: Instant) {
        if self.search.shows_rows() {
            self.search.preview.due = Some(now + PREVIEW_SETTLE);
        }
    }

    /// Fires a debounced search or preview whose pause has passed.
    pub(in crate::app) fn search_tick(&mut self, now: Instant) -> bool {
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
        if let Some(retry) = &mut self.search.empty_retry
            && retry.due.is_some_and(|due| now >= due)
        {
            retry.due = None;
            if let Some(query) = self.search.results.query.clone() {
                self.request_rows(false, query, None);
            }
            changed = true;
        }
        changed
    }

    pub(in crate::app) fn search_wake_in(&self, now: Instant) -> Option<Duration> {
        [
            self.search.due,
            self.search.preview.due,
            self.search.empty_retry.as_ref().and_then(|retry| retry.due),
        ]
        .into_iter()
        .flatten()
        .map(|due| due.saturating_duration_since(now))
        .min()
    }

    /// Returns false when the pending text and kind are what the shown results already answer.
    pub(super) fn run_search(&mut self) -> bool {
        self.search.due = None;
        self.search.empty_retry = None;
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
            SearchKind::Posts | SearchKind::Media => {
                self.search.results = Rows::default();
                let media = kind == SearchKind::Media;
                if self.tabs[SEARCH_TAB].feed.search_media != media {
                    self.clear_search_feed();
                }
                let feed = &mut self.tabs[SEARCH_TAB].feed;
                let kept = (feed.search_sort, feed.search_time);
                feed.set_search(text.clone(), media);
                if self.search.posts_text.as_deref() == Some(text.as_str()) {
                    (feed.search_sort, feed.search_time) = kept;
                }
                self.search.posts_text = Some(text);
                self.load_feed(SEARCH_TAB, false);
                return true;
            }
            SearchKind::Communities => ListSearch::Communities,
            SearchKind::Comments => ListSearch::Comments,
            SearchKind::People => ListSearch::People,
        };
        self.clear_search_feed();
        self.request_rows(false, Query::Search { text, kind: list }, None);
        true
    }

    /// Asks for result or discovery rows; a cursor adds the next page below the shown ones.
    pub(super) fn request_rows(&mut self, discovery: bool, query: Query, after: Option<String>) {
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

    pub(super) fn load_more_rows(&mut self) {
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

    /// Rows replace post results: a fresh feed also drops a post request still in flight.
    pub(super) fn clear_search_feed(&mut self) {
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

    pub(super) fn ensure_discovered(&mut self) {
        if self.resume_search_rows(true) {
            return;
        }
        // Asked once; a failed or empty answer waits for `r`, which resets the rows.
        if self.search.discovered.generation != 0 {
            return;
        }
        self.request_rows(true, Query::Communities, None);
    }

    pub(in crate::app) fn on_search(&mut self, generation: u64, result: Result<Response, String>) {
        let results = &self.search.results;
        if results.generation == generation
            && results.loading
            && !results.appending
            && matches!(results.query, Some(Query::Search { .. }))
            && let Ok(Response::Page(page)) = &result
            && page.entries.is_empty()
            && page.after.is_none()
        {
            let attempt = self
                .search
                .empty_retry
                .as_ref()
                .map_or(0, |retry| retry.attempt);
            if let Some(delay) = EMPTY_RETRY_DELAYS.get(attempt) {
                self.search.results.loading = false;
                self.search.empty_retry = Some(EmptyRetry {
                    attempt: attempt + 1,
                    due: Some(Instant::now() + *delay),
                });
                return;
            }
        }
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
        match result {
            Ok(Response::Page(page)) => {
                icons = page
                    .entries
                    .iter()
                    .filter_map(|entry| {
                        let name = match (&entry.query, &entry.target) {
                            (Some(Query::Community(name)), _) => name,
                            (Some(Query::Thread(_)), Some(target)) => &target.subreddit,
                            _ => return None,
                        };
                        match &entry.icon {
                            SubscriptionIcon::Url(url) => Some((name.clone(), Some(url.clone()))),
                            SubscriptionIcon::Missing => Some((name.clone(), None)),
                            _ => None,
                        }
                    })
                    .collect();
                let entries: Vec<Row> = page.entries.iter().filter_map(row).collect();
                let entries = entries.into_iter();
                if appending {
                    for entry in entries {
                        if !rows.entries.iter().any(|known| {
                            known.target == entry.target
                                && known.comment.as_ref().map(|c| &c.permalink)
                                    == entry.comment.as_ref().map(|c| &c.permalink)
                        }) {
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
    pub(in crate::app) fn reload_search(&mut self) {
        let row = self.search.selected_row();
        let comment = row.as_ref().is_some_and(|row| row.comment.is_some());
        let current = !comment
            && row.is_some_and(|row| self.search.preview.target.as_ref() == Some(&row.target));
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
        self.search.preview_forced = !comment;
        self.request_preview();
    }

    /// A restarted worker pool drops in-flight answers; ask again for what the tab shows.
    pub(in crate::app) fn reset_search_loads(&mut self) {
        self.search.opened_feed = None;
        self.search.results = Rows::default();
        self.search.discovered = Rows::default();
        self.search.preview = Preview::default();
        // Post results keep their pages: reset_pending_loads already asked for them again.
        if !self.search.kind.feed() || self.search.text().is_empty() {
            self.search.searched = None;
        }
        self.search.empty_retry = None;
        self.search.preview_forced = false;
        self.search.due =
            (self.active == SEARCH_TAB && !self.search.exploring() && !self.search.kind.feed())
                .then(Instant::now);
    }
}
