use super::*;

impl App {
    pub(super) fn refresh_group(pool: &Pool, group: &mut Group, generation: u64) {
        group.loading = true;
        if group.error.as_ref().and_then(FeedError::access).is_none() {
            group.error = None;
        }
        group.generation = generation;
        pool.refresh(Job::Group {
            subreddit: group.subreddit.clone(),
            generation: group.generation,
        });
    }

    pub fn groups_drawn(
        &mut self,
        indices: &[usize],
        sidebar_icons: &[String],
        lookahead: &[usize],
    ) {
        self.all_subs.waiting_for_rows = !indices.is_empty();
        self.all_subs.shown = indices.to_vec();
        let lookahead: Vec<_> = lookahead
            .iter()
            .copied()
            .filter(|&index| {
                !indices.is_empty()
                    && self.all_subs.groups.get(index).is_some_and(|group| {
                        group.freshness.is_none()
                            && group.error.is_none()
                            && !group.needs_refresh
                            && (group.posts.is_empty() || group.stream.is_some())
                    })
            })
            .collect();
        let names: Vec<_> = indices
            .iter()
            .chain(&lookahead)
            .filter_map(|&i| self.all_subs.groups.get(i).map(|g| g.subreddit.as_str()))
            .collect();
        self.pool.set_group_demand(&names);
        for &index in indices.iter().chain(&lookahead) {
            if let Some(group) = self.all_subs.groups.get(index) {
                self.pool.want_group(&group.subreddit, group.generation);
            }
        }
        let mut icons: Vec<_> = indices
            .iter()
            .filter_map(|&index| {
                self.all_subs
                    .groups
                    .get(index)
                    .map(|group| group.subreddit.as_str())
            })
            .collect();
        icons.extend(sidebar_icons.iter().map(String::as_str));
        if let Some(name) = self.active_tab().subreddit() {
            icons.push(name);
        }
        self.pool.set_visible_icons(&icons);
        let visible: HashSet<_> = icons.iter().map(|name| name.to_ascii_lowercase()).collect();
        self.icon_demands.retain(|name| visible.contains(name));
        for &index in indices {
            self.ensure_group_loaded(index);
        }
        let visible_ready = indices.iter().all(|&index| {
            self.all_subs.groups.get(index).is_some_and(|group| {
                self.group_posts(group).next().is_some()
                    || (!group.loading && (group.freshness.is_some() || group.error.is_some()))
            })
        });
        self.all_subs.waiting_for_rows = !indices.is_empty() && !visible_ready;
        if visible_ready {
            for name in sidebar_icons {
                self.demand_icon(name);
            }
            for &index in indices {
                self.demand_group_icon(index);
            }
            for index in lookahead {
                self.ensure_group_content(index, true);
            }
        }
    }

    pub fn ensure_group_loaded(&mut self, index: usize) {
        self.ensure_group_content(index, false);
        self.demand_group_icon(index);
    }

    pub(super) fn demand_group_icon(&mut self, index: usize) {
        if let Some(name) = self
            .all_subs
            .groups
            .get(index)
            .filter(|g| g.error.as_ref().and_then(FeedError::access).is_none())
            .map(|g| g.subreddit.clone())
        {
            self.demand_icon(&name);
        }
    }

    pub(super) fn ensure_group_content(&mut self, index: usize, prefetch: bool) {
        if self.popup_open() || self.picture_settle.holding(Instant::now()) {
            return;
        }
        let Some(group) = self.all_subs.groups.get_mut(index) else {
            return;
        };
        if group.loading
            || self.reddit.needs_sign_in()
            || (!group.needs_refresh
                && (group.error.is_some()
                    || group.freshness.is_some_and(|f| {
                        now().saturating_sub(f.fetched_at) < crate::cache::CONTENT_TTL
                    })))
        {
            return;
        }
        let generation = self.all_subs.next_group_generation;
        self.all_subs.next_group_generation += 1;
        if group.stream.take().is_some() {
            group.posts.clear();
        }
        if group.needs_refresh {
            Self::refresh_group(&self.pool, group, generation);
        } else {
            group.loading = true;
            group.generation = generation;
            if prefetch {
                self.pool
                    .prefetch_group(group.subreddit.clone(), generation);
            } else {
                self.pool.submit(Job::Group {
                    subreddit: group.subreddit.clone(),
                    generation,
                });
            }
        }
    }

    pub(super) fn refresh_post(&mut self) {
        let generation = self.generation();
        let Some(view) = &mut self.post_view else {
            return;
        };
        self.post_settle.take();
        view.generation = generation;
        view.loading = true;
        view.error = None;
        view.loading_more.clear();
        self.pool.want_post(Some(generation));
        if view.comment_sort.is_empty() {
            self.pending_posts
                .insert(view.post.permalink.clone(), generation);
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
    pub(super) fn retry_failed_pictures(&mut self) {
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
            .filter(|view| view.page.is_some())
            .and_then(PostView::preview_image)
            .map(std::borrow::Cow::into_owned);
        if self.inline_images_on()
            && let Some(url) = image
        {
            self.request_image(&url);
        }
    }
}
