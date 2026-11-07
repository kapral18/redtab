use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub label: String,
    pub detail: String,
    pub target: FeedTarget,
    /// A People result's avatar and profile summary; communities use the shared icon map.
    pub profile: Option<UserProfile>,
    /// A Comments result: `target` is its community, this the comment it opens at.
    pub comment: Option<CommentRow>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommentRow {
    pub permalink: String,
    /// Community, author and votes on one line.
    pub meta: String,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowsStatus {
    Discovering,
    LoadingMore,
    MoreFailed,
    DiscoveryFailed,
}

#[derive(Default)]
pub struct Rows {
    pub entries: Vec<Row>,
    pub loading: bool,
    pub error: Option<String>,
    pub(super) generation: u64,
    /// The request these rows answer, and Reddit's cursor for its next page.
    pub(super) query: Option<Query>,
    pub(super) after: Option<String>,
    /// The loading request adds a page below the shown entries.
    pub(super) appending: bool,
    pub(super) suspended: bool,
    pub(super) visited: HashSet<String>,
}

#[derive(Default)]
pub struct Preview {
    pub target: Option<FeedTarget>,
    pub posts: Vec<Post>,
    pub loading: bool,
    pub error: Option<String>,
    pub update: Option<Vec<Post>>,
    pub(super) after: Option<String>,
    pub(super) freshness: Option<Freshness>,
    pub(super) first_page_len: usize,
    pub(super) error_source: Option<Source>,
    pub(in crate::app) generation: u64,
    pub(super) due: Option<Instant>,
}

impl Preview {
    pub fn has_new_posts(&self, config: &Config) -> bool {
        self.update
            .as_ref()
            .is_some_and(|posts| super::super::feed_has_new(config, &self.posts, posts))
    }

    pub(super) fn accepts_snapshot(&self, freshness: Freshness) -> bool {
        self.freshness.is_none_or(|shown| {
            freshness.source == shown.source || freshness.source.may_replace(shown.source)
        })
    }

    pub(super) fn clear_error(&mut self, source: Source) {
        if self
            .error_source
            .is_none_or(|shown| source == shown || source.may_replace(shown))
        {
            self.error = None;
            self.error_source = None;
        }
    }

    pub(super) fn record_error(&mut self, error: String, source: Source) {
        if self
            .freshness
            .map(|shown| shown.source)
            .into_iter()
            .chain(self.error_source)
            .any(|shown| shown.compare_requests(source) == Some(std::cmp::Ordering::Greater))
        {
            return;
        }
        self.error = Some(error);
        self.error_source = Some(source);
    }
}

pub struct Search {
    pub query: Field,
    pub editing: bool,
    pub kind: SearchKind,
    /// The selected row of the shown rows, results or Explore.
    pub selected: usize,
    pub offset: usize,
    pub(super) due: Option<Instant>,
    /// The query and kind the shown results answer.
    pub(super) searched: Option<(String, SearchKind)>,
    /// The text of the last Posts or Media search; the same text keeps its sort and time range.
    pub(super) posts_text: Option<String>,
    pub results: Rows,
    /// Communities from Reddit's discovery list, shown under Popular and All Reddit.
    pub discovered: Rows,
    pub preview: Preview,
    pub(in crate::app) opened_feed: Option<u64>,
    /// The comment link a Comments row asked to read; its thread opens beside the rows.
    pub(in crate::app) reading_comment: Option<String>,
    /// The last frame drew the preview pane; a hidden pane loads nothing.
    pub(super) preview_shown: bool,
    /// `r` asked for the next preview from Reddit rather than the cache.
    pub(super) preview_forced: bool,
    /// `q` closed the right pane: the rows fill the width until a row is opened again.
    pub(super) preview_closed: bool,
    /// Empty answers to the current results query so far, and when the next ask is due.
    pub(super) empty_retry: Option<EmptyRetry>,
}

pub(super) struct EmptyRetry {
    pub(super) attempt: usize,
    pub(super) due: Option<Instant>,
}

impl Default for Search {
    fn default() -> Self {
        Self {
            query: Field {
                label: "Search Reddit".into(),
                value: String::new(),
                multiline: false,
                cursor: 0,
                required: false,
            },
            editing: false,
            kind: SearchKind::Posts,
            selected: 0,
            offset: 0,
            due: None,
            searched: None,
            posts_text: None,
            results: Rows::default(),
            discovered: Rows::default(),
            preview: Preview::default(),
            opened_feed: None,
            reading_comment: None,
            preview_shown: false,
            preview_forced: false,
            preview_closed: false,
            empty_retry: None,
        }
    }
}

impl Search {
    pub fn text(&self) -> &str {
        self.query.value.trim()
    }

    /// An empty query explores; communities, comments and people list as rows; posts and
    /// media are feed cards.
    pub fn shows_rows(&self) -> bool {
        self.text().is_empty() || !self.kind.feed()
    }

    pub fn exploring(&self) -> bool {
        self.text().is_empty()
    }

    pub fn rows(&self) -> Vec<Row> {
        if self.exploring() {
            [
                ("popular", "Popular", "Trending posts across Reddit"),
                ("all", "All Reddit", "Every community's posts, unfiltered"),
            ]
            .into_iter()
            .map(|(name, label, detail)| Row {
                label: label.into(),
                detail: detail.into(),
                target: FeedTarget::Subreddit(name.into()),
                profile: None,
                comment: None,
            })
            .chain(self.discovered.entries.iter().cloned())
            .collect()
        } else {
            self.results.entries.clone()
        }
    }

    pub fn selected_row(&self) -> Option<Row> {
        self.rows().into_iter().nth(self.selected)
    }

    /// Whether the rows the user sees are still on their way.
    pub fn rows_loading(&self) -> bool {
        if self.exploring() {
            self.discovered.loading || self.discovered.suspended
        } else {
            self.results.loading
                || self.results.suspended
                || self.due.is_some()
                || self
                    .empty_retry
                    .as_ref()
                    .is_some_and(|retry| retry.due.is_some())
        }
    }

    /// While an empty answer is asked for again: which retry this is, of how many.
    pub fn empty_retry(&self) -> Option<(usize, usize)> {
        self.empty_retry
            .as_ref()
            .filter(|_| !self.exploring() && self.rows_loading())
            .map(|retry| (retry.attempt, EMPTY_RETRY_DELAYS.len()))
    }

    /// What the line under the shown rows says, if anything.
    pub fn rows_status(&self) -> Option<RowsStatus> {
        let rows = if self.exploring() {
            &self.discovered
        } else {
            &self.results
        };
        if rows.loading && rows.appending {
            Some(RowsStatus::LoadingMore)
        } else if self.exploring() && rows.loading {
            Some(RowsStatus::Discovering)
        } else if rows.error.is_some() && rows.after.is_some() {
            Some(RowsStatus::MoreFailed)
        } else if self.exploring() && rows.error.is_some() {
            Some(RowsStatus::DiscoveryFailed)
        } else {
            None
        }
    }

    #[cfg(test)]
    pub(crate) fn discovered_generation(&self) -> u64 {
        self.discovered.generation
    }

    #[cfg(test)]
    pub(crate) fn results_generation(&self) -> u64 {
        self.results.generation
    }

    #[cfg(test)]
    pub(crate) fn results_query(&self) -> Option<&Query> {
        self.results.query.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn preview_forced(&self) -> bool {
        self.preview_forced
    }

    pub fn rows_error(&self) -> Option<&str> {
        if self.exploring() {
            self.discovered.error.as_deref()
        } else {
            self.results.error.as_deref()
        }
    }
}

pub(super) fn row(entry: &crate::reddit::experience::Entry) -> Option<Row> {
    if let (Some(Query::Thread(permalink)), Some(target)) = (&entry.query, &entry.target) {
        let (meta, text) = entry.body.split_once('\n').unwrap_or((&entry.body, ""));
        let text = crate::model::unformatted(text).trim().to_string();
        return Some(Row {
            label: entry.title.clone(),
            detail: text
                .lines()
                .find(|line| !line.trim().is_empty())
                .map_or_else(
                    || meta.to_string(),
                    |line| format!("{meta} · {}", line.trim()),
                ),
            target: FeedTarget::Subreddit(target.subreddit.clone()),
            profile: None,
            comment: Some(CommentRow {
                permalink: permalink.clone(),
                meta: meta.to_string(),
                text,
            }),
        });
    }
    let summary = UserProfile::new(
        match &entry.icon {
            SubscriptionIcon::Url(url) => Some(url.clone()),
            _ => None,
        },
        &entry.body,
    );
    let detail = summary.lines.first().cloned().unwrap_or_default();
    let (target, profile) = match entry.query.as_ref()? {
        Query::Community(name) => (FeedTarget::Subreddit(name.clone()), None),
        Query::Profile(name) => (FeedTarget::User(name.clone()), Some(summary)),
        _ => return None,
    };
    Some(Row {
        label: entry.title.clone(),
        detail,
        target,
        profile,
        comment: None,
    })
}
