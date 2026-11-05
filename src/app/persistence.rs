use super::*;

impl App {
    /// Save state. True when something visible changed: a merge changed the bookmark
    /// list (the selection follows its post), or saving failed. The merge happens even
    /// when the write fails.
    pub(super) fn persist(&mut self) -> bool {
        match self.persist_checked() {
            Ok(changed) => changed,
            Err(error) => {
                self.error(format!("cannot save state: {error:#}"));
                true
            }
        }
    }

    pub(super) fn persist_checked(&mut self) -> anyhow::Result<bool> {
        self.last_save = Instant::now();
        let before = self.bookmark_ids();
        let before_view = self.state.list_view();
        let before_reads = self.state.read_revision();
        let before_badges: Vec<_> = self.tabs.iter().map(|tab| self.badge(tab)).collect();
        let selected = self.selected_bookmark_id();
        let result = self.state.save(&self.state_path, now());
        let changed = self.bookmark_ids() != before;
        if changed {
            self.reselect_bookmark(selected);
        }
        result?;
        Ok(changed
            || before_view != self.state.list_view()
            || before_reads != self.state.read_revision()
            || before_badges
                != self
                    .tabs
                    .iter()
                    .map(|tab| self.badge(tab))
                    .collect::<Vec<_>>())
    }

    /// Bookmarks ignore keyword filters, but still respect the adult-content preference.
    pub fn visible_bookmarks(&self) -> impl Iterator<Item = &crate::state::Bookmark> {
        self.state
            .bookmarks()
            .iter()
            .filter(|b| self.config.show_nsfw || !b.post.nsfw)
    }

    pub(super) fn bookmark_ids(&self) -> Vec<String> {
        self.visible_bookmarks()
            .map(|b| b.post.id.clone())
            .collect()
    }

    pub(super) fn selected_bookmark_id(&self) -> Option<String> {
        let selected = self.tabs[BOOKMARKS_TAB].feed.selected;
        self.visible_bookmarks()
            .nth(selected)
            .map(|b| b.post.id.clone())
    }

    /// Keep the Bookmarks selection on its post after the list changed, else clamp it.
    pub(super) fn reselect_bookmark(&mut self, id: Option<String>) {
        let bookmarks = self.bookmark_ids();
        let cards = self.state.list_view() == ListView::Card;
        let open_here = self.active == BOOKMARKS_TAB
            && id.is_some()
            && self.post_view.as_ref().map(|view| &view.shown_post().id) == id.as_ref();
        let feed = &mut self.tabs[BOOKMARKS_TAB].feed;
        if feed.untouched_top(cards, open_here) {
            feed.show_top(bookmarks.first().cloned());
            self.preview_selection();
            return;
        }
        let screen_row = feed.selected.saturating_sub(feed.offset);
        feed.selected = id
            .and_then(|id| bookmarks.iter().position(|b| *b == id))
            .unwrap_or(feed.selected)
            .min(bookmarks.len().saturating_sub(1));
        feed.offset = feed.selected.saturating_sub(screen_row);
        self.preview_selection();
    }

    pub fn save_state(&mut self) {
        self.persist();
    }

    #[cfg(test)]
    pub fn state_path(&self) -> &std::path::Path {
        &self.state_path
    }

    pub fn toggle_bookmark(&mut self) {
        let Some(post) = self.current_post() else {
            return;
        };
        let selected = self.selected_bookmark_id();
        let saved = self.state.toggle_bookmark(&post, now());
        self.reselect_bookmark(selected);
        self.info(if saved {
            "Bookmarked"
        } else {
            "Bookmark removed"
        });
        // Persist bookmarks at once; after the message so a save error replaces it.
        self.persist();
    }
}
