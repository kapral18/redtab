use super::*;

impl App {
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

    /// After `r` the next request skips the cached copy, even when the rows reload first.
    pub(super) fn request_preview(&mut self) {
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
    pub(in crate::app) fn on_preview(
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

    pub(super) fn preview_matches_feed(&self, index: usize) -> bool {
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

    pub(in crate::app) fn apply_search_preview(&mut self, index: usize) {
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

    pub(super) fn sync_search_preview(&mut self, index: usize) {
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
}
