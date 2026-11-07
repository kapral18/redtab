use super::*;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Scope {
    pub(super) context: Context,
    pub(super) tab: u64,
    pub(super) post: Option<u64>,
    pub(super) screens: usize,
    pub(super) account: Option<String>,
    pub(super) phase: std::mem::Discriminant<crate::auth::Phase>,
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
            _ if app.experience.pending.is_some()
                && !app.experience.loading_more()
                && !app.experience.refreshing =>
            {
                Context::Pending
            }
            Some(Screen::Menu { .. }) => Context::Menu,
            Some(Screen::Browse { .. }) => Context::Browse,
            Some(Screen::Confirm { .. }) => Context::Confirm,
            _ => Context::Pending,
        },
        None if app.search_editing() => Context::Input,
        None if app.post_focused() => Context::Post,
        None if app.focus == Focus::Sidebar => Context::Sidebar,
        None => Context::List,
    }
}

pub(super) fn scope(app: &App) -> Scope {
    Scope {
        context: context(app),
        tab: app.active_tab().id,
        post: app.post_view.as_ref().map(|view| view.generation),
        screens: app.experience.screens.len(),
        account: app.reddit.account().map(|account| account.id),
        phase: std::mem::discriminant(&app.auth.phase),
    }
}

pub(crate) fn line_moves_items(app: &App) -> bool {
    matches!(
        context(app),
        Context::Sidebar | Context::Menu | Context::Settings | Context::Accounts | Context::Sort
    ) || (context(app) == Context::List && app.list_uses_rows())
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
    ) || (matches!(app.active_tab().kind, TabKind::Search)
        && app.active_tab().feed.search.is_some()
        && !app.search.shows_rows());
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
        Right => match context {
            Context::List if app.search_shows_rows() => app.search.selected_row().is_some(),
            Context::List => app.current_post().is_some() || app.post_view.is_some(),
            Context::Post => fold_label(app).is_some(),
            Context::Browse => matches!(app.experience.screens.last(),
                Some(Screen::Browse { page, selected, .. }) if page.entries.get(*selected)
                    .is_some_and(|entry| entry.query.is_some() || entry.target.is_some())),
            Context::Menu => matches!(app.experience.screens.last(),
                Some(Screen::Menu { choices, selected, .. }) if choices.get(*selected).is_some()),
            Context::Accounts => app.auth.choices() > 0,
            Context::Sidebar | Context::Settings | Context::Sort => true,
            _ => false,
        },
        Down | Up | Left | First | Last | PageDown | PageUp | HalfDown | HalfUp | LineDown
        | LineUp | Close => !matches!(
            context,
            Context::Input | Context::Pending | Context::Confirm
        ),
        FocusLeft => matches!(context, Context::List | Context::Post),
        FocusRight => context == Context::Sidebar
            || (context == Context::List
                && (app.post_view.is_some()
                    || (app.search_shows_rows() && app.detail_fits() && available(app, Right)))),
        Fold => context == Context::Post && fold_label(app).is_some(),
        NextPost | PreviousPost => context == Context::Post && app.post_parent_position().is_some(),
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
                    .is_some_and(|post| matches!(&post.media, crate::model::Media::Gallery { images, .. } if !images.is_empty()))
        }
        NextCommunity | PreviousCommunity => {
            matches!(context, Context::List | Context::Post)
                && if app.search_source_context() {
                    app.search.exploring() || !app.search.results.entries.is_empty()
                } else {
                    if matches!(app.active_tab().kind, TabKind::AllSubs) {
                        !app.all_subs.groups.is_empty()
                    } else {
                        app.tabs.iter().any(|tab| tab.subreddit().is_some())
                    }
                }
        }
        Pause => {
            matches!(context, Context::List | Context::Post) && app.video_controls_available()
        }
        Mute => available(app, Pause) && app.selected_video().is_some_and(|player| player.error().is_none()),
        SeekBack | SeekForward => available(app, Pause) && app.selected_video().is_some_and(|player| player.ready() && player.error().is_none()),
        Media => {
            matches!(context, Context::List | Context::Post)
                && app
                    .current_post()
                    .is_some_and(|post| !matches!(post.media, crate::model::Media::None))
        }
        OpenLinks | CopyLinks => matches!(context, Context::Post | Context::Browse),
        Bookmark => {
            matches!(context, Context::List | Context::Post) && app.current_post().is_some()
        }
        CopyLink => {
            matches!(context, Context::List | Context::Post | Context::Browse)
                && (context == Context::Browse || app.copy_link_target().is_ok())
        }
        Upvote | Downvote | Save | Reply => {
            available(app, Actions)
                && app.action_target().is_some_and(|target| target.id.starts_with("t1_") || target.id.starts_with("t3_"))
                && !(action == Downvote && context == Context::Browse && matches!(
                    app.experience.screens.last(),
                    Some(Screen::Browse {
                        query: crate::reddit::experience::Query::Multi { .. },
                        ..
                    })
                ))
        }
        Actions => {
            matches!(context, Context::List | Context::Post) && app.current_post().is_some()
                || matches!(app.experience.screens.last(), Some(Screen::Browse { page, selected, .. })
                    if context == Context::Browse && page.entries.get(*selected).is_some_and(|entry| entry.target.is_some()))
        }
        Membership | Community => reading && app.membership_target().is_some(),
        CommunityFeed => {
            reading && (app.membership_target().is_some() || app.search_row_path().is_some())
        }
        SourceBrowser => reading && app.source_browser_path().is_some(),
        Browser => match context {
            Context::Browse => true,
            Context::Sidebar => app.source_browser_path().is_some(),
            Context::List | Context::Post => {
                app.current_post().is_some()
                    || app.search_comment_link().is_some()
                    || app.source_browser_path().is_some()
            }
            _ => false,
        },
        Sort => context == Context::Post || (app.overlay.is_none() && feed),
        Time => {
            (app.overlay.is_none() && feed && app.active_tab().feed.search.is_some())
                || (app.overlay.is_none()
                    && feed
                    && matches!(
                        app.active_tab().feed.sort,
                        crate::model::Sort::Top | crate::model::Sort::Controversial
                    ))
        }
        CloseTab => app.overlay.is_none(),
        JumpBack | JumpForward => {
            matches!(context, Context::List | Context::Sidebar | Context::Post)
        }
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
