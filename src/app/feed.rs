use super::*;

/// A list picture cut to its slot's shape, centred, so it fills the slot without
/// stretching: thumbnails are `THUMB_COLS`×`THUMB_ROWS` cells and icons 2×1, both two
/// cells across per row.
pub(super) fn fill_slot(
    img: image::DynamicImage,
    font: ratatui_image::FontSize,
) -> image::DynamicImage {
    use crate::ui::{THUMB_COLS, THUMB_ROWS};
    let (w, h) = (u64::from(img.width()), u64::from(img.height()));
    let slot_w = u64::from(THUMB_COLS) * u64::from(font.width);
    let slot_h = u64::from(THUMB_ROWS) * u64::from(font.height);
    if w == 0 || h == 0 || slot_w == 0 || slot_h == 0 {
        return img;
    }
    // The widest centred part of the slot's shape the picture holds.
    let (cw, ch) = if w * slot_h > h * slot_w {
        ((h * slot_w / slot_h).max(1), h)
    } else {
        (w, (w * slot_h / slot_w).max(1))
    };
    img.crop_imm(
        ((w - cw) / 2) as u32,
        ((h - ch) / 2) as u32,
        cw as u32,
        ch as u32,
    )
}

#[derive(Debug, Default, Clone)]
pub struct CardScroll {
    /// First visible card, independent of the selected post.
    pub post: Option<String>,
    pub line: usize,
    pub height: usize,
    pub viewport: usize,
    pub pending: isize,
    /// Selected card, the row it was drawn at, and whether all of it fitted there:
    /// data and size changes keep it there, and keep a card that fitted fitting.
    pub selected: Option<(String, isize, bool)>,
}

#[derive(Debug, Default)]
pub struct Feed {
    pub posts: Vec<Post>,
    pub after: Option<String>,
    pub sort: Sort,
    pub time: TimeRange,
    pub search: Option<String>,
    /// The Search tab's search asks for media posts rather than all posts.
    pub search_media: bool,
    pub search_sort: crate::reddit::experience::SearchSort,
    pub search_time: TimeRange,
    pub(super) failures: HashMap<Option<String>, FeedFailure>,
    pub generation: u64,
    pub selected: usize,
    pub loaded: bool,
    /// First visible list item; kept in sync by the UI.
    pub offset: usize,
    pub cards: CardScroll,
    /// The query `posts` came from: a failed load of another query clears them, of the
    /// same keeps them.
    pub(super) shown_query: Option<FeedQuery>,
    pub freshness: Option<Freshness>,
    pub(super) pages: Vec<FeedPage>,
    pub(super) request_query: Option<FeedQuery>,
    pub(super) requests: HashMap<u64, Option<String>>,
    pub(super) pending: HashSet<u64>,
    pub(super) refreshing: HashSet<u64>,
    pub(super) forced: HashSet<u64>,
    /// Requests from `r`: their arrivals show at once, from the list top.
    pub(super) manual: HashSet<u64>,
    pub(super) deferred: HashMap<Option<String>, bool>,
    /// A first-page answer waiting behind the notice.
    pub(super) update: Option<FeedUpdate>,
    pub(super) displaced_sources: HashMap<String, Source>,
}

#[derive(Debug)]
pub(super) struct FeedUpdate {
    pub(super) listing: Listing,
    pub(super) freshness: Freshness,
}

#[derive(Debug)]
pub(super) struct FeedFailure {
    pub(super) error: FeedError,
    pub(super) source: Source,
}

#[derive(Clone, Copy)]
pub(super) struct CommunityAccess {
    pub(super) denied: Option<AccessDenied>,
    pub(super) source: Source,
}

#[derive(Debug)]
pub(super) struct FeedPage {
    pub(super) cursor: Option<String>,
    pub(super) len: usize,
    pub(super) after: Option<String>,
    pub(super) source: Source,
    pub(super) complete: bool,
}

impl Feed {
    /// A new query starts at relevance over all time and drops a held answer to the old
    /// query; the same query keeps its sort and time.
    pub(super) fn set_search(&mut self, query: String, media: bool) {
        if self.search.as_deref() != Some(query.as_str()) {
            self.search_sort = Default::default();
            self.search_time = TimeRange::All;
            self.update = None;
        }
        self.search_media = media;
        self.search = Some(query);
        self.selected = 0;
        self.cards = CardScroll::default();
    }

    pub(super) fn query(&self) -> FeedQuery {
        match &self.search {
            Some(q) if self.search_media => FeedQuery::Media {
                query: q.clone(),
                sort: self.search_sort,
                time: self.search_time,
            },
            Some(q) => FeedQuery::Search {
                query: q.clone(),
                sort: self.search_sort,
                time: self.search_time,
            },
            // Sorts without a time range send none; the kept range must not split them.
            None => FeedQuery::Listing {
                sort: self.sort,
                time: if self.sort.takes_time() {
                    self.time
                } else {
                    TimeRange::Day
                },
            },
        }
    }

    pub fn refreshing(&self) -> bool {
        !self.refreshing.is_empty()
    }

    pub(super) fn leads_with_new(&self, posts: &[Post], config: &Config) -> bool {
        feed_has_new(config, &self.posts, posts)
    }

    pub fn has_new_posts(&self, config: &Config) -> bool {
        self.update
            .as_ref()
            .is_some_and(|update| self.leads_with_new(&update.listing.posts, config))
    }

    /// Selects the first post, shown from its first row; scroll keys before the next
    /// draw scroll from it.
    pub(super) fn show_top(&mut self, first: Option<String>) {
        self.selected = 0;
        self.offset = 0;
        self.cards = CardScroll {
            post: first,
            viewport: self.cards.viewport,
            ..CardScroll::default()
        };
    }

    /// The first row, not scrolled into in card view, whose post is not open from this
    /// list: updates leave it on the list's first post.
    pub(super) fn untouched_top(&self, card_view: bool, open_here: bool) -> bool {
        let scrolled =
            card_view && (self.cards.line as isize).saturating_add(self.cards.pending) > 0;
        self.selected == 0 && !scrolled && !open_here
    }

    /// `posts` answer the current sort, time range or search, not an earlier one still
    /// shown while the new one loads.
    pub fn shows_current_query(&self) -> bool {
        self.shown_query.as_ref() == Some(&self.query())
    }

    pub fn loading(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn loading_more(&self) -> bool {
        self.requests.iter().any(|(id, cursor)| {
            cursor.is_some()
                && *cursor == self.after
                && (self.pending.contains(id) || self.refreshing.contains(id))
        })
    }

    pub fn receiving(&self) -> bool {
        self.pages.iter().any(|page| {
            !page.complete
                && self.requests.iter().any(|(id, cursor)| {
                    *cursor == page.cursor
                        && (self.pending.contains(id) || self.refreshing.contains(id))
                })
        })
    }

    pub(super) fn failure(&self) -> Option<&FeedFailure> {
        self.failures
            .get(&self.after)
            .or_else(|| self.failures.get(&None))
            .or_else(|| self.failures.values().next())
    }

    pub fn error(&self) -> Option<&str> {
        self.failure().map(|failure| failure.error.as_str())
    }

    pub fn access_denied(&self) -> Option<AccessDenied> {
        self.failures
            .values()
            .find_map(|failure| failure.error.access())
    }

    pub fn page_failed(&self) -> bool {
        self.after.is_some()
            && self.failures.contains_key(&self.after)
            && !self
                .pages
                .iter()
                .any(|page| page.cursor == self.after && page.complete)
    }

    pub(super) fn accepts(&self, generation: u64) -> bool {
        generation == self.generation || self.requests.contains_key(&generation)
    }

    pub(super) fn remove_post(&mut self, id: &str) {
        let Some(index) = self.posts.iter().position(|post| post.id == id) else {
            return;
        };
        self.posts.remove(index);
        self.displaced_sources.remove(id);
        let mut start = 0;
        for page in &mut self.pages {
            if index < start + page.len {
                page.len -= 1;
                break;
            }
            start += page.len;
        }
    }

    /// Replace this cursor's page, not the entire feed or an append-only union.
    pub(super) fn replace_page(
        &mut self,
        cursor: Option<String>,
        mut posts: Vec<Post>,
        after: Option<String>,
        source: Source,
    ) -> bool {
        let index = self
            .pages
            .iter()
            .position(|page| page.cursor == cursor)
            .unwrap_or(self.pages.len());
        if self
            .pages
            .get(index)
            .is_some_and(|page| !source.may_replace(page.source))
        {
            return false;
        }
        let mut protected = HashSet::new();
        let mut start = 0;
        for (i, page) in self.pages.iter().enumerate() {
            if i != index {
                for post in &self.posts[start..start + page.len] {
                    let previous = self
                        .displaced_sources
                        .get(&post.id)
                        .copied()
                        .unwrap_or(page.source);
                    if !source.may_replace(previous)
                        || (previous.compare(source) == Some(std::cmp::Ordering::Equal)
                            && source.cached
                            && i < index)
                    {
                        protected.insert(post.id.as_str());
                    }
                }
            }
            start += page.len;
        }
        posts.retain(|post| !protected.contains(post.id.as_str()));
        let incoming_ids: HashSet<_> = posts.iter().map(|post| post.id.as_str()).collect();
        for id in &incoming_ids {
            self.displaced_sources.remove(*id);
        }
        // Newer refreshed records own their positions; older cached pages cannot move them back.
        let mut pages = self.pages.iter_mut().enumerate();
        let mut current = pages.next();
        let mut remaining = current.as_ref().map_or(0, |(_, page)| page.len);
        self.posts.retain(|post| {
            while remaining == 0 && current.is_some() {
                current = pages.next();
                remaining = current.as_ref().map_or(0, |(_, page)| page.len);
            }
            remaining = remaining.saturating_sub(1);
            let keep = current.as_ref().is_some_and(|(i, _)| *i == index)
                || !incoming_ids.contains(post.id.as_str());
            if !keep && let Some((_, page)) = current.as_mut() {
                page.len -= 1;
            }
            keep
        });
        let start: usize = self.pages[..index].iter().map(|page| page.len).sum();
        let end = start + self.pages.get(index).map_or(0, |page| page.len);
        let page = FeedPage {
            cursor,
            len: posts.len(),
            after,
            source,
            complete: true,
        };
        self.posts.splice(start..end, posts);
        if index == self.pages.len() {
            self.pages.push(page);
        } else {
            self.pages[index] = page;
        }
        self.after = self.pages.last().and_then(|page| page.after.clone());
        if !self.displaced_sources.is_empty() {
            let known: HashSet<_> = self.posts.iter().map(|post| post.id.as_str()).collect();
            self.displaced_sources
                .retain(|id, _| known.contains(id.as_str()));
        }
        true
    }

    pub(super) fn progress_post(
        &mut self,
        cursor: Option<String>,
        post: Post,
        source: Source,
    ) -> bool {
        let index = self
            .pages
            .iter()
            .position(|page| page.cursor == cursor)
            .unwrap_or(self.pages.len());
        if self
            .pages
            .get(index)
            .is_some_and(|page| page.complete || page.source != source)
        {
            return false;
        }
        let start: usize = self.pages[..index].iter().map(|page| page.len).sum();
        let end = start + self.pages.get(index).map_or(0, |page| page.len);
        let mut posts = self.posts[start..end].to_vec();
        if posts.iter().any(|old| old.id == post.id) {
            return false;
        }
        posts.push(post);
        if !self.replace_page(cursor.clone(), posts, self.after.clone(), source) {
            return false;
        }
        self.pages
            .iter_mut()
            .find(|page| page.cursor == cursor)
            .unwrap()
            .complete = false;
        true
    }
}
