use super::*;

#[derive(Debug)]
pub struct PostView {
    pub post: Post,
    pub page: PostContent,
    pub loading: bool,
    pub error: Option<String>,
    pub freshness: Option<Freshness>,
    pub generation: u64,
    pub collapsed: HashSet<String>,
    pub loading_more: HashSet<String>,
    /// Removal evidence outlives a comment so older ancestor snapshots cannot restore it.
    pub(super) removed_comments: HashMap<String, Source>,
    /// Index into [`PostView::rows`].
    pub cursor: usize,
    pub scroll: usize,
    /// Reading by scrolling; the next comment key selects near the top (see [`Anchor`]).
    pub free_scroll: bool,
    /// The comment `h` left, which `l` returns to while it is on screen.
    pub(super) left: Option<RowKey>,
    /// A comment `l` selected while reading (or `G` inside the last one): draws keep its
    /// text in place while it stays selected and visible.
    pub hold_view: Option<Hold>,
    /// Row at the top line, offset into it and scroll, taken before the document changed
    /// above it; the next draw restores it (see `take_reading_anchor`).
    pub(super) keep_reading: Option<(RowKey, isize, usize)>,
    /// The reading position the last draw showed, for loads that land after a fold or
    /// reveal made row positions stale.
    pub(super) drawn_anchor: Option<(RowKey, isize, usize)>,
    /// `r` waits for its answer: unseen comments show the page from the top.
    pub(super) reveal: bool,
    /// The selected row the last draw placed on screen; None while reading.
    pub drawn_cursor: Option<usize>,
    /// Spoiler changes while reading: spoiler state and scroll of the last draw, to find the top line
    /// in the new wrap.
    pub rewrapped: Option<(bool, usize)>,
    /// Last rendered document, moved here after drawing.
    pub rendered_document: Vec<ratatui::text::Line<'static>>,
    /// Whether the capped fixed header repeats its full title in the document.
    pub title_in_document: bool,
    /// A successful content update arrived since that document was drawn.
    pub content_changed: bool,
    /// More rows in the last displayed geometry, even after spoiler invalidation.
    pub rendered_more: Vec<(String, usize)>,
    /// Pictures in the last displayed document: URL, first line, rows. A picture above
    /// the view that grows when it loads moves the view down by as much.
    pub rendered_pictures: Vec<(String, usize, usize)>,
    /// This update redirects a read More row to its first newly inserted reply.
    pub more_handoff: bool,
    /// The last draw showed part of the main image or video.
    pub image_shown: bool,
    /// Document line where each row starts; kept in sync by the UI.
    pub row_starts: Vec<usize>,
    /// Document lines visible in the last draw; pages are this tall.
    pub page_height: usize,
    /// Reveal spoilers: a spoiler-tagged post's body and image, and `>!…!<` text.
    pub reveal_spoilers: bool,
    pub gallery_index: usize,
    pub comment_sort: String,
}

pub enum RowKind<'a> {
    Comment {
        comment: &'a Comment,
        hidden: usize,
    },
    More {
        parent: &'a str,
        more: &'a MoreReplies,
    },
}

#[derive(Debug, Default)]
pub struct PostContent {
    pub(super) page: Option<PostPage>,
    pub(crate) document_cache: Option<crate::ui::CachedDocument>,
}

impl From<Option<PostPage>> for PostContent {
    fn from(page: Option<PostPage>) -> Self {
        Self {
            page,
            document_cache: None,
        }
    }
}

impl std::ops::Deref for PostContent {
    type Target = Option<PostPage>;
    fn deref(&self) -> &Self::Target {
        &self.page
    }
}

impl std::ops::DerefMut for PostContent {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.document_cache = None;
        &mut self.page
    }
}

pub struct Row<'a> {
    pub depth: usize,
    pub kind: RowKind<'a>,
}

impl PostView {
    pub fn preview_image(&self) -> Option<std::borrow::Cow<'_, str>> {
        self.shown_post().media.preview_image_at(self.gallery_index)
    }
    pub fn shown_post(&self) -> &Post {
        self.page.as_ref().map_or(&self.post, |page| &page.post)
    }

    /// A spoiler-tagged post whose body and image stay hidden until revealed.
    pub fn spoiler_hidden(&self) -> bool {
        !self.reveal_spoilers
            && (self.post.spoiler || self.page.as_ref().is_some_and(|p| p.post.spoiler))
    }

    /// Anything revealing spoilers would show: a spoiler tag or `>!…!<` text in the body or comments.
    pub fn has_spoilers(&self) -> bool {
        fn marked(comments: &[Comment]) -> bool {
            comments
                .iter()
                .any(|c| c.body.contains(SPOILER_START) || marked(&c.replies))
        }
        self.post.spoiler
            || self.page.as_ref().is_some_and(|p| {
                p.post.spoiler || p.body.contains(SPOILER_START) || marked(&p.comments)
            })
    }

    /// Comment tree in display order, honoring folds. Empty for a hidden spoiler post: its
    /// comments discuss what the body hides.
    pub fn rows(&self) -> Vec<Row<'_>> {
        let mut rows = Vec::new();
        if let Some(page) = self.page.as_ref()
            && !self.spoiler_hidden()
        {
            for c in &page.comments {
                self.flatten(c, 0, &mut rows);
            }
        }
        rows
    }

    pub(super) fn flatten<'a>(&'a self, c: &'a Comment, depth: usize, rows: &mut Vec<Row<'a>>) {
        if c.id.starts_with(THREAD_ROOT) {
            for reply in &c.replies {
                self.flatten(reply, depth, rows);
            }
            if let Some(more) = &c.more {
                rows.push(Row {
                    depth,
                    kind: RowKind::More {
                        parent: &c.id,
                        more,
                    },
                });
            }
            return;
        }
        let folded = self.collapsed.contains(&c.id);
        rows.push(Row {
            depth,
            kind: RowKind::Comment {
                comment: c,
                hidden: if folded { descendants(c) } else { 0 },
            },
        });
        if folded {
            return;
        }
        for r in &c.replies {
            self.flatten(r, depth + 1, rows);
        }
        if let Some(more) = &c.more {
            rows.push(Row {
                depth: depth + 1,
                kind: RowKind::More {
                    parent: &c.id,
                    more,
                },
            });
        }
    }

    /// The comment a key selects while reading (free scroll); None leaves the post. Uses
    /// the last draw's spans; no key moves the view against its direction.
    pub(super) fn anchor_row(&self, anchor: Anchor) -> Option<usize> {
        let starts = &self.row_starts;
        let bottom = self.scroll + self.page_height.max(1);
        // The comment holding the top line: selecting it moves the view up, if at all.
        let covering_top = || starts.iter().rposition(|&s| s <= self.scroll);
        // The first comment from the top line down: selecting it moves the view down.
        let next_down = || starts.iter().position(|&s| s >= self.scroll);
        match anchor {
            Anchor::Up => covering_top(),
            Anchor::Down => next_down(),
            Anchor::Select => {
                let on_screen = |i: usize| {
                    // A row's blank line belongs to the next row (as in `reading_position`).
                    let end = starts.get(i + 1).map_or(usize::MAX, |&s| s - 1);
                    starts
                        .get(i)
                        .is_some_and(|&s| s < bottom && end > self.scroll)
                };
                let left = self.left.as_ref().and_then(|key| self.row_index(key));
                left.filter(|&i| on_screen(i))
                    .or_else(|| next_down().filter(|&i| starts[i] < bottom))
                    .or_else(covering_top)
                    .or_else(next_down)
            }
        }
    }

    /// Leave free scroll by selecting the comment `anchor` picks; true if it did.
    pub(super) fn anchor_cursor(&mut self, anchor: Anchor) -> bool {
        // No positions before a loaded post's first draw (keep it at its top), and stale
        // ones after a fold in the same batch: wait for the draw.
        if !self.free_scroll || self.row_starts.is_empty() || !self.positions_current() {
            return false;
        }
        let Some(i) = self.anchor_row(anchor) else {
            return false;
        };
        self.cursor = i;
        self.free_scroll = false;
        self.left = None;
        // Selecting what you read must not move it; the other keys point a way.
        self.hold_view = matches!(anchor, Anchor::Select).then(|| self.hold(i));
        true
    }

    /// Whether the last draw's `row_starts` still describe [`PostView::rows`] (no fold,
    /// reveal or load since).
    pub(super) fn positions_current(&self) -> bool {
        self.row_starts.len() == self.rows().len()
    }

    pub(super) fn go_to_top(&mut self) {
        self.scroll = 0;
        self.free_scroll = true;
    }

    pub(super) fn show_top(&mut self) {
        self.cursor = 0;
        self.hold_view = None;
        self.keep_reading = None;
        self.go_to_top();
    }

    /// Hold row `i` where the last draw showed it.
    pub(super) fn hold(&self, i: usize) -> Hold {
        Hold {
            row: i,
            offset: self.scroll as isize - self.row_starts[i] as isize,
            top: false,
        }
    }

    /// Before the document changes above what is read (loads, spoiler rewrap), keep the
    /// last-drawn text in place. Several changes before one draw keep the first anchor.
    pub(super) fn pin_view(&mut self) {
        // At the top with media shown, keep its beginning in view.
        // If a key in this pass already scrolled, the reading position counts instead.
        if self.image_shown && self.scroll == 0 {
            if let Some(hold) = &mut self.hold_view {
                hold.top = true;
            }
            return;
        }
        if self.keep_reading.is_none() {
            self.keep_reading = if self.positions_current() {
                self.reading_anchor()
            } else {
                self.drawn_anchor.clone()
            };
        }
        // A selection the last draw placed also holds its text over the next draws.
        if !self.free_scroll
            && self.hold_view.is_none()
            && self.positions_current()
            && self.drawn_cursor == Some(self.cursor)
        {
            self.hold_view = Some(self.hold(self.cursor));
        }
    }

    /// What the next load keeps in place: the top line while reading, else the selection
    /// the last draw placed (if a key moved it since, the draw shows the new one).
    pub fn record_drawn_anchor(&mut self) {
        self.drawn_anchor = self.reading_anchor();
    }

    pub(super) fn reading_anchor(&self) -> Option<(RowKey, isize, usize)> {
        if self.free_scroll {
            self.reading_position()
                .map(|(key, offset)| (key, offset, self.scroll))
        } else {
            self.drawn_cursor
                .filter(|&d| self.positions_current() && d < self.row_starts.len())
                .map(|d| {
                    let row = RowKey::of(&self.rows()[d]);
                    (
                        row,
                        self.scroll as isize - self.row_starts[d] as isize,
                        self.scroll,
                    )
                })
        }
    }

    /// The row holding the top line and the offset into it, by the last draw. None in the
    /// body (loads land below it) or when positions are stale.
    pub(super) fn reading_position(&self) -> Option<(RowKey, isize)> {
        let rows = self.rows();
        if !self.free_scroll || self.row_starts.len() != rows.len() {
            return None;
        }
        let mut i = self.row_starts.iter().rposition(|&s| s <= self.scroll)?;
        // A row's blank line belongs to the next row, so a loading More row's blank line
        // can't anchor to it.
        if self.row_starts.get(i + 1) == Some(&(self.scroll + 1)) {
            i += 1;
        }
        Some((
            RowKey::of(&rows[i]),
            self.scroll as isize - self.row_starts[i] as isize,
        ))
    }

    /// For the draw: where to put the top line back after the document changed above it,
    /// once. Keys that scrolled since move on from it; a top jump stays at the top.
    pub fn take_reading_anchor(&mut self) -> Option<(usize, isize)> {
        let (key, offset, pinned) = self.keep_reading.take()?;
        if self.scroll == 0 && pinned != 0 {
            return None;
        }
        let moved = self.scroll as isize - pinned as isize;
        Some((self.row_index(&key)?, offset + moved))
    }

    pub(super) fn cursor_key(&self) -> Option<RowKey> {
        self.rows().get(self.cursor).map(RowKey::of)
    }

    pub(super) fn row_index(&self, key: &RowKey) -> Option<usize> {
        self.rows().iter().position(|r| match (key, &r.kind) {
            (RowKey::Comment(id), RowKind::Comment { comment, .. }) => comment.id == *id,
            (RowKey::More(id), RowKind::More { parent, .. }) => parent == id,
            _ => false,
        })
    }

    /// Keep the cursor on the same row after rows changed above it; a loaded More row's
    /// index now holds its first new reply.
    pub(super) fn reselect(&mut self, key: Option<RowKey>) {
        let count = self.rows().len();
        if count == 0 {
            self.cursor = 0;
            self.free_scroll = true;
            self.hold_view = None;
            self.keep_reading = None;
            self.drawn_cursor = None;
            self.left = None;
            return;
        }
        let found = key.and_then(|key| self.row_index(&key));
        let held = self.hold_view.is_some_and(|h| h.row == self.cursor);
        self.cursor = found.unwrap_or(self.cursor).min(count.saturating_sub(1));
        // The comment `l` selected moved: its hold on the view moves with it.
        if held && let Some(hold) = &mut self.hold_view {
            hold.row = self.cursor;
        }
    }
}

/// See [`PostView::hold_view`].
#[derive(Debug, Clone, Copy)]
pub struct Hold {
    pub row: usize,
    /// Top line's offset from the comment's first line when the hold began.
    pub offset: isize,
    /// Hold the post's top instead (the inline image is shown there).
    pub top: bool,
}

/// A comment row by its id, or a "more replies" row by its parent comment's id.
#[derive(Debug, Clone)]
pub(super) enum RowKey {
    Comment(String),
    More(String),
}

impl RowKey {
    pub(super) fn of(row: &Row) -> Self {
        match row.kind {
            RowKind::Comment { comment, .. } => RowKey::Comment(comment.id.clone()),
            RowKind::More { parent, .. } => RowKey::More(parent.to_string()),
        }
    }
}

/// Which comment a key pressed while reading the post selects.
#[derive(Clone, Copy)]
pub(super) enum Anchor {
    /// Upward movement: the comment holding the top line.
    Up,
    /// Downward movement: the first comment from the top line down.
    Down,
    /// Selecting a comment: the comment `h` left if on screen, else the one being read (first
    /// starting on screen, else holding the top line, else the next below).
    Select,
}

/// Replies under `c`, loaded or not: a fold hides both, and says how many.
pub(super) fn descendants(c: &Comment) -> usize {
    let unloaded = c
        .more
        .as_ref()
        .map_or(0, |m| m.count.map_or(1, |n| n as usize));
    unloaded + c.replies.iter().map(|r| 1 + descendants(r)).sum::<usize>()
}

pub(super) fn find_comment_mut<'a>(
    comments: &'a mut [Comment],
    id: &str,
) -> Option<&'a mut Comment> {
    for c in comments {
        if c.id == id {
            return Some(c);
        }
        if let Some(found) = find_comment_mut(&mut c.replies, id) {
            return Some(found);
        }
    }
    None
}

pub(super) fn find_comment<'a>(comments: &'a [Comment], id: &str) -> Option<&'a Comment> {
    comments.iter().find_map(|c| {
        if c.id == id {
            Some(c)
        } else {
            find_comment(&c.replies, id)
        }
    })
}
