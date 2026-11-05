use super::*;

pub(super) fn footer(app: &App, screen: &Screen, width: usize, loader_visible: bool) -> String {
    let can_open = crate::keys::map::available(app, crate::keys::map::Action::Right);
    let loading_more = app.experience.loading_more();
    let failed_read = app.experience.error().is_some()
        && matches!(screen, Screen::Browse { .. } | Screen::Loading(_));
    let scroll_controls = if let Screen::Browse {
        query,
        page,
        selected,
        ..
    } = screen
        && (loading_more
            || page.entries.is_empty()
            || (!matches!(query, crate::reddit::experience::Query::Thread(_))
                && !page.text.is_empty())
            || !preview_text(query, page, *selected).is_empty())
    {
        Some(if !can_open || loading_more {
            fitting_label(
                &[
                    "Ctrl+d/u scroll · Ctrl+e/y line · Esc back",
                    "C-d/u scroll · C-e/y line · Esc",
                    "C-e/y scroll · Esc",
                    "C-e/y · Esc",
                    "^e/y ⎋",
                    "⎋",
                ],
                width,
            )
        } else {
            fitting_label(
                &[
                    "Enter · C-d/u scroll · C-e/y line · Esc",
                    "⏎ · C-d/u C-e/y scroll · Esc",
                    "⏎ · C-e/y scroll · Esc",
                    "⏎ · C-e/y · Esc",
                    "C-e/y · Esc",
                    "^e/y ⎋",
                    "⎋",
                ],
                width,
            )
        })
    } else {
        None
    };
    let submit = if let Screen::Input(form) = screen {
        let key = if form
            .fields
            .get(form.active)
            .is_some_and(|field| !field.multiline)
            && form.active + 1 == form.fields.len()
        {
            "Enter"
        } else {
            "Ctrl+s"
        };
        Some(format!("{key} {}", form.submit_label()))
    } else {
        None
    };
    if matches!(screen, Screen::Browse { page, pagination, .. } if pagination.has_new_entries(page))
    {
        return fitting_label(
            &[
                "New items · r refresh · Enter open · Esc back",
                "New items · r refresh",
                "r refresh",
                "r",
            ],
            width,
        )
        .into();
    }
    if width < 28 {
        let (hint, short) = if app.experience.writing() {
            ("Sending…".into(), "Sending…")
        } else if failed_read {
            (
                "r retry · Esc".into(),
                fitting_label(&["r retry · Esc", "r · Esc", "r Esc", "⎋"], width),
            )
        } else if app.experience.pending.is_some()
            && !loading_more
            && !app.experience.refreshing
            && loader_visible
        {
            (
                "Esc back".into(),
                fitting_label(&["Esc back", "Esc", "⎋"], width),
            )
        } else if app.experience.pending.is_some() && !loading_more && !app.experience.refreshing {
            (
                "Loading · Esc".into(),
                fitting_label(&["… · Esc", "… Esc", "…⎋", "⎋"], width),
            )
        } else if let Some(controls) = scroll_controls {
            (controls.into(), controls)
        } else if let Some(submit) = &submit {
            (
                format!("{} · Esc", submit.replace("Ctrl+s", "C-s")),
                if submit.starts_with("Ctrl+s") {
                    fitting_label(&["C-s · Esc", "^S Esc", "^S⎋", "⎋"], width)
                } else {
                    enter_escape(width)
                },
            )
        } else if matches!(screen, Screen::Loading(_)) {
            (
                "r retry · Esc".into(),
                fitting_label(&["r · Esc", "r Esc", "r⎋", "⎋"], width),
            )
        } else if can_open || matches!(screen, Screen::Confirm { .. }) {
            ("Enter · Esc".into(), enter_escape(width))
        } else {
            (
                "Esc back".into(),
                fitting_label(&["Esc back", "Esc", "⎋"], width),
            )
        };
        return if hint.width() <= width {
            hint
        } else {
            truncate(short, width)
        };
    }
    if app.experience.writing() {
        return "Sending… wait for confirmation".into();
    }
    let mut parts: Vec<&str> = if failed_read {
        vec!["r retry", "Esc back"]
    } else if app.experience.pending.is_some() && !loading_more && !app.experience.refreshing {
        if loader_visible {
            vec!["Esc back"]
        } else {
            vec!["Loading…", "Esc back"]
        }
    } else {
        match screen {
            Screen::Input(_) => vec![submit.as_deref().unwrap_or_default(), "Esc back"],
            Screen::Confirm { .. } => vec!["Enter confirm", "Esc cancel"],
            Screen::Loading(_) => vec!["r retry", "Esc back"],
            Screen::Menu { .. } => vec!["Enter select", "Esc back"],
            Screen::Browse { .. } => {
                if let Some(controls) = scroll_controls {
                    vec![controls]
                } else if can_open {
                    vec!["Enter open", "Esc back"]
                } else {
                    vec!["Esc back"]
                }
            }
        }
    };
    if parts.join(" · ").width() + 10 <= width {
        parts.push("F1 help");
    }
    let mut extras = vec!["Ctrl+o Reddit"];
    match screen {
        Screen::Input(form) if form.fields.len() > 1 => extras.insert(0, "Tab field"),
        Screen::Menu { .. } => extras.insert(0, "j/k move"),
        Screen::Browse {
            query,
            page,
            selected,
            ..
        } if !loading_more => {
            if page
                .entries
                .get(*selected)
                .is_some_and(|entry| entry.target.is_some())
            {
                extras.insert(0, "SPC a actions");
            }
            if matches!(query, crate::reddit::experience::Query::Multi { .. }) {
                extras.extend(["a add", "d remove", "D delete"]);
            }
        }
        _ => {}
    }
    for extra in extras {
        if parts.join(" · ").width() + 3 + extra.width() <= width {
            parts.push(extra);
        }
    }
    parts.join(" · ")
}
