use super::*;

impl App {
    pub(super) fn on_group_progress(
        &mut self,
        subreddit: &str,
        generation: u64,
        post: Post,
        source: Source,
    ) {
        if self.removed_posts.contains(&post.id) {
            return;
        }
        let Some(group) = self
            .all_subs
            .groups
            .iter_mut()
            .find(|group| group.subreddit == subreddit && group.generation == generation)
        else {
            return;
        };
        if !group.loading
            || group.freshness.is_some()
            || (group.stream.is_none() && !group.posts.is_empty())
            || group
                .stream
                .is_some_and(|previous| !source.may_replace(previous))
        {
            return;
        }
        self.state.observe(
            None,
            std::slice::from_ref(&post),
            Freshness {
                fetched_at: now(),
                source,
            },
            now(),
        );
        group.stream = Some(source);
        if let Some(previous) = group
            .posts
            .iter_mut()
            .find(|previous| previous.id == post.id)
        {
            *previous = post;
        } else {
            group.posts.push(post);
        }
    }

    pub(super) fn on_group(
        &mut self,
        subreddit: &str,
        generation: u64,
        result: Result<Listing, FeedError>,
        freshness: Freshness,
        refreshing: bool,
    ) {
        let Some(index) = self
            .all_subs
            .groups
            .iter()
            .position(|g| g.subreddit == subreddit && g.generation == generation)
        else {
            return;
        };
        if result
            .as_ref()
            .err()
            .is_some_and(|error| error.as_str() == crate::reddit::REQUEST_CANCELLED)
        {
            self.all_subs.groups[index].loading = false;
            return;
        }
        let access = match &result {
            Ok(_) => Some(None),
            Err(error) => error.access().map(Some),
        };
        // An island showing nothing, empty or refused, has nothing new to hold back.
        let displayed = self
            .group_posts(&self.all_subs.groups[index])
            .next()
            .is_some();
        let selected_post = (index == self.all_subs.group)
            .then(|| {
                self.group_posts(&self.all_subs.groups[index])
                    .nth(self.all_subs.post)
                    .cloned()
            })
            .flatten();
        let selected = selected_post.as_ref().map(|post| post.id.clone());
        let at_top = self.island_at_top(selected.as_deref());
        let group = &mut self.all_subs.groups[index];
        group.loading = refreshing;
        let manual = group.manual;
        if !freshness.source.cached {
            group.manual = false;
        }
        if result.is_ok()
            && group
                .freshness
                .map(|shown| shown.source)
                .or(group.stream)
                .is_some_and(|shown| !freshness.source.may_replace(shown))
        {
            return;
        }
        match result {
            Ok(mut listing) => {
                group.freshness = Some(freshness);
                group.stream = None;
                if !freshness.source.cached {
                    group.needs_refresh = false;
                }
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
                listing
                    .posts
                    .retain(|post| !self.removed_posts.contains(&post.id));
                let incoming = if seeded_tab.is_some() {
                    listing.posts.clone()
                } else {
                    std::mem::take(&mut listing.posts)
                };
                let held = displayed && island_has_new(&self.config, &group.posts, &incoming);
                let revealed = manual && held;
                if !held || manual {
                    group.posts = incoming;
                    group.arrivals = None;
                } else {
                    // The island keeps its posts and order until `r`; content still updates.
                    let fresh: HashMap<_, _> = incoming
                        .iter()
                        .map(|post| (post.id.as_str(), post))
                        .collect();
                    for post in &mut group.posts {
                        if let Some(fresh) = fresh.get(post.id.as_str()) {
                            *post = (*fresh).clone();
                        }
                    }
                    group.arrivals = Some(incoming);
                }
                group.error = None;
                self.state.observe(
                    Some(subreddit),
                    group.arrivals.as_ref().unwrap_or(&group.posts),
                    freshness,
                    now(),
                );
                if freshness.source.cached {
                    self.pool.cached(Job::Icon {
                        subreddit: subreddit.to_ascii_lowercase(),
                    });
                } else {
                    self.demand_icon(subreddit);
                }
                if !held && !manual {
                    self.keep_read_post_in_slot(index, selected_post);
                }
                if revealed {
                    self.all_subs.show_top();
                } else if index == self.all_subs.group {
                    // A refresh can return fewer posts: keep the selection on one.
                    self.select_island_post(selected.as_deref().filter(|_| !at_top));
                }
                if let Some(i) = seeded_tab {
                    let generation = self.generation();
                    let tab = &mut self.tabs[i];
                    let feed = &mut tab.feed;
                    // Pending first pages stay: a newer one replaces this response; an
                    // older one is rejected by its source.
                    feed.requests
                        .retain(|_, request| request.cursor.is_some() || request.in_flight());
                    for request in feed.requests.values_mut() {
                        if request.cursor.is_none() && request.phase == RequestPhase::Pending {
                            request.phase = RequestPhase::Refreshing;
                        }
                    }
                    feed.generation = generation;
                    feed.requests
                        .insert(generation, FeedRequest::new(None, RequestPhase::Answered));
                    feed.request_query = Some(query);
                    let tab_id = tab.id;
                    self.on_feed(tab_id, generation, false, Ok(listing), freshness, false);
                }
            }
            Err(e) => {
                group.needs_refresh = false;
                if e.access().is_some()
                    || group.error.as_ref().and_then(FeedError::access).is_none()
                {
                    group.error = Some(e);
                }
            }
        }
        if let Some(denied) = access {
            self.observe_community_access(subreddit, denied, freshness.source);
        }
    }
}
