use super::*;
use crate::app::experience::{Form, Screen};
use crate::auth::Phase;
use crate::reddit::experience::{Query, SearchKind};

type Keys = Vec<(&'static str, &'static str)>;

struct Section {
    title: String,
    keys: Keys,
}

fn section(title: impl Into<String>, keys: &[(&'static str, &'static str)]) -> Section {
    Section {
        title: title.into(),
        keys: keys.to_vec(),
    }
}

const LIST: &[(&str, &str)] = &[
    ("j k", "next / previous post"),
    ("Enter l", "open the post, or resume the one shown"),
    ("g G", "first / last post"),
    ("V", "Card / Compact view (saved)"),
    ("PgDn/PgUp · C-d/u", "cards: scroll a page / half page"),
    ("C-e C-y", "cards: scroll down / up one line"),
    ("C-d C-u", "Compact: page down / up"),
    ("J K", "All subs: next / previous community island"),
    ("Space", "open the selected community"),
    ("s · t", "Home/subreddit: sort · time range"),
    ("/ · Esc", "subreddit: search · clear search"),
    ("L", "feed: next page (automatic near end)"),
];
const POST: &[(&str, &str)] = &[
    ("j k", "next / previous comment"),
    ("Space PgDn · PgUp", "page down · up"),
    ("C-d C-u · C-e C-y", "half page · one line"),
    ("l c Enter", "select the comment read (or left with h)"),
    ("l c Enter again", "fold a thread · load more replies"),
    ("n N", "next / previous top-level comment"),
    (";", "actions for the selected post or comment"),
    ("o · y", "open or copy the selected comment/post link"),
    ("f", "comment sort and find in loaded comments"),
    ("v", "show / hide spoilers"),
    ("g G", "top of post / last comment"),
    ("h", "split: list; narrow: back one level"),
    ("Esc q", "back to the list"),
];
const MEDIA: &[(&str, &str)] = &[
    ("P · a", "video: pause / play · sound on / off"),
    (", .", "video: back / forward 5 seconds"),
    ("{ }", "previous / next gallery image"),
    ("m", "media: image viewer, mpv, or browser"),
    ("i", "images and videos on / off"),
];
const TABS: &[(&str, &str)] = &[
    ("Tab ] / S-Tab [", "next / previous tab"),
    ("1-9 / 0", "jump to a tab / the last tab"),
    ("p", "jump to a tab or open any subreddit"),
    ("x", "close a tab opened with p"),
    ("\\", "hide or show the sidebar"),
    ("h · l Enter Esc", "focus the sidebar · back to the list"),
    ("j k · g G", "sidebar: next / previous tab · first / last"),
];
const REDDIT: &[(&str, &str)] = &[
    (":", "Reddit: saved, profiles, inbox, search"),
    (";", "post/comment: vote, save, reply, edit"),
    ("B", "community info, rules, wiki and actions"),
    ("S", "join or leave a community on Reddit"),
    ("C-s (composer)", "submit to Reddit; Tab field, Esc back"),
    ("C-o (Reddit)", "open the corresponding Reddit page"),
    ("A", "saved accounts: switch, add or reconnect"),
];
const GLOBAL: &[(&str, &str)] = &[
    ("o · y", "open on reddit.com · copy link"),
    ("b", "bookmark / remove bookmark"),
    ("r", "reload"),
    ("F2", "settings: all preferences; changes apply immediately"),
    ("? / F1", "contextual help (F1 also in text inputs)"),
    ("q · C-c", "quit (q closes an open post first)"),
];

fn prioritize(keys: &mut Keys, first: &[&str]) {
    keys.sort_by_key(|(key, _)| first.iter().position(|s| s == key).unwrap_or(first.len()));
}

fn list_section(app: &App) -> Section {
    if let Some((_, access)) = app.restricted_community() {
        return section(
            super::access_title(access),
            &[
                ("o", "open this community on Reddit to check access"),
                ("r", "check access again"),
                ("A", "switch or reconnect your Reddit account"),
                ("p", "jump to another community"),
                ("B", "community tools"),
            ],
        );
    }
    let list_title = match app.active_tab().kind {
        TabKind::Home => "Home",
        TabKind::Subreddit { .. } => "Subreddit feed / search",
        TabKind::AllSubs => "All subs",
        TabKind::Bookmarks => "Bookmarks",
    };
    let mode = if app.state.list_view() == ListView::Card {
        "Card"
    } else {
        "Compact"
    };
    let mut lists = section(
        if matches!(app.active_tab().kind, TabKind::AllSubs) {
            "All subs · community islands".into()
        } else {
            format!("{list_title} · {mode} view")
        },
        LIST,
    );
    let page_key = if app.state.list_view() == ListView::Card {
        "PgDn/PgUp · C-d/u"
    } else {
        "C-d C-u"
    };
    let mut first = vec!["j k", "Enter l", page_key];
    if app.state.list_view() == ListView::Card {
        first.push("C-e C-y");
        if app.video_controls_available() {
            lists.keys.extend_from_slice(&MEDIA[..2]);
            first.extend(["P · a", ", ."]);
        }
    }
    first.extend(["V", "g G"]);
    match app.active_tab().kind {
        TabKind::AllSubs => first = vec!["J K", "j k", "Enter l", "Space", "g G"],
        TabKind::Bookmarks => {
            lists.keys.push(("b", "remove the selected bookmark"));
            first.insert(2, "b");
        }
        TabKind::Subreddit { .. } => first.extend(["/ · Esc", "s · t", "L", "Space"]),
        TabKind::Home => first.extend(["s · t", "L", "Space"]),
    }
    prioritize(&mut lists.keys, &first);
    lists
}

fn post_section(app: &App) -> Section {
    let mut post = section("Post · reading", POST);
    let selected = app.post_view.as_ref().is_some_and(|v| !v.free_scroll);
    if selected {
        post.title = "Post · comment selected".into();
        prioritize(&mut post.keys, &["l c Enter again", ";", "o · y", "j k"]);
    }
    let hidden = app.post_view.as_ref().is_some_and(PostView::spoiler_hidden);
    if hidden {
        post.title = "Post · spoiler hidden".into();
        prioritize(&mut post.keys, &["v"]);
    }
    post
}

fn media_section(app: &App) -> Section {
    let mut media = section("Media", MEDIA);
    let shown_post = app.current_post();
    let shown = app
        .post_view
        .as_ref()
        .map(PostView::shown_post)
        .or(shown_post.as_ref());
    let media_active = shown
        .is_some_and(|p| p.media.video_url().is_some() || matches!(p.media, Media::Gallery { .. }));
    if shown.is_some_and(|p| matches!(p.media, Media::Gallery { .. })) {
        media.title = "Gallery".into();
        prioritize(&mut media.keys, &["{ }", "m"]);
    } else if media_active {
        media.title = if app.post_view.is_some() || !app.video_controls_available() {
            "Video / GIF"
        } else {
            "Card video / GIF"
        }
        .into();
        media.keys.retain(|(key, _)| *key != "{ }");
    }
    media
}

fn view_sections(app: &App) -> Vec<Section> {
    let mut tabs = section("Sidebar and tabs", TABS);
    if app.focus == Focus::Sidebar {
        prioritize(&mut tabs.keys, &["j k · g G", "h · l Enter Esc", "p"]);
    }
    let selected = app.post_view.as_ref().is_some_and(|v| !v.free_scroll);
    let media_active = app.post_view.as_ref().is_some_and(|v| {
        v.image_shown
            && !v.spoiler_hidden()
            && app.inline_images_on()
            && (v.shown_post().media.video_url().is_some()
                || matches!(v.shown_post().media, Media::Gallery { .. }))
    });
    let order = if app.post_focused() {
        if media_active && !selected {
            [2, 1, 4, 0, 3, 5]
        } else {
            [1, 2, 4, 0, 3, 5]
        }
    } else if app.focus == Focus::Sidebar {
        [3, 0, 4, 1, 2, 5]
    } else {
        [0, 4, 3, 1, 2, 5]
    };
    let mut sections: Vec<_> = [
        list_section(app),
        post_section(app),
        media_section(app),
        tabs,
        section("Reddit tools", REDDIT),
        section("General shortcuts", GLOBAL),
    ]
    .into_iter()
    .enumerate()
    .collect();
    sections.sort_by_key(|(id, _)| order.iter().position(|i| i == id));
    sections.into_iter().map(|(_, s)| s).collect()
}

fn action_section(app: &App) -> Option<Section> {
    Some(match app.overlay.as_ref()? {
        Overlay::Settings => {
            if app.settings.busy() {
                section(
                    "Saving settings",
                    &[("F1 / Esc", "close help; wait for the settings task")],
                )
            } else if app.settings.managed.is_some() {
                section(
                    "Save through chezmoi",
                    &[
                        ("Enter / y", "update the shown source and apply this config"),
                        ("Esc / n", "keep changes for this session"),
                        ("j k / Page Up Down", "scroll the source path and changes"),
                    ],
                )
            } else if app.settings.editing.is_some() {
                section(
                    "Edit setting",
                    &[
                        ("Enter / C-s", "apply value; save or review managed changes"),
                        ("Left Right / Home End", "move through the value"),
                        ("C-u", "clear value"),
                        ("Esc", "cancel this edit"),
                    ],
                )
            } else {
                section(
                    "Settings",
                    &[
                        ("j k / Up Down", "choose a setting"),
                        ("Enter / Space", "toggle or edit selected value"),
                        ("r", "retry saving unsaved changes"),
                        ("u", "revert all session changes to the file"),
                        ("Esc / F2", "close settings"),
                    ],
                )
            }
        }
        Overlay::Picker(_) => section(
            "Jump / open subreddit",
            &[
                ("Enter", "open the selected result"),
                ("Up Down / Tab", "select a result"),
                ("C-p/k · C-n/j", "previous / next result"),
                ("C-u", "clear input"),
                ("Alt-Backspace", "delete the previous word"),
                ("Esc", "cancel; keep the current view"),
            ],
        ),
        Overlay::Search { .. } => section(
            "Subreddit search",
            &[
                ("Enter", "search this subreddit"),
                ("C-u", "clear input"),
                ("Backspace", "delete the last character"),
                ("Esc", "cancel search input"),
            ],
        ),
        Overlay::Sort { .. } => sort_section(app),
        Overlay::Membership { .. } => section(
            "Confirm Join / Leave",
            &[
                ("Enter", "confirm the membership change"),
                ("Space", "open this community instead"),
                ("Esc q", "cancel"),
            ],
        ),
        Overlay::Auth => account_section(app),
        Overlay::Experience => experience_section(app),
    })
}

fn sort_section(app: &App) -> Section {
    let mut s = section(
        "Choose feed sort",
        &[
            ("j k / Down Up", "select a sort"),
            ("Enter", "apply the selected sort"),
            ("h n t r c", "hot, new, top, rising, controversial"),
        ],
    );
    if app.active_tab().sorts().contains(&Sort::Best) {
        s.keys.push(("b", "Best recommendations"));
    }
    s.keys.push(("Esc q", "cancel"));
    s
}

fn account_section(app: &App) -> Section {
    let auth = &app.auth;
    let mut s = section("Saved accounts / sign-in", &[]);
    match auth.phase {
        Phase::Accounts => {
            s.keys.extend([
                ("j k / Down Up", "select a saved account or Add account"),
                ("Enter", "select saved account / Add account"),
                ("a", "add an account"),
            ]);
            s.keys.push((
                "r",
                if auth.selected < auth.accounts.len() {
                    "reconnect the selected account"
                } else {
                    "rescan browser profiles"
                },
            ));
        }
        Phase::Idle | Phase::Choose => {
            if !auth.sources.is_empty() {
                s.keys.extend([
                    ("j k / Down Up", "select a browser profile or login window"),
                    ("Enter", "connect using the selected source"),
                ]);
            }
            s.keys.push(("r", "rescan browser profiles"));
        }
        Phase::Confirm => {
            s.title = "Confirm verified Reddit account".into();
            s.keys.push(("Enter", "save this verified connection"));
        }
        Phase::DisconnectConfirm => {
            s.title = "Confirm removal from redtab".into();
            s.keys.push(("Enter", "remove the named saved connection"));
        }
        _ => {}
    }
    if auth.can_remove() {
        s.keys.push(("d", "remove the selected saved login"));
    }
    if !matches!(auth.phase, Phase::Saving | Phase::Disconnecting) {
        s.keys.push(("Esc q", "cancel / return"));
    }
    s
}

fn input_section(form: &Form) -> Section {
    let title: String = form.title.chars().filter(|c| !c.is_control()).collect();
    let mut s = section(
        format!("Text entry · {title}"),
        &[
            ("C-s", form.submit_label()),
            ("Left Right", "move the text cursor"),
            ("Home End / C-a/e", "start / end of the line"),
            ("Backspace Delete", "delete before / after the cursor"),
            ("C-u", "clear the current field"),
            ("Alt-Backspace", "delete the previous word"),
            ("Esc", "back to the previous screen"),
        ],
    );
    if form.fields.len() > 1 {
        s.keys.insert(1, ("Tab / S-Tab", "next / previous field"));
    }
    let multiline = form.fields.get(form.active).is_some_and(|f| f.multiline);
    s.keys.insert(
        1,
        (
            "Enter",
            if multiline {
                "insert a new line"
            } else {
                if form.active + 1 < form.fields.len() {
                    "next field"
                } else {
                    form.submit_label()
                }
            },
        ),
    );
    s
}

fn browse_section(
    query: &Query,
    page: &crate::reddit::experience::Page,
    selected: usize,
) -> Section {
    let mut s = section(
        "Reddit collection",
        &[
            ("j k / Down Up", "select an item / scroll empty-page text"),
            ("C-d/u · PgDn/PgUp", "scroll the selected content"),
            ("C-e/y", "scroll the selected content one line"),
            ("r", "refresh this collection"),
            ("Esc q", "back"),
        ],
    );
    if !page.entries.is_empty() {
        s.keys.extend([
            ("Enter", "open the selected item"),
            ("g G", "first / last item"),
        ]);
    }
    if page
        .entries
        .get(selected)
        .is_some_and(|entry| entry.target.is_some())
    {
        s.keys.push((";", "actions for the selected item"));
    }
    if page.after.is_some() {
        s.keys.push(("L", "load the next page"));
    }
    match query {
        Query::Multi { .. } => {
            s.title = "Custom feed".into();
            s.keys.splice(
                2..2,
                [("a d x", "add community · remove community · delete feed")],
            );
        }
        Query::Search { kind, .. } => {
            s.title = "Reddit search results".into();
            if *kind == SearchKind::Posts {
                s.keys.push(("s", "cycle search sort"));
            }
            s.keys.push(("t", "cycle search time range"));
        }
        _ => {}
    }
    s
}

fn experience_section(app: &App) -> Section {
    if app.experience.writing() {
        return section(
            "Sending to Reddit · wait for confirmation",
            &[("C-o", "open the corresponding Reddit page")],
        );
    }
    if app.experience.pending.is_some() {
        return section(
            "Loading from Reddit",
            &[
                ("Esc q", "back / cancel this read"),
                ("C-o", "open the corresponding Reddit page"),
            ],
        );
    }
    let mut s = match app.experience.screens.last() {
        Some(Screen::Input(form)) => input_section(form),
        Some(Screen::Confirm { .. }) => section(
            "Confirm Reddit action",
            &[("Enter", "confirm this action"), ("Esc q", "cancel")],
        ),
        Some(Screen::Loading(_)) => section(
            "Reddit request stopped",
            &[("r", "retry the request"), ("Esc q", "back")],
        ),
        Some(Screen::Browse {
            query,
            page,
            selected,
            ..
        }) => browse_section(query, page, *selected),
        _ => section(
            "Reddit menu / actions",
            &[
                ("j k / Down Up", "select an action"),
                ("g G", "first / last action"),
                ("Enter Right", "open the selected action"),
                ("Esc q", "back"),
            ],
        ),
    };
    s.keys.push(("C-o", "open the corresponding Reddit page"));
    s
}

pub(super) fn entries(app: &App) -> Vec<(String, String)> {
    action_section(app)
        .into_iter()
        .chain(view_sections(app))
        .flat_map(|s| {
            std::iter::once((s.title, String::new()))
                .chain(s.keys.into_iter().map(|(k, v)| (k.into(), v.into())))
        })
        .collect()
}

fn lines(entries: Vec<(String, String)>, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (key, text) in entries {
        if text.is_empty() {
            lines.extend(
                textwrap::wrap(&key, width.max(1))
                    .into_iter()
                    .map(|part| Line::styled(part.into_owned(), Style::new().fg(ACCENT).bold())),
            );
        } else if width >= 42 {
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
