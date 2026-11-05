use super::*;

impl App {
    pub(crate) fn ancestor_following(&self) -> bool {
        self.ancestor_follow.is_some()
    }

    pub(crate) fn cancel_ancestor_follow(&mut self) {
        self.ancestor_follow = None;
    }

    pub(super) fn showing_list_post(&self, index: usize) -> bool {
        self.post_focused()
            && self.list_post(index).is_some_and(|post| {
                self.post_view
                    .as_ref()
                    .is_some_and(|view| view.post.id == post.id)
            })
    }

    pub(super) fn switch_ancestor_tab(&mut self, index: usize) {
        if self.showing_list_post(index) {
            self.enter_tab(index, true);
        } else {
            self.switch_tab(index);
            self.start_ancestor_follow();
        }
    }

    pub(super) fn start_ancestor_follow(&mut self) {
        if self.showing_list_post(self.active) {
            return;
        }
        self.close_post();
        let group = if matches!(self.active_tab().kind, TabKind::AllSubs) {
            self.all_subs
                .groups
                .get(self.all_subs.group)
                .map(|group| group.subreddit.clone())
        } else {
            None
        };
        self.ancestor_follow = Some(AncestorFollow {
            tab: self.active_tab().id,
            group,
            waiting: true,
        });
        self.follow_ancestor();
    }

    pub(super) fn follow_ancestor(&mut self) -> bool {
        let Some(target) = &self.ancestor_follow else {
            return false;
        };
        let group = matches!(self.active_tab().kind, TabKind::AllSubs)
            .then(|| self.all_subs.groups.get(self.all_subs.group))
            .flatten();
        if self.active_tab().id != target.tab
            || target
                .group
                .as_ref()
                .is_some_and(|name| group.is_none_or(|group| &group.subreddit != name))
            || self.overlay.is_some()
            || self.help_scroll.is_some()
            || self.focus != Focus::List
            || self.post_view.is_some()
        {
            self.ancestor_follow = None;
            return false;
        }
        if !target.waiting || !self.keymap.prefix.is_empty() {
            return false;
        }
        if let Some(post) = self.current_post() {
            self.ancestor_follow = None;
            let permalink = post.permalink.clone();
            self.moving();
            if let Some(generation) = self.show_post(post, &permalink) {
                self.focus = Focus::Post;
                self.post_settle.defer(generation, Instant::now());
            }
            return true;
        }
        let finished = match self.active_tab().kind {
            TabKind::Bookmarks => true,
            TabKind::AllSubs => {
                group.is_some_and(|group| {
                    !group.loading
                        && !group.needs_refresh
                        && (group.error.is_some()
                            || group.freshness.is_some_and(|freshness| {
                                now().saturating_sub(freshness.fetched_at)
                                    < crate::cache::CONTENT_TTL
                            }))
                }) || (group.is_none() && !self.all_subs.loading)
            }
            _ => {
                let feed = &self.active_tab().feed;
                let stale = feed.freshness.is_some_and(|freshness| {
                    now().saturating_sub(freshness.fetched_at) >= crate::cache::CONTENT_TTL
                });
                !feed.loading()
                    && !feed.refreshing()
                    && (feed.error().is_some() && !self.feed_settle.holding(Instant::now())
                        || feed.loaded && !stale && feed.deferred.is_empty())
            }
        };
        if (finished || self.reddit.needs_sign_in())
            && let Some(target) = &mut self.ancestor_follow
        {
            target.waiting = false;
        }
        false
    }

    pub fn community(&self) -> Option<String> {
        if self.focus == Focus::Sidebar {
            return self.active_tab().subreddit().map(str::to_string);
        }
        if self.post_focused() {
            return self
                .post_view
                .as_ref()
                .and_then(|view| view.shown_post().community().map(str::to_string));
        }
        if let Some(target) = self.search_row_target() {
            return match target {
                FeedTarget::Subreddit(name) if is_community(&name) => Some(name),
                _ => None,
            };
        }
        match &self.active_tab().kind {
            TabKind::AllSubs => self
                .all_subs
                .groups
                .get(self.all_subs.group)
                .map(|group| group.subreddit.clone()),
            TabKind::Subreddit { name, .. } if is_community(name) => Some(name.clone()),
            _ => self
                .current_post()
                .and_then(|post| post.community().map(str::to_string)),
        }
    }

    pub fn open_current_community(&mut self) {
        let search_row = self.search_row_path().is_some();
        let target = self.membership_target();
        if matches!(self.overlay, Some(Overlay::Experience)) {
            self.clear_experience();
            self.overlay = None;
        }
        self.close_post();
        self.focus_main();
        if search_row {
            self.open_search_row();
            return;
        }
        match target {
            Some(MembershipTarget::Community(name)) => self.open_subreddit(&name),
            Some(MembershipTarget::User(name)) => self.open_user(&name),
            None => {}
        }
    }

    pub fn mark_community_read(&mut self, name: &str) {
        let count = self.state.mark_community_read(name, now());
        self.info(match count {
            0 => format!("No fetched unread posts in r/{name}"),
            1 => format!("Marked 1 fetched post read in r/{name}"),
            n => format!("Marked {n} fetched posts read in r/{name}"),
        });
        self.persist();
    }

    pub(crate) fn source_browser_path(&self) -> Option<String> {
        if let Some(path) = self.search_row_path() {
            return Some(path);
        }
        match self.membership_target() {
            Some(MembershipTarget::User(name)) => Some(format!("/user/{name}/")),
            Some(MembershipTarget::Community(name)) => Some(format!("/r/{name}/")),
            None if self.overlay.is_none() => match &self.active_tab().kind {
                TabKind::Subreddit { name, .. }
                    if name.eq_ignore_ascii_case("popular") || name.eq_ignore_ascii_case("all") =>
                {
                    Some(format!("/r/{name}/"))
                }
                _ => None,
            },
            None => None,
        }
    }

    pub fn open_source_in_browser(&mut self) {
        match self.source_browser_path() {
            Some(path) => self.open_reddit_link(&path),
            None => self.info("No community or profile selected"),
        }
    }

    pub fn restricted_community(&self) -> Option<(&str, AccessDenied)> {
        if self.post_focused() {
            return None;
        }
        match &self.active_tab().kind {
            TabKind::Subreddit { name, .. } => {
                Some((name, self.active_tab().feed.access_denied()?))
            }
            TabKind::AllSubs => {
                let group = self.all_subs.groups.get(self.all_subs.group)?;
                Some((&group.subreddit, group.error.as_ref()?.access()?))
            }
            _ => None,
        }
    }
}
