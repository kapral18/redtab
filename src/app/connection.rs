use super::*;

impl App {
    pub fn start(&mut self) {
        self.load_account_preferences(false);
        self.sync_feed_demand();
        if self.reddit.needs_sign_in() {
            return;
        }
        self.load_feed(0, false);
        self.load_subscriptions(false);
    }

    pub fn show_auth(&mut self) {
        if self.membership.is_some() || self.experience.writing() || self.settings.writing() {
            self.info("Wait for the pending Reddit action before reconnecting");
            return;
        }
        self.auth.active_id = self.reddit.saved_id();
        self.auth.active_name = self.reddit.account().map(|account| account.name);
        self.auth.open(self.reddit.source());
        self.overlay = Some(Overlay::Auth);
    }

    /// Returns whether the account dialog should close.
    pub fn select_auth(&mut self) -> bool {
        if self.settings.writing() {
            self.auth.message =
                Some("Wait for the pending Reddit preference save before changing accounts".into());
            return false;
        }
        if self.auth.phase == crate::auth::Phase::Accounts
            && let Some(account) = self.auth.accounts.get(self.auth.selected)
        {
            if self.auth.active_id.as_ref() == Some(&account.id)
                && self
                    .reddit
                    .account()
                    .is_some_and(|current| current.id == account.id)
                && !self.reddit.needs_sign_in()
            {
                self.auth.cancel();
                return true;
            }
            if let Err(error) = self.save_before_switch() {
                self.auth.message = Some(format!(
                    "Could not save the current reading state; connection unchanged: {error}"
                ));
                return false;
            }
        }
        self.auth.select(self.config.show_nsfw);
        false
    }

    pub fn poll_auth(&mut self) -> bool {
        self.behind_settings(Self::poll_auth_background)
    }

    pub(super) fn poll_auth_background(&mut self) -> bool {
        let (changed, connection) = self.auth.poll(self.config.show_nsfw);
        if let Some(connection) = connection {
            self.install_connection(connection);
        }
        changed
    }

    pub fn confirm_auth(&mut self) {
        if self.settings.writing() {
            self.auth.message =
                Some("Wait for the pending Reddit preference save before changing accounts".into());
            return;
        }
        use crate::auth::Phase;
        if !matches!(self.auth.phase, Phase::Confirm | Phase::DisconnectConfirm) {
            return;
        }
        match self.save_before_switch() {
            Ok(()) if self.auth.phase == Phase::DisconnectConfirm => {
                self.auth.disconnect(self.config.show_nsfw)
            }
            Ok(()) => self.auth.confirm(),
            Err(error) => {
                self.auth.message = Some(format!(
                    "Could not save the current reading state; connection unchanged: {error}"
                ))
            }
        }
    }

    pub fn take_replacement_messages(&mut self) -> Option<std::sync::mpsc::Receiver<Msg>> {
        self.replacement_messages.take()
    }

    /// A state file this session cannot write does not hold the connection change back.
    fn save_before_switch(&mut self) -> anyhow::Result<()> {
        if self.state.read_only() {
            return Ok(());
        }
        self.persist_checked().map(drop)
    }

    pub(super) fn install_connection(&mut self, reddit: Reddit) {
        // Reads and marks observed while the switch ran belong to the account being left.
        if !self.state.read_only() {
            self.persist();
        }
        self.ancestor_follow = None;
        let disconnected = reddit.needs_sign_in();
        let same_account = !disconnected
            && match self.reddit.account().zip(reddit.account()) {
                Some((old, new)) => old.id == new.id,
                None => self.reddit.session_fingerprint() == reddit.session_fingerprint(),
            };
        let account = reddit.account();
        let same_settings_scope = same_account
            && self
                .config
                .active_account
                .as_deref()
                .zip(account.as_ref())
                .is_some_and(|(name, account)| name.eq_ignore_ascii_case(&account.name));
        let show_nsfw = same_settings_scope && self.config.show_nsfw;
        // Content read under another adult-content policy is not kept, as on an F2 change.
        let keep_app = same_account && show_nsfw == self.config.show_nsfw;
        let reddit = reddit.with_nsfw(show_nsfw);
        let confirmed = same_settings_scope
            .then(|| self.settings.account.values.take())
            .flatten();
        self.settings.account = settings::AccountSettings::default();
        self.settings.account.values = confirmed;
        if !same_settings_scope
            && self.settings.editing.is_some()
            && crate::config::schema::SETTINGS[self.settings.selected].scope
                != crate::config::schema::Scope::Device
        {
            self.settings.editing = None;
            self.settings.message =
                Some("Account changed; the previous account's draft was not applied".into());
        }
        self.settings.store.select_account(
            account
                .as_ref()
                .map(|a| a.name.as_str())
                .or_else(|| (!disconnected).then_some("")),
        );
        let mut config = self.settings.store.current.clone();
        config.show_nsfw = show_nsfw;
        config.media.video_autoplay = if account.is_none() {
            crate::config::MediaSettings::default().video_autoplay
        } else {
            same_settings_scope && self.config.media.video_autoplay
        };
        self.pool.stop();
        self.reddit.retire();
        let (pool, messages) = self.pool.restart(reddit.clone());
        let root = if self
            .state_path
            .parent()
            .is_some_and(|p| p.ends_with("signed-out"))
        {
            self.state_path
                .parent()
                .and_then(|p| p.parent())
                .unwrap()
                .to_path_buf()
        } else if self
            .state_path
            .parent()
            .and_then(|p| p.parent())
            .is_some_and(|p| p.ends_with("accounts"))
        {
            self.state_path
                .parent()
                .and_then(|p| p.parent())
                .and_then(|p| p.parent())
                .unwrap()
                .to_path_buf()
        } else {
            self.state_path
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .to_path_buf()
        };
        let state_path = if disconnected {
            root.join("signed-out/state.json")
        } else {
            crate::auth::account_state_path(&root, reddit.account().as_ref())
        };
        let mut state_warning = None;
        if let Some(parent) = state_path.parent() {
            use std::os::unix::fs::DirBuilderExt;
            if let Err(error) = std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)
            {
                state_warning = Some(format!(
                    "Could not create account reading-state directory: {error}"
                ));
            }
        }
        if keep_app {
            self.config = config;
            if self
                .post_view
                .as_ref()
                .is_some_and(|v| self.config.hides(v.shown_post()))
            {
                self.close_post();
            }
            self.pool = pool;
            self.reddit = reddit;
            // A moved state file merges in at once: its bookmarks show now, not at the
            // next periodic save.
            let moved = state_path != self.state_path;
            self.state_path = state_path;
            if moved {
                self.state.move_to_new_file();
                // Reported with the connection message, which would replace a status.
                if let Err(error) = self.persist_checked() {
                    state_warning =
                        state_warning.or_else(|| Some(format!("cannot save state: {error:#}")));
                }
            }
            self.reset_pending_loads();
            self.reset_search_loads();
            self.overlay = None;
            self.start();
            self.ensure_feed_loaded(self.active);
            if self.post_view.is_some() {
                self.refresh_post();
            }
        } else {
            let image_picker = self.image_picker.clone();
            let mut next = Self::new(config, pool, reddit, state_path, image_picker);
            state_warning = state_warning.or_else(|| {
                next.status
                    .as_ref()
                    .filter(|s| s.error)
                    .map(|s| s.text.clone())
            });
            std::mem::swap(&mut next.settings, &mut self.settings);
            next.no_color = self.no_color;
            next.show_images = self.show_images;
            next.show_sidebar = self.show_sidebar;
            next.width = self.width;
            next.help_scroll = self.help_scroll;
            next.start();
            *self = next;
        }
        self.replacement_messages = Some(messages);
        if disconnected {
            self.health = Health::SignedOut;
            if let Some(warning) = state_warning {
                self.error(format!("Disconnected from Reddit; {warning}"));
            } else {
                self.info("Disconnected from Reddit. Press SPC p a to connect.");
            }
            return;
        }
        let name = self.reddit.account().map(|a| a.name).unwrap_or_default();
        if let Some(warning) = state_warning {
            self.error(format!("Connected as u/{name}; {warning}"));
        } else {
            self.info(format!("Connected as u/{name}"));
        }
    }

    pub(super) fn reset_pending_loads(&mut self) {
        self.collections.clear();
        self.clear_experience();
        self.membership = None;
        self.all_subs.loading = false;
        self.all_subs.error = None;
        let generation = self.all_subs.next_group_generation;
        self.all_subs.next_group_generation += 1;
        for group in &mut self.all_subs.groups {
            if group.loading || group.error.is_some() || group.freshness.is_none() {
                group.invalidate(generation);
            }
        }
        let mut feeds = Vec::new();
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            let community = tab.subreddit().is_some();
            // A held answer is dropped with the pool: its first page is asked for again.
            let held = tab.feed.update.take().is_some();
            let mut cursors: HashSet<_> = tab
                .feed
                .requests
                .values()
                .filter(|request| request.in_flight())
                .map(|request| &request.cursor)
                .chain(tab.feed.failures.keys())
                .chain(tab.feed.deferred.keys())
                .cloned()
                .collect();
            if held {
                cursors.insert(None);
            }
            feeds.extend(cursors.into_iter().map(|cursor| (index, cursor)));
            tab.feed.requests.clear();
            tab.feed.deferred.clear();
            tab.feed
                .failures
                .retain(|_, failure| community && failure.error.access().is_some());
        }
        for (index, cursor) in feeds {
            // start() reloads Home's first page after the pool handoff.
            if index != 0 || cursor.is_some() {
                self.fetch_feed(index, cursor, false);
            }
        }
        if let Some(view) = &mut self.post_view {
            view.loading = false;
            view.loading_more.clear();
        }
        self.pending_media.clear();
        self.pending_posts.clear();
        self.queued_images.clear();
        self.icon_lookups.clear();
        self.icon_demands.clear();
        self.profile_lookups.clear();
        // The old pool's refreshes never answer: the next draw asks again.
        for slots in [&mut self.images, &mut self.thumbs] {
            slots.retain(|_, slot| matches!(slot, ImageSlot::Ready { .. }));
            for slot in slots.values_mut() {
                if let ImageSlot::Ready { refresh, .. } = slot
                    && *refresh == ImageRefresh::Loading
                {
                    *refresh = ImageRefresh::Needed;
                }
            }
        }
        self.card_images.retain(|_, slot| {
            matches!(
                slot,
                CardImageSlot::Ready { .. } | CardImageSlot::Cached { .. }
            )
        });
        for slot in self.card_images.values_mut() {
            if let CardImageSlot::Cached { refreshing, .. } = slot {
                *refreshing = false;
            }
        }
        self.video = None;
        self.card_video = None;
        self.video_requested = None;
        self.video_previews.clear();
        self.post_settle = Settle::default();
        self.feed_settle = Settle::default();
        self.picture_settle = Settle::default();
        self.list_prefetch = None;
    }
}
