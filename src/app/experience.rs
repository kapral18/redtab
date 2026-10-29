//! Account collections, composition and contextual actions share one overlay.

use super::*;
use crate::reddit::experience::{
    Action, Details, Entry, Inbox, Page, Query, Request, Response, SearchKind, SearchSort, Target,
    UserFeed, name, reddit_url, thread_location,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Default)]
pub struct Experience {
    pub screens: Vec<Screen>,
    pub error: Option<String>,
    pub pending: Option<(u64, Request)>,
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
    pub(super) fn reset_reads(&mut self, has_post: bool) {
        let keep = |screen: &Screen| match screen {
            Screen::Input(form) => match &form.kind {
                FormKind::Find => has_post,
                FormKind::OpenLink
                | FormKind::Search(_)
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
    visited: HashSet<String>,
    error: Option<String>,
}

#[derive(Clone)]
pub struct Choice {
    pub shortcut: &'static str,
    pub label: String,
    pub section: &'static str,
    command: Command,
}

#[derive(Clone, Copy)]
enum MenuScope {
    Reddit,
    Content,
    Community,
    Comments,
    Message,
    User,
    Links,
}

#[derive(Clone)]
enum Command {
    Read(Query),
    Input(Form),
    Write(Action),
    Confirm(String, Action),
    Native(String),
    BodyLink(crate::model::BodyLink),
    OpenUrl(String),
    CopyUrl(String),
    Subreddit(String),
    User(String),
    Membership(MembershipTarget),
    MarkCommunityRead(String),
    Sort(String),
    Target(Target, Option<Post>),
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
    kind: FormKind,
    pub native: String,
}

#[derive(Clone)]
enum FormKind {
    OpenLink,
    Search(SearchKind),
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

fn field(label: &str, value: &str, multiline: bool) -> Field {
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
            FormKind::Search(_) | FormKind::Find => "search",
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

fn form(title: &str, kind: FormKind, fields: Vec<Field>, native: &str) -> Form {
    Form {
        title: title.into(),
        kind,
        fields,
        active: 0,
        native: native.into(),
    }
}

fn choice(shortcut: &'static str, label: &str, command: Command) -> Choice {
    Choice {
        shortcut,
        label: label.into(),
        section: "",
        command,
    }
}

fn ordered_choices(choices: Vec<Choice>, scope: MenuScope) -> Vec<Choice> {
    let mut ranked: Vec<_> = choices
        .into_iter()
        .map(|mut choice| {
            let (rank, section) = match scope {
                MenuScope::Reddit => match &choice.command {
                    Command::Read(Query::Inbox(_)) => (0, "Notifications"),
                    Command::Native(path) if path == "/notifications" => (0, "Notifications"),
                    Command::Native(path) if path == "/chat/" => (1, "Messages"),
                    Command::Read(Query::User { .. } | Query::Profile(_)) => (2, "Your account"),
                    Command::Input(Form {
                        kind: FormKind::Search(_) | FormKind::Profile,
                        ..
                    }) => (3, "Search"),
                    Command::Read(Query::Multis | Query::Communities) | Command::Subreddit(_) => {
                        (4, "Explore")
                    }
                    Command::Input(_) => (5, "Create"),
                    _ => (6, "On Reddit"),
                },
                MenuScope::Content => match &choice.command {
                    Command::Input(Form {
                        kind: FormKind::Reply(_) | FormKind::Edit(_),
                        ..
                    }) => (0, "Reply and edit"),
                    Command::Write(Action::Vote { .. } | Action::Save { .. }) => {
                        (1, "Vote and save")
                    }
                    Command::Write(Action::Read { .. }) => (2, "Notifications"),
                    Command::Write(_) => (3, "Post controls"),
                    Command::Read(_) => (4, "Related"),
                    Command::Input(Form {
                        kind: FormKind::Report(_),
                        ..
                    }) => (5, "Report"),
                    Command::Confirm(_, _) => (7, "Destructive"),
                    _ => (6, "On Reddit"),
                },
                MenuScope::User => (0, "Profile"),
                MenuScope::Links => (0, "Links"),
                MenuScope::Community => match &choice.command {
                    Command::Subreddit(_) | Command::MarkCommunityRead(_) => (0, "Community"),
                    Command::Native(path) if path.strip_prefix("/r/").is_some_and(is_community) => {
                        (0, "Community")
                    }
                    Command::Read(Query::Community(_) | Query::Rules(_)) => (1, "About"),
                    Command::Read(Query::Wiki { .. }) | Command::Input(_) => (2, "Wiki"),
                    Command::Read(Query::Moderators(_)) => (3, "Moderators"),
                    _ => (4, "On Reddit"),
                },
                MenuScope::Comments => match &choice.command {
                    Command::Input(_) => (0, "Find"),
                    _ => (1, "Sort comments"),
                },
                MenuScope::Message => match &choice.command {
                    Command::Write(_) => (0, "Notifications"),
                    _ => (1, "On Reddit"),
                },
            };
            choice.section = section;
            (rank, choice)
        })
        .collect();
    ranked.sort_by_key(|(rank, _)| *rank);
    ranked.into_iter().map(|(_, choice)| choice).collect()
}
fn read(shortcut: &'static str, label: &str, query: Query) -> Choice {
    choice(shortcut, label, Command::Read(query))
}

fn link_command(input: &str) -> anyhow::Result<Command> {
    let base = url::Url::parse("https://www.reddit.com/").unwrap();
    let input = input.trim();
    let url = if input.starts_with('/') {
        base.join(input)
    } else {
        url::Url::parse(input)
    }
    .map_err(|_| {
        anyhow::anyhow!("Enter a full Reddit URL or /r/name, /user/name or /comments/id")
    })?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        anyhow::bail!("Enter an http or https Reddit link without credentials or a custom port");
    }
    let path = url.path();
    let parts: Vec<_> = path.trim_matches('/').split('/').collect();
    match url.host_str() {
        Some("redd.it" | "www.redd.it") => {
            if let [id] = parts.as_slice() {
                let path = format!("/comments/{id}/");
                if crate::reddit::parse::post_id(&path).is_some() {
                    return Ok(Command::Read(Query::Thread(path)));
                }
            }
        }
        Some(
            "reddit.com" | "www.reddit.com" | "old.reddit.com" | "new.reddit.com" | "np.reddit.com"
            | "m.reddit.com" | "sh.reddit.com",
        ) => {
            if crate::reddit::parse::post_id(path).is_some() {
                return Ok(Command::Read(Query::Thread(path.into())));
            }
            match parts.as_slice() {
                ["r", sub] if is_subreddit_name(sub) => {
                    return Ok(Command::Subreddit((*sub).into()));
                }
                ["u" | "user", user] if name(user).is_ok() => {
                    return Ok(Command::User((*user).into()));
                }
                _ => {}
            }
        }
        _ => anyhow::bail!("This link is not on reddit.com or redd.it"),
    }
    anyhow::bail!(
        "Use a full post, comment, community or profile link; shortened /s/ share links are not supported"
    )
}

fn search_choices() -> Vec<Choice> {
    [
        ("sp", "Search posts across Reddit", SearchKind::Posts),
        ("sc", "Search communities", SearchKind::Communities),
        ("su", "Search people", SearchKind::People),
    ]
    .into_iter()
    .map(|(shortcut, label, kind)| {
        choice(
            shortcut,
            label,
            Command::Input(form(
                label,
                FormKind::Search(kind),
                vec![field("Query", "", false)],
                "/search/",
            )),
        )
    })
    .collect()
}
fn native(shortcut: &'static str, label: &str, path: &str) -> Choice {
    choice(
        shortcut,
        &format!("{label} · open Reddit ↗"),
        Command::Native(path.into()),
    )
}

impl App {
    pub fn show_open_link(&mut self) {
        if self.experience.writing() {
            return;
        }
        self.experience = Experience::default();
        self.experience.screens.push(Screen::Input(form(
            "Open Reddit URL",
            FormKind::OpenLink,
            vec![field("Reddit URL", "", false)],
            "/",
        )));
        self.overlay = Some(Overlay::Experience);
    }

    pub fn search_explore(&mut self) {
        if self.experience.writing() {
            return;
        }
        self.experience = Experience::default();
        let mut choices = search_choices();
        choices.extend([
            read("gc", "Discover communities", Query::Communities),
            choice("gp", "Popular posts", Command::Subreddit("popular".into())),
            choice("ga", "All Reddit", Command::Subreddit("all".into())),
        ]);
        self.push_menu("Search and explore", choices, MenuScope::Reddit);
    }

    pub(crate) fn show_search(&mut self, kind: SearchKind) {
        if self.experience.writing() {
            return;
        }
        self.experience = Experience::default();
        self.experience.screens.push(Screen::Input(form(
            "Search Reddit",
            FormKind::Search(kind),
            vec![field("Query", "", false)],
            "/search/",
        )));
        self.overlay = Some(Overlay::Experience);
    }

    pub(crate) fn show_profile_input(&mut self) {
        if self.experience.writing() {
            return;
        }
        self.experience = Experience::default();
        self.experience.screens.push(Screen::Input(form(
            "User profile",
            FormKind::Profile,
            vec![field("Username", "", false)],
            "/",
        )));
        self.overlay = Some(Overlay::Experience);
    }

    pub fn open_reddit_link(&mut self, path: &str) {
        match reddit_url(path)
            .and_then(|url| media::spawn_detached(&self.config.commands.browser, &[url]))
        {
            Ok(()) => self.info("Opened Reddit in your browser"),
            Err(error) => self.error(format!("Could not open Reddit: {error}")),
        }
    }

    pub fn comment_target(&self) -> Option<Target> {
        if !self.post_focused() {
            return None;
        }
        let view = self.post_view.as_ref()?;
        if view.free_scroll {
            return None;
        }
        let rows = view.rows();
        let RowKind::Comment { comment, .. } = &rows.get(view.cursor)?.kind else {
            return None;
        };
        if comment.id.starts_with(THREAD_ROOT) {
            return None;
        }
        let post = view.shown_post();
        Some(Target {
            id: format!("t1_{}", comment.id),
            author: comment.author.clone(),
            subreddit: post.subreddit.clone(),
            permalink: format!("/comments/{}/_/{}", post.id, comment.id),
        })
    }

    pub fn show_body_links(&mut self) {
        let (text, reveal) = match self.experience.screens.last() {
            Some(Screen::Browse {
                query,
                page,
                selected,
                ..
            }) if matches!(self.overlay, Some(Overlay::Experience)) => {
                let text = if !page.text.is_empty() && !matches!(query, Query::Thread(_)) {
                    page.text.as_str()
                } else {
                    page.entries
                        .get(*selected)
                        .map_or(page.text.as_str(), |entry| entry.body.as_str())
                };
                (text, false)
            }
            _ if self.overlay.is_none() && self.post_focused() => {
                let Some(view) = self
                    .post_view
                    .as_ref()
                    .filter(|view| !view.spoiler_hidden())
                else {
                    self.info("Reveal spoilers with v first");
                    return;
                };
                let text = if !view.free_scroll
                    && let Some(Row {
                        kind: RowKind::Comment { comment, .. },
                        ..
                    }) = view.rows().get(view.cursor)
                    && !comment.id.starts_with(THREAD_ROOT)
                {
                    comment.body.as_str()
                } else {
                    view.page.as_ref().map_or("", |page| page.body.as_str())
                };
                (text, view.reveal_spoilers)
            }
            _ => return,
        };
        let choices: Vec<_> = crate::model::body_links(text, reveal)
            .into_iter()
            .map(|link| {
                let label = link.label.clone();
                choice("", &label, Command::BodyLink(link))
            })
            .collect();
        if choices.is_empty() {
            self.info("No visible links in this text");
            return;
        }
        if self.overlay.is_none() {
            self.experience.screens.clear();
        }
        self.push_menu("Links in text", choices, MenuScope::Links);
    }

    fn action_target(&self) -> Option<Target> {
        if let Some(Screen::Browse { page, selected, .. }) = self.experience.screens.last()
            && self
                .overlay
                .as_ref()
                .is_some_and(|o| matches!(o, Overlay::Experience))
        {
            return page
                .entries
                .get(*selected)
                .and_then(|entry| entry.target.clone());
        }
        self.comment_target()
            .or_else(|| self.current_post().as_ref().map(Target::from_post))
    }

    fn experience_community(&self) -> String {
        if matches!(self.overlay, Some(Overlay::Experience))
            && let Some(Screen::Browse {
                query,
                page,
                selected,
                ..
            }) = self.experience.screens.last()
        {
            let entry = page.entries.get(*selected);
            if entry
                .and_then(|entry| entry.target.as_ref())
                .is_some_and(|target| crate::model::profile_user(&target.permalink).is_some())
            {
                return String::new();
            }
            let name = entry
                .and_then(|entry| entry.target.as_ref())
                .map(|target| target.subreddit.as_str())
                .or_else(|| {
                    entry
                        .and_then(|entry| entry.query.as_ref())
                        .into_iter()
                        .chain([query])
                        .find_map(|query| match query {
                            Query::Community(name)
                            | Query::Rules(name)
                            | Query::Moderators(name)
                            | Query::Wiki {
                                subreddit: name, ..
                            } => Some(name.as_str()),
                            _ => None,
                        })
                });
            return name
                .filter(|name| is_community(name))
                .unwrap_or_default()
                .to_string();
        }
        self.community()
            .filter(|name| is_community(name))
            .unwrap_or_default()
    }

    pub fn reddit_menu(&mut self) {
        if self.experience.writing() {
            self.info("Wait for the pending Reddit action");
            return;
        }
        self.experience = Experience::default();
        let mut choices = Vec::new();
        if let Some(account) = self.reddit.account() {
            choices.push(read(
                "ap",
                "My profile",
                Query::Profile(account.name.clone()),
            ));
            for (shortcut, label, feed) in [
                ("as", "Saved on Reddit", UserFeed::Saved),
                ("ah", "Hidden posts", UserFeed::Hidden),
                ("au", "Upvoted posts", UserFeed::Upvoted),
                ("ad", "Downvoted posts", UserFeed::Downvoted),
            ] {
                choices.push(read(
                    shortcut,
                    label,
                    Query::User {
                        name: account.name.clone(),
                        feed,
                    },
                ));
            }
        }
        for (shortcut, label, kind) in [
            ("nu", "Unread notifications", Inbox::Unread),
            ("nc", "Comment replies", Inbox::Replies),
            ("np", "Post replies", Inbox::PostReplies),
            ("nm", "Mentions", Inbox::Mentions),
            ("ni", "Inbox", Inbox::All),
        ] {
            choices.push(read(shortcut, label, Query::Inbox(kind)));
        }
        choices.extend([
            read("gf", "Custom feeds", Query::Multis),
            read("gc", "Discover communities", Query::Communities),
            choice("gp", "Popular posts", Command::Subreddit("popular".into())),
            choice("ga", "All Reddit", Command::Subreddit("all".into())),
        ]);
        choices.extend(search_choices());
        choices.extend([
            native("sC", "Search comments", "/search/?type=comment"),
            native("sm", "Search media", "/search/?type=media"),
            choice(
                "gu",
                "Open a user profile",
                Command::Input(form(
                    "User profile",
                    FormKind::Profile,
                    vec![field("Username", "", false)],
                    "/",
                )),
            ),
            choice(
                "ct",
                "Create text post",
                Command::Input(self.post_form(false)),
            ),
            choice(
                "cl",
                "Create link post",
                Command::Input(self.post_form(true)),
            ),
            choice(
                "cf",
                "Create a custom feed",
                Command::Input(form(
                    "Create private custom feed",
                    FormKind::CreateMulti,
                    vec![
                        field("Name", "", false),
                        Field {
                            required: false,
                            ..field("Description", "", true)
                        },
                    ],
                    "/",
                )),
            ),
            native("cm", "Upload media, create a poll or AMA", "/submit"),
            native("mc", "Chat", "/chat/"),
            native("nn", "Notifications", "/notifications"),
            native("af", "Following and favorite communities", "/"),
            native(
                "pp",
                "Privacy, blocked users and profile settings",
                "/settings/privacy",
            ),
            native("pn", "Notification settings", "/settings/notifications"),
            native("mq", "Moderation queue", "/mod/queue"),
            native("mm", "Mod mail", "/mod/mail"),
        ]);
        self.experience.screens.push(Screen::Menu {
            title: "Reddit".into(),
            choices: ordered_choices(choices, MenuScope::Reddit),
            selected: 0,
            native: "/".into(),
        });
        self.overlay = Some(Overlay::Experience);
    }

    fn post_form(&self, link: bool) -> Form {
        let sub = self.experience_community();
        form(
            if link {
                "Create link post"
            } else {
                "Create text post"
            },
            FormKind::Submit { link },
            vec![
                field("Subreddit", &sub, false),
                field("Title", "", false),
                field(if link { "URL" } else { "Body (Markdown)" }, "", !link),
            ],
            &if sub.is_empty() {
                "/submit".into()
            } else {
                format!("/r/{sub}/submit")
            },
        )
    }

    pub fn show_actions(&mut self) {
        if self.experience.pending.is_some() {
            self.info("Wait for the pending Reddit request");
            return;
        }
        let Some(target) = self.action_target() else {
            self.community_menu();
            return;
        };
        if target.id.starts_with("t4_") {
            let choices = vec![
                choice(
                    "r",
                    "Mark read",
                    Command::Write(Action::Read {
                        id: target.id.clone(),
                        read: true,
                    }),
                ),
                choice(
                    "u",
                    "Mark unread",
                    Command::Write(Action::Read {
                        id: target.id,
                        read: false,
                    }),
                ),
                native("o", "Notifications", "/notifications"),
            ];
            self.push_menu("Message actions", choices, MenuScope::Message);
        } else {
            self.read_experience(Query::Inspect(target), None);
        }
    }

    fn push_menu(&mut self, title: &str, choices: Vec<Choice>, scope: MenuScope) {
        let native = self.experience_native_path();
        self.experience.error = None;
        self.experience.screens.push(Screen::Menu {
            title: title.into(),
            choices: ordered_choices(choices, scope),
            selected: 0,
            native,
        });
        self.overlay = Some(Overlay::Experience);
    }

    fn details_menu(&mut self, details: Details) {
        let target = &details.target;
        let mut choices = vec![
            choice(
                "u",
                if details.vote == 1 {
                    "Remove upvote"
                } else {
                    "Upvote"
                },
                Command::Write(Action::Vote {
                    id: target.id.clone(),
                    direction: if details.vote == 1 { 0 } else { 1 },
                }),
            ),
            choice(
                "d",
                if details.vote == -1 {
                    "Remove downvote"
                } else {
                    "Downvote"
                },
                Command::Write(Action::Vote {
                    id: target.id.clone(),
                    direction: if details.vote == -1 { 0 } else { -1 },
                }),
            ),
            choice(
                "s",
                if details.saved {
                    "Unsave on Reddit"
                } else {
                    "Save on Reddit"
                },
                Command::Write(Action::Save {
                    id: target.id.clone(),
                    save: !details.saved,
                }),
            ),
            choice(
                "r",
                "Reply",
                Command::Input(form(
                    "Reply (Markdown)",
                    FormKind::Reply(target.clone()),
                    vec![field("Reply", "", true)],
                    &target.permalink,
                )),
            ),
            read("a", "Author profile", Query::Profile(target.author.clone())),
            read(
                "c",
                if crate::model::profile_user(&target.permalink).is_some() {
                    "Profile information"
                } else {
                    "Community information"
                },
                crate::model::profile_user(&target.permalink).map_or_else(
                    || Query::Community(target.subreddit.clone()),
                    |name| Query::Profile(name.to_string()),
                ),
            ),
            choice(
                "R",
                "Report content",
                Command::Input(form(
                    "Report content to Reddit",
                    FormKind::Report(target.clone()),
                    vec![field("Reason (up to 100 characters)", "", false)],
                    &target.permalink,
                )),
            ),
        ];
        if self.experience.screens.iter().any(|screen| {
            matches!(
                screen,
                Screen::Browse {
                    query: Query::Inbox(_),
                    ..
                }
            )
        }) {
            choices.push(choice(
                "nr",
                "Mark notification read",
                Command::Write(Action::Read {
                    id: target.id.clone(),
                    read: true,
                }),
            ));
            choices.push(choice(
                "nu",
                "Mark notification unread",
                Command::Write(Action::Read {
                    id: target.id.clone(),
                    read: false,
                }),
            ));
        }
        if target.id.starts_with("t3_") {
            choices.push(choice(
                "H",
                if details.hidden {
                    "Unhide post"
                } else {
                    "Hide post"
                },
                Command::Write(Action::Hide {
                    id: target.id.clone(),
                    hide: !details.hidden,
                }),
            ));
            choices.push(choice(
                "f",
                "Follow post notifications",
                Command::Write(Action::FollowPost {
                    id: target.id.clone(),
                    follow: true,
                }),
            ));
            choices.push(choice(
                "F",
                "Unfollow post notifications",
                Command::Write(Action::FollowPost {
                    id: target.id.clone(),
                    follow: false,
                }),
            ));
        }
        if self
            .reddit
            .account()
            .is_some_and(|me| me.name.eq_ignore_ascii_case(&target.author))
        {
            if details.editable {
                choices.push(choice(
                    "e",
                    "Edit my content",
                    Command::Input(form(
                        "Edit (Markdown)",
                        FormKind::Edit(target.clone()),
                        vec![field("Body", &details.raw_text, true)],
                        &target.permalink,
                    )),
                ));
            }
            choices.push(choice(
                "D",
                "Delete my content…",
                Command::Confirm(
                    "Delete this content from Reddit? This cannot be undone.".into(),
                    Action::Delete {
                        target: target.clone(),
                    },
                ),
            ));
        }
        if let Some(path) = details.crosspost {
            choices.push(read("x", "Read original crosspost", Query::Thread(path)));
        }
        let profile = format!("/user/{}", target.author);
        let profile_owner = crate::model::profile_user(&target.permalink);
        let sub = profile_owner.map_or_else(
            || format!("/r/{}", target.subreddit),
            |name| format!("/user/{name}/"),
        );
        choices.extend([
            native(
                "op",
                "Open content, vote in poll or view awards",
                &target.permalink,
            ),
            native(
                "oc",
                "Crosspost or manage post notifications",
                &target.permalink,
            ),
            native("oa", "Author profile: follow, block or chat", &profile),
            native(
                "os",
                if profile_owner.is_some() {
                    "Profile controls"
                } else {
                    "Mute/favorite community or select flair"
                },
                &sub,
            ),
            native("om", "Moderate this content", &target.permalink),
        ]);
        if name(&target.author).is_err() {
            choices.retain(|choice| match &choice.command {
                Command::Read(Query::Profile(_)) => false,
                Command::Native(path) => path != &profile,
                _ => true,
            });
        }
        let kind = if target.comment().is_some() {
            "Comment"
        } else {
            "Post"
        };
        self.push_menu(
            &format!("{kind} · u/{}", target.author),
            choices,
            MenuScope::Content,
        );
        if let Some(Screen::Menu { native, .. }) = self.experience.screens.last_mut() {
            *native = target.permalink.clone();
        }
    }

    pub fn membership_target(&self) -> Option<MembershipTarget> {
        if matches!(self.overlay, Some(Overlay::Experience)) {
            if let Some(Screen::Menu {
                choices, native, ..
            }) = self.experience.screens.last()
            {
                return choices
                    .iter()
                    .find_map(|choice| match &choice.command {
                        Command::Membership(target) => Some(target.clone()),
                        _ => None,
                    })
                    .or_else(|| {
                        native
                            .strip_prefix("/r/")
                            .filter(|name| is_community(name))
                            .map(|name| MembershipTarget::Community(name.into()))
                    });
            }
            let Screen::Browse {
                query,
                page,
                selected,
                ..
            } = self.experience.screens.last()?
            else {
                return None;
            };
            let entry = page.entries.get(*selected);
            if let Some(user) = entry
                .and_then(|entry| entry.target.as_ref())
                .and_then(|target| crate::model::profile_user(&target.permalink))
            {
                return Some(MembershipTarget::User(user.into()));
            }
            let sub = self.experience_community();
            if !sub.is_empty() {
                return Some(MembershipTarget::Community(sub));
            }
            return entry
                .and_then(|entry| entry.query.as_ref())
                .into_iter()
                .chain([query])
                .find_map(|query| match query {
                    Query::Profile(name) | Query::User { name, .. } => {
                        Some(MembershipTarget::User(name.clone()))
                    }
                    _ => None,
                })
                .filter(MembershipTarget::is_valid);
        }
        self.profile_context()
            .map(MembershipTarget::User)
            .or_else(|| {
                self.community()
                    .filter(|name| is_community(name))
                    .map(MembershipTarget::Community)
            })
    }

    pub fn community_menu(&mut self) {
        let Some(target) = self.membership_target() else {
            self.reddit_menu();
            return;
        };
        if let MembershipTarget::User(user) = &target {
            let path = format!("/user/{user}/");
            let action = self
                .subscribed(&target)
                .map_or("Check following status", |joined| target.action(!joined));
            self.push_menu(
                &target.label(),
                vec![
                    choice("m", action, Command::Membership(target.clone())),
                    choice("f", "Open submitted posts", Command::User(user.clone())),
                    choice(
                        "o",
                        "Open profile in browser ↗",
                        Command::Native(path.clone()),
                    ),
                    read("i", "Profile info", Query::Profile(user.clone())),
                ],
                MenuScope::User,
            );
            if let Some(Screen::Menu { native, .. }) = self.experience.screens.last_mut() {
                *native = path;
            }
            return;
        }
        let MembershipTarget::Community(sub) = target else {
            unreachable!()
        };
        let path = format!("/r/{sub}");
        self.push_menu(
            &format!("r/{sub}"),
            vec![
                choice("f", "Open community feed", Command::Subreddit(sub.clone())),
                choice(
                    "o",
                    "Open community in browser ↗",
                    Command::Native(path.clone()),
                ),
                choice(
                    "r",
                    "Mark community read",
                    Command::MarkCommunityRead(sub.clone()),
                ),
                read("a", "About and sidebar", Query::Community(sub.clone())),
                read("R", "Rules", Query::Rules(sub.clone())),
                read(
                    "wi",
                    "Wiki index",
                    Query::Wiki {
                        subreddit: sub.clone(),
                        page: "index".into(),
                    },
                ),
                choice(
                    "wp",
                    "Open wiki page",
                    Command::Input(form(
                        "Community wiki",
                        FormKind::Wiki(sub.clone()),
                        vec![field("Page", "index", false)],
                        &format!("{path}/wiki"),
                    )),
                ),
                read("mi", "Moderators", Query::Moderators(sub.clone())),
                native(
                    "mm",
                    "Message the mods",
                    &format!("{path}/about/moderators"),
                ),
                native("mq", "Moderation queue", &format!("{path}/about/modqueue")),
                native("mM", "Mod mail", "/mod/mail"),
            ],
            MenuScope::Community,
        );
        if let Some(Screen::Menu { native, .. }) = self.experience.screens.last_mut() {
            *native = path;
        }
    }

    fn read_experience(&mut self, query: Query, after: Option<String>) {
        if self.experience.pending.is_some() {
            return;
        }
        self.experience.error = None;
        if after.is_some()
            && let Some(Screen::Browse { pagination, .. }) = self.experience.screens.last_mut()
        {
            pagination.error = None;
        }
        if after.is_none() {
            self.experience.screens.push(Screen::Loading(query.clone()));
        }
        let request = Request::Read { query, after };
        let generation = self.generation();
        self.experience.pending = Some((generation, request.clone()));
        self.overlay = Some(Overlay::Experience);
        self.pool.submit(Job::Experience {
            generation,
            request,
            invalidations: Vec::new(),
        });
    }

    fn write_experience(&mut self, action: Action) {
        if self.experience.pending.is_some() || self.membership.is_some() || self.settings.writing()
        {
            self.experience.error = Some("Wait for the pending Reddit action".into());
            return;
        }
        self.experience.error = None;
        let request = Request::Write(action);
        let generation = self.generation();
        self.experience.pending = Some((generation, request.clone()));
        self.pool.submit(Job::Experience {
            generation,
            request,
            invalidations: self.experience_invalidations(),
        });
    }

    fn experience_invalidations(&self) -> Vec<crate::worker::Invalidation> {
        use crate::worker::Invalidation;
        let mut entries = Vec::new();
        entries.push(Invalidation::Post(self.experience_native_path()));
        for tab in &self.tabs {
            if let Some(target) = tab.feed_target() {
                entries.push(Invalidation::Feed {
                    target: target.clone(),
                    query: tab.feed.query(),
                    after: None,
                });
                for page in &tab.feed.pages {
                    entries.push(Invalidation::Feed {
                        target: target.clone(),
                        query: tab.feed.query(),
                        after: page.cursor.clone(),
                    });
                }
            }
        }
        for group in &self.all_subs.groups {
            entries.push(Invalidation::Feed {
                target: FeedTarget::Subreddit(group.subreddit.clone()),
                query: FeedQuery::DEFAULT_LISTING,
                after: None,
            });
        }
        if let Some(view) = &self.post_view {
            entries.push(Invalidation::Post(view.post.permalink.clone()));
            if let Some(page) = &view.page {
                collect_invalidations(&page.comments, &mut entries);
            }
        }
        entries
    }

    pub fn on_experience(&mut self, generation: u64, result: Result<Response, String>) {
        let Some((pending, _)) = &self.experience.pending else {
            return;
        };
        if *pending != generation {
            return;
        }
        let (_, request) = self.experience.pending.take().unwrap();
        match result {
            Err(error) => {
                if let Request::Read {
                    query,
                    after: Some(_),
                    ..
                } = &request
                    && let Some(Screen::Browse {
                        query: current,
                        pagination,
                        ..
                    }) = self.experience.screens.last_mut()
                    && current == query
                {
                    pagination.error = Some(format!(
                        "{error}. r retries loading more; Ctrl+o opens Reddit."
                    ));
                    return;
                }
                self.experience.error = Some(match request {
                    Request::Read { .. } => format!("{error}. r retries; Ctrl+o opens Reddit."),
                    Request::Write(_) => format!(
                        "{error}. Open Reddit with Ctrl+o to check before submitting again."
                    ),
                });
            }
            Ok(Response::Details(details)) => {
                self.experience.screens.pop();
                self.details_menu(details);
            }
            Ok(Response::Page(mut page)) => {
                let Request::Read { query, after } = request else {
                    return;
                };
                if let Some(cursor) = after {
                    if let Some(Screen::Browse {
                        query: current,
                        page: previous,
                        pagination,
                        ..
                    }) = self.experience.screens.last_mut()
                        && *current == query
                    {
                        let mut ids: HashSet<_> = previous.entries.iter().map(entry_key).collect();
                        previous.entries.extend(
                            page.entries
                                .drain(..)
                                .filter(|entry| ids.insert(entry_key(entry))),
                        );
                        pagination.visited.insert(cursor);
                        previous.after = page
                            .after
                            .filter(|cursor| !pagination.visited.contains(cursor));
                    }
                } else {
                    self.experience.screens.pop();
                    self.experience.screens.push(Screen::Browse {
                        pagination: Default::default(),
                        query,
                        page,
                        selected: 0,
                        scroll: 0,
                    });
                }
            }
            Ok(Response::Thread(page)) => {
                let comment = match &request {
                    Request::Read {
                        query: Query::Thread(link),
                        ..
                    } => thread_location(link).ok().and_then(|(_, comment)| comment),
                    _ => None,
                };
                let post = page.post.clone();
                let link = post.permalink.clone();
                self.overlay = None;
                self.focus_main();
                if let Some(user) = post.profile_user() {
                    self.open_user(user);
                } else {
                    self.open_subreddit(&post.subreddit);
                }
                if let Some(generation) = self.show_post(post, &link) {
                    self.focus = Focus::Post;
                    self.on_msg(Msg::Post {
                        generation,
                        result: Ok(page),
                    });
                    if let Some(comment) = comment {
                        self.select_loaded_comment(&comment);
                    }
                }
            }
            Ok(Response::Written { url }) => {
                let Request::Write(action) = request else {
                    return;
                };
                self.after_experience_write(&action);
                self.experience.screens.pop();
                self.experience.error = None;
                self.info("Reddit confirmed the action");
                if matches!(action, Action::DeleteMulti { .. }) {
                    while self.experience.screens.last().is_some_and(|screen| {
                        !matches!(
                            screen,
                            Screen::Browse {
                                query: Query::Multis,
                                ..
                            }
                        )
                    }) {
                        self.experience.screens.pop();
                    }
                    if !self.experience.screens.is_empty() {
                        self.experience.screens.pop();
                    }
                    self.read_experience(Query::Multis, None);
                    return;
                }
                if let Some(url) = url {
                    self.read_experience(Query::Thread(url), None);
                } else if let Some(Screen::Browse { query, .. }) = self.experience.screens.last() {
                    let query = query.clone();
                    self.experience.screens.pop();
                    self.read_experience(query, None);
                } else {
                    self.overlay = None;
                }
            }
        }
    }

    fn after_experience_write(&mut self, action: &Action) {
        let invalid_generation = self.generation();
        self.content_generation_floor = invalid_generation;
        for tab in &mut self.tabs {
            tab.feed.generation = invalid_generation;
            tab.feed.requests.clear();
            tab.feed.pending.clear();
            tab.feed.refreshing.clear();
            tab.feed.forced.clear();
            tab.feed.deferred.clear();
            tab.feed.loaded = false;
        }
        let target_post = thread_location(&self.experience_native_path())
            .ok()
            .and_then(|(path, _)| crate::reddit::parse::post_id(&path).map(str::to_string));
        for group in &mut self.all_subs.groups {
            if group.loading
                || target_post
                    .as_ref()
                    .is_some_and(|id| group.posts.iter().any(|post| &post.id == id))
            {
                let generation = self.all_subs.next_group_generation;
                self.all_subs.next_group_generation += 1;
                group.generation = generation;
                group.loading = false;
                group.needs_refresh = true;
            }
        }
        if let Action::Hide { id, hide: true }
        | Action::Report { id, .. }
        | Action::Delete {
            target: Target { id, .. },
        } = action
            && let Some(id) = id.strip_prefix("t3_")
        {
            for tab in &mut self.tabs {
                tab.feed.remove_post(id);
                tab.feed.selected = tab
                    .feed
                    .selected
                    .min(tab.feed.posts.len().saturating_sub(1));
            }
            for group in &mut self.all_subs.groups {
                group.posts.retain(|post| post.id != id);
            }
        }
        if self.post_view.is_some() {
            self.refresh_post();
        }
        if self.active_tab().feed_target().is_some() {
            self.fetch_feed(self.active, None, true);
        }
        self.sync_feed_demand();
    }

    fn execute_experience(&mut self, command: Command) {
        self.experience.error = None;
        match command {
            Command::Read(query) => self.read_experience(query, None),
            Command::Input(form) => self.experience.screens.push(Screen::Input(form)),
            Command::Write(action) => self.write_experience(action),
            Command::Confirm(label, action) => {
                let native = self.experience_native_path();
                self.experience.screens.push(Screen::Confirm {
                    label,
                    action,
                    native,
                });
            }
            Command::Native(path) => self.open_reddit_link(&path),
            Command::BodyLink(link) => self.push_menu(
                &link.label,
                vec![
                    choice(
                        "o",
                        &format!("Open in browser ↗\n{}", link.url),
                        Command::OpenUrl(link.url.clone()),
                    ),
                    choice("y", "Copy URL", Command::CopyUrl(link.url)),
                ],
                MenuScope::Links,
            ),
            Command::OpenUrl(url) => {
                match media::spawn_detached(&self.config.commands.browser, &[url]) {
                    Ok(()) => self.info("Opened link in your browser"),
                    Err(error) => self.error(format!("Could not open link: {error}")),
                }
            }
            Command::CopyUrl(url) => match media::copy(&url) {
                Ok(tool) => self.info(format!("Copied link ({tool})")),
                Err(error) => self.error(format!("copy failed: {error:#}")),
            },
            Command::Membership(target) => self.show_membership_for(target),
            Command::User(name) => {
                self.experience = Experience::default();
                self.overlay = None;
                self.close_post();
                self.focus_main();
                self.open_user(&name);
            }
            Command::Subreddit(sub) => {
                self.experience = Experience::default();
                self.overlay = None;
                self.close_post();
                self.focus_main();
                self.open_subreddit(&sub);
            }
            Command::MarkCommunityRead(sub) => {
                self.overlay = None;
                self.mark_community_read(&sub);
            }
            Command::Sort(sort) => {
                self.overlay = None;
                self.sort_comments(&sort);
            }
            Command::Target(target, post) => {
                if let Some(comment) = target.comment()
                    && self.select_loaded_comment(comment)
                {
                    self.overlay = None;
                    return;
                }
                if let Some(post) = post {
                    self.overlay = None;
                    self.open_post_permalink(post, target.permalink);
                    self.focus = Focus::Post;
                } else if !target.permalink.is_empty() {
                    self.read_experience(Query::Thread(target.permalink), None);
                } else {
                    self.open_reddit_link("/notifications");
                }
            }
        }
    }

    pub fn experience_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        if ctrl && key.code == KeyCode::Char('o') {
            self.experience_native();
            return;
        }
        if self.experience.writing() {
            return;
        }
        if let Some(Screen::Input(_)) = self.experience.screens.last() {
            if key.code == KeyCode::Esc {
                self.experience_back();
                return;
            }
            if (ctrl && key.code == KeyCode::Char('s')) || (alt && key.code == KeyCode::Enter) {
                self.submit_experience_form();
                return;
            }
            let form = self.experience.input().unwrap();
            let field_count = form.fields.len();
            let current = &mut form.fields[form.active];
            match key.code {
                KeyCode::Tab => form.active = (form.active + 1) % field_count,
                KeyCode::BackTab => form.active = (form.active + field_count - 1) % field_count,
                KeyCode::Enter if current.multiline => current.insert("\n"),
                KeyCode::Enter if form.active + 1 < field_count => form.active += 1,
                KeyCode::Enter => self.submit_experience_form(),
                KeyCode::Backspace if alt => current.delete_word(),
                KeyCode::Backspace => {
                    let start = current.previous();
                    current.value.replace_range(start..current.cursor, "");
                    current.cursor = start;
                }
                KeyCode::Delete => {
                    let end = current.next();
                    current.value.replace_range(current.cursor..end, "");
                }
                KeyCode::Left => current.cursor = current.previous(),
                KeyCode::Right => current.cursor = current.next(),
                KeyCode::Home | KeyCode::Char('a')
                    if !matches!(key.code, KeyCode::Char(_)) || ctrl =>
                {
                    current.cursor = current.value[..current.cursor]
                        .rfind('\n')
                        .map_or(0, |i| i + 1)
                }
                KeyCode::End | KeyCode::Char('e')
                    if !matches!(key.code, KeyCode::Char(_)) || ctrl =>
                {
                    current.cursor = current.value[current.cursor..]
                        .find('\n')
                        .map_or(current.value.len(), |i| current.cursor + i)
                }
                KeyCode::Char('u') if ctrl => {
                    current.value.clear();
                    current.cursor = 0;
                }
                KeyCode::Char(c) if !ctrl && !alt && !c.is_control() => {
                    current.insert(&c.to_string())
                }
                _ => {}
            }
            return;
        }
        if !(key.modifiers.difference(KeyModifiers::SHIFT).is_empty()
            || key.modifiers == KeyModifiers::CONTROL
                && matches!(key.code, KeyCode::Char('d' | 'u' | 'e' | 'y')))
        {
            return;
        }
        if matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) {
            self.experience_back();
            return;
        }
        let reading_navigation = matches!(
            key.code,
            KeyCode::Up
                | KeyCode::Down
                | KeyCode::Home
                | KeyCode::End
                | KeyCode::PageUp
                | KeyCode::PageDown
                | KeyCode::Char('j' | 'k' | 'G')
        ) || (ctrl
            && matches!(key.code, KeyCode::Char('d' | 'u' | 'e' | 'y')));
        if self.experience.pending.is_some()
            && !(self.experience.loading_more() && reading_navigation)
        {
            return;
        }
        if ctrl && matches!(key.code, KeyCode::Char('d' | 'u' | 'e' | 'y')) {
            if let Some(Screen::Browse { scroll, .. }) = self.experience.screens.last_mut() {
                let rows = if matches!(key.code, KeyCode::Char('d' | 'u')) {
                    5
                } else {
                    1
                };
                *scroll = if matches!(key.code, KeyCode::Char('d' | 'e')) {
                    scroll.saturating_add(rows)
                } else {
                    scroll.saturating_sub(rows)
                };
            }
            if matches!(key.code, KeyCode::Char('d' | 'e')) {
                self.prefetch_experience();
            }
            return;
        }
        let mut command = None;
        let mut read_next = None;
        match self.experience.screens.last_mut() {
            Some(Screen::Menu {
                choices, selected, ..
            }) => match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    *selected = (*selected + 1).min(choices.len().saturating_sub(1))
                }
                KeyCode::Char('k') | KeyCode::Up => *selected = selected.saturating_sub(1),
                KeyCode::Home => *selected = 0,
                KeyCode::Char('G') | KeyCode::End => *selected = choices.len().saturating_sub(1),
                KeyCode::Enter | KeyCode::Right => {
                    command = choices.get(*selected).map(|choice| choice.command.clone())
                }
                _ => {}
            },
            Some(Screen::Confirm { action, .. }) if key.code == KeyCode::Enter => {
                command = Some(Command::Write(action.clone()))
            }
            Some(Screen::Loading(query)) if key.code == KeyCode::Char('r') => {
                read_next = Some((query.clone(), None));
            }
            Some(Screen::Browse {
                query,
                page,
                selected,
                scroll,
                ..
            }) => match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    let next = (*selected + 1).min(page.entries.len().saturating_sub(1));
                    if next != *selected {
                        *selected = next;
                        *scroll = 0;
                    } else if page.entries.is_empty() {
                        *scroll += 1;
                    }
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    if *selected > 0 {
                        *selected -= 1;
                        *scroll = 0;
                    } else if page.entries.is_empty() {
                        *scroll = scroll.saturating_sub(1);
                    }
                }
                KeyCode::PageDown => *scroll += 10,
                KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
                KeyCode::Home => {
                    *selected = 0;
                    *scroll = 0;
                }
                KeyCode::Char('G') | KeyCode::End => {
                    let next = page.entries.len().saturating_sub(1);
                    if next != *selected {
                        *selected = next;
                        *scroll = 0;
                    }
                }
                KeyCode::Enter => {
                    command = page.entries.get(*selected).and_then(|entry| {
                        entry
                            .query
                            .clone()
                            .map(|query| match query {
                                Query::Community(sub) => Command::Subreddit(sub),
                                Query::Profile(user) => Command::User(user),
                                query => Command::Read(query),
                            })
                            .or_else(|| {
                                entry
                                    .target
                                    .clone()
                                    .map(|target| Command::Target(target, entry.post.clone()))
                            })
                    })
                }
                KeyCode::Char('a' | 'd' | 'D') if matches!(query, Query::Multi { .. }) => {
                    self.multi_action(key.code);
                    return;
                }
                _ => {}
            },
            _ => {}
        }
        if let Some((query, after)) = read_next {
            if after.is_none() {
                self.experience.screens.pop();
            }
            self.read_experience(query, after);
        }
        if let Some(command) = command {
            self.execute_experience(command);
        }
        if matches!(
            key.code,
            KeyCode::Down | KeyCode::End | KeyCode::PageDown | KeyCode::Char('j' | 'G')
        ) {
            self.prefetch_experience();
        }
    }

    fn prefetch_experience(&mut self) {
        if self.experience.pending.is_some() {
            return;
        }
        let Some(Screen::Browse {
            query,
            page,
            selected,
            pagination,
            ..
        }) = self.experience.screens.last()
        else {
            return;
        };
        let Some(after) = page
            .after
            .as_ref()
            .filter(|cursor| !pagination.visited.contains(*cursor))
        else {
            return;
        };
        if pagination.error.is_none()
            && selected.saturating_add(PREFETCH_MARGIN) >= page.entries.len()
        {
            self.read_experience(query.clone(), Some(after.clone()));
        }
    }

    fn multi_action(&mut self, key: KeyCode) {
        let Some(Screen::Browse {
            query: Query::Multi { owner, name },
            ..
        }) = self.experience.screens.last()
        else {
            return;
        };
        let (owner, name) = (owner.clone(), name.clone());
        if key == KeyCode::Char('D') {
            let native = self.experience_native_path();
            self.experience.screens.push(Screen::Confirm {
                label: format!("Delete custom feed {name} from Reddit?"),
                action: Action::DeleteMulti { owner, name },
                native,
            });
        } else {
            let add = key == KeyCode::Char('a');
            self.experience.screens.push(Screen::Input(form(
                if add {
                    "Add community to custom feed"
                } else {
                    "Remove community from custom feed"
                },
                FormKind::MultiCommunity { owner, name, add },
                vec![field("Subreddit", "", false)],
                "/",
            )));
        }
    }

    fn experience_back(&mut self) {
        if self.experience.writing() {
            return;
        }
        self.experience.pending = None;
        self.experience.error = None;
        self.experience.screens.pop();
        if self.experience.screens.is_empty() {
            self.overlay = None;
        }
    }

    pub(crate) fn experience_native(&mut self) {
        self.open_reddit_link(&self.experience_native_path());
    }

    pub(crate) fn copy_experience_link(&mut self) {
        let path = self
            .action_target()
            .map_or_else(|| self.experience_native_path(), |target| target.permalink);
        match reddit_url(&path).and_then(|url| media::copy(&url)) {
            Ok(tool) => self.info(format!("Copied link ({tool})")),
            Err(error) => self.error(format!("copy failed: {error:#}")),
        }
    }

    pub(crate) fn collection_action(&mut self, action: crate::keys::map::Action) {
        use crate::keys::map::Action as KeyAction;
        if self.experience.pending.is_some() {
            return;
        }
        let Some(Screen::Browse {
            query,
            page,
            pagination,
            ..
        }) = self.experience.screens.last_mut()
        else {
            return;
        };
        let after = (action == KeyAction::Reload && pagination.error.is_some())
            .then(|| page.after.clone())
            .flatten();
        match action {
            KeyAction::Sort => {
                let Query::Search {
                    kind: SearchKind::Posts,
                    sort,
                    ..
                } = query
                else {
                    return;
                };
                *sort = sort.next();
            }
            KeyAction::Time => {
                let Query::Search { time, .. } = query else {
                    return;
                };
                *time = time.next();
            }
            KeyAction::Reload => {}
            _ => return,
        }
        let query = query.clone();
        if after.is_none() {
            self.experience.screens.pop();
        }
        self.read_experience(query, after);
    }

    fn experience_native_path(&self) -> String {
        match self.experience.screens.last() {
            Some(Screen::Input(form)) => form.native.clone(),
            Some(Screen::Menu { native, .. } | Screen::Confirm { native, .. }) => native.clone(),
            Some(Screen::Browse { page, .. }) => page.url.clone(),
            Some(Screen::Loading(query)) => query.url().unwrap_or_else(|_| "/".into()),
            _ => self
                .action_target()
                .map(|target| target.permalink)
                .unwrap_or_else(|| "/".into()),
        }
    }

    fn submit_experience_form(&mut self) {
        if self.experience.pending.is_some() {
            return;
        }
        let Some(Screen::Input(form)) = self.experience.screens.last().cloned() else {
            return;
        };
        if let Some(index) = form
            .fields
            .iter()
            .position(|field| field.required && field.value.trim().is_empty())
        {
            self.experience.error = Some(format!("{} is required", form.fields[index].label));
            if let Some(current) = self.experience.input() {
                current.active = index;
            }
            return;
        }
        let v = |index: usize| form.fields[index].value.clone();
        let action = match form.kind {
            FormKind::OpenLink => {
                match link_command(&v(0)) {
                    Ok(command) => self.execute_experience(command),
                    Err(error) => self.experience.error = Some(error.to_string()),
                }
                return;
            }
            FormKind::Search(kind) => {
                self.read_experience(
                    Query::Search {
                        text: v(0),
                        kind,
                        sort: SearchSort::Relevance,
                        time: TimeRange::All,
                    },
                    None,
                );
                return;
            }
            FormKind::Profile => {
                let user = v(0).trim().trim_start_matches("u/").to_string();
                if let Err(error) = name(&user) {
                    self.experience.error = Some(error.to_string());
                    return;
                }
                self.read_experience(Query::Profile(user), None);
                return;
            }
            FormKind::Reply(target) => Action::Reply {
                parent: target.id,
                text: v(0),
            },
            FormKind::Edit(target) => Action::Edit { target, text: v(0) },
            FormKind::Report(target) => Action::Report {
                id: target.id,
                reason: v(0),
            },
            FormKind::Submit { link } => Action::Submit {
                subreddit: normalize_subreddit(&v(0)).into(),
                title: v(1),
                text: v(2),
                link,
            },
            FormKind::CreateMulti => Action::CreateMulti {
                name: v(0),
                description: v(1),
            },
            FormKind::MultiCommunity { owner, name, add } => Action::MultiCommunity {
                owner,
                name,
                subreddit: normalize_subreddit(&v(0)).into(),
                add,
            },
            FormKind::Wiki(subreddit) => {
                self.read_experience(
                    Query::Wiki {
                        subreddit,
                        page: v(0),
                    },
                    None,
                );
                return;
            }
            FormKind::Find => {
                self.find_comments(&v(0));
                return;
            }
        };
        self.write_experience(action);
    }

    pub fn experience_paste(&mut self, text: &str) {
        if self.experience.writing() {
            return;
        }
        if let Some(form) = self.experience.input() {
            let field = &mut form.fields[form.active];
            for c in text.chars() {
                if !c.is_control() || (field.multiline && matches!(c, '\n' | '\t')) {
                    field.insert(&c.to_string());
                } else if matches!(c, '\n' | '\r' | '\t') {
                    field.insert(" ");
                }
            }
        }
    }

    pub fn comment_tools(&mut self) {
        if self.post_view.is_none() {
            return;
        }
        let mut choices = vec![choice(
            "/",
            "Find in loaded comments",
            Command::Input(form(
                "Find in loaded comments",
                FormKind::Find,
                vec![field("Text or author", "", false)],
                &self.current_post().map(|p| p.permalink).unwrap_or_default(),
            )),
        )];
        for (shortcut, label, sort) in [
            ("b", "Best", "confidence"),
            ("t", "Top", "top"),
            ("n", "New", "new"),
            ("o", "Old", "old"),
            ("c", "Controversial", "controversial"),
            ("Q", "Q&A", "qa"),
        ] {
            choices.push(choice(
                shortcut,
                &format!("Sort comments: {label}"),
                Command::Sort(sort.into()),
            ));
        }
        self.push_menu("Comments", choices, MenuScope::Comments);
    }

    pub(crate) fn find_in_comments(&mut self) {
        if self.post_view.as_ref().is_none_or(PostView::spoiler_hidden) {
            return;
        }
        self.experience = Experience::default();
        self.experience.screens.push(Screen::Input(form(
            "Find in loaded comments",
            FormKind::Find,
            vec![field("Text or author", "", false)],
            &self
                .current_post()
                .map(|post| post.permalink)
                .unwrap_or_default(),
        )));
        self.overlay = Some(Overlay::Experience);
    }

    fn sort_comments(&mut self, sort: &str) {
        let Some(post) = self
            .post_view
            .as_ref()
            .map(|view| view.shown_post().clone())
        else {
            return;
        };
        let permalink = post.permalink.clone();
        if let Some(generation) = self.show_post(post, &permalink) {
            self.post_view.as_mut().unwrap().comment_sort = sort.into();
            self.focus = Focus::Post;
            self.pool.want_post(Some(generation));
            self.pool.submit(Job::SortedPost {
                generation,
                permalink,
                sort: sort.into(),
            });
        }
    }

    fn find_comments(&mut self, text: &str) {
        let Some(view) = &self.post_view else {
            return;
        };
        if view.spoiler_hidden() {
            self.experience.error =
                Some("Reveal this post's spoilers with SPC v s before searching".into());
            return;
        }
        let Some(page) = &view.page else {
            self.experience.error = Some("Wait for the comments to load".into());
            return;
        };
        let mut entries = Vec::new();
        collect_matches(
            &page.comments,
            &page.post,
            &text.to_lowercase(),
            &mut entries,
        );
        self.experience.screens.push(Screen::Browse {
            pagination: Default::default(),
            query: Query::Thread(page.post.permalink.clone()),
            page: Page {
                title: format!("Loaded comments matching {text}"),
                text: "Only comments already loaded in this post are searched.".into(),
                entries,
                after: None,
                url: page.post.reddit_url(),
            },
            selected: 0,
            scroll: 0,
        });
    }

    fn select_loaded_comment(&mut self, id: &str) -> bool {
        let Some(view) = &mut self.post_view else {
            return false;
        };
        if !view
            .page
            .as_ref()
            .is_some_and(|page| has_comment(&page.comments, id))
        {
            return false;
        }
        view.collapsed.clear();
        let index = view.rows().iter().position(
            |row| matches!(&row.kind, RowKind::Comment { comment, .. } if comment.id == id),
        );
        if let Some(index) = index {
            view.cursor = index;
            view.free_scroll = false;
            view.hold_view = None;
            view.drawn_cursor = None;
            view.row_starts.clear();
            view.content_changed = true;
            self.focus = Focus::Post;
            return true;
        }
        false
    }

    pub fn gallery_step(&mut self, forward: bool) {
        let Some(view) = &mut self.post_view else {
            return;
        };
        if view.spoiler_hidden() {
            self.info("Reveal spoilers with v first");
            return;
        }
        let Media::Gallery { images, .. } = &view.shown_post().media else {
            return;
        };
        let len = images.len();
        if len == 0 {
            return;
        }
        view.gallery_index = if forward {
            (view.gallery_index + 1) % len
        } else {
            (view.gallery_index + len - 1) % len
        };
        view.scroll = 0;
        view.free_scroll = true;
        let url = view.preview_image().map(std::borrow::Cow::into_owned);
        if let Some(url) = url {
            self.request_image(&url);
        }
    }
}

fn entry_key(entry: &Entry) -> String {
    entry
        .target
        .as_ref()
        .map(|target| target.id.clone())
        .unwrap_or_else(|| entry.title.clone())
}

fn has_comment(comments: &[Comment], id: &str) -> bool {
    comments
        .iter()
        .any(|comment| comment.id == id || has_comment(&comment.replies, id))
}

fn collect_invalidations(comments: &[Comment], entries: &mut Vec<crate::worker::Invalidation>) {
    for comment in comments {
        if let Some(more) = &comment.more {
            entries.push(crate::worker::Invalidation::Replies(more.cursor.clone()));
        }
        collect_invalidations(&comment.replies, entries);
    }
}

fn collect_matches(comments: &[Comment], post: &Post, query: &str, entries: &mut Vec<Entry>) {
    for comment in comments {
        if !comment.id.starts_with(THREAD_ROOT)
            && (crate::model::unformatted(&comment.body)
                .to_lowercase()
                .contains(query)
                || comment.author.to_lowercase().contains(query))
        {
            entries.push(Entry {
                title: format!("u/{} · {}", comment.author, comment.id),
                body: comment.body.clone(),
                target: Some(Target {
                    id: format!("t1_{}", comment.id),
                    author: comment.author.clone(),
                    subreddit: post.subreddit.clone(),
                    permalink: format!("/comments/{}/_/{}", post.id, comment.id),
                }),
                query: None,
                post: None,
            });
        }
        collect_matches(&comment.replies, post, query, entries);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reddit::tests::{Fixture, Reply, saved_export};
    use ratatui_image::picker::Picker;
    use serde_json::json;

    fn app() -> (App, tempfile::TempDir, Fixture) {
        let fixture = Fixture::new(|_| {
            Reply::json(json!({"kind":"Listing","data":{"children":[],"after":null}}))
        });
        let reddit = fixture.saved_reddit(&saved_export("reader", "reader"));
        let (pool, _) = Pool::start(reddit.clone(), 1, None);
        let dir = tempfile::tempdir().unwrap();
        let app = App::new(
            Config {
                media: crate::config::MediaSettings {
                    image_preview: false,
                    ..Default::default()
                },
                ..Config::default()
            },
            pool,
            reddit,
            dir.path().join("state.json"),
            Picker::halfblocks(),
        );
        (app, dir, fixture)
    }

    fn key(app: &mut App, code: KeyCode) {
        crate::keys::handle(app, KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn sequence(app: &mut App, keys: &str) {
        for c in keys.chars() {
            key(app, KeyCode::Char(c));
        }
    }

    #[test]
    fn when_a_reddit_url_is_pasted_it_should_open_its_thread_or_feed_and_retain_invalid_input() {
        for (link, expected) in [
            (
                "https://old.reddit.com/r/rust/comments/abc/title/def/?context=3",
                "thread",
            ),
            ("https://redd.it/abc", "thread"),
            ("https://reddit.com/r/rust/?share_id=ignored", "rust"),
            ("https://www.reddit.com/r/popular/", "popular"),
            ("https://www.reddit.com/user/spez/", "user"),
            ("https://example.com/r/rust/", "error"),
        ] {
            let (mut app, _dir, _fixture) = app();
            sequence(&mut app, " ou");
            crate::keys::paste(&mut app, link);
            key(&mut app, KeyCode::Enter);

            match expected {
                "thread" => assert!(matches!(&app.experience.pending,
                    Some((_, Request::Read { query: Query::Thread(path), .. }))
                    if path == "/r/rust/comments/abc/title/def/" || path == "/comments/abc/")),
                "rust" | "popular" => assert!(
                    matches!(&app.active_tab().kind, TabKind::Subreddit { name, .. } if name == expected)
                ),
                "user" => assert!(
                    matches!(&app.active_tab().kind, TabKind::User { name, .. } if name == "spez")
                ),
                _ => {
                    assert!(app.experience.error.is_some());
                    assert_eq!(app.experience.input().unwrap().fields[0].value, link);
                    assert!(app.experience.pending.is_none());
                }
            }
        }
    }

    #[test]
    fn when_a_pasted_thread_loads_navigation_should_follow_its_source_and_keep_comment_focus() {
        for profile in [false, true] {
            let (mut app, _dir, _fixture) = app();
            let mut page = post_page();
            let existing = if profile {
                page.post.permalink = "/user/writer/comments/1x4v4sn/title/".into();
                page.post.subreddit = "u_writer".into();
                None
            } else {
                app.open_subreddit("Neovim");
                Some(app.active_tab().id)
            };
            app.switch_tab(0);
            if profile {
                app.show_sidebar = false;
                app.focus_sidebar();
            }
            sequence(&mut app, " ou");
            crate::keys::paste(
                &mut app,
                "https://reddit.com/r/wrong/comments/1x4v4sn/title/c1/",
            );
            key(&mut app, KeyCode::Enter);
            let generation = app.experience.pending.as_ref().unwrap().0;
            assert!(matches!(app.active_tab().kind, TabKind::Home));

            app.on_experience(generation, Err("Unavailable".into()));
            assert!(matches!(app.active_tab().kind, TabKind::Home));
            key(&mut app, KeyCode::Char('r'));
            let generation = app.experience.pending.as_ref().unwrap().0;
            app.on_experience(generation, Ok(Response::Thread(Box::new(page))));

            if profile {
                assert!(
                    matches!(&app.active_tab().kind, TabKind::User { name, .. } if name == "writer")
                );
                assert!(!app.sidebar_peek);
                assert!(!app.show_sidebar);
            } else {
                assert_eq!(app.active_tab().id, existing.unwrap());
                assert_eq!(app.active_tab().subreddit(), Some("Neovim"));
            }
            assert_eq!(app.tabs.len(), 4);
            assert_eq!(app.focus, Focus::Post);
            assert!(app.overlay.is_none());
            assert_eq!(app.post_view.as_ref().unwrap().shown_post().id, "1x4v4sn");
            assert_eq!(app.comment_target().unwrap().id, "t1_c1");
        }
    }

    #[test]
    fn when_exploring_search_should_open_community_and_people_feeds_directly() {
        let (mut app, _dir, _fixture) = app();
        key(&mut app, KeyCode::F(3));
        let Some(Screen::Menu { choices, .. }) = app.experience.screens.last() else {
            panic!("search and explore missing")
        };
        assert!(
            choices
                .iter()
                .any(|choice| matches!(choice.command, Command::Read(Query::Communities)))
        );
        key(&mut app, KeyCode::Enter);
        crate::keys::paste(&mut app, "rust");
        key(&mut app, KeyCode::Enter);
        assert!(matches!(&app.experience.pending,
            Some((_, Request::Read { query: Query::Search { text, kind: SearchKind::Posts, .. }, .. })) if text == "rust"));
        app.experience.pending = None;
        for query in [
            Query::Community("rust".into()),
            Query::Profile("spez".into()),
        ] {
            app.overlay = Some(Overlay::Experience);
            app.experience.screens = vec![Screen::Browse {
                pagination: Default::default(),
                query: Query::Communities,
                page: Page {
                    title: "Results".into(),
                    text: String::new(),
                    entries: vec![Entry {
                        title: "Result".into(),
                        body: String::new(),
                        query: Some(query.clone()),
                        target: None,
                        post: None,
                    }],
                    after: None,
                    url: "/".into(),
                },
                selected: 0,
                scroll: 0,
            }];
            key(&mut app, KeyCode::Enter);
            assert!(app.overlay.is_none());
            match query {
                Query::Community(_) => assert!(
                    matches!(&app.active_tab().kind, TabKind::Subreddit { name, .. } if name == "rust")
                ),
                _ => assert!(
                    matches!(&app.active_tab().kind, TabKind::User { name, .. } if name == "spez")
                ),
            }
        }
    }

    #[test]
    fn intent_order_should_put_unread_notifications_first_in_the_reddit_menu() {
        let (mut app, _dir, _fixture) = app();
        app.reddit_menu();
        let Some(Screen::Menu { choices, .. }) = app.experience.screens.last() else {
            panic!("menu missing")
        };
        assert!(matches!(
            choices[0].command,
            Command::Read(Query::Inbox(Inbox::Unread))
        ));
        sequence(&mut app, "sp");
        assert!(matches!(
            app.experience.screens.last(),
            Some(Screen::Input(Form {
                kind: FormKind::Search(SearchKind::Posts),
                ..
            }))
        ));
    }

    #[test]
    fn intent_order_should_put_reply_first_and_destructive_delete_last() {
        let (mut app, _dir, _fixture) = app();
        app.details_menu(Details {
            target: Target {
                id: "t1_comment".into(),
                author: "reader".into(),
                subreddit: "rust".into(),
                permalink: "/comments/post/_/comment".into(),
            },
            raw_text: "body".into(),
            saved: false,
            hidden: false,
            vote: 0,
            editable: true,
            crosspost: None,
        });
        let Some(Screen::Menu { choices, .. }) = app.experience.screens.last() else {
            panic!("menu missing")
        };
        assert!(matches!(
            &choices[0].command,
            Command::Input(Form {
                kind: FormKind::Reply(_),
                ..
            })
        ));
        assert!(matches!(
            &choices.last().unwrap().command,
            Command::Confirm(_, Action::Delete { .. })
        ));
        sequence(&mut app, "r");
        assert!(
            matches!(app.experience.screens.last(), Some(Screen::Input(Form { kind: FormKind::Reply(target), .. })) if target.id == "t1_comment")
        );
        key(&mut app, KeyCode::Esc);
        sequence(&mut app, "D");
        assert!(
            matches!(app.experience.screens.last(), Some(Screen::Confirm { action: Action::Delete { target }, .. }) if target.id == "t1_comment")
        );
        key(&mut app, KeyCode::Char('l'));
        key(&mut app, KeyCode::Right);
        crate::keys::handle(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL),
        );
        assert!(app.experience.pending.is_none());
        assert!(matches!(
            app.experience.screens.last(),
            Some(Screen::Confirm { .. })
        ));
    }

    #[test]
    fn deleted_authors_should_not_offer_profile_or_chat_actions() {
        let (mut app, _dir, _fixture) = app();
        app.details_menu(Details {
            target: Target {
                id: "t1_comment".into(),
                author: "[deleted]".into(),
                subreddit: "rust".into(),
                permalink: "/comments/post/_/comment".into(),
            },
            raw_text: "body".into(),
            saved: false,
            hidden: false,
            vote: 0,
            editable: true,
            crosspost: None,
        });
        let Some(Screen::Menu { choices, .. }) = app.experience.screens.last() else {
            panic!("menu missing")
        };
        assert!(
            !choices
                .iter()
                .any(|choice| matches!(&choice.command, Command::Read(Query::Profile(_))))
        );
        assert!(!choices.iter().any(
            |choice| matches!(&choice.command,Command::Native(path) if path.contains("[deleted]"))
        ));
    }

    #[test]
    fn when_browsing_to_another_item_its_preview_should_start_at_the_top() {
        for (selected, code, expected) in [
            (0, KeyCode::Char('j'), 1),
            (1, KeyCode::Char('k'), 0),
            (0, KeyCode::Char('G'), 1),
        ] {
            let (mut app, _dir, _fixture) = app();
            app.overlay = Some(Overlay::Experience);
            app.experience.screens.push(Screen::Browse {
                pagination: Default::default(),
                query: Query::Inbox(crate::reddit::experience::Inbox::All),
                page: Page {
                    title: "Inbox".into(),
                    text: String::new(),
                    entries: ["first", "second"]
                        .map(|title| Entry {
                            title: title.into(),
                            body: "short preview".into(),
                            target: None,
                            query: None,
                            post: None,
                        })
                        .into(),
                    after: None,
                    url: "/message/inbox/".into(),
                },
                selected,
                scroll: 30,
            });

            key(&mut app, code);

            assert!(
                matches!(app.experience.screens.last(), Some(Screen::Browse {
                selected, scroll: 0, ..
            }) if *selected == expected)
            );
        }
    }

    #[test]
    fn when_hiding_a_loaded_post_page_lengths_should_remain_valid_for_the_refresh() {
        let (mut app, _dir, _fixture) = app();
        let mut first = post_page().post;
        first.id = "first".into();
        let mut second = first.clone();
        second.id = "second".into();
        app.tabs[0]
            .feed
            .replace_page(None, vec![first, second.clone()], None, Source::default());
        app.after_experience_write(&Action::Hide {
            id: "t3_first".into(),
            hide: true,
        });
        assert_eq!(app.tabs[0].feed.pages[0].len, 1);
        assert!(
            app.tabs[0]
                .feed
                .replace_page(None, vec![second], None, Source::default())
        );
        assert_eq!(app.tabs[0].feed.posts[0].id, "second");
    }

    #[test]
    fn when_an_old_default_thread_arrives_after_sorting_it_should_not_replace_the_sorted_request() {
        let (mut app, _dir, _fixture) = app();
        let page = post_page();
        let post = page.post.clone();
        let link = post.permalink.clone();
        let old = app.show_post(post, &link).unwrap();
        app.sort_comments("new");
        let current = app.post_view.as_ref().unwrap().generation;
        app.on_msg(Msg::Post {
            generation: old,
            result: Ok(Box::new(page)),
        });
        let view = app.post_view.as_ref().unwrap();
        assert_eq!(view.generation, current);
        assert!(view.page.is_none());
        assert_eq!(view.comment_sort, "new");
    }

    #[test]
    fn when_a_sorted_thread_arrives_after_reopening_the_default_view_it_should_be_ignored() {
        let (mut app, _dir, _fixture) = app();
        let page = post_page();
        let post = page.post.clone();
        let link = post.permalink.clone();
        app.show_post(post.clone(), &link);
        app.sort_comments("new");
        let old = app.post_view.as_ref().unwrap().generation;
        let current = app.show_post(post, &link).unwrap();
        app.on_msg(Msg::SortedPost {
            generation: old,
            sort: "new".into(),
            result: Ok(Box::new(page)),
        });
        let view = app.post_view.as_ref().unwrap();
        assert_eq!(view.generation, current);
        assert!(view.page.is_none());
    }

    #[test]
    fn when_an_action_menu_targets_a_collection_item_its_browser_link_and_invalidation_should_use_that_item()
     {
        let (mut app, _dir, _fixture) = app();
        let target = Target {
            id: "t3_changed".into(),
            author: "reader".into(),
            subreddit: "rust".into(),
            permalink: "/r/rust/comments/changed/title/".into(),
        };
        app.details_menu(Details {
            target,
            raw_text: "body".into(),
            saved: false,
            hidden: false,
            vote: 0,
            editable: true,
            crosspost: None,
        });
        assert_eq!(
            app.experience_native_path(),
            "/r/rust/comments/changed/title/"
        );
        assert!(app.experience_invalidations().iter().any(|entry| matches!(entry, crate::worker::Invalidation::Post(link) if link == "/r/rust/comments/changed/title/")));
    }

    #[test]
    fn when_a_custom_feed_is_deleted_it_should_reload_the_custom_feed_index() {
        let (mut app, _dir, _fixture) = app();
        app.experience.screens.push(Screen::Browse {
            pagination: Default::default(),
            query: Query::Multi {
                owner: "reader".into(),
                name: "tools".into(),
            },
            page: Page {
                title: "tools".into(),
                text: String::new(),
                entries: Vec::new(),
                after: None,
                url: "/user/reader/m/tools".into(),
            },
            selected: 0,
            scroll: 0,
        });
        let action = Action::DeleteMulti {
            owner: "reader".into(),
            name: "tools".into(),
        };
        app.experience.screens.push(Screen::Confirm {
            label: "Delete tools".into(),
            action: action.clone(),
            native: "/user/reader/m/tools".into(),
        });
        app.experience.pending = Some((91, Request::Write(action)));
        app.on_experience(91, Ok(Response::Written { url: None }));
        assert!(matches!(
            &app.experience.pending,
            Some((
                _,
                Request::Read {
                    query: Query::Multis,
                    after: None
                }
            ))
        ));
    }

    #[test]
    fn when_a_write_invalidates_an_inactive_feed_its_old_response_should_not_mark_it_loaded() {
        let (mut app, _dir, _fixture) = app();
        let index = app.push_tab(TabKind::Subreddit {
            name: "rust".into(),
            pinned: false,
        });
        let tab = app.tabs[index].id;
        let old = app.tabs[index].feed.generation;
        app.after_experience_write(&Action::Save {
            id: "t3_post".into(),
            save: true,
        });
        app.on_msg(Msg::Feed {
            tab,
            generation: old,
            append: false,
            result: Ok(Listing {
                posts: vec![post_page().post],
                after: None,
            }),
        });
        assert!(!app.tabs[index].feed.loaded);
        assert!(app.tabs[index].feed.posts.is_empty());
    }

    #[test]
    fn when_a_write_restarts_a_group_the_next_refresh_should_receive_a_distinct_generation() {
        let (mut app, _dir, _fixture) = app();
        app.all_subs.next_group_generation = 8;
        app.all_subs.groups.push(Group {
            subreddit: "rust".into(),
            posts: Vec::new(),
            loading: true,
            error: None,
            freshness: None,
            generation: 7,
            needs_refresh: false,
        });
        app.after_experience_write(&Action::Save {
            id: "t3_post".into(),
            save: true,
        });
        assert_eq!(app.all_subs.groups[0].generation, 8);
        assert_eq!(app.all_subs.next_group_generation, 9);
    }

    #[test]
    fn when_an_old_read_finishes_after_a_write_only_the_post_write_refresh_should_replace_content()
    {
        let (mut app, _dir, _fixture) = app();
        let page = post_page();
        let post = page.post.clone();
        let link = post.permalink.clone();
        let old = app.show_post(post, &link).unwrap();
        app.after_experience_write(&Action::Edit {
            target: Target {
                id: "t3_1x4v4sn".into(),
                author: "reader".into(),
                subreddit: "rust".into(),
                permalink: "/comments/1x4v4sn".into(),
            },
            text: "edited body".into(),
        });
        let current = app.post_view.as_ref().unwrap().generation;
        app.on_msg(Msg::Post {
            generation: old,
            result: Ok(Box::new(page.clone())),
        });
        assert_eq!(app.post_view.as_ref().unwrap().generation, current);
        assert!(app.post_view.as_ref().unwrap().page.is_none());
        let mut fresh = page;
        fresh.body = "edited body".into();
        app.on_msg(Msg::Post {
            generation: current,
            result: Ok(Box::new(fresh)),
        });
        assert_eq!(
            app.post_view.as_ref().unwrap().page.as_ref().unwrap().body,
            "edited body"
        );
    }

    #[test]
    fn when_reopening_a_post_after_a_write_pre_write_reads_should_still_be_ignored() {
        let (mut app, _dir, _fixture) = app();
        let page = post_page();
        let post = page.post.clone();
        let link = post.permalink.clone();
        let old = app.show_post(post.clone(), &link).unwrap();
        app.after_experience_write(&Action::Save {
            id: "t3_1x4v4sn".into(),
            save: true,
        });
        let current = app.show_post(post, &link).unwrap();
        app.on_msg(Msg::Post {
            generation: old,
            result: Ok(Box::new(page)),
        });
        assert_eq!(app.post_view.as_ref().unwrap().generation, current);
        assert!(app.post_view.as_ref().unwrap().page.is_none());
    }

    fn reply_form(app: &mut App) {
        app.overlay = Some(Overlay::Experience);
        app.experience.screens.push(Screen::Input(form(
            "Reply",
            FormKind::Reply(Target {
                id: "t1_original".into(),
                author: "author".into(),
                subreddit: "rust".into(),
                permalink: "/r/rust/comments/post/title/original/".into(),
            }),
            vec![field("Reply", "", true)],
            "/r/rust/comments/post/title/original/",
        )));
    }

    #[test]
    fn when_a_composer_receives_paste_and_alt_text_it_should_preserve_the_draft_without_running_keys()
     {
        let (mut app, _dir, _fixture) = app();
        reply_form(&mut app);
        crate::keys::paste(&mut app, "Հայերեն\nq;: $()\u{1b}");
        crate::keys::handle(
            &mut app,
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::ALT),
        );
        assert!(!app.quit);
        assert!(app.experience.pending.is_none());
        assert!(matches!(app.overlay, Some(Overlay::Experience)));
        assert_eq!(
            app.experience.input().unwrap().fields[0].value,
            "Հայերեն\nq;: $()"
        );
    }

    #[test]
    fn when_editing_unicode_in_the_middle_of_a_draft_it_should_preserve_valid_text() {
        let (mut app, _dir, _fixture) = app();
        reply_form(&mut app);
        crate::keys::paste(&mut app, "a界b");
        key(&mut app, KeyCode::Left);
        key(&mut app, KeyCode::Backspace);
        key(&mut app, KeyCode::Char('é'));
        assert_eq!(app.experience.input().unwrap().fields[0].value, "aéb");
    }

    #[test]
    fn when_enter_is_pressed_in_a_reply_it_should_add_a_line_instead_of_publishing() {
        let (mut app, _dir, _fixture) = app();
        reply_form(&mut app);
        key(&mut app, KeyCode::Char('a'));
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.experience.input().unwrap().fields[0].value, "a\n");
        assert!(app.experience.pending.is_none());
    }

    #[test]
    fn when_submitting_a_reply_it_should_capture_the_original_target_and_prevent_duplicate_writes()
    {
        let (mut app, _dir, _fixture) = app();
        reply_form(&mut app);
        crate::keys::paste(&mut app, "reply");
        crate::keys::handle(
            &mut app,
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
        );
        let generation = app.experience.pending.as_ref().unwrap().0;
        crate::keys::handle(
            &mut app,
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
        );
        assert_eq!(app.experience.pending.as_ref().unwrap().0, generation);
        assert!(
            matches!(&app.experience.pending, Some((_, Request::Write(Action::Reply { parent, text }))) if parent == "t1_original" && text == "reply")
        );
    }

    #[test]
    fn a_failed_action_read_should_keep_the_target_and_offer_read_retry_advice() {
        let (mut app, _dir, _fixture) = app();
        let target = Target {
            id: "t1_original".into(),
            author: "author".into(),
            subreddit: "rust".into(),
            permalink: "/comments/post/_/original".into(),
        };
        let query = Query::Inspect(target.clone());
        app.overlay = Some(Overlay::Experience);
        app.experience.screens.push(Screen::Loading(query.clone()));
        app.experience.pending = Some((91, Request::Read { query, after: None }));

        app.on_experience(91, Err("Reddit denied this request (HTTP 403)".into()));

        let error = app.experience.error.as_ref().unwrap();
        assert!(error.contains("HTTP 403"));
        assert!(error.contains("r retries"));
        assert!(!error.contains("submitting"));
        assert!(
            matches!(app.experience.screens.last(), Some(Screen::Loading(Query::Inspect(actual))) if actual == &target)
        );
        assert!(app.experience.pending.is_none());
        key(&mut app, KeyCode::Char('r'));
        assert!(
            matches!(&app.experience.pending, Some((_, Request::Read { query: Query::Inspect(actual), after: None })) if actual == &target)
        );
    }

    #[test]
    fn when_a_write_fails_it_should_keep_the_draft_and_display_the_error() {
        let (mut app, _dir, _fixture) = app();
        reply_form(&mut app);
        crate::keys::paste(&mut app, "unsent draft");
        app.experience.pending = Some((
            91,
            Request::Write(Action::Reply {
                parent: "t1_original".into(),
                text: "unsent draft".into(),
            }),
        ));
        app.on_experience(91, Err("Reddit denied this request (HTTP 403)".into()));
        assert_eq!(
            app.experience.input().unwrap().fields[0].value,
            "unsent draft"
        );
        assert!(app.experience.error.as_ref().unwrap().contains("HTTP 403"));
        assert!(
            app.experience
                .error
                .as_ref()
                .unwrap()
                .contains("before submitting again")
        );
        assert!(!app.experience.writing());
    }

    #[test]
    fn when_a_mutation_is_pending_account_switching_and_escape_should_not_drop_it() {
        let (mut app, _dir, _fixture) = app();
        reply_form(&mut app);
        app.experience.pending = Some((
            91,
            Request::Write(Action::Save {
                id: "t3_post".into(),
                save: true,
            }),
        ));
        app.show_auth();
        key(&mut app, KeyCode::Esc);
        assert!(app.experience.writing());
        assert!(matches!(app.overlay, Some(Overlay::Experience)));
    }

    #[test]
    fn when_a_read_is_abandoned_its_late_response_should_not_replace_the_new_screen() {
        let (mut app, _dir, _fixture) = app();
        app.reddit_menu();
        app.experience.pending = Some((
            91,
            Request::Read {
                query: Query::Multis,
                after: None,
            },
        ));
        app.experience.screens.push(Screen::Loading(Query::Multis));
        key(&mut app, KeyCode::Esc);
        app.on_experience(
            91,
            Ok(Response::Page(Page {
                title: "stale".into(),
                text: String::new(),
                entries: Vec::new(),
                after: None,
                url: "/".into(),
            })),
        );
        assert!(
            matches!(app.experience.screens.last(), Some(Screen::Menu { title, .. }) if title == "Reddit")
        );
    }

    #[test]
    fn pagination_scroll_should_append_retry_and_stop_cursor_cycles_without_losing_position() {
        let (mut app, _dir, _fixture) = app();
        let entries: Vec<_> = (0..8)
            .map(|i| Entry {
                title: format!("community {i}"),
                body: "Description\n".repeat(30),
                target: None,
                query: Some(Query::Community(format!("s{i}"))),
                post: None,
            })
            .collect();
        app.overlay = Some(Overlay::Experience);
        app.experience.screens.push(Screen::Browse {
            pagination: Default::default(),
            query: Query::Communities,
            page: Page {
                title: "Communities".into(),
                text: String::new(),
                entries,
                after: Some("a".into()),
                url: "/subreddits/popular".into(),
            },
            selected: 2,
            scroll: 0,
        });

        key(&mut app, KeyCode::Char('j'));
        let generation = app
            .experience
            .pending
            .as_ref()
            .expect("scrolling should fetch more")
            .0;
        assert!(
            matches!(&app.experience.pending, Some((_, Request::Read { after: Some(cursor), .. })) if cursor == "a")
        );
        key(&mut app, KeyCode::Char('j'));
        key(&mut app, KeyCode::PageDown);
        key(&mut app, KeyCode::Enter);
        assert!(matches!(
            app.experience.screens.last(),
            Some(Screen::Browse {
                selected: 4,
                scroll: 10,
                ..
            })
        ));
        assert_eq!(app.experience.pending.as_ref().unwrap().0, generation);
        app.on_experience(generation, Err("offline".into()));
        key(&mut app, KeyCode::Char('G'));
        assert!(
            app.experience.pending.is_none(),
            "scrolling must not retry failures"
        );
        app.experience.screens.push(Screen::Menu {
            title: "Nested menu".into(),
            choices: Vec::new(),
            selected: 0,
            native: "/".into(),
        });
        key(&mut app, KeyCode::Esc);
        key(&mut app, KeyCode::Char('r'));
        let generation = app
            .experience
            .pending
            .as_ref()
            .expect("r must retry the failed continuation")
            .0;
        assert!(
            matches!(&app.experience.pending, Some((_, Request::Read { after: Some(cursor), .. })) if cursor == "a")
        );
        assert!(
            matches!(app.experience.screens.last(), Some(Screen::Browse { page, selected: 7, .. }) if page.entries.len() == 8)
        );
        let item = Entry {
            title: "new community".into(),
            body: "New description".into(),
            target: None,
            query: Some(Query::Community("new".into())),
            post: None,
        };
        app.on_experience(
            generation,
            Ok(Response::Page(Page {
                title: "Communities".into(),
                text: String::new(),
                entries: vec![item.clone(), item],
                after: Some("b".into()),
                url: "/".into(),
            })),
        );
        assert!(
            matches!(app.experience.screens.last(), Some(Screen::Browse { page, selected: 7, .. }) if page.entries.len() == 9)
        );
        key(&mut app, KeyCode::Char('G'));
        let generation = app
            .experience
            .pending
            .as_ref()
            .expect("scrolling should fetch the next cursor")
            .0;
        app.on_experience(
            generation,
            Ok(Response::Page(Page {
                title: "Communities".into(),
                text: String::new(),
                entries: Vec::new(),
                after: Some("a".into()),
                url: "/".into(),
            })),
        );
        key(&mut app, KeyCode::Char('j'));
        assert!(
            app.experience.pending.is_none(),
            "a longer cursor cycle must stop"
        );
        assert!(
            matches!(app.experience.screens.last(), Some(Screen::Browse { page, selected: 8, .. }) if page.entries.len() == 9 && page.after.is_none())
        );
    }

    #[test]
    fn pagination_scroll_should_advance_empty_collections_and_ignore_abandoned_replies() {
        let (mut app, _dir, _fixture) = app();
        app.overlay = Some(Overlay::Experience);
        app.experience.screens.push(Screen::Browse {
            pagination: Default::default(),
            query: Query::Communities,
            page: Page {
                title: "Communities".into(),
                text: String::new(),
                entries: Vec::new(),
                after: Some("a".into()),
                url: "/subreddits/popular".into(),
            },
            selected: 0,
            scroll: 0,
        });
        key(&mut app, KeyCode::PageDown);
        let generation = app
            .experience
            .pending
            .as_ref()
            .expect("scrolling an empty result should fetch more")
            .0;
        app.on_experience(
            generation,
            Ok(Response::Page(Page {
                title: "Communities".into(),
                text: String::new(),
                entries: Vec::new(),
                after: Some("b".into()),
                url: "/".into(),
            })),
        );
        assert!(
            app.experience.pending.is_none(),
            "one gesture must not spin through empty pages"
        );
        key(&mut app, KeyCode::Char('j'));
        let generation = app
            .experience
            .pending
            .as_ref()
            .expect("the next scroll should continue")
            .0;
        assert!(
            matches!(&app.experience.pending, Some((_, Request::Read { after: Some(cursor), .. })) if cursor == "b")
        );
        key(&mut app, KeyCode::Esc);
        app.on_experience(
            generation,
            Ok(Response::Page(Page {
                title: "Abandoned result".into(),
                text: String::new(),
                entries: Vec::new(),
                after: None,
                url: "/".into(),
            })),
        );
        assert!(app.overlay.is_none());
        assert!(app.experience.screens.is_empty());
    }

    #[test]
    fn when_pagination_repeats_items_and_its_cursor_it_should_deduplicate_and_stop() {
        let (mut app, _dir, _fixture) = app();
        let entry = Entry {
            title: "r/rust".into(),
            body: String::new(),
            target: None,
            query: Some(Query::Community("rust".into())),
            post: None,
        };
        app.overlay = Some(Overlay::Experience);
        app.experience.screens.push(Screen::Browse {
            pagination: Default::default(),
            query: Query::Communities,
            page: Page {
                title: "Communities".into(),
                text: String::new(),
                entries: vec![entry.clone()],
                after: Some("loop".into()),
                url: "/".into(),
            },
            selected: 0,
            scroll: 0,
        });
        app.experience.pending = Some((
            91,
            Request::Read {
                query: Query::Communities,
                after: Some("loop".into()),
            },
        ));
        app.on_experience(
            91,
            Ok(Response::Page(Page {
                title: "Communities".into(),
                text: String::new(),
                entries: vec![entry],
                after: Some("loop".into()),
                url: "/".into(),
            })),
        );
        let Some(Screen::Browse { page, .. }) = app.experience.screens.last() else {
            panic!("Expected browser");
        };
        assert_eq!(page.entries.len(), 1);
        assert!(page.after.is_none());
    }

    fn post_page() -> PostPage {
        crate::reddit::parse::parse_post_page(include_str!("../../tests/fixtures/post.json"))
            .unwrap()
    }

    #[test]
    fn when_reading_links_it_should_follow_comment_selection_and_keep_spoilers_hidden() {
        let (mut app, _dir, _fixture) = app();
        let mut source: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/post.json")).unwrap();
        source[0]["data"]["children"][0]["data"]["selftext_html"] =
            json!("<p><a href='https://example.org/post?x=1&amp;y=2'>post guide</a></p>");
        source[1]["data"]["children"][0]["data"]["body_html"] = json!(
            "<p><a href='/r/rust/wiki/index'>comment guide</a> <span class='md-spoiler-text'><a href='https://example.org/secret'>secret</a></span></p>"
        );
        let page = crate::reddit::parse::parse_post_page(&source.to_string()).unwrap();
        app.overlay = Some(Overlay::Experience);
        app.experience.screens.push(Screen::Browse {
            pagination: Default::default(),
            query: Query::Inbox(Inbox::All),
            page: Page {
                title: "Saved post".into(),
                text: String::new(),
                entries: vec![Entry {
                    title: page.post.title.clone(),
                    body: String::new(),
                    target: Some(Target::from_post(&page.post)),
                    query: None,
                    post: Some(page.post.clone()),
                }],
                after: None,
                url: "/saved".into(),
            },
            selected: 0,
            scroll: 0,
        });
        key(&mut app, KeyCode::Enter);
        assert!(app.overlay.is_none());
        let generation = app.post_view.as_ref().unwrap().generation;
        app.on_msg(Msg::Post {
            generation,
            result: Ok(Box::new(page)),
        });
        app.focus = Focus::Post;
        app.post_view.as_mut().unwrap().free_scroll = true;

        sequence(&mut app, " ol");
        let Some(Screen::Menu { choices, .. }) = app.experience.screens.last() else {
            panic!("missing post links")
        };
        assert_eq!(choices.len(), 1);
        assert_eq!(choices[0].label, "post guide");
        key(&mut app, KeyCode::Enter);
        let Some(Screen::Menu { choices, .. }) = app.experience.screens.last() else {
            panic!("missing link actions")
        };
        assert!(
            matches!(&choices[0].command, Command::OpenUrl(url) if url == "https://example.org/post?x=1&y=2")
        );
        assert!(
            matches!(&choices[1].command, Command::CopyUrl(url) if url == "https://example.org/post?x=1&y=2")
        );
        app.config.commands.browser = vec!["/redtab-test-missing-browser".into()];
        key(&mut app, KeyCode::Enter);
        assert!(
            app.status
                .as_ref()
                .is_some_and(|status| status.text.contains("Could not open link"))
        );
        key(&mut app, KeyCode::Esc);
        key(&mut app, KeyCode::Esc);

        let view = app.post_view.as_mut().unwrap();
        view.free_scroll = false;
        view.cursor = 0;
        sequence(&mut app, " ol");
        let Some(Screen::Menu { choices, .. }) = app.experience.screens.last() else {
            panic!("missing comment links")
        };
        assert_eq!(choices.len(), 1);
        assert_eq!(choices[0].label, "comment guide");
        assert!(
            matches!(&choices[0].command, Command::BodyLink(link) if link.url == "https://www.reddit.com/r/rust/wiki/index")
        );
        assert!(app.experience.pending.is_none());
        key(&mut app, KeyCode::Esc);
        app.post_view.as_mut().unwrap().reveal_spoilers = true;
        sequence(&mut app, " ol");
        let Some(Screen::Menu { choices, .. }) = app.experience.screens.last() else {
            panic!("missing revealed links")
        };
        assert_eq!(choices.len(), 2);
        assert_eq!(choices[1].label, "secret");
        key(&mut app, KeyCode::Esc);
        let view = app.post_view.as_mut().unwrap();
        view.reveal_spoilers = false;
        view.page.as_mut().unwrap().post.spoiler = true;
        sequence(&mut app, " ol");
        assert!(app.overlay.is_none());
    }

    #[test]
    fn when_a_reading_dialog_has_links_it_should_use_the_visible_text_and_restore_the_dialog() {
        let (mut app, _dir, _fixture) = app();
        let mut source: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/post.json")).unwrap();
        source[0]["data"]["children"][0]["data"]["selftext_html"] =
            json!("<p><a href='https://example.org/about'>about guide</a></p>");
        source[1]["data"]["children"][0]["data"]["body_html"] =
            json!("<p><a href='https://example.org/comment'>comment guide</a></p>");
        let page = crate::reddit::parse::parse_post_page(&source.to_string()).unwrap();
        app.overlay = Some(Overlay::Experience);
        app.experience.screens.push(Screen::Browse {
            pagination: Default::default(),
            query: Query::Community("rust".into()),
            page: Page {
                title: "About".into(),
                text: page.body,
                entries: vec![Entry {
                    title: "Comment".into(),
                    body: page.comments[0].replies[0].body.clone(),
                    query: None,
                    target: None,
                    post: None,
                }],
                after: None,
                url: "/r/rust/about".into(),
            },
            selected: 0,
            scroll: 3,
        });

        sequence(&mut app, " ol");
        let Some(Screen::Menu { choices, .. }) = app.experience.screens.last() else {
            panic!("missing dialog links")
        };
        assert_eq!(choices[0].label, "about guide");
        key(&mut app, KeyCode::Esc);
        let Some(Screen::Browse { query, scroll, .. }) = app.experience.screens.last_mut() else {
            panic!("lost dialog")
        };
        assert_eq!(*scroll, 3);
        *query = Query::Thread("/comments/post".into());
        sequence(&mut app, " ol");
        let Some(Screen::Menu { choices, .. }) = app.experience.screens.last() else {
            panic!("missing selected entry links")
        };
        assert_eq!(choices[0].label, "comment guide");
        assert!(app.experience.pending.is_none());
    }

    #[test]
    fn when_a_comment_is_selected_actions_should_target_the_comment_instead_of_its_post() {
        let (mut app, _dir, _fixture) = app();
        let page = post_page();
        let post = page.post.clone();
        let link = post.permalink.clone();
        let generation = app.show_post(post, &link).unwrap();
        app.focus = Focus::Post;
        app.on_msg(Msg::Post {
            generation,
            result: Ok(Box::new(page)),
        });
        let view = app.post_view.as_mut().unwrap();
        view.cursor = 0;
        view.free_scroll = false;
        let actual = app.comment_target().unwrap();
        assert!(actual.id.starts_with("t1_"));
        assert!(actual.permalink.starts_with("/comments/1x4v4sn/_/"));
        app.show_actions();
        assert!(matches!(
            &app.experience.pending,
            Some((_, Request::Read { query: Query::Inspect(target), after: None })) if target == &actual
        ));
        app.post_view.as_mut().unwrap().free_scroll = true;
        assert!(app.comment_target().is_none());
    }

    #[test]
    fn when_galleries_wrap_in_both_directions_the_preview_should_follow_the_selected_image() {
        let (mut app, _dir, _fixture) = app();
        let mut page = post_page();
        page.post.media = Media::Gallery {
            images: vec![
                "https://i.redd.it/a.png".into(),
                "https://i.redd.it/b.png".into(),
            ],
            previews: vec![
                Some("https://preview.redd.it/a.png?width=1080".into()),
                Some("https://preview.redd.it/b.png?width=1080".into()),
            ],
            thumb: None,
        };
        let post = page.post.clone();
        let link = post.permalink.clone();
        let generation = app.show_post(post, &link).unwrap();
        app.on_msg(Msg::Post {
            generation,
            result: Ok(Box::new(page)),
        });
        app.gallery_step(false);
        assert_eq!(
            app.post_view.as_ref().unwrap().preview_image().as_deref(),
            Some("https://preview.redd.it/b.png?width=1080")
        );
        app.gallery_step(true);
        assert_eq!(
            app.post_view.as_ref().unwrap().preview_image().as_deref(),
            Some("https://preview.redd.it/a.png?width=1080")
        );
    }

    #[test]
    fn when_comment_sort_changes_existing_comments_should_be_replaced_by_an_uncached_sorted_request()
     {
        let (mut app, _dir, _fixture) = app();
        let page = post_page();
        let post = page.post.clone();
        let link = post.permalink.clone();
        let generation = app.show_post(post, &link).unwrap();
        app.on_msg(Msg::Post {
            generation,
            result: Ok(Box::new(page)),
        });
        app.sort_comments("new");
        let view = app.post_view.as_ref().unwrap();
        assert_eq!(view.comment_sort, "new");
        assert!(view.page.is_none());
        assert!(view.loading);
        assert_ne!(view.generation, generation);
    }
    #[test]
    fn ui_audit_required_field_validation_should_name_and_focus_the_field() {
        let (mut app, _dir, _fixture) = app();
        let mut input = app.post_form(true);
        input.fields[0].value = "rust".into();
        input.fields[2].value = "https://example.com".into();
        app.experience.screens.push(Screen::Input(input));
        app.overlay = Some(Overlay::Experience);

        app.experience_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));

        assert_eq!(app.experience.input().unwrap().active, 1);
        assert_eq!(app.experience.error.as_deref(), Some("Title is required"));
        assert!(app.experience.pending.is_none());
    }

    #[test]
    fn ui_audit_editor_navigation_and_deletion_should_preserve_grapheme_boundaries() {
        let (mut app, _dir, _fixture) = app();
        app.experience.screens.push(Screen::Input(form(
            "Search people",
            FormKind::Search(SearchKind::People),
            vec![field("Query", "a👩‍💻b", false)],
            "/search",
        )));
        app.overlay = Some(Overlay::Experience);

        key(&mut app, KeyCode::Left);
        key(&mut app, KeyCode::Left);
        key(&mut app, KeyCode::Right);
        key(&mut app, KeyCode::Delete);
        key(&mut app, KeyCode::Backspace);

        assert_eq!(app.experience.input().unwrap().fields[0].value, "a");
    }

    #[test]
    fn ui_audit_read_errors_should_offer_the_reload_key() {
        let (mut app, _dir, _fixture) = app();
        app.experience.pending = Some((
            71,
            Request::Read {
                query: Query::Search {
                    text: "rust".into(),
                    kind: SearchKind::Posts,
                    sort: SearchSort::Relevance,
                    time: TimeRange::All,
                },
                after: Some("next".into()),
            },
        ));

        app.on_experience(71, Err("Page failed".into()));

        assert!(
            app.experience
                .error
                .as_deref()
                .unwrap()
                .contains("r retries")
        );
    }
    #[test]
    fn settings_draft_reload_should_keep_reply_edit_and_submit_fields_for_both_change_paths() {
        for external in [false, true] {
            for kind in 0..3 {
                let (mut app, directory, _fixture) = app();
                let path = directory.path().join("config.toml");
                std::fs::write(&path, "[media]\nimage_preview=false\n").unwrap();
                app.settings = super::super::settings::Settings::new(
                    crate::config::store::Store::open(Some(&path)).unwrap(),
                );
                let target = Target {
                    id: "t1_original".into(),
                    author: "reader".into(),
                    subreddit: "rust".into(),
                    permalink: "/comments/post/_/original".into(),
                };
                let mut draft = match kind {
                    0 => form(
                        "Reply",
                        FormKind::Reply(target),
                        vec![field("Reply", "", true)],
                        "/comments/post",
                    ),
                    1 => form(
                        "Edit",
                        FormKind::Edit(target),
                        vec![field("Body", "", true)],
                        "/comments/post",
                    ),
                    _ => app.post_form(false),
                };
                draft.active = draft.fields.len() - 1;
                draft.fields[draft.active].value = "unsent café\nsecond line".into();
                draft.fields[draft.active].cursor = 5;
                app.experience.screens = vec![
                    Screen::Browse {
                        pagination: Default::default(),
                        query: Query::Communities,
                        page: Page {
                            title: "old collection".into(),
                            text: String::new(),
                            entries: vec![],
                            after: None,
                            url: "/".into(),
                        },
                        selected: 0,
                        scroll: 0,
                    },
                    Screen::Input(draft.clone()),
                ];
                app.overlay = Some(Overlay::Experience);
                if !external {
                    app.show_settings();
                }
                crate::app::tests::confirm_preferences(&mut app, true);
                if !external {
                    crate::keys::handle(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
                }
                assert!(app.config.show_nsfw);
                assert!(
                    matches!(app.overlay, Some(Overlay::Experience)),
                    "composer lost: external={external} kind={kind}"
                );
                let current = app
                    .experience
                    .input()
                    .expect("draft must survive policy reload");
                assert_eq!(current.active, draft.active);
                for (actual, expected) in current.fields.iter().zip(&draft.fields) {
                    assert_eq!(actual.value, expected.value);
                    assert_eq!(actual.cursor, expected.cursor);
                }
                assert_eq!(
                    app.experience.screens.len(),
                    1,
                    "policy-dependent browse data must be invalidated"
                );
            }
        }
    }
    #[test]
    fn settings_draft_errors_should_follow_retained_inputs_not_removed_read_screens() {
        for write_error in [false, true] {
            let (mut app, _directory, _fixture) = app();
            reply_form(&mut app);
            app.experience_paste("retained draft");
            let generation = if write_error {
                app.experience.pending = Some((
                    95,
                    Request::Write(Action::Save {
                        id: "t3_post".into(),
                        save: true,
                    }),
                ));
                95
            } else {
                app.read_experience(Query::Communities, None);
                app.experience.pending.as_ref().unwrap().0
            };
            app.on_experience(generation, Err("fixture error".into()));
            assert!(app.experience.error.is_some());
            crate::app::tests::confirm_preferences(&mut app, true);
            assert!(app.experience.input().is_some());
            assert_eq!(
                app.experience.error.is_some(),
                write_error,
                "read-screen feedback must not migrate onto a text field"
            );
        }
    }
    #[test]
    fn settings_find_should_follow_its_backing_post_for_nsfw_and_filter_changes() {
        for external in [false, true] {
            for (adult, filter, keep) in [
                (true, false, false),
                (false, false, true),
                (false, true, false),
            ] {
                let (mut app, directory, _fixture) = app();
                crate::app::tests::confirm_preferences(&mut app, true);
                let mut page = post_page();
                page.post.nsfw = adult;
                page.post.title = "needle".into();
                let generation = app
                    .show_post(page.post.clone(), &page.post.permalink)
                    .unwrap();
                app.on_msg(Msg::Post {
                    generation,
                    result: Ok(Box::new(page)),
                });
                let path = directory.path().join("config.toml");
                std::fs::write(&path, "[media]\nimage_preview=false\n").unwrap();
                app.settings = super::super::settings::Settings::new(
                    crate::config::store::Store::open(Some(&path)).unwrap(),
                );
                app.experience.screens = vec![Screen::Input(form(
                    "Find in loaded comments",
                    FormKind::Find,
                    vec![field("Query", "needle", false)],
                    "/comments/post",
                ))];
                app.overlay = Some(Overlay::Experience);
                if filter {
                    if external {
                        let username = app.reddit.account().unwrap().name;
                        std::fs::write(&path, format!("[media]\nimage_preview=false\n[accounts.{username}.filters]\nkeywords=[\"needle\"]\n")).unwrap();
                        app.settings.store.select_account(Some(&username));
                        app.poll_settings(Instant::now() + Duration::from_secs(2));
                    } else {
                        app.change_setting(
                            "filters.keywords",
                            toml::Value::Array(vec!["needle".into()]),
                        );
                    }
                } else {
                    crate::app::tests::confirm_preferences(&mut app, false);
                }
                assert_eq!(app.post_view.is_some(), keep);
                assert_eq!(
                    matches!(app.overlay, Some(Overlay::Experience)),
                    keep,
                    "Find must close with its post: external={external} adult={adult} filter={filter}"
                );
                if keep {
                    key(&mut app, KeyCode::Enter);
                    assert!(matches!(
                        app.experience.screens.last(),
                        Some(Screen::Browse { .. })
                    ));
                } else {
                    assert!(app.experience.screens.is_empty());
                }
            }
        }
    }
    #[test]
    fn when_opening_the_current_community_feed_from_a_post_it_should_reveal_the_feed() {
        let (mut app, _dir, _fixture) = app();
        app.open_subreddit("s");
        let expected_tab = app.active_tab().id;
        let mut post = post_page().post;
        post.subreddit = "s".into();
        let mut other = post.clone();
        other.id = "other".into();
        let feed = &mut app.tabs[app.active].feed;
        feed.posts = vec![other, post.clone()];
        feed.selected = 1;
        feed.loaded = true;
        app.resize(80);
        app.show_post(post.clone(), &post.permalink);
        app.focus = Focus::Post;

        sequence(&mut app, " c");
        key(&mut app, KeyCode::Enter);

        assert!(app.overlay.is_none());
        assert!(app.post_view.is_none(), "the post still hides the feed");
        assert_eq!(app.focus, Focus::List);
        assert_eq!(app.active_tab().id, expected_tab);
        assert_eq!(app.tabs.len(), 4);
        assert_eq!(app.active_tab().feed.selected, 1);
        assert!(app.experience.screens.is_empty());
    }

    #[test]
    fn when_opening_a_community_feed_from_the_sidebar_it_should_focus_the_feed() {
        let (mut app, _dir, _fixture) = app();
        app.open_subreddit("s");
        app.show_sidebar = false;
        app.focus_sidebar();

        sequence(&mut app, " c");
        key(&mut app, KeyCode::Enter);

        assert_eq!(app.focus, Focus::List);
        assert!(!app.sidebar_peek);
        assert!(!app.show_sidebar);
        assert!(app.overlay.is_none());
    }

    #[test]
    fn when_opening_a_posts_existing_community_feed_it_should_reuse_its_tab() {
        let (mut app, _dir, _fixture) = app();
        app.open_subreddit("s");
        let expected_tab = app.active_tab().id;
        app.switch_tab(0);
        let mut post = post_page().post;
        post.subreddit = "s".into();
        app.show_post(post.clone(), &post.permalink);
        app.focus = Focus::Post;

        sequence(&mut app, " c");
        key(&mut app, KeyCode::Enter);

        assert_eq!(app.active_tab().id, expected_tab);
        assert_eq!(app.tabs.len(), 4);
        assert!(app.post_view.is_none());
        assert_eq!(app.focus, Focus::List);
        assert!(app.overlay.is_none());
    }

    #[test]
    fn when_opening_a_posts_new_community_feed_it_should_open_its_tab() {
        let (mut app, _dir, _fixture) = app();
        let mut post = post_page().post;
        post.subreddit = "new".into();
        app.show_post(post.clone(), &post.permalink);
        app.focus = Focus::Post;

        sequence(&mut app, " c");
        key(&mut app, KeyCode::Enter);

        assert_eq!(app.active_tab().subreddit(), Some("new"));
        assert_eq!(app.tabs.len(), 4);
        assert!(app.post_view.is_none());
        assert_eq!(app.focus, Focus::List);
        assert!(app.overlay.is_none());
    }

    #[test]
    fn when_opening_the_current_user_feed_it_should_reveal_and_focus_the_feed() {
        let (mut app, _dir, _fixture) = app();
        app.open_user("writer");
        let expected_tab = app.active_tab().id;
        let post = post_page().post;
        app.show_post(post.clone(), &post.permalink);
        app.focus = Focus::Sidebar;

        sequence(&mut app, " c");
        key(&mut app, KeyCode::Char('j'));
        key(&mut app, KeyCode::Enter);

        assert_eq!(app.active_tab().id, expected_tab);
        assert_eq!(app.tabs.len(), 4);
        assert!(app.post_view.is_none());
        assert_eq!(app.focus, Focus::List);
        assert!(app.overlay.is_none());
        assert!(app.experience.screens.is_empty());
    }

    #[test]
    fn community_menu_should_offer_a_local_read_action() {
        let (mut app, _dir, _fixture) = app();
        app.open_subreddit("s");
        app.community_menu();
        let Some(Screen::Menu { choices, .. }) = app.experience.screens.last() else {
            panic!("menu missing")
        };
        assert!(
            choices
                .iter()
                .any(|choice| choice.label == "Mark community read")
        );
    }
    #[test]
    fn community_read_menu_action_should_keep_its_captured_target_and_save_locally() {
        let (mut app, dir, _fixture) = app();
        app.open_subreddit("s");
        let mut post = post_page().post;
        post.subreddit = "s".into();
        app.state.observe(
            Some("s"),
            std::slice::from_ref(&post),
            Freshness {
                fetched_at: now(),
                source: Source::default(),
            },
            now(),
        );
        app.community_menu();
        let Some(Screen::Menu { choices, .. }) = app.experience.screens.last() else {
            panic!("menu missing")
        };
        let command = choices
            .iter()
            .find(|choice| choice.label == "Mark community read")
            .unwrap()
            .command
            .clone();
        app.active = 0;

        app.execute_experience(command);

        assert!(app.state.is_read(&post.id));
        assert_eq!(
            State::load(&dir.path().join("state.json"))
                .0
                .unread("s", &app.config),
            Some(0)
        );
        assert!(app.overlay.is_none());
        assert!(app.experience.pending.is_none());
    }

    #[test]
    fn user_feed_tools_should_put_unfollow_first_and_capture_the_person() {
        let (mut app, _dir, _fixture) = app();
        app.on_msg(Msg::Subscriptions {
            generation: 0,
            result: Ok(vec![Subscription::User {
                name: "blackvault".into(),
                icon: None,
            }]),
        });
        app.open_user("blackvault");
        app.tabs[app.active].feed.posts.push(post_page().post);
        app.focus = Focus::List;

        app.community_menu();

        let Some(Screen::Menu { title, choices, .. }) = app.experience.screens.last() else {
            panic!("profile menu missing")
        };
        assert_eq!(title, "u/blackvault");
        assert_eq!(choices[0].label, "Unfollow");
        key(&mut app, KeyCode::Enter);
        assert!(
            matches!(&app.overlay, Some(Overlay::Membership { target: MembershipTarget::User(name), join: false }) if name == "blackvault")
        );
        assert!(
            app.membership.is_none(),
            "confirmation must not write before Enter"
        );
        key(&mut app, KeyCode::Esc);
        assert!(app.membership.is_none());
    }

    #[test]
    fn fresh_profile_post_inspection_should_keep_profile_actions_and_native_identity() {
        for (subreddit, subtype, permalink, profile) in [
            (
                "u_blackvault",
                "user",
                "/r/u_blackvault/comments/post/title/",
                true,
            ),
            (
                "u_ordinary",
                "public",
                "/r/u_ordinary/comments/post/title/",
                false,
            ),
        ] {
            let fixture = Fixture::new(move |_| {
                Reply::json(json!([
                    {"kind":"Listing","data":{"children":[{"kind":"t3","data":{
                        "id":"post", "name":"t3_post", "title":"Submission", "author":"reader",
                        "subreddit":subreddit, "subreddit_type":subtype, "permalink":permalink,
                        "selftext":"Markdown source", "saved":true, "likes":true
                    }}]}},
                    {"kind":"Listing","data":{"children":[]}}
                ]))
            });
            let canonical = if profile {
                "/user/blackvault/comments/post/title/"
            } else {
                permalink
            };
            let Response::Details(details) = fixture
                .reddit()
                .experience(&Request::Read {
                    query: Query::Inspect(Target {
                        id: "t3_post".into(),
                        author: "reader".into(),
                        subreddit: subreddit.into(),
                        permalink: canonical.into(),
                    }),
                    after: None,
                })
                .unwrap()
            else {
                panic!("details missing")
            };
            assert_eq!(details.target.permalink, canonical);
            assert!(details.saved);
            assert_eq!(details.vote, 1);
            assert_eq!(details.raw_text, "Markdown source");
            let (mut app, _dir, _fixture) = app();
            app.details_menu(details);
            let Some(Screen::Menu {
                choices, native, ..
            }) = app.experience.screens.last()
            else {
                panic!("menu missing")
            };
            assert_eq!(native, canonical);
            assert_eq!(
                choices
                    .iter()
                    .any(|choice| choice.label == "Profile information"),
                profile
            );
            assert_eq!(
                choices
                    .iter()
                    .any(|choice| choice.label == "Community information"),
                !profile
            );
            assert_eq!(
                choices
                    .iter()
                    .any(|choice| choice.label.starts_with("Profile controls ·")),
                profile
            );
        }
    }

    #[test]
    fn collection_source_tools_should_use_the_selected_post_before_sidebar_or_query_context() {
        for focus in [Focus::Sidebar, Focus::List] {
            for (subreddit, permalink, title) in [
                ("rust", "/r/rust/comments/post/title/", "r/rust"),
                (
                    "u_blackvault",
                    "/user/blackvault/comments/post/title/",
                    "u/blackvault",
                ),
                (
                    "u_ordinary",
                    "/r/u_ordinary/comments/post/title/",
                    "r/u_ordinary",
                ),
            ] {
                let (mut app, _dir, _fixture) = app();
                app.open_user("collection-owner");
                app.focus = focus;
                let mut post = post_page().post;
                post.subreddit = subreddit.into();
                post.permalink = permalink.into();
                app.experience.screens.push(Screen::Browse {
                    pagination: Default::default(),
                    query: Query::User {
                        name: "collection-owner".into(),
                        feed: UserFeed::Posts,
                    },
                    page: Page {
                        title: "Posts".into(),
                        url: "/user/collection-owner/submitted/".into(),
                        entries: vec![Entry {
                            title: "Selected post".into(),
                            body: String::new(),
                            target: Some(Target::from_post(&post)),
                            post: Some(post),
                            query: None,
                        }],
                        after: None,
                        text: String::new(),
                    },
                    selected: 0,
                    scroll: 0,
                });
                app.overlay = Some(Overlay::Experience);

                sequence(&mut app, " c");

                let Some(Screen::Menu {
                    title: actual,
                    choices,
                    ..
                }) = app.experience.screens.last()
                else {
                    panic!("menu missing")
                };
                assert_eq!(actual, title);
                assert_eq!(
                    choices
                        .iter()
                        .any(|choice| choice.label == "Mark community read"),
                    title.starts_with("r/")
                );
            }
        }
    }

    #[test]
    fn community_menu_should_target_an_empty_selected_island() {
        let (mut app, _dir, _fixture) = app();
        app.on_msg(Msg::Subscriptions {
            generation: 0,
            result: Ok((vec!["empty".into()])
                .into_iter()
                .map(|name| crate::model::Subscription::Community {
                    name,
                    icon: crate::model::SubscriptionIcon::Unknown,
                })
                .collect()),
        });
        app.active = ALL_SUBS_TAB;

        app.community_menu();

        let Some(Screen::Menu { title, native, .. }) = app.experience.screens.last() else {
            panic!("menu missing")
        };
        assert_eq!(title, "r/empty");
        assert_eq!(native, "/r/empty");
    }

    #[test]
    fn community_menu_should_target_the_selected_community_discovery_result() {
        let (mut app, _dir, _fixture) = app();
        app.open_subreddit("underneath");
        app.experience.screens.push(Screen::Browse {
            pagination: Default::default(),
            query: Query::Communities,
            page: Page {
                url: "/subreddits".into(),
                title: "Communities".into(),
                entries: vec![Entry {
                    title: "r/picked".into(),
                    body: String::new(),
                    target: None,
                    query: Some(Query::Community("picked".into())),
                    post: None,
                }],
                after: None,
                text: String::new(),
            },
            selected: 0,
            scroll: 0,
        });
        app.overlay = Some(Overlay::Experience);

        sequence(&mut app, " c");

        let Some(Screen::Menu { title, .. }) = app.experience.screens.last() else {
            panic!("menu missing")
        };
        assert_eq!(title, "r/picked");
    }
    #[test]
    fn community_menu_on_a_moderator_list_should_keep_its_parent_community() {
        let (mut app, _dir, _fixture) = app();
        app.open_subreddit("underneath");
        app.experience.screens.push(Screen::Browse {
            pagination: Default::default(),
            query: Query::Moderators("moderated".into()),
            page: Page {
                title: "Moderators".into(),
                text: String::new(),
                after: None,
                url: "/r/moderated/about/moderators".into(),
                entries: vec![Entry {
                    title: "u/mod".into(),
                    body: String::new(),
                    target: None,
                    query: Some(Query::Profile("mod".into())),
                    post: None,
                }],
            },
            selected: 0,
            scroll: 0,
        });
        app.overlay = Some(Overlay::Experience);

        sequence(&mut app, " c");

        let Some(Screen::Menu { title, .. }) = app.experience.screens.last() else {
            panic!("menu missing")
        };
        assert_eq!(title, "r/moderated");
    }
}
