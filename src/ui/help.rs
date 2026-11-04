use super::*;
use crate::app::experience::Screen;
use crate::auth::Phase;
use crate::keys::map::Action;
use crate::keys::map::{self as bindings, Context, Target};

type Row = (u8, String, String);

pub(super) fn entries(app: &App) -> Vec<(String, String)> {
    let context = bindings::context(app);
    let title = match context {
        Context::Post => "Post and comments",
        Context::Sidebar => "Sidebar and buffers",
        Context::List if app.active == crate::app::SEARCH_TAB => {
            if app.search.exploring() {
                "Explore"
            } else {
                match app.search.kind {
                    crate::reddit::experience::SearchKind::Posts => "Search posts",
                    crate::reddit::experience::SearchKind::Communities => "Search communities",
                    crate::reddit::experience::SearchKind::People => "Search people",
                }
            }
        }
        Context::List => "Feed",
        Context::Browse => "Reading dialog",
        Context::Menu => "Action menu",
        Context::Settings => "Settings",
        Context::Managed => "Review managed settings save",
        Context::Accounts => "Accounts",
        Context::Sort => "Choose sort",
        Context::Confirm => "Confirmation",
        Context::Input => "Text entry",
        Context::Pending => "Pending operation",
    };
    let mut result = vec![(
        format!("{title} · close help to use these keys"),
        String::new(),
    )];
    let mut add = |key: &str, description: &str| result.push((key.into(), description.into()));
    match context {
        Context::Input => {
            if let Some(Screen::Input(form)) = app.experience.screens.last()
                && matches!(app.overlay, Some(Overlay::Experience))
            {
                add(
                    "C-s / Alt-Enter",
                    &format!("{} this form", form.submit_label()),
                );
            }
            if app.search_editing() {
                add(
                    "Up / Down · C-n / C-p · C-j / C-k",
                    "Choose a result while typing",
                );
                add(
                    "Enter",
                    "Run a pending search and leave the query for the results",
                );
                add("Tab / S-Tab", "Switch Posts / Communities / People");
                add("Esc", "Leave the query for list keys");
                add("Left / Right · Home / End · C-a / C-e", "Move the cursor");
                add("Delete", "Delete the next character");
            } else {
                add("Esc", "Cancel / return; keep protected drafts");
                add(
                    "Enter",
                    "Accept a single-line field; add a newline in a multiline field",
                );
            }
            add("C-u", "Clear the current field");
            add("C-w / Alt-Backspace", "Delete the word before the cursor");
            add("Backspace", "Delete the previous character");
            match &app.overlay {
                Some(Overlay::Picker(_)) => add(
                    "C-n/j · C-p/k · Up/Down · Tab",
                    "Choose the next / previous result",
                ),
                Some(Overlay::Settings) => {
                    add("Left/Right · Home/End · C-a/e", "Move through the value");
                    add("C-s", "Apply this setting value");
                    add("Delete", "Delete the next character");
                }
                Some(Overlay::Experience) => {
                    add("Left/Right · Home/End · C-a/e", "Move through the field");
                    add("Delete", "Delete the next character");
                    if let Some(Screen::Input(form)) = app.experience.screens.last()
                        && form.fields.len() > 1
                    {
                        add("Tab / S-Tab", "Next / previous field");
                    }
                }
                _ => {}
            }
        }
        Context::Confirm => {
            add("Enter", "Confirm the displayed action");
            add("Esc / q", "Cancel");
            if matches!(app.overlay, Some(Overlay::Membership { .. })) {
                add("o", "Open the community or profile instead");
            }
        }
        Context::Pending => {
            let auth_commit = matches!(app.overlay, Some(Overlay::Auth))
                && matches!(app.auth.phase, Phase::Saving | Phase::Disconnecting);
            if !app.experience.writing()
                && !app.settings.managed_busy()
                && !app.settings.writing()
                && !auth_commit
            {
                add("Esc / q", "Back / cancel this read");
                if matches!(app.overlay, Some(Overlay::Experience))
                    && app.experience.pending.is_none()
                {
                    add("r", "Retry a stopped request");
                }
            }
            if matches!(app.overlay, Some(Overlay::Auth)) && app.auth.can_remove() {
                add("d", "Remove the saved account; requires confirmation");
            }
            if app.experience.writing() || app.settings.writing() {
                add(
                    "Sending to Reddit",
                    "Wait for acknowledgment; the draft and target stay protected",
                );
            }
        }
        Context::List if bindings::search_kind_keys(app) => {
            add("Tab / S-Tab", "Switch Posts / Communities / People");
        }
        _ => {}
    }
    if !matches!(
        context,
        Context::Input | Context::Pending | Context::Confirm
    ) {
        let candidates = bindings::candidates(app);
        let mut sections: Vec<(u8, &str, Vec<Row>)> = Vec::new();
        let mut i = 0;
        while i < candidates.len() {
            let candidate = &candidates[i];
            let action = match candidate.target {
                Target::Action(action) => Some(action),
                Target::Menu(_) => None,
            };
            let mut keys = bindings::label(&candidate.keys);
            let mut description = match action {
                Some(Action::Search) if context == Context::Post => {
                    "Find text or author in loaded comments".into()
                }
                Some(Action::Search)
                    if app.active_tab().subreddit().is_some() && app.overlay.is_none() =>
                {
                    "Search this community".into()
                }
                Some(Action::Search) if app.active == crate::app::SEARCH_TAB => {
                    "Edit the search query".into()
                }
                Some(Action::Search) => "Search Reddit in the Search tab".into(),
                _ => candidate.description.clone(),
            };
            let partner = action.and_then(|action| pair(app, context, action));
            if let (Some((second, joined)), Some(next)) = (partner, candidates.get(i + 1))
                && matches!(next.target, Target::Action(actual) if actual == second)
            {
                keys = format!("{keys} / {}", bindings::label(&next.keys));
                description = joined.into();
                i += 1;
            }
            let (rank, title) = match action.and_then(|action| priority(app, context, action)) {
                Some(priority) => (priority, "Here"),
                None => section(action, &candidate.keys),
            };
            match sections.iter_mut().find(|(_, name, _)| *name == title) {
                Some((_, _, rows)) => rows.push((rank, keys, description)),
                None => sections.push((rank, title, vec![(rank, keys, description)])),
            }
            i += 1;
        }
        for (rank, _, rows) in &mut sections {
            rows.sort_by_key(|(row, _, _)| *row);
            *rank = rows[0].0;
        }
        if let Some((_, _, rows)) = sections
            .iter_mut()
            .find(|(_, title, _)| *title == "Space leader")
        {
            rows.insert(
                0,
                (
                    0,
                    "SPC".into(),
                    "Show mnemonic action groups; Esc cancels a pending sequence".into(),
                ),
            );
        }
        sections.sort_by_key(|(rank, _, _)| *rank);
        let mut view_rows_at = result.len();
        for (_, title, rows) in sections {
            result.push((String::new(), String::new()));
            result.push((title.into(), String::new()));
            result.extend(
                rows.into_iter()
                    .map(|(_, keys, description)| (keys, description)),
            );
            if matches!(title, "Here" | "Menu shortcuts") {
                view_rows_at = result.len();
            }
        }
        let start = result.len();
        let mut add = |key: &str, description: &str| result.push((key.into(), description.into()));
        match context {
            Context::Settings => {
                add(
                    "r",
                    "Reload Reddit preferences or retry saving file settings",
                );
                add(
                    "u",
                    "Undo unsaved file settings; Reddit settings save immediately",
                );
                add("F2", "Close settings");
            }
            Context::Managed => {
                add(
                    "Enter / y",
                    "Apply the displayed source changes through chezmoi",
                );
                add("Esc / n / q", "Keep edits for this session");
            }
            Context::Accounts => {
                if app.auth.phase == Phase::Accounts {
                    add("a", "Add an account");
                }
                if !app.auth.busy() {
                    add("r", "Reconnect the selected account / rescan sources");
                }
                if app.auth.can_remove() {
                    add("d", "Remove the selected account; requires confirmation");
                }
            }
            Context::Sort => {
                for sort in app.active_tab().sorts() {
                    let key = if *sort == crate::model::Sort::Hot {
                        "H"
                    } else {
                        &sort.as_str()[..1]
                    };
                    add(key, sort.as_str());
                }
            }
            Context::Browse
                if !app.experience.loading_more()
                    && matches!(
                        app.experience.screens.last(),
                        Some(Screen::Browse {
                            query: crate::reddit::experience::Query::Multi { .. },
                            ..
                        })
                    ) =>
            {
                add("a", "Add a community to this custom feed");
                add("d", "Remove a community from this custom feed");
                add("D", "Delete this custom feed; requires confirmation");
            }
            _ => {}
        }
        if result.len() > start {
            let mut rows: Vec<_> = result.drain(start..).collect();
            rows.splice(
                0..0,
                [
                    (String::new(), String::new()),
                    ("This view".into(), String::new()),
                ],
            );
            result.splice(view_rows_at..view_rows_at, rows);
        }
        result.push((String::new(), String::new()));
        result.push(("Also".into(), String::new()));
        if app.overlay.is_none() && !bindings::search_kind_keys(app) {
            result.push(("Tab / S-Tab".into(), "Next / previous buffer".into()));
        }
        result.push((
            if bindings::available(app, Action::Right) {
                "Enter / arrows"
            } else {
                "Arrows"
            }
            .into(),
            "Terminal equivalents for available navigation".into(),
        ));
        result.push((
            "Home/End · PgUp/Dn".into(),
            "Terminal equivalents for edges and paging".into(),
        ));
    }
    if matches!(app.overlay, Some(Overlay::Experience)) {
        result.push((
            "C-o".into(),
            "Open the corresponding Reddit page in browser".into(),
        ));
    }
    result.push((
        "F1".into(),
        "Help without changing the current draft or selection".into(),
    ));
    result.push(("j/k · C-f/b · C-d/u".into(), "Scroll this help".into()));
    result.push(("gg / G".into(), "Top / bottom of this help".into()));
    result.push((
        "Esc".into(),
        "Close help; never runs a covered action".into(),
    ));
    result.push(("C-c".into(), "Quit redtab".into()));
    result
}

fn priority(app: &App, context: Context, action: Action) -> Option<u8> {
    use Action::*;
    let denied = app.active_tab().feed.access_denied().is_some() && app.overlay.is_none();
    let video_first =
        context != Context::Post || app.post_view.as_ref().is_some_and(|view| view.image_shown);
    Some(match action {
        Spoilers => 0,
        Browser | Reload if denied => 0,
        Pause | Mute | SeekBack | SeekForward if video_first => 1,
        NextImage | PreviousImage => 2,
        Down | Up => 3,
        Left | Right
            if !matches!(
                context,
                Context::List | Context::Sidebar | Context::Post | Context::Browse
            ) =>
        {
            3
        }
        Fold | Actions => 4,
        Upvote | Downvote | Save | Reply if context == Context::Post => 4,
        NextThread | PreviousThread | NextPost | PreviousPost | Bookmark | ListView
        | NextCommunity | PreviousCommunity | Search => 5,
        _ => return None,
    })
}

fn section(action: Option<Action>, keys: &str) -> (u8, &'static str) {
    use Action::*;
    let Some(action) = action else {
        return (10, "Menu shortcuts");
    };
    match action {
        _ if keys.starts_with(' ') => (60, "Space leader"),
        Down | Up | Left | Right | First | Last | PageDown | PageUp | HalfDown | HalfUp
        | LineDown | LineUp | FocusLeft | FocusRight | NextThread | PreviousThread | NextPost
        | PreviousPost | NextImage | PreviousImage | Fold | Search => (20, "Move"),
        Upvote | Downvote | Save | Reply | Bookmark | Browser | Media | OpenLinks | CopyLinks
        | CopyLink | Reload | Close => (30, "Act on the item"),
        Pause | Mute | SeekBack | SeekForward => (35, "Video"),
        Spoilers | Images | ListView | Sidebar => (40, "View"),
        _ => (50, "Go"),
    }
}

fn pair(app: &App, context: Context, action: Action) -> Option<(Action, &'static str)> {
    use Action::*;
    let all_subs = matches!(app.active_tab().kind, TabKind::AllSubs);
    Some(match action {
        Down if context == Context::Post => (Up, "Next / previous comment; scroll"),
        Down if context == Context::Sidebar => (Up, "Next / previous sidebar tab"),
        Down => (Up, "Move down / up"),
        Left => (Right, "Back / open; focus left / right"),
        First => (Last, "First / last item; top / bottom"),
        PageDown => (PageUp, "Page down / up"),
        HalfDown => (HalfUp, "Half page down / up"),
        LineDown if bindings::line_moves_items(app) => (LineUp, "Next / previous item"),
        LineDown => (LineUp, "Scroll one line down / up"),
        NextTab => (PreviousTab, "Next / previous tab"),
        JumpBack => (JumpForward, "Older / newer location in the jump list"),
        FocusLeft => (FocusRight, "Focus the pane left / right"),
        NextCommunity if app.search_source_context() => (
            PreviousCommunity,
            "Next / previous Search source; return to results",
        ),
        NextCommunity if all_subs => (PreviousCommunity, "Next / previous community group"),
        NextCommunity => (PreviousCommunity, "Next / previous subreddit feed"),
        NextPost => (PreviousPost, "Next / previous post; keep details open"),
        NextThread => (PreviousThread, "Next / previous root comment thread"),
        NextImage => (PreviousImage, "Next / previous gallery image"),
        Upvote => (Downvote, "Upvote / downvote; again removes the vote"),
        SeekBack => (SeekForward, "Seek backward / forward five seconds"),
        Browser => (Media, "Open on Reddit / open media externally"),
        FirstTab => (LastTab, "First / last buffer"),
        _ => return None,
    })
}

pub(super) fn lines(entries: Vec<(String, String)>, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (key, text) in entries {
        if text.is_empty() {
            lines.extend(
                textwrap::wrap(&key, width.max(1))
                    .into_iter()
                    .map(|part| Line::styled(part.into_owned(), Style::new().fg(ACCENT).bold())),
            );
        } else if width >= 42 && key.width() <= 20 {
            for (i, part) in textwrap::wrap(&text, width.saturating_sub(22))
                .into_iter()
                .enumerate()
            {
                lines.push(Line::from(vec![
                    Span::styled(
                        if i == 0 {
                            format!("  {key:<20}")
                        } else {
                            " ".repeat(22)
                        },
                        Style::new().fg(LAVENDER),
                    ),
                    Span::styled(part.into_owned(), Style::new().fg(SUBTEXT)),
                ]));
            }
        } else if width >= 42 {
            lines.extend(
                textwrap::wrap(&key, width.saturating_sub(2))
                    .into_iter()
                    .map(|part| Line::styled(format!("  {part}"), Style::new().fg(LAVENDER))),
            );
            for part in textwrap::wrap(&text, width.saturating_sub(22)) {
                lines.push(Line::styled(
                    format!("{}{part}", " ".repeat(22)),
                    Style::new().fg(SUBTEXT),
                ));
            }
        } else {
            lines.extend(
                textwrap::wrap(&key, width.max(1))
                    .into_iter()
                    .map(|part| Line::styled(part.into_owned(), Style::new().fg(LAVENDER))),
            );
            let indent = " ".repeat(width.saturating_sub(1).min(2));
            for part in textwrap::wrap(&text, width.saturating_sub(indent.len()).max(1)) {
                lines.push(Line::styled(
                    format!("{indent}{part}"),
                    Style::new().fg(SUBTEXT),
                ));
            }
        }
    }
    lines
}

pub(super) fn draw(f: &mut Frame, app: &mut App) -> Option<Rect> {
    let entries = entries(app);
    let available = popup(f.area(), 100, u16::MAX);
    let title = fitting_label(
        &[
            "Help · j/k scroll · Esc back",
            "Help ↑↓ Esc",
            "Help Esc",
            "↑↓ ⎋",
            "⎋",
        ],
        usize::from(available.width.saturating_sub(4)),
    );
    let block = popup_block(title, available.width);
    let width = block.inner(available).width as usize;
    let lines = lines(entries, width);
    let area = popup(
        f.area(),
        100,
        (lines.len().saturating_add(2).min(32)) as u16,
    );
    let inner = block.inner(area);
    app.keymap.help_rows = inner.height as usize;
    let scroll = app.help_scroll.as_mut()?;
    *scroll = (*scroll).min(lines.len().saturating_sub(inner.height as usize));
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    f.render_widget(
        Paragraph::new(lines.into_iter().skip(*scroll).collect::<Vec<_>>()),
        inner,
    );
    Some(area)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiny_help_rows_should_fit_without_losing_description_characters() {
        let rendered = lines(vec![("j".into(), "abcdefghij".into())], 10);

        assert_eq!(
            rendered.iter().map(ToString::to_string).collect::<Vec<_>>(),
            ["j", "  abcdefgh", "  ij"]
        );
    }

    #[test]
    fn wide_help_keys_should_keep_descriptions_in_the_aligned_column() {
        let rendered = lines(
            vec![
                ("a key wider than the column".into(), "wide".into()),
                ("C-c".into(), "quit".into()),
            ],
            60,
        );

        let column = |line: &Line| line.to_string().find(|c: char| c.is_alphabetic());
        assert_eq!(column(&rendered[1]), Some(22));
        assert_eq!(rendered[2].to_string().find("quit"), Some(22));
    }
}
