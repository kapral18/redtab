use super::*;

impl App {
    pub(super) fn cancel_collection_refresh(&mut self) {
        if std::mem::take(&mut self.experience.refreshing) {
            self.experience.pending = None;
            self.pool.want_experience(None);
        }
    }

    pub(super) fn read_experience(&mut self, query: Query, after: Option<String>) {
        self.cancel_collection_refresh();
        if self.experience.pending.is_some() {
            return;
        }
        self.experience.prefetch = None;
        self.experience.error = None;
        if after.is_some()
            && let Some(Screen::Browse { pagination, .. }) = self.experience.screens.last_mut()
        {
            pagination.error = None;
        }
        if after.is_none() {
            self.experience.screens.push(Screen::Loading(query.clone()));
        }
        let generation = self.generation();
        self.overlay = Some(Overlay::Experience);
        if let Some(index) = self
            .collections
            .iter()
            .position(|entry| entry.query == query && entry.after == after)
        {
            let cached = self.collections.remove(index).unwrap();
            let fresh =
                cached.fetched.elapsed() < Duration::from_secs(crate::cache::CONTENT_TTL as u64);
            if fresh || after.is_none() {
                self.show_collection(
                    generation,
                    query.clone(),
                    after.clone(),
                    cached.page.clone(),
                );
            }
            self.collections.push_back(cached);
            if fresh {
                return;
            }
            self.experience.refreshing = after.is_none();
        }
        let request = Request::Read { query, after };
        self.experience.pending = Some((generation, request.clone()));
        self.pool.submit(Job::Experience {
            generation,
            request,
            invalidations: Vec::new(),
        });
    }

    pub(super) fn show_collection(
        &mut self,
        generation: u64,
        query: Query,
        after: Option<String>,
        mut page: Page,
    ) {
        if let Some(cursor) = after {
            if let Some(Screen::Browse {
                query: current,
                page: previous,
                pagination,
                ..
            }) = self.experience.screens.last_mut()
                && *current == query
            {
                let mut ids: HashSet<_> = previous.entries.iter().map(entry_key).collect();
                previous.entries.extend(
                    page.entries
                        .drain(..)
                        .filter(|entry| ids.insert(entry_key(entry))),
                );
                pagination.visited.insert(cursor);
                previous.after = page
                    .after
                    .filter(|cursor| !pagination.visited.contains(cursor));
            }
            return;
        }
        let kept = match self.experience.screens.last() {
            Some(Screen::Browse {
                query: current,
                page,
                selected,
                scroll,
                ..
            }) if *current == query => page
                .entries
                .get(*selected)
                .map(|entry| (entry_key(entry), *selected, *scroll)),
            _ => self
                .experience
                .kept
                .take()
                .filter(|(read, ..)| *read == generation)
                .map(|(_, key, index, scroll)| (key, index, scroll)),
        };
        let (selected, scroll) = kept.map_or((0, 0), |(key, index, scroll)| {
            page.entries
                .iter()
                .position(|entry| entry_key(entry) == key)
                .map_or(
                    (index.min(page.entries.len().saturating_sub(1)), 0),
                    |found| (found, scroll),
                )
        });
        self.experience.screens.pop();
        self.experience.screens.push(Screen::Browse {
            pagination: Default::default(),
            query,
            page,
            selected,
            scroll,
        });
    }

    pub(super) fn write_experience(&mut self, action: Action) {
        self.cancel_collection_refresh();
        if self.experience.pending.is_some() || self.membership.is_some() || self.settings.writing()
        {
            self.experience.error = Some("Wait for the pending Reddit action".into());
            return;
        }
        self.experience.error = None;
        let invalidations = if action.changes_reader_content() {
            self.experience_invalidations()
        } else {
            Vec::new()
        };
        self.collections.clear();
        let request = Request::Write(action);
        let generation = self.generation();
        self.experience.pending = Some((generation, request.clone()));
        self.pool.submit(Job::Experience {
            generation,
            request,
            invalidations,
        });
    }

    pub(super) fn experience_invalidations(&self) -> Vec<crate::worker::Invalidation> {
        use crate::worker::Invalidation;
        let mut entries = Vec::new();
        entries.push(Invalidation::Post(self.experience_native_path()));
        for tab in &self.tabs {
            if let Some(target) = tab.feed_target() {
                entries.push(Invalidation::Feed {
                    target: target.clone(),
                    query: tab.feed.query(),
                    after: None,
                });
                for page in &tab.feed.pages {
                    entries.push(Invalidation::Feed {
                        target: target.clone(),
                        query: tab.feed.query(),
                        after: page.cursor.clone(),
                    });
                }
            }
        }
        for group in &self.all_subs.groups {
            entries.push(Invalidation::Feed {
                target: FeedTarget::Subreddit(group.subreddit.clone()),
                query: FeedQuery::DEFAULT_LISTING,
                after: None,
            });
        }
        if let Some(target) = &self.search.preview.target {
            entries.push(Invalidation::Feed {
                target: target.clone(),
                query: FeedQuery::DEFAULT_LISTING,
                after: None,
            });
        }
        if let Some(view) = &self.post_view {
            entries.push(Invalidation::Post(view.post.permalink.clone()));
            if let Some(page) = view.page.as_ref() {
                collect_invalidations(&page.comments, &mut entries);
            }
        }
        entries
    }

    pub(in crate::app) fn load_settled_collection(&mut self, now: Instant) -> bool {
        if self
            .experience
            .prefetch
            .as_ref()
            .is_none_or(|(_, at)| *at > now)
        {
            return false;
        }
        let (query, _) = self.experience.prefetch.take().unwrap();
        if matches!(self.overlay, Some(Overlay::Experience))
            && matches!(self.experience.screens.last(), Some(Screen::Browse { query: current, .. }) if *current == query)
        {
            self.prefetch_experience();
        }
        true
    }

    pub(super) fn prefetch_experience(&mut self) {
        if self.experience.pending.is_some() {
            return;
        }
        let Some(Screen::Browse {
            query,
            page,
            selected,
            pagination,
            ..
        }) = self.experience.screens.last()
        else {
            return;
        };
        let Some(after) = page
            .after
            .as_ref()
            .filter(|cursor| !pagination.visited.contains(*cursor))
        else {
            return;
        };
        if pagination.error.is_none()
            && selected.saturating_add(PREFETCH_MARGIN) >= page.entries.len()
        {
            self.read_experience(query.clone(), Some(after.clone()));
        }
    }

    pub(super) fn experience_back(&mut self) {
        if self.experience.writing() {
            return;
        }
        self.experience.pending = None;
        self.experience.prefetch = None;
        self.experience.refreshing = false;
        self.experience.kept = None;
        self.experience.quick = None;
        self.pool.want_experience(None);
        self.experience.error = None;
        self.experience.screens.pop();
        if self.experience.screens.is_empty() {
            self.overlay = None;
        }
    }

    pub(crate) fn collection_action(&mut self, action: crate::keys::map::Action) {
        self.cancel_collection_refresh();
        use crate::keys::map::Action as KeyAction;
        if self.experience.pending.is_some() {
            return;
        }
        if action == KeyAction::Reload
            && let [
                ..,
                Screen::Input(form),
                Screen::Browse {
                    query: Query::Thread(_),
                    page,
                    selected,
                    scroll,
                    ..
                },
            ] = self.experience.screens.as_slice()
            && matches!(form.kind, FormKind::Find)
        {
            let text = form.fields[0].value.clone();
            let kept = page.entries.get(*selected).map(entry_key);
            let previous = (*selected, *scroll);
            self.experience.screens.pop();
            self.find_comments(&text);
            if let Some(Screen::Browse {
                page,
                selected,
                scroll,
                ..
            }) = self.experience.screens.last_mut()
            {
                *selected = kept
                    .and_then(|key| {
                        page.entries
                            .iter()
                            .position(|entry| entry_key(entry) == key)
                    })
                    .unwrap_or(previous.0.min(page.entries.len().saturating_sub(1)));
                *scroll = previous.1;
            }
            return;
        }
        let Some(Screen::Browse {
            query,
            page,
            pagination,
            ..
        }) = self.experience.screens.last_mut()
        else {
            return;
        };
        if action != KeyAction::Reload {
            return;
        }
        if let Some(update) = pagination.update.take() {
            let query = query.clone();
            let generation = self.generation();
            self.show_collection(generation, query, None, *update);
            return;
        }
        let after = pagination
            .error
            .is_some()
            .then(|| page.after.clone())
            .flatten();
        if after.is_none() {
            self.reread_list();
            return;
        }
        let query = query.clone();
        self.collections.retain(|entry| entry.query != query);
        self.read_experience(query, after);
    }

    /// Reads the open list again, keeping its selected entry and preview scroll.
    pub(super) fn reread_list(&mut self) {
        let Some(Screen::Browse {
            query,
            page,
            selected,
            scroll,
            ..
        }) = self.experience.screens.last()
        else {
            return;
        };
        let query = query.clone();
        self.collections.retain(|entry| entry.query != query);
        let kept = page
            .entries
            .get(*selected)
            .map(|entry| (entry_key(entry), *selected, *scroll));
        self.experience.screens.pop();
        self.read_experience(query, None);
        self.experience.kept = kept
            .zip(self.experience.pending.as_ref())
            .map(|((key, index, scroll), (read, _))| (*read, key, index, scroll));
    }
}

pub(super) fn entry_key(entry: &Entry) -> String {
    entry
        .target
        .as_ref()
        .map(|target| target.id.clone())
        .unwrap_or_else(|| entry.title.clone())
}

pub(super) fn has_comment(comments: &[Comment], id: &str) -> bool {
    comments
        .iter()
        .any(|comment| comment.id == id || has_comment(&comment.replies, id))
}

pub(super) fn collect_invalidations(
    comments: &[Comment],
    entries: &mut Vec<crate::worker::Invalidation>,
) {
    for comment in comments {
        if let Some(more) = &comment.more {
            entries.push(crate::worker::Invalidation::Replies(more.cursor.clone()));
        }
        collect_invalidations(&comment.replies, entries);
    }
}
