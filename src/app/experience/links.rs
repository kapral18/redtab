use super::*;

impl App {
    pub(crate) fn clear_experience(&mut self) {
        self.experience = Experience::default();
        self.pool.want_experience(None);
    }

    pub fn show_open_link(&mut self) {
        if self.experience.writing() {
            return;
        }
        self.clear_experience();
        self.experience.screens.push(Screen::Input(form(
            "Open Reddit URL",
            FormKind::OpenLink,
            vec![field("Reddit URL", "", false)],
            "/",
        )));
        self.overlay = Some(Overlay::Experience);
    }

    pub(crate) fn show_profile_input(&mut self) {
        if self.experience.writing() {
            return;
        }
        self.clear_experience();
        self.experience.screens.push(Screen::Input(form(
            "User profile",
            FormKind::Profile,
            vec![field("Username", "", false)],
            "/",
        )));
        self.overlay = Some(Overlay::Experience);
    }

    pub fn open_reddit_link(&mut self, path: &str) {
        match reddit_url(path)
            .and_then(|url| media::spawn_detached(&self.config.commands.browser, &[url]))
        {
            Ok(()) => self.info("Opened Reddit in your browser"),
            Err(error) => self.error(format!("Could not open Reddit: {error}")),
        }
    }

    pub fn comment_target(&self) -> Option<Target> {
        if !self.post_focused() {
            return None;
        }
        let view = self.post_view.as_ref()?;
        if view.free_scroll {
            return None;
        }
        let rows = view.rows();
        let RowKind::Comment { comment, .. } = &rows.get(view.cursor)?.kind else {
            return None;
        };
        if comment.id.starts_with(THREAD_ROOT) {
            return None;
        }
        let post = view.shown_post();
        Some(Target {
            id: format!("t1_{}", comment.id),
            author: comment.author.clone(),
            subreddit: post.subreddit.clone(),
            permalink: format!("/comments/{}/_/{}", post.id, comment.id),
        })
    }

    pub fn show_body_links(&mut self, copy: bool) {
        let (text, reveal) = match self.experience.screens.last() {
            Some(Screen::Browse {
                query,
                page,
                selected,
                ..
            }) if matches!(self.overlay, Some(Overlay::Experience)) => {
                let text = if !page.text.is_empty() && !matches!(query, Query::Thread(_)) {
                    page.text.as_str()
                } else {
                    page.entries
                        .get(*selected)
                        .map_or(page.text.as_str(), |entry| entry.body.as_str())
                };
                (text, false)
            }
            _ if self.overlay.is_none() && self.post_focused() => {
                let Some(view) = self
                    .post_view
                    .as_ref()
                    .filter(|view| !view.spoiler_hidden())
                else {
                    self.info(format!(
                        "Reveal spoilers with {} first",
                        crate::keys::map::action_label(crate::keys::map::Action::Spoilers)
                    ));
                    return;
                };
                let text = if !view.free_scroll
                    && let Some(Row {
                        kind: RowKind::Comment { comment, .. },
                        ..
                    }) = view.rows().get(view.cursor)
                    && !comment.id.starts_with(THREAD_ROOT)
                {
                    comment.body.as_str()
                } else {
                    view.page.as_ref().map_or("", |page| page.body.as_str())
                };
                (text, view.reveal_spoilers)
            }
            _ => return,
        };
        let choices: Vec<_> = crate::model::body_links(text, reveal)
            .into_iter()
            .map(|link| {
                let label = link.label.clone();
                choice("", &label, Command::BodyLink(link, copy))
            })
            .collect();
        if choices.is_empty() {
            self.info("No visible links in this text");
            return;
        }
        if self.overlay.is_none() {
            self.experience.screens.clear();
        }
        self.push_menu("Links in text", choices, MenuScope::Links);
    }

    pub(crate) fn action_target(&self) -> Option<Target> {
        if let Some(Screen::Browse { page, selected, .. }) = self.experience.screens.last()
            && self
                .overlay
                .as_ref()
                .is_some_and(|o| matches!(o, Overlay::Experience))
        {
            return page
                .entries
                .get(*selected)
                .and_then(|entry| entry.target.clone());
        }
        self.comment_target()
            .or_else(|| self.current_post().as_ref().map(Target::from_post))
    }

    pub(super) fn experience_community(&self) -> String {
        if matches!(self.overlay, Some(Overlay::Experience))
            && let Some(Screen::Browse {
                query,
                page,
                selected,
                ..
            }) = self.experience.screens.last()
        {
            let entry = page.entries.get(*selected);
            if entry
                .and_then(|entry| entry.target.as_ref())
                .is_some_and(|target| crate::model::profile_user(&target.permalink).is_some())
            {
                return String::new();
            }
            let name = entry
                .and_then(|entry| entry.target.as_ref())
                .map(|target| target.subreddit.as_str())
                .or_else(|| {
                    entry
                        .and_then(|entry| entry.query.as_ref())
                        .into_iter()
                        .chain([query])
                        .find_map(|query| match query {
                            Query::Community(name)
                            | Query::Rules(name)
                            | Query::Moderators(name)
                            | Query::Wiki {
                                subreddit: name, ..
                            } => Some(name.as_str()),
                            _ => None,
                        })
                });
            return name
                .filter(|name| is_community(name))
                .unwrap_or_default()
                .to_string();
        }
        self.community()
            .filter(|name| is_community(name))
            .unwrap_or_default()
    }

    pub(crate) fn experience_native(&mut self) {
        self.open_reddit_link(&self.experience_native_path());
    }

    pub(crate) fn open_experience_item(&mut self) {
        self.open_reddit_link(&self.experience_item_path());
    }

    pub(super) fn experience_item_path(&self) -> String {
        if let Some(Screen::Browse { page, selected, .. }) = self.experience.screens.last()
            && let Some(entry) = page.entries.get(*selected)
        {
            if let Some(path) = entry.query.as_ref().and_then(|query| query.url().ok()) {
                return path;
            }
            if let Some(target) = &entry.target
                && !target.permalink.is_empty()
            {
                return target.permalink.clone();
            }
        }
        self.experience_native_path()
    }

    pub(crate) fn copy_experience_link(&mut self) {
        let path = self.experience_item_path();
        match reddit_url(&path) {
            Ok(url) => self.copy_url(&url, false),
            Err(error) => self.error(format!("copy failed: {error:#}")),
        }
    }

    pub(super) fn experience_native_path(&self) -> String {
        match self.experience.screens.last() {
            Some(Screen::Input(form)) => form.native.clone(),
            Some(Screen::Menu { native, .. } | Screen::Confirm { native, .. }) => native.clone(),
            Some(Screen::Browse { page, .. }) => page.url.clone(),
            Some(Screen::Loading(query)) => query.url().unwrap_or_else(|_| "/".into()),
            _ => self
                .action_target()
                .map(|target| target.permalink)
                .unwrap_or_else(|| "/".into()),
        }
    }
}
