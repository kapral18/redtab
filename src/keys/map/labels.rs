use super::*;

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

/// Tab and Shift+Tab switch the search kind anywhere inside Search: its
/// results, a post opened from them, and a feed opened beside them. The sidebar keeps
/// buffer switching.
pub fn search_kind_keys(app: &App) -> bool {
    app.overlay.is_none()
        && app.focus != Focus::Sidebar
        && (app.active == crate::app::SEARCH_TAB || app.search_feed_open())
        && (app.search_editing() || !app.ancestor_following())
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

pub(super) fn description(app: &App, binding: &Binding) -> &'static str {
    use Action::*;
    match binding.action {
        Down if context(app) == Context::Post => "Next comment / scroll down",
        Up if context(app) == Context::Post => "Previous comment / scroll up",
        Down if context(app) == Context::Sidebar => "Next sidebar tab",
        Up if context(app) == Context::Sidebar => "Previous sidebar tab",
        LineDown | LineUp if line_moves_items(app) => {
            if binding.action == LineDown {
                "Next item"
            } else {
                "Previous item"
            }
        }
        NextCommunity if app.search_source_context() => "Next Search source; return to results",
        PreviousCommunity if app.search_source_context() => {
            "Previous Search source; return to results"
        }
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

pub fn prefix_name(prefix: &str) -> &'static str {
    match prefix {
        " " => "Actions",
        " b" => "Buffers",
        " f" => "Feed",
        "g" => "Go",
        " o" => "Open",
        " p" => "Preferences",
        " s" => "Search",
        "[" => "Previous",
        "]" => "Next",
        "z" => "Folds and view",
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
