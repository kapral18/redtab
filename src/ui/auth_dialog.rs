use super::*;

pub(super) fn draw_auth(f: &mut Frame, app: &App) -> Rect {
    use crate::auth::Phase;
    let auth = &app.auth;
    let available = popup(f.area(), 76, 22);
    let block = popup_block(
        if matches!(auth.phase, Phase::DisconnectConfirm | Phase::Disconnecting) {
            "Remove from redtab"
        } else if matches!(
            auth.phase,
            Phase::Accounts | Phase::LoadingAccounts | Phase::Switching
        ) {
            "Accounts"
        } else {
            "Connect Reddit"
        },
        available.width,
    );
    let available_inner = block.inner(available);
    let measured = auth_lines(
        app,
        usize::from(available_inner.width).max(1),
        usize::from(available_inner.height),
    );
    let area = popup(
        f.area(),
        76,
        measured.len().min(u16::MAX as usize - 2) as u16 + 2,
    );
    let inner = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    let lines = auth_lines(
        app,
        usize::from(inner.width).max(1),
        usize::from(inner.height),
    );
    f.render_widget(Paragraph::new(lines).style(Style::new().fg(TEXT)), inner);
    area
}

pub(super) fn auth_lines(app: &App, width: usize, height: usize) -> Vec<Line<'static>> {
    use crate::auth::Phase;
    let auth = &app.auth;
    let wrapped = |text: &str, color| -> Vec<Line<'static>> {
        textwrap::wrap(text, width)
            .into_iter()
            .map(|row| Line::styled(row.into_owned(), Style::new().fg(color)))
            .collect()
    };
    let primary_controls = enter_escape(width);
    // Keep primary action/cancel on one row before adding optional guidance.
    let fit_controls = |full: Vec<Line<'static>>, compact: &[&str], reserve: usize| {
        let budget = height.saturating_sub(reserve);
        if full.len() <= budget {
            return full;
        }
        let mut rows = Vec::new();
        for text in compact {
            let next = wrapped(text, TEXT);
            if rows.len() + next.len() <= budget {
                rows.extend(next);
            }
        }
        rows
    };
    let target = |label: &str, name: &str, color| {
        let wrap = |text: &str| identity_lines(text, width, color);
        let rows = height.saturating_sub(1);
        let mut identity = wrap(label);
        if identity.len() > rows {
            identity = wrap(name);
        }
        if identity.len() > rows {
            identity.truncate(rows);
            if let Some(last) = identity.last_mut() {
                *last = Line::styled(truncate(&format!("{last}…"), width), last.style);
            }
        }
        identity
    };
    let account_status = |index: usize| {
        auth.accounts.get(index).map_or(("", TEXT), |account| {
            let active = auth.active_id.as_ref() == Some(&account.id);
            let status = match (active, account.expired) {
                (true, true) => "[active, expired]",
                (true, false) => "[active]",
                (false, true) => "[expired]",
                (false, false) => "",
            };
            (status, if account.expired { YELLOW } else { TEAL })
        })
    };
    let status_fits = |label: &str, status: &str| {
        status.is_empty() || 3 + label.width() + status.width() <= width
    };
    let mut lines = auth
        .message
        .as_deref()
        .map(|message| wrapped(message, YELLOW))
        .unwrap_or_default();
    let mut controls = Vec::new();
    let mut target_rows = 1;
    match auth.phase {
        Phase::Accounts => {
            let saved = auth.selected < auth.accounts.len();
            let status_rows = auth
                .accounts
                .iter()
                .enumerate()
                .map(|(index, account)| {
                    let (status, color) = account_status(index);
                    let rows = wrapped(status.trim_matches(['[', ']']), color).len();
                    if !status_fits(&account.name, status) && rows + 2 <= height {
                        rows
                    } else {
                        0
                    }
                })
                .max()
                .unwrap_or(0);
            let footers = [true, false].map(|saved| {
                let mut footer = wrapped(
                    if saved {
                        "Enter selects · j/k choose · a Add account"
                    } else {
                        "Enter adds · j/k choose · a Add account"
                    },
                    TEXT,
                );
                footer.extend(wrapped(
                    if saved {
                        "r Reconnect selected · Esc close"
                    } else {
                        "r Rescan browsers · Esc close"
                    },
                    TEXT,
                ));
                if !auth.accounts.is_empty() {
                    let removal = wrapped("d Remove from redtab", TEXT);
                    footer.extend(if saved {
                        removal
                    } else {
                        vec![Line::default(); removal.len()]
                    });
                }
                footer
            });
            let full_rows = footers.iter().map(Vec::len).max().unwrap();
            let footers = footers
                .into_iter()
                .zip([true, false])
                .map(|(mut footer, saved)| {
                    footer.resize(full_rows, Line::default());
                    let mut compact = vec![
                        primary_controls,
                        "F1 help",
                        "j/k choose",
                        if saved { "r reconnect" } else { "r rescan" },
                        "a add",
                    ];
                    if saved && !auth.accounts.is_empty() {
                        compact.push("d remove");
                    }
                    fit_controls(footer, &compact, 1 + status_rows)
                })
                .collect::<Vec<_>>();
            let footer_rows = footers.iter().map(Vec::len).max().unwrap();
            let mut footer = footers[usize::from(!saved)].clone();
            footer.resize(footer_rows, Line::default());
            let reserved = footer.len() + 1 + status_rows;
            lines.truncate(height.saturating_sub(reserved));
            let guidance = wrapped("Choose a saved account or add another.", TEXT);
            if lines.is_empty() && guidance.len() <= height.saturating_sub(reserved) {
                lines.extend(guidance);
            }
            let visible = height.saturating_sub(lines.len() + footer.len() + status_rows);
            let start = auth.selected.saturating_sub(visible.saturating_sub(1));
            for index in (start..auth.choices()).take(visible) {
                let selected = index == auth.selected;
                let style = choice_style(selected);
                let label = auth
                    .accounts
                    .get(index)
                    .map_or("+ Add account", |account| account.name.as_str());
                let (status, color) = account_status(index);
                let status = if status_fits(label, status) {
                    status
                } else {
                    ""
                };
                let mut spans = vec![Span::styled(
                    format!("{} ", if selected { "▌" } else { " " }),
                    style,
                )];
                let room =
                    width.saturating_sub(2 + status.width() + usize::from(!status.is_empty()));
                spans.push(Span::styled(truncate(label, room), style));
                pad_to(&mut spans, width.saturating_sub(status.width()), style);
                if !status.is_empty() {
                    spans.push(Span::styled(status, style.fg(color)));
                }
                lines.push(Line::from(spans));
            }
            let (status, color) = account_status(auth.selected);
            let status = if auth
                .accounts
                .get(auth.selected)
                .is_some_and(|account| !status_fits(&account.name, status))
            {
                status.trim_matches(['[', ']'])
            } else {
                ""
            };
            let mut status_lines = wrapped(status, color);
            status_lines.truncate(status_rows);
            status_lines.resize(status_rows, Line::default());
            lines.extend(status_lines);
            lines.extend(footer);
        }
        Phase::LoadingAccounts => {
            lines.extend(loading::wrapped(
                loading::Kind::Initial,
                "Loading saved accounts…",
                width,
                app.loading_frame,
            ));
            controls = wrapped("Esc cancels", TEXT);
        }
        Phase::Switching => {
            lines.extend(loading::wrapped(
                loading::Kind::Action,
                "Verifying account…",
                width,
                app.loading_frame,
            ));
            lines.extend(wrapped(
                "A failed switch leaves your connection unchanged.",
                SUBTEXT,
            ));
            controls = wrapped("Esc cancels", TEXT);
        }
        Phase::Idle | Phase::Choose => {
            let mut footer = wrapped(
                if auth.sources.is_empty() {
                    "r rescan · Esc cancel"
                } else {
                    "Enter connects · j/k choose · r rescan · Esc cancel"
                },
                TEXT,
            );
            if auth.active_id.is_some() {
                footer.extend(wrapped("d Remove from redtab", TEXT));
            }
            let disclosure = wrapped("Only Reddit cookies; OS access may prompt.", SUBTEXT);
            if lines.len() + footer.len() + disclosure.len() + 2 <= height {
                footer.extend(disclosure);
            }
            let compact = if auth.sources.is_empty() {
                vec!["r · Esc", "F1 help"]
            } else {
                vec![primary_controls, "F1 help", "j/k choose", "r rescan"]
            };
            footer = fit_controls(footer, &compact, usize::from(!auth.sources.is_empty()));
            // Errors and controls stay visible; labels occupy one row each.
            lines.truncate(
                height.saturating_sub(footer.len() + usize::from(!auth.sources.is_empty())),
            );
            if lines.is_empty() && height > footer.len() + 1 {
                let guidance = wrapped("Choose a profile or a new login window.", TEXT);
                if guidance.len() < height.saturating_sub(footer.len()) {
                    lines.extend(guidance);
                }
            }
            let visible = height.saturating_sub(lines.len() + footer.len()).max(1);
            let start = auth.selected.saturating_sub(visible.saturating_sub(1));
            for (index, source) in auth.sources.iter().enumerate().skip(start).take(visible) {
                let selected = index == auth.selected;
                let style = choice_style(selected);
                let mut spans = vec![Span::styled(
                    truncate(
                        &format!("{} {}", if selected { "▌" } else { " " }, source.label()),
                        width,
                    ),
                    style,
                )];
                pad_to(&mut spans, width, style);
                lines.push(Line::from(spans));
            }
            lines.extend(footer);
        }
        Phase::Discovering => {
            lines.extend(loading::wrapped(
                loading::Kind::Action,
                "Finding browser profiles…",
                width,
                app.loading_frame,
            ));
            controls = wrapped(
                if auth.can_remove() {
                    "d Remove from redtab · Esc cancels"
                } else {
                    "Esc cancels"
                },
                TEXT,
            );
        }
        Phase::Importing => {
            let window = matches!(
                auth.sources.get(auth.selected),
                Some(crate::auth::Source::Window { .. })
            );
            lines.extend(loading::wrapped(
                loading::Kind::Action,
                if window {
                    "Sign in to Reddit in the separate browser window…"
                } else {
                    "Reading Reddit session and verifying account…"
                },
                width,
                app.loading_frame,
            ));
            lines.extend(wrapped("Allow OS access if prompted.", TEXT));
            controls = wrapped("Esc cancels", TEXT);
            if auth.can_remove() {
                controls.extend(wrapped("d Remove from redtab", TEXT));
            }
        }
        Phase::Confirm => {
            let name = format!("u/{}", auth.account_name().unwrap_or_default());
            let identity = target(&format!("Verified {name}"), &name, GREEN);
            let message = std::mem::take(&mut lines);
            target_rows = identity.len();
            lines = identity;
            lines.extend(message);
            controls = wrapped(
                "Enter saves this connection · Esc keeps the current account",
                TEXT,
            );
        }
        Phase::Saving => lines.extend(loading::wrapped(
            loading::Kind::Action,
            "Saving account…",
            width,
            app.loading_frame,
        )),
        Phase::DisconnectConfirm => {
            let who = auth
                .removal_name
                .as_deref()
                .unwrap_or("the current account");
            let identity = target(&format!("Remove {who} from redtab?"), who, YELLOW);
            let message = std::mem::take(&mut lines);
            lines = identity;
            target_rows = lines.len();
            lines.extend(message);
            lines.extend(wrapped(
                if auth.removing_current {
                    "Remove its saved session and disconnect redtab."
                } else {
                    "Remove only this saved session."
                },
                TEXT,
            ));
            lines.extend(wrapped(
                "Other accounts, bookmarks and history are kept.",
                SUBTEXT,
            ));
            lines.extend(wrapped("Your browser session is unchanged.", SUBTEXT));
            controls = wrapped("Enter removes · Esc cancels", TEXT);
        }
        Phase::Disconnecting => lines.extend(loading::wrapped(
            loading::Kind::Action,
            "Removing saved session…",
            width,
            app.loading_frame,
        )),
    }
    let mut compact = if matches!(auth.phase, Phase::Confirm | Phase::DisconnectConfirm) {
        vec![primary_controls, "F1 help"]
    } else {
        vec!["Esc", "F1 help"]
    };
    if auth.can_remove() && matches!(auth.phase, Phase::Discovering | Phase::Importing) {
        compact.push("d remove");
    }
    controls = fit_controls(
        controls,
        &compact,
        target_rows.min(height.saturating_sub(1)).min(lines.len()),
    );
    lines.truncate(height.saturating_sub(controls.len()));
    lines.extend(controls);
    lines
}
