use super::*;

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

    /// Whether Reddit preferences can be read now, or which connection step comes first.
    pub fn preference_access(&self) -> PreferenceAccess {
        if self.reddit.needs_sign_in() {
            PreferenceAccess::SignIn
        } else if self.reddit.account().is_none() {
            PreferenceAccess::Reconnect
        } else {
            PreferenceAccess::Readable
        }
    }

    /// What to do before Reddit preferences can change, or None when they can be read now.
    pub(super) fn preference_access_message(&self) -> Option<&'static str> {
        match self.preference_access() {
            PreferenceAccess::SignIn => Some("Sign in with SPC p a to change Reddit preferences"),
            PreferenceAccess::Reconnect => Some("Reconnect with SPC p a to identify this account"),
            PreferenceAccess::Readable => None,
        }
    }

    /// Loaded Reddit preferences F2 may show and change: none once the connection cannot read them.
    pub fn shown_preferences(&self) -> Option<&crate::reddit::Preferences> {
        self.settings
            .account
            .values
            .as_ref()
            .filter(|_| self.preference_access() == PreferenceAccess::Readable)
    }

    pub(in crate::app) fn load_account_preferences(&mut self, refresh: bool) {
        let Some(account) = self
            .reddit
            .account()
            .filter(|_| self.preference_access() == PreferenceAccess::Readable)
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

    pub(super) fn change_account_preference(&mut self, key: &str, value: toml::Value) {
        if let Some(message) = self.preference_access_message() {
            self.settings.message = Some(message.into());
            return;
        }
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
            .shown_preferences()
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

    pub(super) fn poll_account_preferences(&mut self) -> bool {
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
}
