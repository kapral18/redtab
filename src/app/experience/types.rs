use super::*;

#[derive(Default)]
pub struct Experience {
    pub screens: Vec<Screen>,
    pub error: Option<String>,
    pub pending: Option<(u64, Request)>,
    pub refreshing: bool,
    pub(in crate::app) prefetch: Option<(Query, Instant)>,
    /// A list re-read after a write: that read's generation, then its selected entry,
    /// index and preview scroll.
    pub(super) kept: Option<(u64, String, usize, usize)>,
    /// The content-menu shortcut a direct key runs once the target's details load.
    pub(super) quick: Option<&'static str>,
}

impl Experience {
    pub fn loading_more(&self) -> bool {
        matches!((&self.pending, self.screens.last()),
            (Some((_, Request::Read { query, after: Some(_), .. })), Some(Screen::Browse { query: current, .. })) if query == current)
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref().or_else(|| match self.screens.last() {
            Some(Screen::Browse { pagination, .. }) => pagination.error.as_deref(),
            _ => None,
        })
    }

    pub fn writing(&self) -> bool {
        matches!(&self.pending, Some((_, Request::Write(_))))
    }
    pub(in crate::app) fn reset_reads(&mut self, has_post: bool) {
        let keep = |screen: &Screen| match screen {
            Screen::Input(form) => match &form.kind {
                FormKind::Find => has_post,
                FormKind::OpenLink
                | FormKind::Profile
                | FormKind::Reply(_)
                | FormKind::Edit(_)
                | FormKind::Report(_)
                | FormKind::Submit { .. }
                | FormKind::Wiki(_)
                | FormKind::CreateMulti
                | FormKind::MultiCommunity { .. } => true,
            },
            _ => false,
        };
        let keep_error = self.screens.last().is_some_and(keep);
        self.pending = None;
        self.prefetch = None;
        self.refreshing = false;
        self.kept = None;
        self.quick = None;
        self.screens.retain(keep);
        if !keep_error {
            self.error = None;
        }
    }

    pub fn input(&mut self) -> Option<&mut Form> {
        match self.screens.last_mut() {
            Some(Screen::Input(form)) => Some(form),
            _ => None,
        }
    }
}

pub(in crate::app) struct CachedCollection {
    pub(super) query: Query,
    pub(super) after: Option<String>,
    pub(super) page: Page,
    pub(super) fetched: Instant,
}

#[derive(Clone)]
pub enum Screen {
    Menu {
        title: String,
        choices: Vec<Choice>,
        selected: usize,
        native: String,
    },
    Input(Form),
    Browse {
        query: Query,
        page: Page,
        selected: usize,
        scroll: usize,
        pagination: Pagination,
    },
    Loading(Query),
    Confirm {
        label: String,
        action: Action,
        native: String,
    },
}

#[derive(Clone, Default)]
pub struct Pagination {
    pub(super) visited: HashSet<String>,
    pub(super) error: Option<String>,
    pub update: Option<Box<Page>>,
}

impl Pagination {
    pub fn has_new_entries(&self, shown: &Page) -> bool {
        self.update.as_ref().is_some_and(|update| {
            update.entries.iter().any(|entry| {
                !shown
                    .entries
                    .iter()
                    .any(|known| entry_key(known) == entry_key(entry))
            })
        })
    }
}

#[derive(Clone)]
pub struct Choice {
    pub shortcut: &'static str,
    pub label: String,
    pub section: &'static str,
    pub(super) command: Command,
}

#[derive(Clone, Copy)]
pub(super) enum MenuScope {
    Reddit,
    Content,
    Community,
    Comments,
    Message,
    User,
    Links,
}

#[derive(Clone)]
pub(super) enum Command {
    Read(Query),
    Input(Form),
    Write(Action),
    Confirm(String, Action),
    Native(String),
    BodyLink(crate::model::BodyLink, bool),
    OpenUrl(String),
    CopyUrl(String),
    Subreddit(String),
    User(String),
    Membership(MembershipTarget),
    MarkCommunityRead(String),
    Sort(String),
    Target(Target, Option<Post>),
    /// Switches to the Search tab with this kind selected.
    Search(SearchKind),
}

#[derive(Clone)]
pub struct Field {
    pub label: String,
    pub value: String,
    pub multiline: bool,
    pub cursor: usize,
    pub required: bool,
}

#[derive(Clone)]
pub struct Form {
    pub title: String,
    pub fields: Vec<Field>,
    pub active: usize,
    pub(super) kind: FormKind,
    pub native: String,
}

#[derive(Clone)]
pub(super) enum FormKind {
    OpenLink,
    Profile,
    Reply(Target),
    Edit(Target),
    Report(Target),
    Submit {
        link: bool,
    },
    Find,
    Wiki(String),
    CreateMulti,
    MultiCommunity {
        owner: String,
        name: String,
        add: bool,
    },
}

pub(super) fn field(label: &str, value: &str, multiline: bool) -> Field {
    Field {
        label: label.into(),
        value: value.into(),
        multiline,
        cursor: value.len(),
        required: true,
    }
}

impl Field {
    pub fn delete_word(&mut self) {
        let start = self.value[..self.cursor]
            .trim_end()
            .trim_end_matches(|c: char| !c.is_whitespace())
            .len();
        self.value.replace_range(start..self.cursor, "");
        self.cursor = start;
    }
    pub(crate) fn insert(&mut self, text: &str) {
        self.value.insert_str(self.cursor, text);
        self.cursor += text.len();
    }
    pub(crate) fn previous(&self) -> usize {
        self.value[..self.cursor]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(index, _)| index)
    }
    pub(crate) fn next(&self) -> usize {
        self.value[self.cursor..]
            .graphemes(true)
            .next()
            .map_or(self.cursor, |c| self.cursor + c.len())
    }
}

impl Form {
    pub fn submit_label(&self) -> &'static str {
        match self.kind {
            FormKind::Find => "search",
            FormKind::OpenLink | FormKind::Profile | FormKind::Wiki(_) => "open",
            FormKind::Reply(_) => "reply",
            FormKind::Edit(_) => "save",
            FormKind::Report(_) => "report",
            FormKind::Submit { .. } => "post",
            FormKind::CreateMulti => "create",
            FormKind::MultiCommunity { add: true, .. } => "add",
            FormKind::MultiCommunity { add: false, .. } => "remove",
        }
    }
}

pub(super) fn form(title: &str, kind: FormKind, fields: Vec<Field>, native: &str) -> Form {
    Form {
        title: title.into(),
        kind,
        fields,
        active: 0,
        native: native.into(),
    }
}

pub(super) fn choice(shortcut: &'static str, label: &str, command: Command) -> Choice {
    Choice {
        shortcut,
        label: label.into(),
        section: "",
        command,
    }
}

pub(super) fn read(shortcut: &'static str, label: &str, query: Query) -> Choice {
    choice(shortcut, label, Command::Read(query))
}

pub(super) fn native(shortcut: &'static str, label: &str, path: &str) -> Choice {
    choice(
        shortcut,
        &format!("{label} · open Reddit ↗"),
        Command::Native(path.into()),
    )
}
