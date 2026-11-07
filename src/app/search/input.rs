use super::*;

impl App {
    /// The tab key and command for F3, `SPC s e`, `SPC s p/c/C/m/u` and `/` outside communities.
    pub fn open_search(&mut self, kind: Option<SearchKind>) {
        if self.experience.writing() {
            return;
        }
        self.clear_experience();
        self.overlay = None;
        if SEARCH_TAB != self.active {
            self.switch_tab(SEARCH_TAB);
        }
        self.edit_search();
        if let Some(kind) = kind
            && kind != self.search.kind
        {
            self.search.kind = kind;
            self.search.selected = 0;
            self.search.offset = 0;
            self.schedule_search(Instant::now());
        }
        self.search_arrived();
    }

    pub(in crate::app) fn search_arrived(&mut self) {
        if self.search.exploring() {
            self.ensure_discovered();
        } else if self.search.searched.as_ref()
            != Some(&(self.search.text().to_owned(), self.search.kind))
        {
            self.schedule_search(Instant::now());
        } else {
            self.resume_search_rows(false);
        }
        self.schedule_preview(Instant::now());
    }

    pub(in crate::app) fn search_departed(&mut self) {
        self.retire_search_rows(false);
        self.retire_search_rows(true);
        // A pending ask-again waits for the return, like a request in flight.
        if self
            .search
            .empty_retry
            .as_mut()
            .is_some_and(|retry| retry.due.take().is_some())
        {
            self.search.results.suspended = true;
        }
        self.search.due = None;
        self.search.preview.due = None;
        self.search.preview_shown = false;
        self.search.preview_forced = false;
        if self.search.preview.loading {
            self.search.preview = Preview::default();
        }
    }

    pub(super) fn retire_search_rows(&mut self, discovery: bool) {
        let rows = if discovery {
            &mut self.search.discovered
        } else {
            &mut self.search.results
        };
        if let Some(query) = &rows.query {
            self.pool.want_search(query, None);
        }
        if rows.loading {
            rows.loading = false;
            rows.suspended = true;
        }
    }

    pub(super) fn resume_search_rows(&mut self, discovery: bool) -> bool {
        let rows = if discovery {
            &self.search.discovered
        } else {
            &self.search.results
        };
        if !rows.suspended {
            return false;
        }
        let Some(query) = rows.query.clone() else {
            return false;
        };
        let after = rows.appending.then(|| rows.after.clone()).flatten();
        self.request_rows(discovery, query, after);
        true
    }

    pub fn search_editing(&self) -> bool {
        self.search.editing
            && self.overlay.is_none()
            && self.active == SEARCH_TAB
            && !self.post_focused()
    }

    pub fn search_shows_rows(&self) -> bool {
        self.active == SEARCH_TAB && self.search.shows_rows()
    }

    /// `/` on the Search tab: back to typing, with the rows in view.
    pub fn edit_search(&mut self) {
        self.close_post();
        self.focus_main();
        self.search.editing = true;
    }

    pub fn search_paste(&mut self, text: &str) {
        let text: String = text
            .chars()
            .map(|c| {
                if matches!(c, '\n' | '\r' | '\t') {
                    ' '
                } else {
                    c
                }
            })
            .filter(|c| !c.is_control())
            .collect();
        if text.is_empty() {
            return;
        }
        self.search.query.insert(&text);
        self.search.selected = 0;
        self.search.offset = 0;
        self.schedule_search(Instant::now());
    }

    pub fn search_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers == KeyModifiers::CONTROL;
        let alt = key.modifiers == KeyModifiers::ALT;
        let field = &mut self.search.query;
        let before = field.value.clone();
        match key.code {
            KeyCode::Esc => {
                self.search.editing = false;
                return;
            }
            KeyCode::Down | KeyCode::Up => {
                self.move_search_result(if key.code == KeyCode::Down { 1 } else { -1 });
                return;
            }
            KeyCode::Char('n' | 'j') if ctrl => {
                self.move_search_result(1);
                return;
            }
            KeyCode::Char('p' | 'k') if ctrl => {
                self.move_search_result(-1);
                return;
            }
            // Enter commits the query: a pending search runs now and the results take the keys.
            KeyCode::Enter => {
                if self.search.due.is_some() {
                    self.run_search();
                }
                self.search.editing = false;
                return;
            }
            KeyCode::Tab | KeyCode::BackTab => {
                self.cycle_search_kind(key.code == KeyCode::Tab);
                return;
            }
            KeyCode::Backspace if alt => field.delete_word(),
            KeyCode::Char('w') if ctrl => field.delete_word(),
            KeyCode::Backspace => {
                let start = field.previous();
                field.value.replace_range(start..field.cursor, "");
                field.cursor = start;
            }
            KeyCode::Delete => {
                let end = field.next();
                field.value.replace_range(field.cursor..end, "");
            }
            KeyCode::Left => field.cursor = field.previous(),
            KeyCode::Right => field.cursor = field.next(),
            KeyCode::Home => field.cursor = 0,
            KeyCode::Char('a') if ctrl => field.cursor = 0,
            KeyCode::End => field.cursor = field.value.len(),
            KeyCode::Char('e') if ctrl => field.cursor = field.value.len(),
            KeyCode::Char('u') if ctrl => {
                field.value.clear();
                field.cursor = 0;
            }
            KeyCode::Char(c) if key.modifiers.difference(KeyModifiers::SHIFT).is_empty() => {
                field.insert(&c.to_string())
            }
            _ => return,
        }
        if self.search.query.value != before {
            self.search.selected = 0;
            self.search.offset = 0;
            self.schedule_search(Instant::now());
        }
    }

    /// Tab / S-Tab from the query or the result list; a typed query is searched again at once.
    pub fn cycle_search_kind(&mut self, forward: bool) {
        // From a feed opened beside the results, the switch happens back in Search.
        self.return_to_search();
        let kinds = SearchKind::ALL;
        let at = kinds
            .iter()
            .position(|kind| *kind == self.search.kind)
            .unwrap_or(0);
        let step = if forward { 1 } else { kinds.len() - 1 };
        self.search.kind = kinds[(at + step) % kinds.len()];
        self.search.selected = 0;
        self.search.offset = 0;
        // Another kind's results replace the post opened from these ones.
        self.close_post();
        self.schedule_search(Instant::now() - DEBOUNCE);
    }

    /// Up/Down while typing move through the shown results without leaving the query.
    pub(super) fn move_search_result(&mut self, delta: isize) {
        if self.search.shows_rows() {
            self.move_search(delta);
        } else {
            self.move_selection(delta);
        }
    }

    pub fn move_search(&mut self, delta: isize) {
        let len = self.search.rows().len();
        if len == 0 {
            return;
        }
        let selected = self
            .search
            .selected
            .saturating_add_signed(delta)
            .min(len - 1);
        if selected != self.search.selected {
            self.search.selected = selected;
            // A comment's thread beside the rows gives way to the next row's preview.
            if self.active == SEARCH_TAB {
                self.close_post();
            }
            self.schedule_preview(Instant::now());
        }
        self.load_more_rows();
    }

    /// The selected row while the Search rows have focus.
    fn search_row(&self) -> Option<Row> {
        (self.active == SEARCH_TAB
            && self.search.shows_rows()
            && self.focus == super::super::Focus::List
            && self.post_view.is_none())
        .then(|| self.search.selected_row())
        .flatten()
    }

    /// The community or person a selected row stands for, for `SPC c` and `SPC m`.
    pub(in crate::app) fn search_row_target(&self) -> Option<FeedTarget> {
        self.search_row().map(|row| row.target)
    }

    /// The selected Comments row's comment link; `search_row_path` stays its community.
    pub(crate) fn search_comment_link(&self) -> Option<String> {
        self.search_row()?.comment.map(|comment| comment.permalink)
    }

    pub(crate) fn search_row_path(&self) -> Option<String> {
        if self.overlay.is_some() {
            return None;
        }
        match self.search_row_target()? {
            FeedTarget::Subreddit(name) => {
                let name = crate::config::normalize_subreddit(&name);
                crate::config::is_subreddit_name(name).then(|| format!("/r/{name}/"))
            }
            FeedTarget::User(name) if crate::model::is_username(&name) => {
                Some(format!("/user/{name}/"))
            }
            _ => None,
        }
    }

    pub fn search_feed_open(&self) -> bool {
        self.search.opened_feed == Some(self.active_tab().id) && self.search.shows_rows()
    }

    pub(crate) fn search_source_context(&self) -> bool {
        self.search_shows_rows() || self.search_feed_open()
    }

    /// `q` on a feed opened beside the results closes it, preview included.
    pub fn close_search_feed(&mut self) -> bool {
        let closed = self.return_to_search();
        self.search.preview_closed |= closed;
        closed
    }

    /// `q` on the results closes the preview beside them before `q` can quit.
    pub fn close_search_preview(&mut self) -> bool {
        let open =
            self.active == SEARCH_TAB && self.search.preview_shown && !self.search.preview_closed;
        self.search.preview_closed |= open;
        open
    }

    pub fn search_preview_closed(&self) -> bool {
        self.search.preview_closed
    }

    pub fn return_to_search(&mut self) -> bool {
        if self.search.opened_feed != Some(self.active_tab().id) {
            return false;
        }
        self.sync_search_preview(self.active);
        self.switch_tab(SEARCH_TAB);
        self.focus_main();
        self.search.editing = false;
        true
    }

    /// Enter or `l` on a row opens its feed as a tab; a comment opens its thread at it,
    /// beside the rows.
    pub fn open_search_row(&mut self) {
        match self.search.selected_row().and_then(|row| row.comment) {
            Some(comment) => {
                self.search.editing = false;
                self.search.preview_closed = false;
                self.search.reading_comment = Some(comment.permalink.clone());
                self.read_experience(Query::Thread(comment.permalink), None);
            }
            None => self.open_search_row_feed(),
        }
    }

    /// The row's feed as a tab: a comment's community.
    pub fn open_search_row_feed(&mut self) {
        let Some(row) = self.search.selected_row() else {
            return;
        };
        self.search.editing = false;
        self.search.preview_closed = false;
        match row.target {
            FeedTarget::Subreddit(name) => self.open_subreddit(&name),
            FeedTarget::User(name) => self.open_user(&name),
            FeedTarget::Home => {}
        }
    }
}
