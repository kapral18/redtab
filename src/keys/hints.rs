use super::*;

pub fn footer_hints(app: &App) -> Vec<(String, String)> {
    use map::{Action::*, Target};
    if app.help_scroll.is_some() {
        return vec![
            ("j/k".into(), "scroll help".into()),
            ("C-f/b".into(), "page".into()),
            ("Esc".into(), "back".into()),
        ];
    }
    if app.keymap.hint_visible() {
        return vec![
            ("Esc".into(), "cancel prefix".into()),
            ("PgUp/Dn".into(), "scroll keys".into()),
        ];
    }
    let candidates = map::candidates(app);
    let mut result = Vec::new();
    let context = map::context(app);
    if app.active == crate::app::SEARCH_TAB
        && app.overlay.is_none()
        && matches!(context, map::Context::Input | map::Context::List)
    {
        if app.search_editing() {
            return [("↑↓", "choose"), ("Tab", "kind"), ("⏎/Esc", "results")]
                .map(|(key, label)| (key.to_string(), label.to_string()))
                .to_vec();
        }
        result.push(("/".into(), "type".into()));
        if map::search_kind_keys(app) {
            result.push(("Tab".into(), "kind".into()));
        }
    }
    let priorities = [
        (
            Pause,
            if app.selected_video().is_some_and(|player| !player.paused()) {
                "pause"
            } else {
                "play"
            },
        ),
        (Mute, "sound"),
        (Fold, map::fold_label(app).unwrap_or("fold")),
        (
            Down,
            if context == map::Context::Sidebar {
                "tabs"
            } else if context == map::Context::Post {
                "comments"
            } else {
                "move"
            },
        ),
        (
            Right,
            if context == map::Context::Sidebar {
                "list"
            } else {
                "open"
            },
        ),
        (
            NextCommunity,
            if app.search_source_context() {
                "next source"
            } else {
                "next community"
            },
        ),
        (NextThread, "next root"),
        (NextPost, "next post"),
        (
            Membership,
            match app
                .membership_target()
                .as_ref()
                .map(|target| (target, app.subscribed(target)))
            {
                Some((crate::model::MembershipTarget::User(_), Some(true))) => "unfollow",
                Some((crate::model::MembershipTarget::User(_), Some(false))) => "follow",
                Some((_, Some(true))) => "leave",
                Some((_, Some(false))) => "join",
                _ => "check membership",
            },
        ),
        (
            Bookmark,
            if app
                .current_post()
                .is_some_and(|post| app.state.is_bookmarked(&post.id))
            {
                "unbookmark"
            } else {
                "bookmark"
            },
        ),
        (Time, "time"),
        (CloseTab, "close tab"),
        (OpenLinks, "links"),
        (Community, "source"),
        (Spoilers, "spoilers"),
        (Actions, "actions"),
        (Sort, "sort"),
        (Media, "media"),
        (CopyLink, "yank"),
        (Jump, "buffers"),
        (ListView, "view"),
        (Left, "back"),
    ];
    // Reading a post, the way out outranks the motions that stay in it.
    let mut priorities = priorities.to_vec();
    if context == map::Context::Post
        && let Some(at) = priorities.iter().position(|(action, _)| *action == Left)
    {
        let back = priorities.remove(at);
        let after = priorities
            .iter()
            .position(|(action, _)| *action == Right)
            .map_or(0, |right| right + 1);
        priorities.insert(after, back);
    }
    for (action, description) in priorities {
        if action == CloseTab
            && !matches!(
                app.active_tab().kind,
                TabKind::Subreddit { pinned: false, .. }
                    | TabKind::User {
                        followed: false,
                        ..
                    }
            )
        {
            continue;
        }
        if candidates
            .iter()
            .any(|candidate| matches!(candidate.target, Target::Action(actual) if actual == action))
        {
            let key = if action == Down {
                format!("{}/{}", map::action_label(Down), map::action_label(Up))
            } else {
                map::action_label(action)
            };
            result.push((key, description.into()));
        }
    }
    if candidates
        .iter()
        .any(|candidate| candidate.keys.starts_with(' '))
    {
        result.push(("SPC".into(), "actions".into()));
    }
    result
}
