use super::*;

impl App {
    pub fn toggle_list_view(&mut self) {
        for tab in &mut self.tabs {
            tab.feed.cards = CardScroll::default();
        }
        self.state.toggle_list_view();
        self.persist();
    }

    pub fn toggle_images(&mut self) {
        if self.no_images_possible() {
            self.info("No images: NO_COLOR is set and this terminal has no image protocol");
            return;
        }
        self.change_setting(
            "media.image_preview",
            toml::Value::Boolean(!self.show_images),
        );
    }

    pub(super) fn apply_image_setting(&mut self) {
        self.show_images = self.config.media.image_preview;
        if !self.show_images {
            self.video_previews.clear();
        }
        self.reload_image_sources();
        self.info(if self.show_images {
            "Images on"
        } else {
            "Images off"
        });
    }

    /// The pictures the current screen shows are fetched again for the current settings.
    pub(super) fn reload_image_sources(&mut self) {
        self.icon_demands.clear();
        // Icons are looked up only while thumbnails show: fetch the ones skipped meanwhile.
        let names: Vec<String> = (self.tabs.iter().filter_map(Tab::subreddit))
            .chain(self.all_subs.groups.iter().map(|g| g.subreddit.as_str()))
            .map(str::to_string)
            .collect();
        for name in names {
            self.pool.cached(Job::Icon {
                subreddit: name.to_ascii_lowercase(),
            });
        }
        let selected = self
            .active_tab()
            .subreddit()
            .map(str::to_string)
            .or_else(|| {
                matches!(self.active_tab().kind, TabKind::AllSubs)
                    .then(|| {
                        self.all_subs
                            .groups
                            .get(self.all_subs.group)
                            .map(|g| g.subreddit.clone())
                    })
                    .flatten()
            });
        if let Some(name) = selected {
            self.demand_icon(&name);
        }
        if self.inline_images_on()
            && let Some(view) = &mut self.post_view
            && let Some(page) = view.page.as_ref()
        {
            // Turned on at the post's top: show the image now; same-pass changes treat the
            // top as showing it.
            let media = &page.post.media;
            let url = view.preview_image().map(std::borrow::Cow::into_owned);
            if view.scroll == 0 {
                view.image_shown = media.shows_inline() && !view.spoiler_hidden();
            }
            if let Some(url) = url {
                self.request_image(&url);
            }
        }
    }

    pub fn post_focused(&self) -> bool {
        self.focus == Focus::Post && self.post_view.is_some()
    }

    pub fn popup_open(&self) -> bool {
        self.overlay.is_some() || self.help_scroll.is_some() || self.keymap.hint_visible()
    }

    pub fn sidebar_width(&self) -> u16 {
        if !self.show_sidebar {
            0
        } else if self.width >= 110 {
            SIDEBAR_WIDE
        } else {
            SIDEBAR_NARROW
        }
    }

    pub fn detail_fits(&self) -> bool {
        self.width.saturating_sub(self.sidebar_width()) >= MAIN_MIN_WIDTH + DETAIL_MIN_WIDTH
    }

    pub fn resize(&mut self, width: u16) {
        self.width = width;
        if self.focus == Focus::List && self.post_view.is_some() && !self.detail_fits() {
            self.focus = Focus::Post;
        }
    }

    pub fn toggle_sidebar(&mut self) {
        self.show_sidebar = !self.show_sidebar;
        if self.focus == Focus::Sidebar {
            self.focus_main();
        }
        self.sidebar_peek = false;
        self.resize(self.width);
    }

    /// Tab indexes in sidebar order after Home, Bookmarks and All subs.
    pub fn sidebar_tabs(&self) -> (Vec<usize>, Vec<usize>, Vec<usize>) {
        let mut communities = Vec::new();
        let mut following = Vec::new();
        let mut opened = Vec::new();
        for (index, tab) in self.tabs.iter().enumerate() {
            match tab.kind {
                TabKind::Subreddit { pinned: true, .. } => communities.push(index),
                TabKind::User { followed: true, .. } => following.push(index),
                TabKind::Subreddit { pinned: false, .. }
                | TabKind::User {
                    followed: false, ..
                } => opened.push(index),
                _ => {}
            }
        }
        (communities, following, opened)
    }

    /// `h` in a list: move focus to the sidebar, showing it while focused if hidden.
    pub fn focus_sidebar(&mut self) {
        if !self.show_sidebar {
            self.show_sidebar = true;
            self.sidebar_peek = true;
        }
        self.focus = Focus::Sidebar;
    }

    pub fn focus_main(&mut self) {
        self.focus = Focus::List;
        if self.sidebar_peek {
            self.show_sidebar = false;
            self.sidebar_peek = false;
        }
        self.resize(self.width);
    }

    /// Switch to the tab `delta` rows away in the sidebar, stopping at the ends.
    pub fn move_sidebar(&mut self, delta: isize) {
        let order = self.sidebar_order();
        let at = order.iter().position(|&i| i == self.active).unwrap_or(0);
        self.switch_tab(order[step(at, delta, order.len())]);
    }

    pub(super) fn sidebar_order(&self) -> Vec<usize> {
        let (pinned, following, opened) = self.sidebar_tabs();
        [0, BOOKMARKS_TAB, ALL_SUBS_TAB, SEARCH_TAB]
            .into_iter()
            .chain(pinned)
            .chain(following)
            .chain(opened)
            .collect()
    }

    pub fn sidebar_edge(&mut self, last: bool) {
        self.move_sidebar(if last { isize::MAX } else { isize::MIN });
    }
}
