use super::*;
use crate::app::experience::Screen;
use crate::auth::Phase;
use crate::keys::map::Action;
use crate::keys::map::{self as bindings, Context, Target};

pub(super) fn entries(app: &App) -> Vec<(String, String)> {
    let context = bindings::context(app);
    let title = match context {
        Context::Post => "Post and comments",
        Context::Sidebar => "Sidebar and buffers",
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
            add("Esc", "Cancel / return; keep protected drafts");
            add(
                "Enter",
                "Accept a single-line field; add a newline in a multiline field",
            );
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
        _ => {}
    }
    if !matches!(
        context,
        Context::Input | Context::Pending | Context::Confirm
    ) {
        if bindings::candidates(app)
            .iter()
            .any(|candidate| candidate.keys.starts_with(' '))
        {
            result.push((
                "SPC".into(),
                "Show mnemonic action groups; Esc cancels a pending sequence".into(),
            ));
        }
        let mut candidates = bindings::candidates(app);
        candidates.sort_by_key(|candidate| {
            let Target::Action(action) = candidate.target else {
                return 15;
            };
            match action {
                Action::Spoilers => 0,
                Action::Pause | Action::Mute | Action::SeekBack | Action::SeekForward
                    if context != Context::Post
                        || app.post_view.as_ref().is_some_and(|view| view.image_shown) =>
                {
                    1
                }
                Action::NextImage | Action::PreviousImage => 2,
                Action::Down | Action::Up => 3,
                Action::Fold | Action::Actions => 4,
                Action::NextThread
                | Action::PreviousThread
                | Action::NextPost
                | Action::PreviousPost
                | Action::Bookmark
                | Action::ListView
                | Action::NextCommunity
                | Action::PreviousCommunity
                | Action::Search => 5,
                Action::Browser | Action::Reload
                    if app.active_tab().feed.access_denied().is_some() && app.overlay.is_none() =>
                {
                    0
                }
                _ => 20,
            }
        });
        for candidate in candidates {
            let description = if matches!(
                candidate.target,
                Target::Action(crate::keys::map::Action::Search)
            ) {
                if context == Context::Post {
                    "Find text or author in loaded comments".into()
                } else if app.active_tab().subreddit().is_some() && app.overlay.is_none() {
                    "Search this community".into()
                } else {
                    "Search posts across Reddit".into()
                }
            } else {
                candidate.description
            };
            result.push((bindings::label(&candidate.keys), description));
        }
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
        result.push((
            if app.experience.loading_more() {
                "Arrows"
            } else {
                "Enter / arrows"
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
    result.push((
        "Help: j/k · C-f/b · C-d/u · gg/G · Esc".into(),
        "Scroll help; dismissing help never runs a covered action".into(),
    ));
    result.push(("C-c".into(), "Quit redtab".into()));
    result
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
}
