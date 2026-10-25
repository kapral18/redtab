//! Settings editing and application; persistence belongs to config::store.
use super::{App, CardScroll, Feed, Group, Overlay, TabKind, experience::Field};
use crate::config::{
    managed::{Chezmoi, Plan, Verification},
    schema::{Kind, SETTINGS},
    store::Store,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

pub struct Task<T> {
    receiver: Receiver<Result<T, String>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl<T: Send + 'static> Task<T> {
    fn start(work: impl FnOnce() -> anyhow::Result<T> + Send + 'static) -> Self {
        let (sender, receiver) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let _ = sender.send(work().map_err(|error| format!("{error:#}")));
        });
        Self {
            receiver,
            thread: Some(thread),
        }
    }

    fn poll(&self) -> Option<Result<T, String>> {
        match self.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                Some(Err("Settings task stopped; retry saving".into()))
            }
        }
    }
}

impl<T> Drop for Task<T> {
    fn drop(&mut self) {
        // Commands are bounded; shutdown must finish an approved source/apply transaction.
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub enum ManagedSave {
    Checking(Task<Option<Plan>>),
    Confirm(Box<Plan>),
    Applying(Task<()>, Verification),
}

pub struct Settings {
    pub store: Store,
    pub selected: usize,
    pub editing: Option<Field>,
    edit_base: Option<toml::Value>,
    return_overlay: Option<Overlay>,
    pub message: Option<String>,
    pub managed: Option<ManagedSave>,
    pub managed_scroll: usize,
    pub(crate) chezmoi: Chezmoi,
    last_poll: Instant,
}
impl Settings {
    pub fn new(store: Store) -> Self {
        Self {
            store,
            selected: 0,
            editing: None,
            edit_base: None,
            return_overlay: None,
            message: None,
            managed: None,
            managed_scroll: 0,
            chezmoi: Chezmoi::default(),
            last_poll: Instant::now(),
        }
    }

    pub fn busy(&self) -> bool {
        matches!(
            self.managed,
            Some(ManagedSave::Checking(_) | ManagedSave::Applying(..))
        )
    }

    #[cfg(test)]
    pub fn wait_managed(&mut self) {
        let thread = match &mut self.managed {
            Some(ManagedSave::Checking(task)) => task.thread.take(),
            Some(ManagedSave::Applying(task, _)) => task.thread.take(),
            _ => None,
        };
        if let Some(thread) = thread {
            thread.join().unwrap();
        }
    }
}

impl App {
    /// Async completions update the covered dialog without taking Settings' keys.
    pub(super) fn behind_settings<T>(&mut self, update: impl FnOnce(&mut Self) -> T) -> T {
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

    fn settings_allowed(&mut self) -> bool {
        if self.settings.managed.is_some() {
            self.info("Finish reviewing or saving the managed config first");
            return false;
        }
        if self.membership.is_some() || self.experience.writing() {
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

    pub(super) fn poll_settings(&mut self, now: Instant) -> bool {
        if self.settings.managed.is_some() {
            return self.poll_managed_save();
        }
        if now.saturating_duration_since(self.settings.last_poll) < Duration::from_millis(500)
            || self.membership.is_some()
            || self.experience.writing()
        {
            return false;
        }
        self.settings.last_poll = now;
        let before = (
            self.settings.store.persistence,
            self.settings.store.error.clone(),
        );
        self.settings.store.reload();
        let changed = self.apply_settings();
        changed
            || before
                != (
                    self.settings.store.persistence,
                    self.settings.store.error.clone(),
                )
    }

    fn begin_managed_save(&mut self) {
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

    fn poll_managed_save(&mut self) -> bool {
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

    fn apply_settings(&mut self) -> bool {
        let next = self.settings.store.current.clone();
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
            self.video_resume = None;
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
            let count = self
                .all_subs
                .groups
                .get(self.all_subs.group)
                .map_or(0, |g| self.group_posts(g).count());
            self.all_subs.post = self
                .all_subs
                .groups
                .get(self.all_subs.group)
                .and_then(|g| {
                    group_selected.and_then(|id| self.group_posts(g).position(|p| p.id == id))
                })
                .unwrap_or(self.all_subs.post)
                .min(count.saturating_sub(1));
            self.all_subs.offset = 0;
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

    fn reset_experience_reads(&mut self) {
        self.experience.reset_reads(self.post_view.is_some());
        if self.experience.screens.is_empty() {
            if matches!(self.overlay, Some(Overlay::Experience)) {
                self.overlay = None;
            }
            if matches!(self.settings.return_overlay, Some(Overlay::Experience)) {
                self.settings.return_overlay = None;
            }
        }
    }

    fn reload_content_policy(&mut self) {
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
        self.video_resume = None;
        self.card_video = None;
        self.pending_media.clear();
        self.queued_images.clear();
        self.icon_lookups.clear();
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
                ..Feed::default()
            };
        }
        for group in &mut self.all_subs.groups {
            *group = Group {
                subreddit: group.subreddit.clone(),
                ..Group::default()
            };
        }
        self.all_subs.loading = false;
        self.all_subs.error = None;
        self.all_subs.generation += 1;
        self.post_settle = super::Settle::default();
        self.feed_settle = super::Settle::default();
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

    pub fn settings_key(&mut self, key: KeyEvent) {
        if self.settings.managed.is_some() {
            self.managed_save_key(key);
            return;
        }
        if self.settings.editing.is_some() {
            self.settings_edit_key(key);
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::F(2) | KeyCode::Char('q') => {
                self.overlay = self.settings.return_overlay.take()
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.settings.selected = (self.settings.selected + 1).min(SETTINGS.len() - 1)
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.settings.selected = self.settings.selected.saturating_sub(1)
            }
            KeyCode::Home | KeyCode::Char('g') => self.settings.selected = 0,
            KeyCode::End | KeyCode::Char('G') => self.settings.selected = SETTINGS.len() - 1,
            KeyCode::Enter | KeyCode::Char(' ') => {
                let field = &SETTINGS[self.settings.selected];
                let value = field.value(&self.config);
                if field.kind == Kind::Boolean {
                    self.change_setting(field.key, toml::Value::Boolean(!value.as_bool().unwrap()));
                } else {
                    let text = value.to_string();
                    self.settings.editing = Some(Field {
                        label: field.key.into(),
                        cursor: text.len(),
                        value: text,
                        multiline: false,
                        required: true,
                    });
                    self.settings.edit_base = Some(value);
                    self.settings.message = None;
                }
            }
            KeyCode::Char('r') if self.settings_allowed() => {
                self.settings.store.save();
                self.apply_settings();
                self.settings.message = None;
                self.begin_managed_save();
            }
            KeyCode::Char('u') if self.settings_allowed() => {
                self.settings.store.revert();
                self.apply_settings();
            }
            _ => {}
        }
    }

    fn managed_save_key(&mut self, key: KeyEvent) {
        if self.settings.busy() || !key.modifiers.is_empty() {
            return;
        }
        match key.code {
            KeyCode::Enter | KeyCode::Char('y') => {
                if let Some(ManagedSave::Confirm(plan)) = self.settings.managed.take() {
                    let verification = plan.verification();
                    self.settings.managed = Some(ManagedSave::Applying(
                        Task::start(move || (*plan).apply()),
                        verification,
                    ));
                    self.settings.message = None;
                }
            }
            KeyCode::Esc | KeyCode::F(2) | KeyCode::Char('n' | 'q') => {
                self.settings.managed = None;
                self.settings.message = Some("Changes kept for this session.".into());
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.settings.managed_scroll = self.settings.managed_scroll.saturating_add(1)
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.settings.managed_scroll = self.settings.managed_scroll.saturating_sub(1)
            }
            KeyCode::PageDown => {
                self.settings.managed_scroll = self.settings.managed_scroll.saturating_add(8)
            }
            KeyCode::PageUp => {
                self.settings.managed_scroll = self.settings.managed_scroll.saturating_sub(8)
            }
            KeyCode::Home => self.settings.managed_scroll = 0,
            KeyCode::End => self.settings.managed_scroll = usize::MAX,
            _ => {}
        }
    }

    fn settings_edit_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if key.code == KeyCode::Esc {
            self.settings.editing = None;
            self.settings.message = None;
            return;
        }
        if key.code == KeyCode::Enter || (ctrl && key.code == KeyCode::Char('s')) {
            let field = &SETTINGS[self.settings.selected];
            let input = &self.settings.editing.as_ref().unwrap().value;
            let parsed = toml::from_str::<toml::Table>(&format!("value = {input}"))
                .ok()
                .and_then(|mut t| {
                    if t.len() == 1 {
                        t.remove("value")
                    } else {
                        None
                    }
                });
            let Some(value) = parsed else {
                self.settings.message = Some(format!("Expected {}", field.input_hint()));
                return;
            };
            if self.settings.edit_base.as_ref() != Some(&field.value(&self.config))
                && value != field.value(&self.config)
            {
                self.settings.message = Some("This value changed on disk while editing. Esc to review it; your draft has not been applied.".into());
                return;
            }
            self.change_setting(field.key, value);
            if self.settings.message.is_none() {
                self.settings.editing = None;
            }
            return;
        }
        let input = self.settings.editing.as_mut().unwrap();
        match key.code {
            KeyCode::Char('u') if ctrl => {
                input.value.clear();
                input.cursor = 0;
            }
            KeyCode::Left => input.cursor = input.previous(),
            KeyCode::Right => input.cursor = input.next(),
            KeyCode::Home => input.cursor = 0,
            KeyCode::End => input.cursor = input.value.len(),
            KeyCode::Backspace if !key.modifiers.contains(KeyModifiers::ALT) => {
                let start = input.previous();
                input.value.replace_range(start..input.cursor, "");
                input.cursor = start;
            }
            KeyCode::Delete => {
                let end = input.next();
                input.value.replace_range(input.cursor..end, "");
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::CONTROL) =>
            {
                input.insert(&c.to_string())
            }
            _ => {}
        }
    }

    pub fn settings_paste(&mut self, text: &str) {
        if let Some(input) = &mut self.settings.editing {
            input.insert(&text.chars().filter(|c| !c.is_control()).collect::<String>());
        }
    }
}
