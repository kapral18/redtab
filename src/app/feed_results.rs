use super::*;

impl App {
    pub(super) fn on_feed_progress(
        &mut self,
        tab_id: u64,
        generation: u64,
        post: Post,
        source: Source,
    ) {
        if self.removed_posts.contains(&post.id) {
            return;
        }
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) else {
            return;
        };
        let feed = &mut tab.feed;
        let Some(cursor) = feed
            .in_flight(generation)
            .map(|request| request.cursor.clone())
        else {
            return;
        };
        let same_query = feed.shows_current_query();
        let selected = self
            .config
            .visible(&feed.posts)
            .nth(feed.selected)
            .map(|post| post.id.clone());
        let screen_row = feed.selected.saturating_sub(feed.offset);
        if !same_query {
            feed.posts.clear();
            feed.displaced_sources.clear();
            feed.pages.clear();
            feed.after = None;
            feed.freshness = None;
        }
        let observed_id = post.id.clone();
        if !feed.progress_post(cursor, post, source) {
            return;
        }
        if let Some(observed) = feed.posts.iter().find(|post| post.id == observed_id) {
            self.state.observe(
                None,
                std::slice::from_ref(observed),
                Freshness {
                    fetched_at: now(),
                    source,
                },
                now(),
            );
        }
        feed.shown_query = Some(feed.query());
        let visible: Vec<_> = self.config.visible(&feed.posts).collect();
        feed.selected = selected
            .and_then(|id| visible.iter().position(|post| post.id == id))
            .unwrap_or(if same_query { feed.selected } else { 0 })
            .min(visible.len().saturating_sub(1));
        feed.offset = feed.selected.saturating_sub(screen_row);
    }

    pub(super) fn on_feed(
        &mut self,
        tab_id: u64,
        generation: u64,
        append: bool,
        result: Result<Listing, FeedError>,
        freshness: Freshness,
        refreshing: bool,
    ) {
        if tab_id == crate::worker::PREVIEW_FEED {
            self.on_preview(generation, result, freshness, refreshing);
            return;
        }
        let active = self.tabs[self.active].id == tab_id;
        let open = self
            .post_view
            .as_ref()
            .map(|view| view.shown_post().id.clone());
        let cards = self.state.list_view() == ListView::Card;
        let mut restitch = None;
        let mut next_page = None;
        let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) else {
            return;
        };
        let name = tab.subreddit().map(str::to_owned);
        let access = match &result {
            Ok(_) => Some(None),
            Err(error) => error.access().map(Some),
        };
        let community = tab.subreddit().is_some();
        // Home pages are personalized batches, not ranks after a post.
        let ranked = !matches!(tab.kind, TabKind::Home);
        let feed = &mut tab.feed;
        if !feed.accepts(generation) {
            return;
        }
        let (forced, manual, requested) = match feed.requests.get_mut(&generation) {
            Some(request) => {
                request.phase = RequestPhase::Answered;
                let flags = (request.forced, request.manual, request.cursor.clone());
                if !refreshing {
                    request.forced = false;
                    request.manual = false;
                }
                flags
            }
            None => (false, false, None),
        };
        let cursor = if append {
            requested.or_else(|| feed.after.clone())
        } else {
            None
        };
        if result
            .as_ref()
            .is_err_and(|error| error.as_str() == crate::reddit::REQUEST_CANCELLED)
        {
            feed.requests.remove(&generation);
            feed.deferred
                .entry(cursor)
                .and_modify(|force| *force |= forced)
                .or_insert(forced);
            if active {
                let index = self.active;
                self.ensure_feed_loaded(index);
            }
            return;
        }
        match result {
            Ok(mut listing) => {
                listing
                    .posts
                    .retain(|post| !self.removed_posts.contains(&post.id));
                let mut displaced_sources = HashMap::new();
                if (cursor.is_none()
                    && feed.update.as_ref().is_some_and(|update| {
                        !freshness.source.may_replace(update.freshness.source)
                    }))
                    || (feed.shows_current_query()
                        && feed.pages.iter().any(|page| {
                            page.cursor == cursor && !freshness.source.may_replace(page.source)
                        }))
                {
                    return;
                }
                if cursor.is_none() {
                    feed.update = None;
                }
                self.state
                    .observe(name.as_deref(), &listing.posts, freshness, now());
                let leads = feed.leads_with_new(&listing.posts, &self.config);
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
                // An untouched list top stays the top, whatever the answer ranks there; a
                // post open beside it or a card scrolled into is being read.
                let at_top = feed.untouched_top(cards, active && open == selected);
                // The post being read, any selection but the untouched top, keeps its row when a
                // background answer drops it; `r` shows the list as Reddit returns it.
                let read = selected.clone().filter(|_| active && !manual && !at_top);
                let mut lost = false;
                if feed.shows_current_query()
                    && feed.access_denied().is_none()
                    && let Some(index) = feed
                        .pages
                        .iter()
                        .position(|page| page.cursor == cursor && page.complete)
                {
                    let page = &feed.pages[index];
                    let start: usize = feed.pages[..index].iter().map(|page| page.len).sum();
                    let hold = !manual && cursor.is_none() && leads;
                    let known: HashSet<_> = feed.posts.iter().map(|post| post.id.clone()).collect();
                    let lost_at = selected
                        .as_ref()
                        .filter(|id| !listing.posts.iter().any(|post| &post.id == *id))
                        .and_then(|id| {
                            feed.posts[start..start + page.len]
                                .iter()
                                .position(|post| &post.id == id)
                        });
                    lost = lost_at.is_some();
                    let dropped = lost_at
                        .filter(|_| read.is_some())
                        .map(|at| (at, feed.posts[start + at].clone()));
                    // Rows missing from a page may have moved onto the next one, loaded or
                    // requested before they did: they stay until that page answers again.
                    let next = match feed.pages.get(index + 1) {
                        Some(next) => Some(next.cursor.clone()),
                        None => page
                            .after
                            .clone()
                            .filter(|after| {
                                feed.requests
                                    .values()
                                    .any(|request| request.cursor.as_ref() == Some(after))
                            })
                            .map(Some),
                    };
                    let later =
                        ranked && listing.after.is_some() && (cursor.is_some() || next.is_some());
                    if hold || later {
                        let incoming: HashSet<_> =
                            listing.posts.iter().map(|post| post.id.as_str()).collect();
                        // Missing rows may have moved past the page boundary.
                        let displaced: Vec<_> = feed.posts[start..start + page.len]
                            .iter()
                            .filter(|post| {
                                !self.config.hides(post) && !incoming.contains(post.id.as_str())
                            })
                            .map(|post| {
                                let source = feed
                                    .displaced_sources
                                    .get(&post.id)
                                    .copied()
                                    .unwrap_or(page.source);
                                displaced_sources.insert(post.id.clone(), source);
                                post.clone()
                            })
                            .filter(|post| Some(&post.id) != read.as_ref())
                            .collect();
                        if hold {
                            feed.update = Some(FeedUpdate {
                                listing: listing.clone(),
                                freshness,
                            });
                            // Everything unseen waits with them, so nothing new shows
                            // above the rows kept below.
                            listing.posts.retain(|post| known.contains(&post.id));
                        }
                        if later {
                            next_page = next.map(|next| (next, !displaced.is_empty()));
                        }
                        listing.posts.extend(displaced);
                    }
                    if let Some((at, post)) = dropped {
                        listing.posts.insert(at.min(listing.posts.len()), post);
                    }
                }
                let failed_next = feed.page_failed().then(|| feed.after.clone()).flatten();
                if !same_query {
                    feed.posts.clear();
                    feed.displaced_sources.clear();
                    feed.pages.clear();
                }
                // A newer kept copy may stand in for a post this answer lists.
                let answered: HashSet<String> =
                    listing.posts.iter().map(|post| post.id.clone()).collect();
                if !feed.replace_page(
                    cursor.clone(),
                    listing.posts,
                    listing.after,
                    freshness.source,
                ) {
                    return;
                }
                feed.displaced_sources.extend(displaced_sources);
                if ranked
                    && let Some(index) = feed.pages.iter().position(|page| page.cursor == cursor)
                {
                    // The page before kept rows awaiting this answer: those not in it left.
                    if index > 0 && feed.pages[index - 1].after == cursor {
                        let start: usize =
                            feed.pages[..index - 1].iter().map(|page| page.len).sum();
                        let end = start + feed.pages[index - 1].len;
                        let left: Vec<_> = feed.posts[start..end]
                            .iter()
                            .filter(|post| {
                                feed.displaced_sources.contains_key(&post.id)
                                    && !answered.contains(&post.id)
                                    && Some(&post.id) != read.as_ref()
                            })
                            .map(|post| post.id.clone())
                            .collect();
                        for id in left {
                            feed.remove_post(&id);
                        }
                    }
                    // The next page, loaded or requested after this page's old end, may hold
                    // the rows kept here or no longer follow it: fetch it again from the new end.
                    if let Some((next, kept)) = next_page.take()
                        && let Some(after) = feed.pages[index].after.clone()
                        && (kept || next.as_ref() != Some(&after))
                    {
                        let after = Some(after);
                        if let Some(page) = feed.pages.get_mut(index + 1) {
                            page.cursor = after.clone();
                        }
                        feed.requests.retain(|_, request| request.cursor != next);
                        if let Some(failure) = feed.failures.remove(&next) {
                            feed.failures.insert(after.clone(), failure);
                        }
                        if let Some(deferred) = feed.deferred.remove(&next) {
                            feed.deferred.insert(after.clone(), deferred);
                        }
                        restitch = Some(after);
                    }
                }
                // Any provably newer successful page can establish access again.
                feed.failures.retain(|_, failure| {
                    !community
                        || failure.error.access().is_none()
                        || !freshness.source.may_replace(failure.source)
                });
                if !feed.failures.get(&cursor).is_some_and(|failure| {
                    (community && failure.error.access().is_some())
                        || failure.source.compare_requests(freshness.source)
                            == Some(std::cmp::Ordering::Greater)
                }) {
                    feed.failures.remove(&cursor);
                }
                if let Some(failed) = failed_next
                    && feed.after.as_ref() != Some(&failed)
                {
                    let cursor = Some(failed);
                    if feed
                        .failures
                        .get(&cursor)
                        .is_none_or(|failure| !community || failure.error.access().is_none())
                    {
                        feed.failures.remove(&cursor);
                    }
                }
                if !append {
                    feed.freshness = Some(freshness);
                }
                let visible: Vec<_> = self.config.visible(&feed.posts).collect();
                // A selection nobody reads keeps its row when its post leaves.
                let keep_row = at_top || (lost && read.is_none());
                feed.selected = selected
                    .filter(|_| !keep_row)
                    .and_then(|id| visible.iter().position(|p| p.id == id))
                    .unwrap_or(if same_query { feed.selected } else { 0 })
                    .min(visible.len().saturating_sub(1));
                feed.offset = feed.selected.saturating_sub(screen_row);
                // An untouched top, or `r` bringing new posts, shows the list from its first
                // post's first row.
                if at_top || (manual && same_query && !append && leads) {
                    feed.show_top(visible.first().map(|post| post.id.clone()));
                }
                feed.loaded = true;
                feed.shown_query = Some(feed.query());
                if !append && let Some(name) = tab.subreddit().map(str::to_string) {
                    if freshness.source.cached {
                        self.pool.cached(Job::Icon {
                            subreddit: name.to_ascii_lowercase(),
                        });
                    } else {
                        self.demand_icon(&name);
                    }
                }
            }
            Err(e) => {
                let access = community && e.access().is_some();
                // Suppress only when reservations or observed starts prove this failure
                // predates the shown page.
                if feed
                    .pages
                    .iter()
                    .filter(|page| access || page.cursor == cursor)
                    .any(|page| {
                        page.source.compare_requests(freshness.source)
                            == Some(std::cmp::Ordering::Greater)
                    })
                {
                    return;
                }
                if !community
                    || e.access().is_some()
                    || feed
                        .failures
                        .get(&cursor)
                        .is_none_or(|failure| failure.error.access().is_none())
                {
                    feed.failures.insert(
                        cursor,
                        FeedFailure {
                            error: e.clone(),
                            source: freshness.source,
                        },
                    );
                }
                // A failed load of a new query must not leave the old query's posts and cursor
                // (a continuation would append to them); a failed reload of the same query keeps them.
                if !append && feed.shown_query.as_ref() != Some(&feed.query()) {
                    feed.posts.clear();
                    feed.displaced_sources.clear();
                    feed.after = None;
                    feed.selected = 0;
                    feed.offset = 0;
                    feed.shown_query = None;
                    feed.freshness = None;
                    feed.pages.clear();
                }
                let title = tab.title();
                if e.access().is_none() || tab.subreddit().is_none() {
                    self.error(format!("{title}: {e}"));
                } else if active {
                    self.status = None;
                }
            }
        }
        if let (Some(name), Some(denied)) = (name, access) {
            self.observe_community_access(&name, denied, freshness.source);
        }
        // The new end's cached page may predate the page shown and would be refused.
        if let Some(after) = restitch
            && let Some(index) = self.tabs.iter().position(|tab| tab.id == tab_id)
        {
            self.fetch_feed(index, after, true);
        }
        // This answer may have made a held answer's new posts known.
        self.settle_arrivals();
    }
}
