//! Settings editing and application; persistence belongs to config::store.
use super::{App, CardScroll, Feed, Group, Overlay, TabKind, experience::Field};
use crate::config::{
    managed::{Chezmoi, Plan, Verification},
    schema::{Kind, SETTINGS, Scope},
    store::Store,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

pub struct Task<T> {
    receiver: Receiver<Result<T, String>>,
    thread: Option<std::thread::JoinHandle<()>>,
    join_on_drop: bool,
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
            join_on_drop: true,
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
        if let Some(thread) = self.thread.take().filter(|_| self.join_on_drop) {
            let _ = thread.join();
        }
    }
}

pub enum ManagedSave {
    Checking(Task<Option<Plan>>),
    Confirm(Box<Plan>),
    Applying(Task<()>, Verification),
}

#[derive(Default)]
pub struct AccountSettings {
    pub values: Option<crate::reddit::Preferences>,
    task: Option<Task<crate::reddit::Preferences>>,
    owner: Option<String>,
    pub writing: bool,
    pub error: Option<String>,
    pub loaded: bool,
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
    pub account: AccountSettings,
    pub(crate) chezmoi: Chezmoi,
    last_poll: Instant,
}
impl AccountSettings {
    pub fn pending(&self) -> bool {
        self.task.is_some()
    }
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
            account: AccountSettings::default(),
            chezmoi: Chezmoi::default(),
            last_poll: Instant::now(),
        }
    }

    pub fn busy(&self) -> bool {
        self.managed_busy() || self.account.task.is_some()
    }

    pub fn managed_busy(&self) -> bool {
        matches!(
            self.managed,
            Some(ManagedSave::Checking(_) | ManagedSave::Applying(..))
        )
    }

    pub fn writing(&self) -> bool {
        self.account.writing
    }

    pub fn scope_label(&self, scope: Scope, account: Option<&crate::reddit::Account>) -> String {
        let name = account.map_or_else(
            || {
                if self.store.current.active_account.as_deref() == Some("") {
                    "Unidentified account".into()
                } else {
                    "Signed out".into()
                }
            },
            |a| format!("u/{}", a.name),
        );
        match scope {
            Scope::Reddit => format!("Reddit settings · {name}"),
            Scope::Reader => format!("Reader settings · {name}"),
            Scope::Device => "This device · all accounts".into(),
        }
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
    pub fn install_settings_store(&mut self, mut store: Store) {
        let account = self.reddit.account();
        let username = account
            .as_ref()
            .map(|a| a.name.as_str())
            .or_else(|| (!self.reddit.needs_sign_in()).then_some(""));
        store.select_account(username);
        self.settings.store = store;
        self.apply_settings();
    }

    pub(super) fn load_account_preferences(&mut self, refresh: bool) {
        let Some(account) = self
            .reddit
            .account()
            .filter(|_| !self.reddit.needs_sign_in())
        else {
            return;
        };
        if self.settings.account.task.is_some() || (!refresh && self.settings.account.loaded) {
            return;
        }
        let reddit = self.reddit.clone();
        let mut task = Task::start(move || reddit.preferences());
        task.join_on_drop = false;
        self.settings.account.owner = Some(account.id);
        self.settings.account.task = Some(task);
        self.settings.account.writing = false;
        self.settings.account.error = None;
    }

    fn change_account_preference(&mut self, key: &str, value: toml::Value) {
        if self.settings.account.task.is_some() {
            return;
        }
        if matches!(
            self.auth.phase,
            crate::auth::Phase::Saving
                | crate::auth::Phase::Switching
                | crate::auth::Phase::Disconnecting
        ) {
            self.settings.message =
                Some("Wait for the account change before saving Reddit preferences".into());
            return;
        }
        let Some(current) = self
            .settings
            .account
            .values
            .as_ref()
            .filter(|_| self.settings.account.error.is_none())
        else {
            self.settings.message =
                Some("Reddit preferences are unavailable; r retries loading them".into());
            return;
        };
        let Some(value) = value.as_bool() else {
            self.settings.message = Some("Expected true or false".into());
            return;
        };
        if key == "show_nsfw" && !current.mature_content_editable {
            self.settings.message = Some("Reddit requires you to complete this setting in the browser. Open reddit.com/settings/preferences.".into());
            return;
        }
        let key = if key == "show_nsfw" {
            crate::reddit::Preference::MatureContent
        } else {
            crate::reddit::Preference::Autoplay
        };
        if key.value(current) == value {
            return;
        }
        let Some(account) = self.reddit.account() else {
            return;
        };
        let reddit = self.reddit.clone();
        self.settings.account.owner = Some(account.id);
        self.settings.account.writing = true;
        self.settings.account.error = None;
        self.settings.message = None;
        self.settings.account.task = Some(Task::start(move || reddit.set_preference(key, value)));
    }

    fn poll_account_preferences(&mut self) -> bool {
        if self.membership.is_some() || self.experience.writing() {
            return false;
        }
        let Some(result) = self.settings.account.task.as_ref().and_then(Task::poll) else {
            return false;
        };
        self.settings.account.task = None;
        let was_writing = std::mem::take(&mut self.settings.account.writing);
        if self.reddit.account().map(|a| a.id) != self.settings.account.owner {
            return false;
        }
        self.settings.account.loaded = true;
        match result {
            Ok(values) => {
                self.settings.account.values = Some(values);
                self.settings.account.error = None;
                self.apply_settings();
            }
            Err(error) => {
                if was_writing {
                    self.settings.account.values = None;
                }
                self.settings.account.error = Some(error);
            }
        }
        true
    }

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
        if self.membership.is_some() || self.experience.writing() || self.settings.writing() {
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
        self.load_account_preferences(true);
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
        if field.scope == Scope::Reddit {
            self.change_account_preference(field.key, value);
            return;
        }
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
        let account_changed = self.poll_account_preferences();
        if self.settings.managed.is_some() {
            return self.poll_managed_save() || account_changed;
        }
        if now.saturating_duration_since(self.settings.last_poll) < Duration::from_millis(500)
            || self.membership.is_some()
            || self.experience.writing()
            || self.settings.writing()
        {
            return account_changed;
        }
        self.settings.last_poll = now;
        let before = (
            self.settings.store.persistence,
            self.settings.store.error.clone(),
        );
        self.settings.store.reload();
        let changed = self.apply_settings();
        changed
            || account_changed
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

    pub(super) fn apply_settings(&mut self) -> bool {
        let mut next = self.settings.store.current.clone();
        if let Some(values) = &self.settings.account.values {
            next.show_nsfw = values.show_nsfw;
            next.media.video_autoplay = values.video_autoplay;
        } else {
            next.show_nsfw = self.config.show_nsfw;
            next.media.video_autoplay = self.config.media.video_autoplay;
        }
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
            self.video_previews.clear();
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
        self.video_previews.clear();
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
                if SETTINGS[self.settings.selected].scope == Scope::Reddit {
                    self.load_account_preferences(true);
                    return;
                }
                self.settings.store.save();
                self.apply_settings();
                self.settings.message = None;
                self.begin_managed_save();
            }
            KeyCode::Char('u') if self.settings_allowed() => {
                if SETTINGS[self.settings.selected].scope == Scope::Reddit {
                    self.settings.message = Some("Reddit saves immediately. Toggle the preference to change it; r reloads account values.".into());
                    return;
                }
                self.settings.store.revert();
                self.apply_settings();
            }
            _ => {}
        }
    }

    fn managed_save_key(&mut self, key: KeyEvent) {
        if self.settings.managed_busy() || !key.modifiers.is_empty() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reddit::{
        Preferences,
        tests::{Fixture, Reply, saved_export},
    };
    use crate::{config::Config, worker::Pool};
    use ratatui_image::picker::Picker;
    use serde_json::json;

    fn fixture() -> Fixture {
        Fixture::new(|request| {
            match request.url.as_str() {
            "/settings/preferences" => Reply { status:200, body:"<settings-preferences initialstate='{\"identity\":{\"preferences\":{\"isNsfwContentShown\":false,\"isVideoAutoplayDisabled\":true}}}'></settings-preferences>".into(), headers:vec![] },
            _ => Reply::json(json!({"kind":"Listing","data":{"children":[]}})),
        }
        })
    }

    fn app(fixture: &Fixture, config: Config, directory: &std::path::Path) -> App {
        let reddit = fixture.saved_reddit(&saved_export("first", "reader"));
        let (pool, _) = Pool::start(reddit.clone(), 1, None);
        App::new(
            config,
            pool,
            reddit,
            directory.join("accounts/first/state.json"),
            Picker::halfblocks(),
        )
    }

    fn finish(app: &mut App) {
        if let Some(thread) = app
            .settings
            .account
            .task
            .as_mut()
            .and_then(|t| t.thread.take())
        {
            thread.join().unwrap();
        }
        app.poll_account_preferences();
    }

    fn completed_preference_read(app: &mut App) {
        let mut task = Task::start(|| {
            Ok(Preferences {
                show_nsfw: true,
                video_autoplay: true,
                mature_content_editable: true,
            })
        });
        task.thread.take().unwrap().join().unwrap();
        app.settings.account.task = Some(task);
        app.settings.account.owner = Some("first".into());
    }

    #[test]
    fn scoped_settings_completed_preference_read_should_wait_for_membership_write() {
        let directory = tempfile::tempdir().unwrap();
        let fixture = fixture();
        let mut app = app(&fixture, Config::default(), directory.path());
        completed_preference_read(&mut app);
        app.membership = Some(crate::app::MembershipChange {
            generation: 9,
            target: crate::model::MembershipTarget::Community("rust".into()),
            join: true,
            checking: false,
            error: None,
        });
        assert!(!app.poll_account_preferences());
        assert!(app.settings.account.pending());
        assert!(app.settings.account.values.is_none());
        assert!(app.membership.is_some());
        assert!(app.take_replacement_messages().is_none());
        app.membership = None;
        assert!(app.poll_account_preferences());
        assert!(app.config.show_nsfw);
        assert!(!app.settings.account.pending());
    }

    #[test]
    fn scoped_settings_completed_preference_read_should_preserve_a_pending_experience_write() {
        use crate::reddit::experience::{Action, Request};
        let directory = tempfile::tempdir().unwrap();
        let fixture = fixture();
        let mut app = app(&fixture, Config::default(), directory.path());
        completed_preference_read(&mut app);
        app.experience.pending = Some((
            91,
            Request::Write(Action::Save {
                id: "t3_post".into(),
                save: true,
            }),
        ));
        assert!(!app.poll_account_preferences());
        assert!(app.settings.account.pending());
        assert!(app.experience.writing());
        assert!(!app.config.show_nsfw);
        assert!(app.take_replacement_messages().is_none());
    }

    fn managed_with_pending_account_read() -> (
        App,
        crate::config::managed::tests::Fixture,
        std::sync::mpsc::Sender<()>,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let reddit_fixture = fixture();
        let mut app = app(&reddit_fixture, Config::default(), directory.path());
        let managed = crate::config::managed::tests::Fixture::new();
        app.install_settings_store(managed.store());
        app.settings.chezmoi = managed.tool.clone();
        app.overlay = Some(Overlay::Settings);
        let (release, gate) = mpsc::channel();
        let mut task = Task::start(move || {
            let _ = gate.recv_timeout(Duration::from_secs(5));
            Ok(Preferences {
                show_nsfw: false,
                video_autoplay: false,
                mature_content_editable: true,
            })
        });
        task.join_on_drop = false;
        app.settings.account.task = Some(task);
        app.settings.account.owner = Some("first".into());
        app.change_setting("media.image_preview", false.into());
        app.settings.wait_managed();
        app.poll_managed_save();
        assert!(matches!(
            app.settings.managed,
            Some(ManagedSave::Confirm(_))
        ));
        (app, managed, release)
    }

    #[test]
    fn scoped_settings_pending_reddit_read_should_not_block_managed_save_confirmation() {
        let (mut app, fixture, release) = managed_with_pending_account_read();
        app.settings_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(
            app.settings.managed,
            Some(ManagedSave::Applying(..))
        ));
        app.settings.wait_managed();
        app.poll_managed_save();
        assert!(app.settings.managed.is_none());
        assert!(
            !Config::load(Some(&fixture.target))
                .unwrap()
                .media
                .image_preview
        );
        let _ = release.send(());
    }

    #[test]
    fn scoped_settings_pending_reddit_read_should_not_block_managed_save_cancellation() {
        let (mut app, _fixture, release) = managed_with_pending_account_read();
        app.settings_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.settings.managed.is_none());
        assert!(app.settings.account.pending());
        let _ = release.send(());
    }

    #[test]
    fn scoped_settings_pending_write_should_guard_an_already_open_account_selection() {
        let directory = tempfile::tempdir().unwrap();
        let fixture = fixture();
        let mut app = app(&fixture, Config::default(), directory.path());
        app.auth.phase = crate::auth::Phase::Accounts;
        app.auth.active_id = Some("first".into());
        app.auth.accounts = vec![crate::reddit::SavedAccount {
            id: "first".into(),
            name: "reader".into(),
            expired: false,
            source: None,
        }];
        app.settings.account.writing = true;
        assert!(!app.select_auth());
        assert_eq!(app.auth.phase, crate::auth::Phase::Accounts);
        assert!(
            app.auth
                .message
                .as_ref()
                .unwrap()
                .contains("preference save")
        );
        app.settings.account.writing = false;
    }

    #[test]
    fn scoped_settings_pending_write_should_guard_an_already_open_account_confirmation() {
        let directory = tempfile::tempdir().unwrap();
        let fixture = fixture();
        let mut app = app(&fixture, Config::default(), directory.path());
        app.auth.phase = crate::auth::Phase::Confirm;
        app.settings.account.writing = true;
        app.confirm_auth();
        assert_eq!(app.auth.phase, crate::auth::Phase::Confirm);
        assert!(
            app.auth
                .message
                .as_ref()
                .unwrap()
                .contains("preference save")
        );
        app.settings.account.writing = false;
    }

    #[test]
    fn scoped_settings_unidentified_session_should_not_use_or_edit_signed_out_rules() {
        let directory = tempfile::tempdir().unwrap();
        let reddit = crate::reddit::Reddit::test_at("http://127.0.0.1:9", false);
        let (pool, _) = Pool::start(reddit.clone(), 1, None);
        let config: Config =
            toml::from_str("[signed_out.filters]\nkeywords=[\"signed out\"]\n").unwrap();
        let mut app = App::new(
            config.clone(),
            pool,
            reddit,
            directory.path().join("state.json"),
            Picker::halfblocks(),
        );
        app.install_settings_store(Store::memory(config));
        assert!(app.config.filters.keywords.is_empty());
        app.change_setting("filters.keywords", toml::Value::Array(vec!["wrong".into()]));
        assert!(app.settings.message.as_ref().unwrap().contains("identify"));
        assert!(!app.settings.store.dirty());
        assert_eq!(
            app.settings.store.current.signed_out.filters.keywords,
            ["signed out"]
        );
    }

    #[test]
    fn scoped_settings_switch_should_replace_reader_rules_and_keep_device_preferences() {
        let directory = tempfile::tempdir().unwrap();
        let fixture = fixture();
        let config:Config=toml::from_str("[accounts.ReAdEr.filters]\nkeywords=[\"first\"]\n[accounts.Other.highlight]\nkeywords=[\"second\"]\n[media]\nimage_preview=false\n").unwrap();
        let mut app = app(&fixture, config, directory.path());
        assert_eq!(app.config.filters.keywords, ["first"]);
        app.install_connection(fixture.saved_reddit(&saved_export("second", "other")));
        assert!(app.config.filters.keywords.is_empty());
        assert_eq!(app.config.highlight.keywords, ["second"]);
        assert!(!app.show_images);
        app.install_connection(fixture.saved_reddit(&saved_export("first", "READER")));
        assert_eq!(app.config.filters.keywords, ["first"]);
        assert!(app.config.highlight.keywords.is_empty());
        assert!(!app.show_images);
    }

    #[test]
    fn scoped_settings_switch_should_not_wait_for_an_obsolete_preference_read() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        let directory = tempfile::tempdir().unwrap();
        let fixture = fixture();
        let mut app = app(&fixture, Config::default(), directory.path());
        let (release, gate) = mpsc::channel();
        let completed = Arc::new(AtomicBool::new(false));
        let done = completed.clone();
        let mut task = Task::start(move || {
            let _ = gate.recv_timeout(Duration::from_secs(2));
            done.store(true, Ordering::SeqCst);
            Ok(Preferences {
                show_nsfw: true,
                video_autoplay: true,
                mature_content_editable: true,
            })
        });
        task.join_on_drop = false;
        app.settings.account.task = Some(task);
        app.settings.account.owner = Some("first".into());
        app.install_connection(fixture.saved_reddit(&saved_export("second", "other")));
        assert!(!completed.load(Ordering::SeqCst));
        assert!(!app.config.show_nsfw && !app.config.media.video_autoplay);
        let _ = release.send(());
    }

    #[test]
    fn scoped_settings_stale_account_completion_should_not_change_the_active_policy() {
        let directory = tempfile::tempdir().unwrap();
        let fixture = fixture();
        let mut app = app(&fixture, Config::default(), directory.path());
        app.settings.account.owner = Some("other".into());
        app.settings.account.task = Some(Task::start(|| {
            Ok(Preferences {
                show_nsfw: true,
                video_autoplay: true,
                mature_content_editable: true,
            })
        }));
        finish(&mut app);
        assert!(app.settings.account.values.is_none());
        assert!(!app.config.show_nsfw);
    }

    #[test]
    fn scoped_settings_failed_write_should_require_readback_before_another_change() {
        let directory = tempfile::tempdir().unwrap();
        let fixture = fixture();
        let mut app = app(&fixture, Config::default(), directory.path());
        app.settings.account.values = Some(Preferences {
            show_nsfw: false,
            video_autoplay: true,
            mature_content_editable: true,
        });
        app.settings.account.owner = Some("first".into());
        app.settings.account.writing = true;
        app.settings.account.task = Some(Task::start(|| anyhow::bail!("Save may have succeeded")));
        finish(&mut app);
        assert!(app.settings.account.values.is_none());
        assert!(
            app.settings
                .account
                .error
                .as_ref()
                .unwrap()
                .contains("may have succeeded")
        );
        app.change_setting("media.video_autoplay", false.into());
        assert!(app.settings.account.task.is_none());
        assert!(!app.settings.writing());
        assert!(!app.settings.store.dirty());
    }

    #[test]
    fn scoped_settings_loading_reddit_should_leave_file_edits_usable() {
        let directory = tempfile::tempdir().unwrap();
        let fixture = fixture();
        let mut app = app(&fixture, Config::default(), directory.path());
        app.load_account_preferences(false);
        app.change_setting("filters.keywords", toml::Value::Array(vec!["local".into()]));
        assert_eq!(app.config.filters.keywords, ["local"]);
        finish(&mut app);
        assert_eq!(app.config.filters.keywords, ["local"]);
        assert!(!app.config.media.video_autoplay);
    }
}
