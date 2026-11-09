use super::*;

impl App {
    /// Async completions update the covered dialog without taking Settings' keys.
    pub(in crate::app) fn behind_settings<T>(&mut self, update: impl FnOnce(&mut Self) -> T) -> T {
        let covered = matches!(self.overlay, Some(Overlay::Settings));
        if covered {
            self.overlay = self.settings.return_overlay.take();
        }
        let result = update(self);
        if covered {
            self.settings.return_overlay = self.overlay.take();
            self.overlay = Some(Overlay::Settings);
        }
        result
    }

    pub(super) fn settings_allowed(&mut self) -> bool {
        if self.settings.managed.is_some() {
            self.info("Finish reviewing or saving the managed config first");
            return false;
        }
        if self.membership.is_some() || self.experience.writing() || self.settings.writing() {
            self.info("Wait for the pending Reddit action before changing settings");
            false
        } else {
            true
        }
    }

    pub fn show_settings(&mut self) {
        if !self.settings_allowed() {
            return;
        }
        self.settings.store.reload();
        self.apply_settings();
        self.settings.return_overlay = self.overlay.take();
        self.overlay = Some(Overlay::Settings);
        self.load_account_preferences(true);
        self.begin_managed_save();
    }

    pub fn change_setting(&mut self, key: &str, value: toml::Value) {
        if !self.settings_allowed() {
            return;
        }
        let field = SETTINGS
            .iter()
            .find(|field| field.key == key)
            .expect("known setting");
        if field.scope == Scope::Reddit {
            self.change_account_preference(field.key, value);
            return;
        }
        match self.settings.store.change(field, value) {
            Ok(()) => {
                self.settings.message = None;
                self.apply_settings();
                if self.show_images != self.config.media.image_preview {
                    self.apply_image_setting();
                }
                self.begin_managed_save();
            }
            Err(error) => self.settings.message = Some(error.to_string()),
        }
    }

    pub(in crate::app) fn poll_settings(&mut self, now: Instant) -> bool {
        let account_changed = self.poll_account_preferences();
        if self.settings.managed.is_some() {
            return self.poll_managed_save() || account_changed;
        }
        if now.saturating_duration_since(self.settings.last_poll) < Duration::from_millis(500)
            || self.membership.is_some()
            || self.experience.writing()
            || self.settings.writing()
        {
            return account_changed;
        }
        self.settings.last_poll = now;
        let before = (
            self.settings.store.persistence,
            self.settings.store.error.clone(),
        );
        self.settings.store.reload();
        let changed = self.apply_settings();
        changed
            || account_changed
            || before
                != (
                    self.settings.store.persistence,
                    self.settings.store.error.clone(),
                )
    }

    pub(super) fn begin_managed_save(&mut self) {
        if !matches!(self.overlay, Some(Overlay::Settings)) || self.settings.managed.is_some() {
            return;
        }
        if let Some(request) = self.settings.store.managed_request() {
            let tool = self.settings.chezmoi.clone();
            self.settings.managed = Some(ManagedSave::Checking(Task::start(move || {
                request.discover(tool)
            })));
            self.settings.managed_scroll = 0;
        }
    }

    pub(super) fn poll_managed_save(&mut self) -> bool {
        let state = self.settings.managed.take().unwrap();
        match state {
            ManagedSave::Checking(task) => match task.poll() {
                Some(Ok(plan)) => {
                    self.settings.managed = plan.map(|plan| ManagedSave::Confirm(Box::new(plan)));
                    true
                }
                Some(Err(error)) => {
                    self.settings.message = Some(error);
                    true
                }
                None => {
                    self.settings.managed = Some(ManagedSave::Checking(task));
                    false
                }
            },
            ManagedSave::Applying(task, verification) => match task.poll() {
                Some(result) => {
                    self.settings
                        .store
                        .managed_finished(verification, result.map_err(anyhow::Error::msg));
                    self.apply_settings();
                    true
                }
                None => {
                    self.settings.managed = Some(ManagedSave::Applying(task, verification));
                    false
                }
            },
            state @ ManagedSave::Confirm(_) => {
                self.settings.managed = Some(state);
                false
            }
        }
    }

    pub(in crate::app) fn apply_settings(&mut self) -> bool {
        let mut next = self.settings.store.current.clone();
        if let Some(values) = &self.settings.account.values {
            next.show_nsfw = values.show_nsfw;
            next.media.video_autoplay = values.video_autoplay;
        } else {
            next.show_nsfw = self.config.show_nsfw;
            next.media.video_autoplay = self.config.media.video_autoplay;
        }
        if self.config == next {
            return false;
        }
        let had_post = self.post_view.is_some();
        let nsfw_changed = self.config.show_nsfw != next.show_nsfw;
        let images_changed = self.config.media.image_preview != next.media.image_preview;
        let filters_changed = self.config.filters != next.filters || nsfw_changed;
        let count_changed = self.config.all_subs_posts_per_group != next.all_subs_posts_per_group;
        let autoplay_stopped = self.config.media.video_autoplay && !next.media.video_autoplay;
        let selected: Vec<_> = self
            .tabs
            .iter()
            .map(|tab| {
                if matches!(tab.kind, TabKind::Bookmarks) {
                    self.visible_bookmarks()
                        .nth(tab.feed.selected)
                        .map(|b| b.post.id.clone())
                } else {
                    self.config
                        .visible(&tab.feed.posts)
                        .nth(tab.feed.selected)
                        .map(|p| p.id.clone())
                }
            })
            .collect();
        let group_selected = self
            .all_subs
            .groups
            .get(self.all_subs.group)
            .and_then(|g| self.group_posts(g).nth(self.all_subs.post))
            .map(|p| p.id.clone());
        self.config = next;
        if nsfw_changed {
            self.reload_content_policy();
        }
        if images_changed {
            self.apply_image_setting();
        }
        if autoplay_stopped {
            self.video = None;
            self.video_requested = None;
            self.video_previews.clear();
        }
        if filters_changed || count_changed {
            let bookmarks = self.bookmark_ids();
            for (index, tab) in self.tabs.iter_mut().enumerate() {
                let ids: Vec<_> = if matches!(tab.kind, TabKind::Bookmarks) {
                    bookmarks.clone()
                } else {
                    self.config
                        .visible(&tab.feed.posts)
                        .map(|p| p.id.clone())
                        .collect()
                };
                tab.feed.selected = selected[index]
                    .as_ref()
                    .and_then(|id| ids.iter().position(|p| p == id))
                    .unwrap_or(tab.feed.selected)
                    .min(ids.len().saturating_sub(1));
                tab.feed.offset = tab.feed.offset.min(tab.feed.selected);
                tab.feed.cards = CardScroll::default();
            }
            self.settle_keeping_island_post(group_selected.as_deref());
            if self
                .post_view
                .as_ref()
                .is_some_and(|view| self.config.hides(view.shown_post()))
            {
                self.close_post();
                self.video = None;
                self.video_requested = None;
            }
        }
        if nsfw_changed || (had_post && self.post_view.is_none()) {
            self.reset_experience_reads();
        }
        true
    }

    pub(super) fn reset_experience_reads(&mut self) {
        self.collections.clear();
        self.experience.reset_reads(self.post_view.is_some());
        self.pool.want_experience(None);
        if self.experience.screens.is_empty() {
            if matches!(self.overlay, Some(Overlay::Experience)) {
                self.overlay = None;
            }
            if matches!(self.settings.return_overlay, Some(Overlay::Experience)) {
                self.settings.return_overlay = None;
            }
        }
    }

    pub(super) fn reload_content_policy(&mut self) {
        // A policy-only client shares credentials and health; retiring it would retire both.
        let reddit = self.reddit.with_nsfw(self.config.show_nsfw);
        self.pool.stop();
        let (pool, messages) = self.pool.restart(reddit.clone());
        self.pool = pool;
        self.reddit = reddit;
        self.replacement_messages = Some(messages);
        let focus = self.focus;
        let detail = self
            .post_view
            .take()
            .filter(|view| self.config.show_nsfw || !view.shown_post().nsfw);
        self.close_post();
        self.post_view = detail;
        if self.post_view.is_some() {
            self.focus = focus;
        }
        self.video = None;
        self.video_requested = None;
        self.video_previews.clear();
        self.card_video = None;
        self.pending_media.clear();
        self.queued_images.clear();
        self.icon_lookups.clear();
        // Profiles were read under the old policy; shown tabs ask again.
        self.profiles.clear();
        self.profile_lookups.clear();
        self.icon_demands.clear();
        self.images.clear();
        self.thumbs.clear();
        self.card_images.clear();
        self.card_previews.clear();
        self.card_image_order.clear();
        self.card_preview_order.clear();
        self.card_renderer.clear();
        for tab in &mut self.tabs {
            let old = &mut tab.feed;
            *old = Feed {
                sort: old.sort,
                time: old.time,
                search: old.search.take(),
                search_media: old.search_media,
                search_sort: old.search_sort,
                search_time: old.search_time,
                ..Feed::default()
            };
        }
        for group in &mut self.all_subs.groups {
            *group = Group {
                subreddit: group.subreddit.clone(),
                ..Group::default()
            };
        }
        // Fresh islands keep what is known about private or banned communities.
        self.sync_community_access();
        self.all_subs.loading = false;
        self.all_subs.error = None;
        self.all_subs.generation += 1;
        self.reset_search_loads();
        // Requests of the stopped pool never answer, as on an account switch.
        self.pending_posts.clear();
        if let Some(view) = &mut self.post_view {
            view.loading = false;
            view.loading_more.clear();
        }
        self.post_settle = super::super::Settle::default();
        self.feed_settle = super::super::Settle::default();
        if !self.reddit.needs_sign_in() {
            if self.post_view.is_some() {
                self.refresh_post();
            }
            self.start();
            if self.active != 0 && !matches!(self.active_tab().kind, TabKind::Bookmarks) {
                self.ensure_feed_loaded(self.active);
            }
        }
    }
}
