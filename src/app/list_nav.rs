use super::*;

impl App {
    pub fn move_selection(&mut self, delta: isize) {
        let tab = self.active_tab();
        if tab.subreddit().is_some() && tab.feed.access_denied().is_some() {
            return;
        }
        let before = self.active_tab().feed.selected;
        match self.active_tab().kind {
            TabKind::AllSubs => self.move_all_subs(delta),
            TabKind::Search if self.search.shows_rows() => self.move_search(delta),
            TabKind::Bookmarks => {
                let len = self.visible_bookmarks().count();
                let feed = &mut self.tabs[self.active].feed;
                feed.selected = step(feed.selected, delta, len);
            }
            TabKind::Home | TabKind::Subreddit { .. } | TabKind::User { .. } | TabKind::Search => {
                let len = self
                    .config
                    .visible(&self.tabs[self.active].feed.posts)
                    .count();
                let feed = &mut self.tabs[self.active].feed;
                feed.selected = step(feed.selected, delta, len);
            }
        }
        self.moving();
        self.prefetch_list(self.active_tab().feed.selected);
        if self.active_tab().feed.selected != before {
            self.tabs[self.active].feed.cards = CardScroll::default();
        }
        self.preview_selection();
    }

    pub(crate) fn list_uses_rows(&self) -> bool {
        self.search_shows_rows()
            || self.state.list_view() != ListView::Card
            || matches!(self.active_tab().kind, TabKind::AllSubs)
    }

    pub fn page_list(&mut self, direction: isize, divisor: usize) {
        if self.list_uses_rows() || self.active_tab().feed.cards.viewport < 3 {
            self.move_selection(direction * (10 / divisor.max(1)).max(1) as isize);
            return;
        }
        let rows = (self.active_tab().feed.cards.viewport / divisor).max(1);
        self.scroll_card(direction * rows as isize);
    }

    pub fn scroll_card(&mut self, delta: isize) {
        if self.list_uses_rows() {
            self.move_selection(delta);
            return;
        }
        self.moving();
        if delta > 0 && self.current_post().is_none() {
            self.prefetch_list(self.active_tab().feed.selected);
        }
        if self.current_post().is_none() || self.active_tab().feed.cards.post.is_none() {
            return;
        }
        let cards = &mut self.tabs[self.active].feed.cards;
        cards.pending = cards.pending.saturating_add(delta);
    }

    pub(super) fn prefetch_list(&mut self, last_visible: usize) {
        let tab = self.active_tab();
        let search_feed = matches!(tab.kind, TabKind::Search) && !self.search.shows_rows();
        if !(search_feed
            || matches!(
                tab.kind,
                TabKind::Home | TabKind::Subreddit { .. } | TabKind::User { .. }
            ))
            || (tab.subreddit().is_some() && tab.feed.access_denied().is_some())
        {
            self.list_prefetch = None;
            return;
        }
        if self.picture_settle.holding(Instant::now()) {
            self.list_prefetch = Some((tab.id, last_visible));
            return;
        }
        let feed = &tab.feed;
        let len = self.config.visible(&feed.posts).count();
        if feed.after.is_none()
            || feed.loading()
            || feed.loading_more()
            || feed.receiving()
            || !feed.shows_current_query()
            || feed.pages.iter().any(|page| page.cursor == feed.after)
            || feed.page_failed()
        {
            self.list_prefetch = None;
            return;
        }
        if last_visible.saturating_add(PREFETCH_MARGIN) >= len {
            self.list_prefetch = None;
            self.load_feed(self.active, true);
        }
    }

    pub fn set_list_viewport(
        &mut self,
        selected: usize,
        offset: usize,
        scroll: CardScroll,
        last_visible: Option<usize>,
    ) {
        let scrolling = self.active_tab().feed.cards.pending != 0;
        let before = self.active_tab().feed.selected;
        if selected != before {
            self.move_selection(selected as isize - before as isize);
        }
        let feed = &mut self.tabs[self.active].feed;
        feed.offset = offset;
        feed.cards = scroll;
        if let Some(last) = last_visible
            && (scrolling
                || self
                    .list_prefetch
                    .is_some_and(|(tab, _)| tab == self.active_tab().id))
        {
            self.list_prefetch = None;
            if !self.popup_open() {
                self.prefetch_list(last);
            }
        }
    }

    pub fn select_edge(&mut self, end: bool) {
        match self.active_tab().kind {
            TabKind::AllSubs => {
                self.all_subs.group = if end {
                    self.all_subs.groups.len().saturating_sub(1)
                } else {
                    0
                };
                self.all_subs.post = if end {
                    self.all_subs
                        .groups
                        .get(self.all_subs.group)
                        .map_or(0, |group| self.group_posts(group).count().saturating_sub(1))
                } else {
                    0
                };
                self.moving();
                self.preview_selection();
            }
            _ => {
                self.move_selection(if end { isize::MAX / 2 } else { isize::MIN / 2 });
                self.tabs[self.active].feed.cards = CardScroll::default();
            }
        }
    }

    /// Posts shown in an All subs island: the first `all_subs_posts_per_group` of the hot listing
    /// that the config does not hide, pinned posts left out.
    pub fn group_posts<'a>(&self, group: &'a Group) -> impl Iterator<Item = &'a Post> {
        let accessible = group.error.as_ref().and_then(FeedError::access).is_none();
        self.config
            .visible(&group.posts)
            .filter(move |p| accessible && !p.stickied)
            .take(self.config.all_subs_posts_per_group)
    }

    pub(super) fn move_all_subs(&mut self, delta: isize) {
        if self.all_subs.groups.is_empty() {
            return;
        }
        let mut remaining = delta;
        while remaining != 0 {
            let count = self
                .group_posts(&self.all_subs.groups[self.all_subs.group])
                .count();
            if remaining > 0 {
                if self.all_subs.post + 1 < count {
                    self.all_subs.post += 1;
                } else if self.all_subs.group + 1 < self.all_subs.groups.len() {
                    self.all_subs.group += 1;
                    self.all_subs.post = 0;
                } else {
                    break;
                }
                remaining -= 1;
            } else {
                if self.all_subs.post > 0 {
                    self.all_subs.post -= 1;
                } else if self.all_subs.group > 0 {
                    self.all_subs.group -= 1;
                    let count = self
                        .group_posts(&self.all_subs.groups[self.all_subs.group])
                        .count();
                    self.all_subs.post = count.saturating_sub(1);
                } else {
                    break;
                }
                remaining += 1;
            }
        }
    }

    pub fn move_all_subs_group(&mut self, delta: isize) {
        let group = step(self.all_subs.group, delta, self.all_subs.groups.len());
        if group == self.all_subs.group {
            return;
        }
        self.all_subs.group = group;
        self.all_subs.post = 0;
        self.moving();
        self.preview_selection();
    }

    pub fn move_community(&mut self, forward: bool) {
        if self.search_source_context() {
            self.return_to_search();
            self.close_post();
            self.focus_main();
            self.search.editing = false;
            self.move_search(if forward { 1 } else { -1 });
            return;
        }
        let follow = self.post_focused() || self.ancestor_follow.is_some();
        if matches!(self.active_tab().kind, TabKind::AllSubs) {
            if self.post_focused()
                && let Some(view) = &self.post_view
            {
                let anchor = self
                    .all_subs
                    .groups
                    .iter()
                    .enumerate()
                    .find_map(|(group, entry)| {
                        self.group_posts(entry)
                            .position(|post| post.id == view.post.id)
                            .map(|post| (group, post))
                    })
                    .or_else(|| {
                        self.all_subs
                            .groups
                            .iter()
                            .position(|group| {
                                group.subreddit.eq_ignore_ascii_case(&view.post.subreddit)
                            })
                            .map(|group| (group, 0))
                    });
                if let Some((group, post)) = anchor {
                    self.all_subs.group = group;
                    self.all_subs.post = post;
                }
            }
            let before = self.all_subs.group;
            self.move_all_subs_group(if forward { 1 } else { -1 });
            if follow && self.all_subs.group != before {
                self.start_ancestor_follow();
            }
            return;
        }
        let order = self.sidebar_order();
        let at = order
            .iter()
            .position(|&index| index == self.active)
            .unwrap_or(0);
        let is_community = |&&index: &&usize| self.tabs[index].subreddit().is_some();
        let next = if forward {
            order.iter().skip(at + 1).find(is_community)
        } else {
            order[..at].iter().rev().find(is_community)
        };
        if let Some(&index) = next {
            if follow {
                self.switch_ancestor_tab(index);
            } else {
                self.switch_tab(index);
            }
        }
    }
}
