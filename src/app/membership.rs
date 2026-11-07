use super::*;

pub(super) struct MembershipChange {
    pub(super) generation: u64,
    pub(super) target: MembershipTarget,
    pub(super) join: bool,
    pub(super) checking: bool,
    pub(super) error: Option<String>,
}

impl App {
    pub(super) fn load_subscriptions(&mut self, force: bool) {
        if self.reddit.needs_sign_in() {
            return;
        }
        if self
            .membership
            .as_ref()
            .is_some_and(|change| !change.checking || self.all_subs.loading)
        {
            return;
        }
        self.all_subs.generation += 1;
        self.all_subs.loading = true;
        self.all_subs.error = None;
        let job = Job::Subscriptions {
            generation: self.all_subs.generation,
        };
        if force {
            self.pool.refresh(job);
        } else {
            self.pool.submit(job);
        }
    }

    pub(super) fn sync_subscription_tabs(&mut self, entries: &[Subscription]) {
        let active = self.active_tab().id;
        let communities: HashMap<_, _> = entries
            .iter()
            .filter_map(|entry| entry.community())
            .enumerate()
            .map(|(index, name)| (name.to_ascii_lowercase(), index))
            .collect();
        let follows: HashMap<_, _> = entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| match entry {
                Subscription::User { name, icon } => {
                    Some((name.to_ascii_lowercase(), (index, icon.clone())))
                }
                _ => None,
            })
            .collect();
        for tab in &mut self.tabs {
            match &mut tab.kind {
                TabKind::Subreddit { name, pinned } => {
                    *pinned = communities.contains_key(&name.to_ascii_lowercase())
                }
                TabKind::User {
                    name,
                    followed,
                    icon,
                } => {
                    *followed = follows.contains_key(&name.to_ascii_lowercase());
                    if let Some(updated) = follows.get(&name.to_ascii_lowercase()) {
                        icon.clone_from(&updated.1);
                    }
                }
                _ => {}
            }
        }
        for entry in entries {
            let existing = self.tabs.iter().any(|tab| match (&tab.kind, entry) {
                (TabKind::Subreddit { name, .. }, Subscription::Community { name: wanted, .. })
                | (TabKind::User { name, .. }, Subscription::User { name: wanted, .. }) => {
                    name.eq_ignore_ascii_case(wanted)
                }
                _ => false,
            });
            if !existing {
                self.push_tab(match entry {
                    Subscription::Community { name, .. } => TabKind::Subreddit {
                        name: name.clone(),
                        pinned: true,
                    },
                    Subscription::User { name, icon } => TabKind::User {
                        name: name.clone(),
                        icon: icon.clone(),
                        followed: true,
                    },
                });
            }
        }
        self.tabs.sort_by_key(|tab| match &tab.kind {
            TabKind::Home => (0, 0),
            TabKind::Bookmarks => (1, 0),
            TabKind::AllSubs => (2, 0),
            TabKind::Search => (3, 0),
            TabKind::Subreddit { name, pinned: true } => {
                (4, communities[&name.to_ascii_lowercase()])
            }
            TabKind::User {
                name,
                followed: true,
                ..
            } => (5, follows[&name.to_ascii_lowercase()].0),
            _ => (6, 0),
        });
        self.active = self
            .tabs
            .iter()
            .position(|tab| tab.id == active)
            .unwrap_or(0);
    }

    pub(super) fn sync_islands(&mut self, names: Vec<String>) {
        // Drawn indices point into the old island list; the next draw records new ones.
        self.all_subs.shown.clear();
        let selected = self
            .all_subs
            .groups
            .get(self.all_subs.group)
            .map(|g| g.subreddit.clone());
        let mut groups: HashMap<_, _> = std::mem::take(&mut self.all_subs.groups)
            .into_iter()
            .map(|group| (group.subreddit.to_ascii_lowercase(), group))
            .collect();
        let generation = self.all_subs.next_group_generation;
        self.all_subs.next_group_generation += 1;
        self.all_subs.groups = names
            .into_iter()
            .map(|name| {
                groups
                    .remove(&name.to_ascii_lowercase())
                    .unwrap_or_else(|| {
                        self.pool.cached(Job::Group {
                            subreddit: name.clone(),
                            generation,
                        });
                        Group {
                            subreddit: name,
                            generation,
                            ..Group::default()
                        }
                    })
            })
            .collect();
        self.sync_community_access();
        self.all_subs.group = selected
            .and_then(|name| {
                self.all_subs
                    .groups
                    .iter()
                    .position(|g| g.subreddit == name)
            })
            .unwrap_or(self.all_subs.group)
            .min(self.all_subs.groups.len().saturating_sub(1));
        let count = self
            .all_subs
            .groups
            .get(self.all_subs.group)
            .map_or(0, |g| self.group_posts(g).count());
        self.all_subs.post = self.all_subs.post.min(count.saturating_sub(1));
        self.all_subs.offset = self.all_subs.offset.min(self.all_subs.group);
    }

    pub(super) fn on_subscriptions(
        &mut self,
        generation: u64,
        result: Result<Vec<Subscription>, String>,
        freshness: Freshness,
        refreshing: bool,
    ) {
        if generation != self.all_subs.generation {
            return;
        }
        if self
            .membership
            .as_ref()
            .is_some_and(|change| !change.checking || freshness.source.cached)
            || self
                .all_subs
                .freshness
                .is_some_and(|current| !freshness.source.may_replace(current.source))
        {
            // A final answer the list keeps out still ends this request.
            if !refreshing {
                self.all_subs.loading = false;
            }
            return;
        }
        self.all_subs.loading = refreshing;
        let confirmation = result.as_ref().ok().and_then(|names| {
            self.membership
                .as_ref()
                .filter(|change| change.checking)
                .map(|change| names.iter().any(|entry| change.target.matches(entry)) == change.join)
        });
        match result {
            Ok(names) => {
                for entry in &names {
                    if let Subscription::Community { name, icon } = entry {
                        match icon {
                            crate::model::SubscriptionIcon::Unknown => {}
                            crate::model::SubscriptionIcon::Missing => {
                                self.observe_icon(name, None, freshness.source)
                            }
                            crate::model::SubscriptionIcon::Url(url) => {
                                self.observe_icon(name, Some(url.clone()), freshness.source)
                            }
                        }
                    }
                }
                self.sync_subscription_tabs(&names);
                self.sync_islands(
                    names
                        .into_iter()
                        .filter_map(|entry| match entry {
                            Subscription::Community { name, .. } => Some(name),
                            _ => None,
                        })
                        .collect(),
                );
                self.all_subs.error = None;
                self.all_subs.freshness = Some(freshness);
            }
            Err(error) => {
                self.error(format!("Subscriptions: {error}"));
                self.all_subs.error = Some(error);
            }
        }
        if self
            .membership
            .as_ref()
            .is_some_and(|change| change.checking)
        {
            let change = self.membership.take().expect("checked pending membership");
            if confirmation == Some(true) {
                self.info(format!(
                    "{} {} on Reddit",
                    match (&change.target, change.join) {
                        (MembershipTarget::User(_), true) => "Following",
                        (MembershipTarget::User(_), false) => "Unfollowed",
                        (_, true) => "Joined",
                        (_, false) => "Left",
                    },
                    change.target.label()
                ));
            } else {
                self.error(change.error.unwrap_or_else(|| format!(
                    "Could not confirm membership for {}; r in All subs refreshes the account list",
                    change.target.label()
                )));
            }
        }
    }

    pub(super) fn on_membership(&mut self, generation: u64, result: Result<(), String>) {
        let Some(change) = &mut self.membership else {
            return;
        };
        if change.generation != generation || change.checking {
            return;
        }
        change.checking = true;
        change.error = result.err();
        // A rejected session cannot read the list back, so nothing would settle the change.
        if self.reddit.needs_sign_in() {
            let change = self.membership.take().expect("checked pending membership");
            self.error(change.error.unwrap_or_else(|| {
                format!(
                    "Reddit signed this session out; reconnect to confirm membership for {}",
                    change.target.label()
                )
            }));
            return;
        }
        self.load_subscriptions(true);
    }

    pub(super) fn push_tab(&mut self, kind: TabKind) -> usize {
        self.next_id += 1;
        let feed = if matches!(kind, TabKind::User { .. }) {
            Feed {
                sort: Sort::New,
                ..Feed::default()
            }
        } else {
            Feed::default()
        };
        self.tabs.push(Tab {
            id: self.next_id,
            kind,
            feed,
        });
        self.sync_community_access();
        self.tabs.len() - 1
    }

    pub fn joined(&self, name: &str) -> Option<bool> {
        (self.all_subs.freshness.is_some() && self.all_subs.error.is_none()).then(|| {
            self.all_subs
                .groups
                .iter()
                .any(|group| group.subreddit.eq_ignore_ascii_case(name))
        })
    }

    pub fn membership_progress(&self) -> Option<String> {
        self.membership.as_ref().map(|change| {
            format!(
                "{} {}…",
                if change.checking {
                    "Checking membership for"
                } else {
                    match (&change.target, change.join) {
                        (MembershipTarget::User(_), true) => "Following",
                        (MembershipTarget::User(_), false) => "Unfollowing",
                        (_, true) => "Joining",
                        (_, false) => "Leaving",
                    }
                },
                change.target.label()
            )
        })
    }

    pub fn show_membership(&mut self) {
        if let Some(progress) = self.membership_progress() {
            self.info(progress);
            return;
        }
        let Some(target) = self.membership_target() else {
            self.info("Select an individual community or user first");
            return;
        };
        self.show_membership_for(target);
    }

    pub fn subscribed(&self, target: &MembershipTarget) -> Option<bool> {
        match target {
            MembershipTarget::Community(name) => self.joined(name),
            MembershipTarget::User(name) => (self.all_subs.freshness.is_some() && self.all_subs.error.is_none()).then(|| {
                self.tabs.iter().any(|tab| matches!(&tab.kind,
                    TabKind::User { name: actual, followed: true, .. } if actual.eq_ignore_ascii_case(name)))
            }),
        }
    }

    pub fn show_membership_for(&mut self, target: MembershipTarget) {
        if !target.is_valid() {
            self.info("Select an individual community or user first");
            return;
        }
        if let Some(progress) = self.membership_progress() {
            self.info(progress);
            return;
        }
        self.cancel_collection_refresh();
        if self.experience.pending.is_some() {
            self.info("Wait for the pending Reddit request");
            return;
        }
        if self.reddit.needs_sign_in() {
            self.show_auth();
            return;
        }
        let Some(joined) = self.subscribed(&target) else {
            self.load_subscriptions(true);
            self.info("Loading account subscriptions; press SPC m again when ready");
            return;
        };
        self.overlay = Some(Overlay::Membership {
            target,
            join: !joined,
        });
    }

    pub fn set_membership(&mut self, target: MembershipTarget, join: bool) {
        if self.membership.is_some()
            || self.experience.pending.is_some()
            || self.settings.writing()
            || !target.is_valid()
        {
            return;
        }
        let generation = self.generation();
        // Older membership reads must not settle this write or undo its confirmation.
        self.all_subs.generation += 1;
        self.all_subs.loading = false;
        self.membership = Some(MembershipChange {
            generation,
            target: target.clone(),
            join,
            checking: false,
            error: None,
        });
        self.status = None;
        self.pool.submit(Job::Membership {
            generation,
            target,
            join,
        });
    }
}
