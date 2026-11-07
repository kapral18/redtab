use super::*;

impl App {
    pub fn toggle_spoilers(&mut self) {
        let Some(view) = &mut self.post_view else {
            return;
        };
        if !view.has_spoilers() {
            self.info("No spoilers in this post");
            return;
        }
        // Masking rewraps lines: keep the text being read in place (reading: the draw
        // finds the top line again; a selection holds its text).
        if view.free_scroll {
            let (was, scroll) = (view.reveal_spoilers, view.scroll);
            view.rewrapped.get_or_insert((was, scroll));
        } else {
            view.pin_view();
        }
        view.reveal_spoilers = !view.reveal_spoilers;
        view.row_starts.clear();
        let shown = view.reveal_spoilers;
        if view.spoiler_hidden() {
            // The comments went away with the post: read from the top; positions are stale
            // until the next draw.
            view.go_to_top();
            view.keep_reading = None;
            view.hold_view = None;
            view.rewrapped = None;
        }
        self.info(if shown {
            "Spoilers shown"
        } else {
            "Spoilers hidden"
        });
    }

    pub fn close_post(&mut self) {
        self.ancestor_follow = None;
        self.post_view = None;
        if self.focus == Focus::Post {
            self.focus = Focus::List;
        }
        // Full-size previews are megabytes each: keep them only while their post is open.
        self.images.clear();
        self.pool.retain_images(self.card_images.keys());
        self.pool.want_post(None);
    }

    /// `h` in a post: leave a comment selection for reading; false if there is none.
    pub fn leave_comment(&mut self) -> bool {
        match &mut self.post_view {
            Some(view) if !view.free_scroll => {
                view.left = view.cursor_key();
                view.free_scroll = true;
                true
            }
            _ => false,
        }
    }

    pub fn move_comment_cursor(&mut self, delta: isize) {
        self.moving_in_post();
        let Some(view) = &mut self.post_view else {
            return;
        };
        if view.free_scroll {
            // At the top `k` stays: anchoring would select the first comment, so held `k`
            // would flip.
            if delta < 0 && view.scroll == 0 {
                view.go_to_top();
                return;
            }
            // Before the post draws, or with positions stale after a fold in this batch, stay
            // put. A post without comments reads by line.
            if view.page.is_none() || !view.positions_current() {
                return;
            }
            let anchor = if delta < 0 { Anchor::Up } else { Anchor::Down };
            // With no comment that way, read on by a line, like C-y / C-e.
            if !view.anchor_cursor(anchor) {
                if delta < 0 && view.scroll <= 1 {
                    view.go_to_top();
                } else {
                    view.scroll = view.scroll.saturating_add_signed(delta.signum());
                }
            }
            return;
        }
        let len = view.rows().len();
        if delta < 0 && view.cursor == 0 {
            view.go_to_top();
            return;
        }
        view.cursor = step(view.cursor, delta, len);
    }

    pub fn post_top(&mut self) {
        self.moving_in_post();
        if let Some(view) = &mut self.post_view {
            view.cursor = 0;
            view.go_to_top();
        }
    }

    pub fn post_bottom(&mut self) {
        self.moving_in_post();
        if let Some(view) = &mut self.post_view {
            let len = view.rows().len();
            // A hidden spoiler post has no comments to select.
            if len > 0 {
                view.cursor = len - 1;
                view.free_scroll = false;
                view.left = None;
                // Already reading inside the last comment: stay rather than jump to its header;
                // otherwise the draw scrolls to it. Positions from before a fold or reveal in
                // this batch don't count.
                let inside = view.row_starts.len() == len && view.row_starts[len - 1] < view.scroll;
                view.hold_view = inside.then(|| view.hold(len - 1));
            } else if view.page.is_some() && !view.spoiler_hidden() {
                view.scroll = usize::MAX;
                view.free_scroll = true;
                view.left = None;
                view.hold_view = None;
            }
        }
    }

    pub fn scroll_post(&mut self, delta: isize) {
        self.moving_in_post();
        if let Some(view) = &mut self.post_view {
            if delta < 0 && view.scroll <= delta.unsigned_abs() {
                view.go_to_top();
            } else {
                view.scroll = view.scroll.saturating_add_signed(delta);
                view.free_scroll = true;
            }
        }
    }

    /// Scroll by `num/den` of the visible document; a full page keeps one line of context.
    pub fn page_post(&mut self, num: isize, den: isize) {
        let height = self.post_view.as_ref().map_or(0, |v| v.page_height) as isize;
        let step = if num.abs() == den {
            height - 1
        } else {
            height * num.abs() / den
        };
        self.scroll_post(step.max(1) * num.signum());
    }

    pub fn jump_top_level(&mut self, forward: bool) {
        self.moving_in_post();
        let Some(view) = &mut self.post_view else {
            return;
        };
        let was_free = view.free_scroll;
        // As with `k`: going back from the post's top stays there.
        if was_free && !forward && view.scroll == 0 {
            view.go_to_top();
            return;
        }
        // As with `j`/`k`: wait for the draw before the post loads or after a fold in this batch.
        if was_free && (view.page.is_none() || !view.positions_current()) {
            return;
        }
        // Anchoring commits a selection; with no thread that way it is undone.
        let reading = was_free.then(|| (view.cursor, view.left.clone()));
        let anchor = if forward { Anchor::Down } else { Anchor::Up };
        if was_free && !view.anchor_cursor(anchor) {
            // From the body the previous thread is the post; inside the last comment none follows.
            if !forward {
                view.go_to_top();
            }
            return;
        }
        let target = {
            let rows = view.rows();
            let mut indexed = rows.iter().enumerate();
            if forward {
                // Right after anchoring, a top-level comment under the cursor counts as next.
                let stay = was_free && rows.get(view.cursor).is_some_and(|r| r.depth == 0);
                let from = if stay { view.cursor } else { view.cursor + 1 };
                indexed
                    .find(|(i, r)| *i >= from && r.depth == 0)
                    .map(|(i, _)| i)
            } else {
                // Right after anchoring below a header, that header's thread counts as previous.
                let stay = was_free && view.row_starts.get(view.cursor) < Some(&view.scroll);
                let before = if stay { view.cursor + 1 } else { view.cursor };
                indexed
                    .rev()
                    .find(|(i, r)| *i < before && r.depth == 0)
                    .map(|(i, _)| i)
            }
        };
        match (target, reading) {
            (Some(i), _) => view.cursor = i,
            // No thread that way: keep reading. Up from the first thread, the previous one
            // is the post.
            (None, Some((cursor, left))) => {
                view.cursor = cursor;
                view.left = left;
                view.free_scroll = true;
                if !forward {
                    view.go_to_top();
                }
            }
            (None, None) if !forward => view.go_to_top(),
            (None, None) => {}
        }
    }

    /// Fold or unfold the comment under the cursor, or load more replies; while reading
    /// it only selects (see [`Anchor::Select`]).
    pub fn activate_comment(&mut self) {
        enum Action {
            Toggle(String),
            More { parent: String, cursor: String },
        }
        let Some(view) = &mut self.post_view else {
            return;
        };
        if view.free_scroll {
            view.anchor_cursor(Anchor::Select);
            return;
        }
        let action = {
            let rows = view.rows();
            match rows.get(view.cursor).map(|r| &r.kind) {
                Some(RowKind::Comment { comment, .. })
                    if !comment.replies.is_empty() || comment.more.is_some() =>
                {
                    Some(Action::Toggle(comment.id.clone()))
                }
                Some(RowKind::More { parent, more }) => Some(Action::More {
                    parent: parent.to_string(),
                    cursor: more.cursor.clone(),
                }),
                _ => None,
            }
        };
        match action {
            Some(Action::Toggle(id)) => {
                let was_folded = view.collapsed.remove(&id);
                if !was_folded {
                    view.collapsed.insert(id);
                }
                // Equal row counts can still describe different rows and heights.
                view.row_starts.clear();
                // Show the header with its fold chip: the fold's result is there.
                view.hold_view = None;
            }
            Some(Action::More { parent, cursor }) => {
                let first_request = view.loading_more.insert(parent.clone());
                let generation = view.generation;
                if first_request {
                    let minimum = view
                        .page
                        .as_mut()
                        .and_then(|page| find_comment_mut(&mut page.comments, &parent))
                        .and_then(|comment| comment.sources.map(|sources| sources.more))
                        .or_else(|| view.freshness.map(|freshness| freshness.source));
                    self.pool.submit(Job::MoreReplies {
                        generation,
                        comment_id: parent,
                        cursor,
                        minimum,
                    });
                }
            }
            None => {}
        }
    }
}
