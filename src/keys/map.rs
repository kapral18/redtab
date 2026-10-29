use super::*;
use crate::app::experience::Screen;
use std::time::{Duration, Instant};

pub(crate) const HINT_DELAY: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Context {
    List,
    Sidebar,
    Post,
    Menu,
    Browse,
    Settings,
    Managed,
    Accounts,
    Sort,
    Confirm,
    Input,
    Pending,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Down,
    Up,
    Left,
    Right,
    First,
    Last,
    PageDown,
    PageUp,
    HalfDown,
    HalfUp,
    LineDown,
    LineUp,
    NextTab,
    PreviousTab,
    FirstTab,
    LastTab,
    CloseTab,
    NextCommunity,
    PreviousCommunity,
    NextPost,
    PreviousPost,
    NextThread,
    PreviousThread,
    NextImage,
    PreviousImage,
    Fold,
    FocusLeft,
    FocusRight,
    Home,
    Bookmarks,
    AllSubs,
    Popular,
    All,
    Jump,
    CommunityFeed,
    Profile,
    Actions,
    Community,
    Reddit,
    Membership,
    Bookmark,
    Search,
    SearchPosts,
    SearchCommunities,
    SearchPeople,
    Explore,
    Sort,
    Time,
    Reload,
    OpenUrl,
    OpenLinks,
    Browser,
    SourceBrowser,
    CopyLink,
    Media,
    Pause,
    Mute,
    SeekBack,
    SeekForward,
    Images,
    ListView,
    Sidebar,
    Spoilers,
    Settings,
    Accounts,
    Close,
}

pub struct Binding {
    pub keys: &'static str,
    pub action: Action,
    pub description: &'static str,
}

macro_rules! binding {
    ($keys:literal, $action:ident, $description:literal) => {
        Binding {
            keys: $keys,
            action: Action::$action,
            description: $description,
        }
    };
}

pub const BINDINGS: &[Binding] = &[
    binding!("j", Down, "Move down"),
    binding!("k", Up, "Move up"),
    binding!("h", Left, "Go back / focus left"),
    binding!("l", Right, "Open / focus right"),
    binding!("gg", First, "First item / top"),
    binding!("G", Last, "Last item / bottom"),
    binding!("\u{6}", PageDown, "Page down"),
    binding!("\u{2}", PageUp, "Page up"),
    binding!("\u{4}", HalfDown, "Half page down"),
    binding!("\u{15}", HalfUp, "Half page up"),
    binding!("\u{5}", LineDown, "Scroll down one line"),
    binding!("\u{19}", LineUp, "Scroll up one line"),
    binding!("gt", NextTab, "Next tab"),
    binding!("gT", PreviousTab, "Previous tab"),
    binding!("\u{17}h", FocusLeft, "Focus the pane to the left"),
    binding!("\u{17}l", FocusRight, "Focus the pane to the right"),
    binding!("]s", NextCommunity, "Next subreddit feed"),
    binding!("[s", PreviousCommunity, "Previous subreddit feed"),
    binding!("]p", NextPost, "Next post; keep details open"),
    binding!("[p", PreviousPost, "Previous post; keep details open"),
    binding!("]]", NextThread, "Next root comment thread"),
    binding!("[[", PreviousThread, "Previous root comment thread"),
    binding!("]i", NextImage, "Next gallery image"),
    binding!("[i", PreviousImage, "Previous gallery image"),
    binding!("za", Fold, "Select / fold / unfold comment thread"),
    binding!("/", Search, "Search the current reading context"),
    binding!("r", Reload, "Reload / retry"),
    binding!("yy", CopyLink, "Yank the Reddit permalink"),
    binding!("q", Close, "Close the current view / quit"),
    binding!(" aa", Actions, "Reddit content actions"),
    binding!(" c", Community, "Community / profile tools"),
    binding!(" r", Reddit, "Reddit collections and tools"),
    binding!(" bb", Jump, "Choose / open a buffer"),
    binding!(" bf", FirstTab, "First buffer"),
    binding!(" bl", LastTab, "Last buffer"),
    binding!(" bd", CloseTab, "Delete the current opened buffer"),
    binding!(" gh", Home, "Go Home"),
    binding!(" gb", Bookmarks, "Go to local bookmarks"),
    binding!(" ga", AllSubs, "Go to all subscriptions"),
    binding!(" gp", Popular, "Go to Popular"),
    binding!(" gr", All, "Go to all Reddit"),
    binding!(" gc", CommunityFeed, "Go to the current source feed"),
    binding!(" gu", Profile, "Go to a user profile"),
    binding!(" fs", Sort, "Choose feed / comment sort"),
    binding!(" ft", Time, "Cycle feed time range"),
    binding!(" sp", SearchPosts, "Search posts across Reddit"),
    binding!(" sc", SearchCommunities, "Search communities"),
    binding!(" su", SearchPeople, "Search people"),
    binding!(" se", Explore, "Search and explore Reddit"),
    binding!(" ou", OpenUrl, "Open a Reddit URL in redtab"),
    binding!(" ol", OpenLinks, "Choose a body link to open or copy"),
    binding!(" or", Browser, "Open the current item on Reddit"),
    binding!(
        " oc",
        SourceBrowser,
        "Open the community / profile in browser"
    ),
    binding!(" om", Media, "Open media externally"),
    binding!(" mp", Pause, "Play / pause / retry video"),
    binding!(" mm", Mute, "Mute / unmute video"),
    binding!(" mh", SeekBack, "Seek backward five seconds"),
    binding!(" ml", SeekForward, "Seek forward five seconds"),
    binding!(" vi", Images, "Toggle image and video previews"),
    binding!(" vc", ListView, "Toggle Card / Compact view"),
    binding!(" vb", Sidebar, "Toggle the sidebar"),
    binding!(" vs", Spoilers, "Reveal / hide spoilers"),
    binding!(" ps", Settings, "Settings and preferences"),
    binding!(" pa", Accounts, "Accounts: switch / add / reconnect"),
    binding!(" ab", Bookmark, "Bookmark the current post locally"),
    binding!(" am", Membership, "Join / leave or follow / unfollow"),
];

#[derive(Clone, PartialEq, Eq)]
struct Scope {
    context: Context,
    tab: u64,
    post: Option<u64>,
    screens: usize,
    account: Option<String>,
    phase: std::mem::Discriminant<crate::auth::Phase>,
}

#[derive(Default)]
pub struct State {
    pub prefix: String,
    pub scroll: usize,
    pub help_g: bool,
    pub hint_rows: usize,
    pub help_rows: usize,
    scope: Option<Scope>,
    hint_since: Option<Instant>,
    hint_visible: bool,
}

impl State {
    pub fn clear(&mut self) {
        self.prefix.clear();
        self.scroll = 0;
        self.scope = None;
        self.help_g = false;
        self.hint_rows = 0;
        self.hint_since = None;
        self.hint_visible = false;
    }

    pub fn hint_visible(&self) -> bool {
        self.hint_visible
    }

    pub(crate) fn schedule_hint(&mut self, now: Instant) {
        if !self.hint_visible {
            self.hint_since = Some(now);
        }
    }

    pub(crate) fn tick(&mut self, now: Instant) -> bool {
        if self
            .hint_since
            .is_some_and(|since| now.saturating_duration_since(since) >= HINT_DELAY)
        {
            self.hint_since = None;
            self.hint_visible = true;
            return true;
        }
        false
    }

    pub(crate) fn wake_in(&self, now: Instant) -> Option<Duration> {
        self.hint_since
            .map(|since| HINT_DELAY.saturating_sub(now.saturating_duration_since(since)))
    }
}

#[derive(Clone, Copy)]
pub enum Target {
    Action(Action),
    Menu(usize),
}

pub struct Candidate {
    pub keys: String,
    pub description: String,
    pub target: Target,
}

pub fn context(app: &App) -> Context {
    match &app.overlay {
        Some(Overlay::Picker(_) | Overlay::Search { .. }) => Context::Input,
        Some(Overlay::Settings)
            if matches!(
                app.settings.managed,
                Some(crate::app::settings::ManagedSave::Confirm(_))
            ) =>
        {
            Context::Managed
        }
        Some(Overlay::Settings) if app.settings.managed.is_some() => Context::Pending,
        Some(Overlay::Settings) if app.settings.editing.is_some() => Context::Input,
        Some(Overlay::Settings) => Context::Settings,
        Some(Overlay::Auth)
            if matches!(
                app.auth.phase,
                crate::auth::Phase::Confirm | crate::auth::Phase::DisconnectConfirm
            ) =>
        {
            Context::Confirm
        }
        Some(Overlay::Auth) if app.auth.busy() => Context::Pending,
        Some(Overlay::Auth) => Context::Accounts,
        Some(Overlay::Sort { .. }) => Context::Sort,
        Some(Overlay::Membership { .. }) => Context::Confirm,
        Some(Overlay::Experience) if app.experience.writing() => Context::Pending,
        Some(Overlay::Experience) => match app.experience.screens.last() {
            Some(Screen::Input(_)) => Context::Input,
            _ if app.experience.pending.is_some() && !app.experience.loading_more() => {
                Context::Pending
            }
            Some(Screen::Menu { .. }) => Context::Menu,
            Some(Screen::Browse { .. }) => Context::Browse,
            Some(Screen::Confirm { .. }) => Context::Confirm,
            _ => Context::Pending,
        },
        None if app.post_focused() => Context::Post,
        None if app.focus == Focus::Sidebar => Context::Sidebar,
        None => Context::List,
    }
}

fn scope(app: &App) -> Scope {
    Scope {
        context: context(app),
        tab: app.active_tab().id,
        post: app.post_view.as_ref().map(|view| view.generation),
        screens: app.experience.screens.len(),
        account: app.reddit.account().map(|account| account.id),
        phase: std::mem::discriminant(&app.auth.phase),
    }
}

pub fn sync(app: &mut App) {
    if !app.keymap.prefix.is_empty() && app.keymap.scope.as_ref() != Some(&scope(app)) {
        app.keymap.clear();
    }
}

pub fn label(keys: &str) -> String {
    keys.chars()
        .map(|c| match c {
            ' ' => "SPC".into(),
            '\u{1}'..='\u{1a}' => format!("C-{}", char::from_u32(c as u32 + 96).unwrap()),
            _ => c.to_string(),
        })
        .collect::<Vec<_>>()
        .join(
            if keys.contains(' ') || keys.chars().any(char::is_control) {
                " "
            } else {
                ""
            },
        )
}

pub fn action_label(action: Action) -> String {
    label(
        BINDINGS
            .iter()
            .find(|binding| binding.action == action)
            .unwrap()
            .keys,
    )
}

pub fn fold_label(app: &App) -> Option<&'static str> {
    use crate::app::RowKind;
    let view = app.post_view.as_ref()?;
    let rows = view.rows();
    if view.free_scroll {
        return (!rows.is_empty()).then_some("select");
    }
    match &rows.get(view.cursor)?.kind {
        RowKind::More { parent, .. } if view.loading_more.contains(*parent) => None,
        RowKind::More { .. } => Some("load replies"),
        RowKind::Comment { comment, .. } if view.collapsed.contains(&comment.id) => Some("unfold"),
        RowKind::Comment { comment, .. }
            if !comment.replies.is_empty() || comment.more.is_some() =>
        {
            Some("fold")
        }
        _ => None,
    }
}

fn description(app: &App, binding: &Binding) -> &'static str {
    use Action::*;
    match binding.action {
        Down if context(app) == Context::Post => "Next comment / scroll down",
        Up if context(app) == Context::Post => "Previous comment / scroll up",
        Down if context(app) == Context::Sidebar => "Next sidebar tab",
        Up if context(app) == Context::Sidebar => "Previous sidebar tab",
        NextCommunity if matches!(app.active_tab().kind, TabKind::AllSubs) => {
            "Next community group"
        }
        PreviousCommunity if matches!(app.active_tab().kind, TabKind::AllSubs) => {
            "Previous community group"
        }
        Fold => fold_label(app).unwrap_or(binding.description),
        Bookmark
            if app
                .current_post()
                .is_some_and(|post| app.state.is_bookmarked(&post.id)) =>
        {
            "Remove the local bookmark"
        }
        _ => binding.description,
    }
}

pub fn available(app: &App, action: Action) -> bool {
    use Action::*;
    if app.experience.loading_more()
        && matches!(app.overlay, Some(Overlay::Experience))
        && !matches!(
            action,
            Down | Up
                | Left
                | First
                | Last
                | PageDown
                | PageUp
                | HalfDown
                | HalfUp
                | LineDown
                | LineUp
                | Close
        )
    {
        return false;
    }
    let context = context(app);
    let reading = matches!(
        context,
        Context::List | Context::Sidebar | Context::Post | Context::Browse
    );
    let feed = matches!(
        app.active_tab().kind,
        TabKind::Home | TabKind::Subreddit { .. } | TabKind::User { .. }
    );
    let browse = match app.experience.screens.last() {
        Some(Screen::Browse { query, page, .. }) if context == Context::Browse => {
            Some((query, page))
        }
        _ => None,
    };
    match action {
        Settings => {
            !matches!(
                context,
                Context::Settings
                    | Context::Managed
                    | Context::Input
                    | Context::Pending
                    | Context::Confirm
            ) && !app.experience.writing()
                && !app.settings.writing()
        }
        Accounts => {
            !matches!(
                context,
                Context::Accounts
                    | Context::Managed
                    | Context::Input
                    | Context::Pending
                    | Context::Confirm
            ) && !app.experience.writing()
                && !app.settings.writing()
        }
        Right => !matches!(
            context,
            Context::Input | Context::Pending | Context::Confirm | Context::Managed
        ),
        Down | Up | Left | First | Last | PageDown | PageUp | HalfDown | HalfUp | LineDown
        | LineUp | Close => !matches!(
            context,
            Context::Input | Context::Pending | Context::Confirm
        ),
        FocusLeft | FocusRight => app.overlay.is_none(),
        Fold => context == Context::Post && fold_label(app).is_some(),
        NextPost | PreviousPost => context == Context::Post,
        NextThread | PreviousThread => {
            context == Context::Post
                && app
                    .post_view
                    .as_ref()
                    .is_some_and(|view| !view.rows().is_empty())
        }
        Spoilers => {
            context == Context::Post
                && app
                    .post_view
                    .as_ref()
                    .is_some_and(crate::app::PostView::has_spoilers)
        }
        NextImage | PreviousImage => {
            context == Context::Post
                && app
                    .post_view
                    .as_ref()
                    .is_some_and(|view| !view.spoiler_hidden())
                && app
                    .current_post()
                    .is_some_and(|post| matches!(post.media, crate::model::Media::Gallery { .. }))
        }
        NextCommunity | PreviousCommunity => {
            matches!(context, Context::List | Context::Post)
                && (matches!(app.active_tab().kind, TabKind::AllSubs)
                    || app.tabs.iter().any(|tab| tab.subreddit().is_some()))
        }
        Pause | Mute | SeekBack | SeekForward => {
            app.overlay.is_none() && app.video_controls_available()
        }
        Media => {
            app.overlay.is_none()
                && app
                    .current_post()
                    .is_some_and(|post| !matches!(post.media, crate::model::Media::None))
        }
        OpenLinks => matches!(context, Context::Post | Context::Browse),
        Bookmark => {
            matches!(context, Context::List | Context::Post) && app.current_post().is_some()
        }
        CopyLink => {
            matches!(context, Context::List | Context::Post | Context::Browse)
                && (context == Context::Browse || app.current_post().is_some())
        }
        Actions => {
            matches!(context, Context::List | Context::Post) && app.current_post().is_some()
                || matches!(app.experience.screens.last(), Some(Screen::Browse { page, selected, .. })
                    if context == Context::Browse && page.entries.get(*selected).is_some_and(|entry| entry.target.is_some()))
        }
        Membership | Community | CommunityFeed | SourceBrowser => {
            reading && app.membership_target().is_some()
        }
        Sort => {
            context == Context::Post
                || browse.is_some_and(|(query, _)| {
                    matches!(
                        query,
                        crate::reddit::experience::Query::Search {
                            kind: crate::reddit::experience::SearchKind::Posts,
                            ..
                        }
                    )
                })
                || (app.overlay.is_none() && feed)
        }
        Time => {
            browse.is_some_and(|(query, _)| {
                matches!(query, crate::reddit::experience::Query::Search { .. })
            }) || (app.overlay.is_none()
                && feed
                && app.active_tab().feed.search.is_none()
                && matches!(
                    app.active_tab().feed.sort,
                    crate::model::Sort::Top | crate::model::Sort::Controversial
                ))
        }
        CloseTab => app.overlay.is_none(),
        ListView => {
            matches!(context, Context::List | Context::Sidebar)
                && !matches!(app.active_tab().kind, TabKind::AllSubs)
        }
        Search if context == Context::Post => app
            .post_view
            .as_ref()
            .is_some_and(|view| !view.spoiler_hidden()),
        _ => reading,
    }
}

pub fn candidates(app: &App) -> Vec<Candidate> {
    let mut result: Vec<_> = BINDINGS
        .iter()
        .filter(|binding| available(app, binding.action))
        .map(|binding| Candidate {
            keys: binding.keys.into(),
            description: description(app, binding).into(),
            target: Target::Action(binding.action),
        })
        .collect();
    if context(app) == Context::Menu
        && let Some(Screen::Menu { choices, .. }) = app.experience.screens.last()
    {
        result.extend(
            choices
                .iter()
                .enumerate()
                .filter(|(_, choice)| !choice.shortcut.is_empty())
                .map(|(index, choice)| Candidate {
                    keys: choice.shortcut.into(),
                    description: choice.label.clone(),
                    target: Target::Menu(index),
                }),
        );
    }
    result
}

pub fn prefix_name(prefix: &str) -> &'static str {
    match prefix {
        " " => "Actions",
        " a" => "Content actions",
        " b" => "Buffers",
        " f" => "Feed",
        " g" | "g" => "Go",
        " m" => "Media",
        " o" => "Open",
        " p" => "Preferences",
        " s" => "Search",
        " v" => "View",
        "[" => "Previous",
        "]" => "Next",
        "z" => "Folds",
        "y" => "Yank",
        "\u{17}" => "Focus pane",
        _ => "Choose action",
    }
}

pub fn hints(app: &App) -> Vec<(String, String)> {
    let mut result = Vec::new();
    for candidate in candidates(app) {
        let Some(rest) = candidate.keys.strip_prefix(&app.keymap.prefix) else {
            continue;
        };
        let Some(next) = rest.chars().next() else {
            continue;
        };
        let key = label(&next.to_string());
        let description = if rest.chars().count() == 1 {
            candidate.description
        } else {
            prefix_name(&format!("{}{next}", app.keymap.prefix)).into()
        };
        if !result.iter().any(|(existing, _)| existing == &key) {
            result.push((key, description));
        }
    }
    result
}

pub fn resolve(app: &mut App, key: KeyEvent) -> Option<Target> {
    sync(app);
    let c = match key.code {
        KeyCode::Char(c) if key.modifiers == KeyModifiers::CONTROL && c.is_ascii_lowercase() => {
            char::from_u32(c as u32 - 96)?
        }
        KeyCode::Char(c)
            if !key.modifiers.intersects(
                KeyModifiers::CONTROL
                    | KeyModifiers::ALT
                    | KeyModifiers::SUPER
                    | KeyModifiers::HYPER
                    | KeyModifiers::META,
            ) =>
        {
            c
        }
        _ => return None,
    };
    if app.keymap.prefix.is_empty() {
        app.keymap.scope = Some(scope(app));
    }
    app.keymap.prefix.push(c);
    let candidates = candidates(app);
    let exact = candidates
        .iter()
        .find(|candidate| candidate.keys == app.keymap.prefix)
        .map(|candidate| candidate.target);
    if exact.is_some()
        || !candidates
            .iter()
            .any(|candidate| candidate.keys.starts_with(&app.keymap.prefix))
    {
        app.keymap.clear();
    } else {
        app.keymap.schedule_hint(Instant::now());
    }
    exact
}
