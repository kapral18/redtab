use super::*;

impl App {
    pub fn on_experience(
        &mut self,
        generation: u64,
        result: Result<Response, String>,
        freshness: Freshness,
    ) {
        let Some((pending, _)) = &self.experience.pending else {
            return;
        };
        if *pending != generation {
            return;
        }
        let (_, request) = self.experience.pending.take().unwrap();
        let refreshing = std::mem::take(&mut self.experience.refreshing);
        if matches!(request, Request::Write(_)) {
            self.collections.clear();
        }
        let quick = self.experience.quick.take();
        if matches!(request, Request::Read { .. }) {
            self.pool.want_experience(None);
        }
        match result {
            Err(error) => {
                if let Request::Read {
                    query,
                    after: Some(_),
                    ..
                } = &request
                    && let Some(Screen::Browse {
                        query: current,
                        pagination,
                        ..
                    }) = self.experience.screens.last_mut()
                    && current == query
                {
                    pagination.error = Some(format!(
                        "{error}. r retries loading more; Ctrl+o opens Reddit."
                    ));
                    return;
                }
                self.experience.error = Some(match request {
                    Request::Read { .. } => format!("{error}. r retries; Ctrl+o opens Reddit."),
                    Request::Write(_) => format!(
                        "{error}. Open Reddit with Ctrl+o to check before submitting again."
                    ),
                });
            }
            Ok(Response::Details(details)) => {
                self.experience.screens.pop();
                self.details_menu(details);
                if let Some(Screen::Menu {
                    choices, selected, ..
                }) = self.experience.screens.last_mut()
                    && let Some(index) = quick.and_then(|shortcut| {
                        choices
                            .iter()
                            .position(|choice| choice.shortcut == shortcut)
                    })
                {
                    *selected = index;
                    let command = choices[index].command.clone();
                    self.execute_experience(command);
                }
            }
            Ok(Response::Page(page)) => {
                let Request::Read { query, after } = request else {
                    return;
                };
                self.collections
                    .retain(|entry| entry.query != query || entry.after != after);
                self.collections.push_back(CachedCollection {
                    query: query.clone(),
                    after: after.clone(),
                    page: page.clone(),
                    fetched: Instant::now(),
                });
                while self.collections.len() > 32 {
                    self.collections.pop_front();
                }
                if refreshing
                    && after.is_none()
                    && let Some(Screen::Browse {
                        query: current,
                        page: shown,
                        pagination,
                        ..
                    }) = self
                        .experience
                        .screens
                        .iter_mut()
                        .rev()
                        // A menu opened over the list keeps its place; the list updates.
                        .find(|screen| !matches!(screen, Screen::Menu { .. }))
                    && *current == query
                {
                    // A list with nothing shown, or a text page, takes the answer whole.
                    if shown.entries.is_empty() {
                        *shown = page;
                        return;
                    }
                    let fresh: HashMap<_, _> = page
                        .entries
                        .iter()
                        .map(|entry| (entry_key(entry), entry))
                        .collect();
                    for entry in &mut shown.entries {
                        if let Some(fresh) = fresh.get(&entry_key(entry)) {
                            *entry = (*fresh).clone();
                        }
                    }
                    if shown
                        .entries
                        .iter()
                        .map(entry_key)
                        .ne(page.entries.iter().map(entry_key))
                    {
                        pagination.update = Some(Box::new(page));
                    } else {
                        shown.after = page.after;
                        shown.title = page.title;
                        shown.text = page.text;
                        shown.url = page.url;
                    }
                } else {
                    self.show_collection(generation, query, after, page);
                }
            }
            Ok(Response::Thread(page)) => {
                let comment = match &request {
                    Request::Read {
                        query: Query::Thread(link),
                        ..
                    } => thread_location(link).ok().and_then(|(_, comment)| comment),
                    _ => None,
                };
                let post = page.post.clone();
                let link = post.permalink.clone();
                // A Comments search result reads beside its rows on the Search tab.
                let from_search = self.active == SEARCH_TAB
                    && matches!(&request, Request::Read { query: Query::Thread(read), .. }
                        if self.search.reading_comment.as_ref() == Some(read));
                self.search.reading_comment = None;
                self.leave_experience();
                self.focus_main();
                if !from_search {
                    match post.profile_user() {
                        Some(user) => self.open_user(user),
                        None => self.open_subreddit(&post.subreddit),
                    }
                }
                self.cancel_reveal();
                if let Some(generation) = self.show_post(post, &link) {
                    self.focus = Focus::Post;
                    // The result already showed the comment; a spoiler cover would hide it.
                    if from_search && let Some(view) = &mut self.post_view {
                        view.reveal_spoilers = true;
                    }
                    self.on_snapshot(
                        Msg::Post {
                            generation,
                            result: Ok(page),
                        },
                        freshness,
                        false,
                    );
                    if let Some(comment) = comment {
                        self.select_loaded_comment(&comment);
                    }
                }
            }
            Ok(Response::Written { url }) => {
                let Request::Write(action) = request else {
                    return;
                };
                self.after_experience_write(&action);
                self.experience.screens.pop();
                self.experience.error = None;
                self.info("Reddit confirmed the action");
                if matches!(action, Action::DeleteMulti { .. }) {
                    while self.experience.screens.last().is_some_and(|screen| {
                        !matches!(
                            screen,
                            Screen::Browse {
                                query: Query::Multis,
                                ..
                            }
                        )
                    }) {
                        self.experience.screens.pop();
                    }
                    if !self.experience.screens.is_empty() {
                        self.experience.screens.pop();
                    }
                    self.read_experience(Query::Multis, None);
                    return;
                }
                if let Some(url) = url {
                    self.read_experience(Query::Thread(url), None);
                } else if matches!(self.experience.screens.last(), Some(Screen::Browse { .. })) {
                    self.reread_list();
                } else {
                    self.leave_experience();
                }
            }
        }
    }

    pub(super) fn after_experience_write(&mut self, action: &Action) {
        if !action.changes_reader_content() {
            return;
        }
        // Its refetches answer the write, not an earlier `r`.
        self.cancel_reveal();
        let invalid_generation = self.generation();
        self.content_generation_floor = invalid_generation;
        self.search.preview = super::super::search::Preview::default();
        for tab in &mut self.tabs {
            tab.feed.generation = invalid_generation;
            tab.feed.requests.clear();
            tab.feed.pending.clear();
            tab.feed.refreshing.clear();
            tab.feed.forced.clear();
            tab.feed.deferred.clear();
            tab.feed.update = None;
            tab.feed.loaded = false;
        }
        let target_post = thread_location(&self.experience_native_path())
            .ok()
            .and_then(|(path, _)| crate::reddit::parse::post_id(&path).map(str::to_string));
        for group in &mut self.all_subs.groups {
            if group.loading
                || target_post.as_ref().is_some_and(|id| {
                    group
                        .posts
                        .iter()
                        .chain(group.arrivals.iter().flatten())
                        .any(|post| &post.id == id)
                })
            {
                let generation = self.all_subs.next_group_generation;
                self.all_subs.next_group_generation += 1;
                group.generation = generation;
                group.loading = false;
                group.needs_refresh = true;
            }
        }
        if let Action::Hide { id, hide: false } = action
            && let Some(id) = id.strip_prefix("t3_")
        {
            self.removed_posts.remove(id);
        }
        if let Action::Hide { id, hide: true }
        | Action::Report { id, .. }
        | Action::Delete {
            target: Target { id, .. },
        } = action
            && let Some(id) = id.strip_prefix("t3_")
        {
            self.removed_posts.insert(id.to_string());
            // A removed post can no longer be opened, so it must not stay unread.
            self.state.mark_read_id(id, crate::app::now());
            let selected = self
                .all_subs
                .groups
                .get(self.all_subs.group)
                .and_then(|group| self.group_posts(group).nth(self.all_subs.post))
                .map(|post| post.id.clone());
            // Bookmarks list saved records, not feed posts.
            for tab in self
                .tabs
                .iter_mut()
                .filter(|tab| !matches!(tab.kind, TabKind::Bookmarks))
            {
                let selected_post = self
                    .config
                    .visible(&tab.feed.posts)
                    .nth(tab.feed.selected)
                    .map(|post| post.id.clone());
                let screen_row = tab.feed.selected.saturating_sub(tab.feed.offset);
                tab.feed.remove_post(id);
                let visible: Vec<_> = self.config.visible(&tab.feed.posts).collect();
                tab.feed.selected = selected_post
                    .and_then(|id| visible.iter().position(|post| post.id == id))
                    .unwrap_or(tab.feed.selected)
                    .min(visible.len().saturating_sub(1));
                tab.feed.offset = tab.feed.selected.saturating_sub(screen_row);
            }
            for group in &mut self.all_subs.groups {
                group.posts.retain(|post| post.id != id);
                if let Some(arrivals) = &mut group.arrivals {
                    arrivals.retain(|post| post.id != id);
                }
            }
            self.settle_keeping_island_post(selected.as_deref());
        }
        if self.post_view.is_some() {
            self.refresh_post();
        }
        if self.active_tab().feed_target().is_some() {
            self.fetch_feed(self.active, None, true);
        }
        self.sync_feed_demand();
    }

    pub(super) fn execute_experience(&mut self, command: Command) {
        self.experience.error = None;
        match command {
            Command::Read(query) => self.read_experience(query, None),
            Command::Input(form) => self.experience.screens.push(Screen::Input(form)),
            Command::Search(kind) => self.open_search(Some(kind)),
            Command::Write(action) => self.write_experience(action),
            Command::Confirm(label, action) => {
                let native = self.experience_native_path();
                self.experience.screens.push(Screen::Confirm {
                    label,
                    action,
                    native,
                });
            }
            Command::Native(path) => self.open_reddit_link(&path),
            Command::BodyLink(link, copy) => {
                self.push_menu(
                    &link.label,
                    vec![
                        choice(
                            "o",
                            &format!("Open in browser ↗\n{}", link.url),
                            Command::OpenUrl(link.url.clone()),
                        ),
                        choice("y", "Copy URL", Command::CopyUrl(link.url)),
                    ],
                    MenuScope::Links,
                );
                if let Some(Screen::Menu { selected, .. }) = self.experience.screens.last_mut() {
                    *selected = usize::from(copy);
                }
            }
            Command::OpenUrl(url) => {
                match media::spawn_detached(&self.config.commands.browser, &[url]) {
                    Ok(()) => self.info("Opened link in your browser"),
                    Err(error) => self.error(format!("Could not open link: {error}")),
                }
            }
            Command::CopyUrl(url) => self.copy_url(&url, false),
            Command::Membership(target) => self.show_membership_for(target),
            Command::User(name) => {
                self.clear_experience();
                self.overlay = None;
                self.close_post();
                self.focus_main();
                self.open_user(&name);
            }
            Command::Subreddit(sub) => {
                self.clear_experience();
                self.overlay = None;
                self.close_post();
                self.focus_main();
                self.open_subreddit(&sub);
            }
            Command::MarkCommunityRead(sub) => {
                self.leave_experience();
                self.mark_community_read(&sub);
            }
            Command::Sort(sort) => {
                self.leave_experience();
                self.sort_comments(&sort);
            }
            Command::Target(target, post) => {
                if let Some(comment) = target.comment()
                    && self.select_loaded_comment(comment)
                {
                    self.leave_experience();
                    return;
                }
                if let Some(post) = post {
                    self.leave_experience();
                    self.open_post_permalink(post, target.permalink);
                    self.focus = Focus::Post;
                } else if !target.permalink.is_empty() {
                    self.read_experience(Query::Thread(target.permalink), None);
                } else {
                    self.open_reddit_link("/notifications");
                }
            }
        }
    }
}
