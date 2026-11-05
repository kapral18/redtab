use super::*;

pub(super) struct IconMetadata {
    pub(super) url: Option<String>,
    pub(super) source: Source,
}

impl App {
    /// The icon to draw for a subreddit: thumbnails on, icon known and not failed.
    pub fn shown_icon(&self, subreddit: &str) -> Option<&str> {
        if !self.thumbnails_on() {
            return None;
        }
        // Names are ASCII; most are already lowercase, so skip the allocation then.
        let icon = if subreddit.bytes().any(|b| b.is_ascii_uppercase()) {
            self.icons.get(&subreddit.to_ascii_lowercase())
        } else {
            self.icons.get(subreddit)
        };
        self.usable_thumb(icon.and_then(|metadata| metadata.url.as_deref()))
    }

    pub fn user_profile(&self, user: &str) -> Option<&crate::model::UserProfile> {
        self.profiles.get(&user.to_ascii_lowercase())
    }

    /// Asks once per session for a shown person's profile; `r` on their tab asks again.
    pub(super) fn request_profile(&mut self, user: &str, again: bool) {
        let key = user.to_ascii_lowercase();
        if self.profile_loading.contains(&key)
            || (!again && (self.profiles.contains_key(&key) || self.profile_failed.contains(&key)))
        {
            return;
        }
        self.profile_failed.remove(&key);
        self.profile_loading.insert(key);
        self.pool.submit(Job::Profile {
            user: user.to_string(),
        });
    }

    pub fn profile_failed(&self, user: &str) -> bool {
        self.profile_failed.contains(&user.to_ascii_lowercase())
    }

    pub(crate) fn observe_profile(&mut self, user: &str, profile: crate::model::UserProfile) {
        self.profiles.insert(user.to_ascii_lowercase(), profile);
    }

    pub(super) fn observe_icon(&mut self, name: &str, url: Option<String>, source: Source) {
        let key = name.to_ascii_lowercase();
        if self
            .icons
            .get(&key)
            .is_none_or(|old| source.may_replace(old.source))
        {
            self.icons.insert(key, IconMetadata { url, source });
        }
    }

    /// Request missing icon metadata once per visible demand. Leaving the view or
    /// explicitly reloading permits another attempt; redraws do not retry failures.
    pub fn demand_icon(&mut self, subreddit: &str) {
        if !self.thumbnails_on()
            || self.popup_open()
            || self.picture_settle.holding(Instant::now())
            || !self.pool.icon_wanted(subreddit)
            || (matches!(self.active_tab().kind, TabKind::AllSubs)
                && self.all_subs.waiting_for_rows)
        {
            return;
        }
        if self.icon_demands.insert(subreddit.to_ascii_lowercase()) {
            self.request_icon(subreddit);
        }
    }

    pub(super) fn request_icon(&mut self, subreddit: &str) {
        let key = subreddit.to_ascii_lowercase();
        if !self.thumbnails_on()
            || matches!(key.as_str(), "all" | "popular")
            || key.contains('+')
            || self.icons.contains_key(&key)
            || self.icon_lookups.contains(&key)
        {
            return;
        }
        self.icon_lookups.insert(key.clone());
        self.pool.submit(Job::Icon { subreddit: key });
    }
}
