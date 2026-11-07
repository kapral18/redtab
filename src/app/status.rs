use super::*;

impl App {
    pub(super) fn generation(&mut self) -> u64 {
        self.next_generation += 1;
        self.next_generation
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.status = Some(Status {
            text: text.into(),
            error: false,
            at: Instant::now(),
            cache_warning: false,
        });
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.status = Some(Status {
            text: text.into(),
            error: true,
            at: Instant::now(),
            cache_warning: false,
        });
    }

    /// After a draw: a cache warning the footer showed is not repeated this session.
    pub fn status_drawn(&mut self) {
        if let Some(status) = self.status.as_mut().filter(|s| s.cache_warning) {
            status.cache_warning = false;
            self.cache_warnings.insert(status.text.clone());
        }
    }

    /// Timers: held-back navigation loads, status expiry, Reddit health, saves, video
    /// frames. True when something visible changed.
    pub fn tick(&mut self) -> bool {
        self.tick_at(Instant::now())
    }

    pub(crate) fn tick_at(&mut self, now: Instant) -> bool {
        let was_visible = self.keymap.hint_visible();
        crate::keys::map::sync(self);
        let mut changed = std::mem::take(&mut self.read_redraw);
        if let Some((result, label)) = self
            .clipboard
            .as_ref()
            .and_then(|(task, label)| task.poll().map(|result| (result, *label)))
        {
            self.clipboard = None;
            match result {
                Ok(media::CopyStatus::Copied(tool)) => {
                    self.info(format!("Copied {label} ({tool})"))
                }
                Ok(media::CopyStatus::Queued) => self.info(format!("Copy queued: {label}")),
                Err(error) => self.error(format!("copy failed: {error}")),
            }
            changed = true;
        }
        changed |= was_visible && !self.keymap.hint_visible();
        changed |= self.keymap.tick(now);
        changed |= self.poll_settings(now);
        if let Some(since) = self.loading_since
            && self.loading_active()
            && now.saturating_duration_since(since) >= Duration::from_millis(100)
        {
            self.loading_frame = self.loading_frame.wrapping_add(1);
            self.loading_since = Some(now);
            changed = true;
        }
        changed |= self.load_settled(now);
        changed |= self.poll_shown_feed(now);
        changed |= self.follow_ancestor();
        changed |= self.card_renderer.poll();
        if self.picture_settle.due(now).is_some() {
            if let Some((tab, last)) = self.list_prefetch {
                if self.active_tab().id == tab && !self.popup_open() {
                    self.prefetch_list(last);
                } else {
                    self.list_prefetch = None;
                }
            }
            self.prefetch_short_home();
            changed = true;
        }
        changed |= self.sync_video(now);
        changed |= self.search_tick(now);
        if self
            .status
            .as_ref()
            .is_some_and(|s| s.at.elapsed() > STATUS_TTL)
        {
            self.status = None;
            changed = true;
        }
        let pause = self.reddit.pause();
        if pause != self.pause {
            self.pause = pause;
            changed = true;
        }
        let health = self.reddit.health();
        if health != self.health {
            self.health = health;
            changed = true;
        }
        if self.last_save.elapsed() > SAVE_INTERVAL && self.persist() {
            changed = true;
        }
        changed
    }

    pub fn images_loading(&self) -> bool {
        self.inline_images_on()
            && (self.card_renderer.images_pending()
                || self.card_images.values().any(|slot| {
                    matches!(
                        slot,
                        CardImageSlot::Loading
                            | CardImageSlot::Cached {
                                refreshing: true,
                                ..
                            }
                    )
                })
                || self
                    .images
                    .values()
                    .chain(self.thumbs.values().filter(|_| self.thumbnails_on()))
                    .any(|slot| {
                        matches!(
                            slot,
                            ImageSlot::Loading
                                | ImageSlot::Ready {
                                    refresh: ImageRefresh::Loading,
                                    ..
                                }
                        )
                    }))
    }

    pub fn loading_active(&self) -> bool {
        if self.help_scroll.is_some() {
            return false;
        }
        match self.overlay {
            Some(Overlay::Auth) => {
                return matches!(
                    self.auth.phase,
                    crate::auth::Phase::LoadingAccounts
                        | crate::auth::Phase::Switching
                        | crate::auth::Phase::Discovering
                        | crate::auth::Phase::Importing
                        | crate::auth::Phase::Saving
                        | crate::auth::Phase::Disconnecting
                );
            }
            Some(Overlay::Experience) => return self.experience.pending.is_some(),
            Some(Overlay::Settings) => return self.settings.busy(),
            Some(_) => return false,
            None => {}
        }
        if self.media_loading()
            || (self.sidebar_width() > 0 && self.all_subs.loading)
            || self.membership.is_some()
            || self.images_loading()
            || self.video_loading().is_some()
        {
            return true;
        }
        if self.post_view.as_ref().is_some_and(|view| {
            view.loading
                || (view.page.is_none() && view.error.is_none())
                || !view.loading_more.is_empty()
        }) {
            return true;
        }
        if self.post_view.is_some() && !self.detail_fits() {
            return false;
        }
        match self.active_tab().kind {
            TabKind::Search if self.search.shows_rows() => {
                self.search.rows_loading() || self.search.preview.loading
            }
            TabKind::Home | TabKind::Subreddit { .. } | TabKind::User { .. } | TabKind::Search => {
                let feed = &self.active_tab().feed;
                feed.loading()
                    || feed.refreshing()
                    || (!feed.loaded && feed.error().is_none() && !self.reddit.needs_sign_in())
            }
            TabKind::AllSubs => {
                self.all_subs.loading || self.all_subs.groups.iter().any(|g| g.loading)
            }
            TabKind::Bookmarks => false,
        }
    }

    pub fn loading_drawn(&mut self, visible: bool) {
        if visible {
            self.loading_since.get_or_insert_with(Instant::now);
        } else {
            self.loading_since = None;
        }
    }

    /// Start the load a quick run of moves held back, once the moves paused and only if
    /// its post or tab is still the one shown.
    /// Refetches the shown feed or islands once stale, so new posts reach the notice
    /// without `r`.
    pub(super) fn poll_shown_feed(&mut self, now: Instant) -> bool {
        if now.saturating_duration_since(self.feed_poll) < FEED_POLL
            || self.feed_settle.holding(now)
        {
            return false;
        }
        self.feed_poll = now;
        if matches!(self.active_tab().kind, TabKind::AllSubs) {
            let loading = self.all_subs.groups.iter().filter(|g| g.loading).count();
            for index in self.all_subs.shown.clone() {
                self.ensure_group_content(index, false);
            }
            self.all_subs.groups.iter().filter(|g| g.loading).count() != loading
        } else if self.active_tab().feed.failures.contains_key(&None) {
            // A failed first page waits for `r`, as a failed island does.
            false
        } else {
            let generation = self.active_tab().feed.generation;
            self.ensure_feed_loaded(self.active);
            self.active_tab().feed.generation != generation
        }
    }

    pub(super) fn load_settled(&mut self, now: Instant) -> bool {
        let mut changed = false;
        if let Some(generation) = self.post_settle.due(now)
            && let Some(view) = self.post_view.as_ref()
            && view.generation == generation
        {
            let permalink = view.post.permalink.clone();
            self.load_post(permalink);
            changed = true;
        }
        if let Some(tab) = self.feed_settle.due(now)
            && self.active_tab().id == tab
        {
            self.ensure_feed_loaded(self.active);
            changed = true;
        }
        changed |= self.load_settled_collection(now);
        changed
    }

    /// How long the main loop may wait for input: next video frame, resize settling, a
    /// held-back load, or the timers.
    pub fn wake_in(&self) -> Duration {
        const IDLE: Duration = Duration::from_millis(100);
        let rendering = self.card_renderer.pending();
        let media = (!self.queued_images.is_empty() || rendering || self.loading_active())
            .then_some(Duration::from_millis(16));
        let now = Instant::now();
        let settle = self.video_resize.map(|(_, since)| {
            VIDEO_RESIZE_SETTLE.saturating_sub(now.saturating_duration_since(since))
        });
        self.video
            .as_ref()
            .and_then(|p| p.wake_in(now))
            .map(|wait| {
                if rendering {
                    wait.max(Duration::from_millis(10))
                } else {
                    wait
                }
            })
            .into_iter()
            .chain(settle)
            .chain(self.card_renderer.video_wake_in(now))
            .chain(self.read_redraw.then_some(Duration::ZERO))
            .chain(media)
            .chain(self.post_settle.wake_in(now))
            .chain(self.feed_settle.wake_in(now))
            .chain(self.picture_settle.wake_in(now))
            .chain(self.search_wake_in(now))
            .chain(
                self.experience
                    .prefetch
                    .as_ref()
                    .map(|(_, at)| at.saturating_duration_since(now)),
            )
            .chain(self.keymap.wake_in(now))
            .chain(self.card_video.as_ref().and_then(|card| {
                let left = NAV_SETTLE.saturating_sub(now.saturating_duration_since(card.since));
                (!left.is_zero()).then_some(left)
            }))
            .fold(IDLE, Duration::min)
    }
}
