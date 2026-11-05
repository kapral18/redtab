use super::*;

pub(super) fn draw_all_subs(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    spots: &mut Vec<ImageSpot>,
    sidebar_icons: &[String],
) -> Option<Rect> {
    if app.all_subs.groups.is_empty() {
        app.groups_drawn(&[], sidebar_icons, &[]);
    }
    let total_new: usize = app
        .all_subs
        .groups
        .iter()
        .filter(|group| {
            group
                .error
                .as_ref()
                .and_then(crate::worker::FeedError::access)
                .is_none()
        })
        .map(|g| match app.community_badge(&g.subreddit) {
            UnreadBadge::Count(count) => count,
            _ => 0,
        })
        .sum();
    let unknown = app
        .all_subs
        .groups
        .iter()
        .filter(|group| app.community_badge(&group.subreddit) == UnreadBadge::Unknown)
        .count();
    let loading = app.all_subs.groups.iter().filter(|g| g.loading).count();
    let mut chips = vec![chip(
        match app.all_subs.groups.len() {
            1 => "1 subreddit".to_string(),
            n => format!("{n} subreddits"),
        },
        SUBTEXT,
        SURFACE0,
    )];
    if app.all_subs.has_new_posts(&app.config) {
        chips.insert(0, new_posts_chip(app));
    }
    if total_new > 0 {
        chips.push(chip(format!("● {total_new} unread"), CRUST, ACCENT));
    }
    if unknown > 0 {
        chips.push(chip(format!("{unknown} unread unknown"), OVERLAY, SURFACE0));
    }
    if loading > 0 {
        chips.push(chip(
            format!("{} loading {loading}", loading::glyph(app.loading_frame)),
            OVERLAY,
            SURFACE0,
        ));
    }
    if app.all_subs.loading && !app.all_subs.groups.is_empty() {
        chips.push(chip(
            format!(
                "{} syncing subscriptions",
                loading::glyph(app.loading_frame)
            ),
            OVERLAY,
            SURFACE0,
        ));
    }
    if app.all_subs.error.is_some() {
        chips.splice(
            0..0,
            [
                chip("✗ subs failed", RED, SURFACE0),
                chip(
                    format!("{} retry", search::list_reading_key(app, "r")),
                    RED,
                    SURFACE0,
                ),
            ],
        );
    }
    if let Some(freshness) = app.all_subs.freshness {
        chips.push(chip(
            freshness.label(
                crate::app::now(),
                app.all_subs.loading,
                app.all_subs.error.is_some(),
                app.health == Health::Down,
            ),
            OVERLAY,
            SURFACE0,
        ));
    }
    let body = view_header(f, area, "▦ All subs", chips);
    if app.reddit.needs_sign_in() && app.all_subs.groups.is_empty() {
        empty_state(
            f,
            body,
            "○",
            "Connect Reddit",
            "Press SPC p a to connect your account",
        );
        return None;
    }
    if app.all_subs.groups.is_empty() {
        let (title, hint) = if let Some(error) = &app.all_subs.error {
            (
                "Could not load subscriptions",
                format!(
                    "{error} — press {} to retry",
                    search::list_reading_key(app, "r")
                ),
            )
        } else if app.all_subs.loading {
            return loading::draw(
                f,
                body,
                loading::Kind::Initial,
                "Loading subscriptions",
                app.loading_frame,
            );
        } else {
            (
                "No joined communities",
                format!(
                    "join communities on Reddit, then press {}; SPC b b opens any subreddit",
                    search::list_reading_key(app, "r")
                ),
            )
        };
        empty_state(f, body, "▦", title, &hint);
        return None;
    }
    let cols = (body.width / CARD_MIN_WIDTH).clamp(1, 3) as usize;
    let row_height = card_height(app) + CARD_ROW_GAP;
    let rows = app.all_subs.groups.len().div_ceil(cols);
    let selected_row = app.all_subs.group / cols;
    let selected = &app.all_subs.groups[app.all_subs.group].subreddit;
    let offset = match &app.all_subs.drawn {
        Some((name, above)) if name == selected => selected_row.saturating_sub(*above),
        _ => app.all_subs.offset.min(selected_row),
    };
    let visible_rows =
        ((usize::from(body.height) + usize::from(CARD_ROW_GAP)) / usize::from(row_height)).max(1);
    let offset = offset.max((selected_row + 1).saturating_sub(visible_rows));
    app.all_subs.offset = offset;
    app.all_subs.drawn = Some((selected.clone(), selected_row - offset));

    let bottom = body.y + body.height;
    let mut y = body.y;
    let mut shown = 0;
    let mut visible = Vec::new();
    for row_index in offset..rows {
        let entries = (app.all_subs.groups.len() - row_index * cols).min(cols);
        let card = row_height - CARD_ROW_GAP;
        if y >= bottom {
            break;
        }
        let row_area = Rect {
            x: body.x,
            y,
            // Keep the last column free for the scrollbar.
            width: body.width.saturating_sub(2),
            height: card.min(bottom - y),
        };
        let cells = Layout::horizontal(vec![Constraint::Ratio(1, cols as u32); cols])
            .spacing(2)
            .split(row_area);
        for (column, cell) in cells.iter().take(entries).enumerate() {
            visible.push((row_index * cols + column, *cell));
        }
        y = y.saturating_add(row_height);
        shown += 1;
    }
    let lookahead = visible.last().map_or_else(Vec::new, |(index, _)| {
        let start = (index / cols + 1) * cols;
        (start..(start + cols).min(app.all_subs.groups.len())).collect::<Vec<_>>()
    });
    app.groups_drawn(
        &visible.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
        sidebar_icons,
        &lookahead,
    );
    for (index, cell) in visible {
        draw_group_card(f, app, index, cell, spots);
    }
    draw_scrollbar(f, body, offset, shown.max(1), rows);
    None
}

pub(super) fn draw_group_card(
    f: &mut Frame,
    app: &App,
    index: usize,
    area: Rect,
    spots: &mut Vec<ImageSpot>,
) {
    let group = &app.all_subs.groups[index];
    let selected_card = index == app.all_subs.group;
    let access = group
        .error
        .as_ref()
        .and_then(crate::worker::FeedError::access);
    let badge = app.community_badge(&group.subreddit);
    let focused = selected_card && app.focus == Focus::List;
    let border = if focused {
        Style::new().fg(ACCENT)
    } else if selected_card {
        Style::new().fg(OVERLAY)
    } else {
        Style::new().fg(SURFACE1)
    };
    let icon = app.shown_icon(&group.subreddit);
    // Reserve the icon and its gap so the name stays put when graphics load.
    if let Some(url) = icon
        && area.width > ICON_COLS + 3
    {
        spots.push(ImageSpot {
            url: url.to_string(),
            area: Rect {
                x: area.x + 2,
                y: area.y,
                width: ICON_COLS,
                height: 1,
            },
        });
    }
    let mut title = vec![
        Span::styled(
            if icon.is_some() { "    " } else { " #  " },
            Style::new().fg(if selected_card { ACCENT } else { OVERLAY }),
        ),
        Span::styled(
            format!("{} ", group.subreddit),
            Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
        ),
    ];
    if access.is_none()
        && let Some(badge) = unread_chip(badge)
    {
        title.push(badge);
        title.push(Span::raw(" "));
    }
    if let Some(freshness) = group.freshness.filter(|_| access.is_none()) {
        title.push(Span::styled(
            freshness.label(
                crate::app::now(),
                group.loading,
                group.error.is_some(),
                app.health == Health::Down,
            ),
            Style::new().fg(OVERLAY),
        ));
    }
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(if focused {
            BorderType::Double
        } else {
            BorderType::Rounded
        })
        .border_style(border)
        .title(Line::from(fit_spans(
            title,
            usize::from(area.width.saturating_sub(2)),
        )))
        .padding(Padding {
            left: PANEL_PAD,
            right: PANEL_PAD,
            top: PANEL_PAD,
            bottom: PANEL_PAD,
        });
    if access.is_some() && focused {
        block = block.title_bottom(Line::styled(" o Reddit ", Style::new().fg(YELLOW)));
    } else if group.error.is_some() && focused {
        block = block.title_bottom(Line::styled(" ✗ r retry ", Style::new().fg(RED)));
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    let width = inner.width as usize;
    let posts: Vec<_> = app.group_posts(group).collect();
    // (line in the card, URL) for each post row with a thumbnail.
    let mut thumbs: Vec<(u16, std::borrow::Cow<'_, str>)> = Vec::new();
    let lines: Vec<Line> = if access.is_some() {
        let title = if width < 24 {
            "Private"
        } else {
            "Private community"
        };
        vec![
            Line::styled(format!(" 🔒 {title}"), Style::new().fg(YELLOW)),
            Line::styled(" Approved members only", Style::new().fg(OVERLAY)),
        ]
    } else if group.posts.is_empty()
        && (group.loading || (group.freshness.is_none() && group.error.is_none()))
    {
        loading::draw(
            f,
            inner,
            loading::Kind::Initial,
            "Loading posts",
            app.loading_frame,
        );
        return;
    } else if let Some(e) = group.error.as_ref().filter(|_| group.posts.is_empty()) {
        vec![Line::styled(
            format!("   ✗ {}", truncate(e.as_str(), width.saturating_sub(5))),
            Style::new().fg(RED),
        )]
    } else if group.posts.is_empty() && !app.config.show_nsfw {
        vec![
            Line::styled(
                fitting_label(
                    &[
                        " No visible posts · NSFW off",
                        " NSFW off · no posts",
                        " NSFW off",
                    ],
                    width,
                ),
                Style::new().fg(OVERLAY),
            ),
            Line::styled(
                fitting_label(
                    &[
                        "F2 Settings: enable Show NSFW",
                        "F2: enable Show NSFW",
                        "F2: Show NSFW",
                    ],
                    width,
                ),
                Style::new().fg(OVERLAY),
            ),
        ]
    } else if posts.is_empty() {
        // Non-pinned posts exist, so the config hid them.
        let text = if group.posts.iter().any(|p| !p.stickied) {
            "   all posts hidden by your config"
        } else {
            "   no posts"
        };
        vec![Line::styled(text, Style::new().fg(OVERLAY))]
    } else {
        let reserve = thumb_reserve(app, posts.iter().copied());
        let mut lines = Vec::new();
        for (i, p) in posts.iter().enumerate() {
            if i > 0 {
                lines.push(Line::raw(""));
            }
            let opts = CardOpts {
                selected: selected_card && i == app.all_subs.post,

                show_subreddit: false,
                reserve,
            };
            if let Some(url) = row_thumb(app, p) {
                thumbs.push((lines.len() as u16, url));
            }
            lines.extend(post_card(app, p, width, &opts));
        }
        lines
    };
    // In a short pane the card is clipped: scroll it so the selected post shows.
    let selected_end = if selected_card && !posts.is_empty() {
        app.all_subs.post as u16 * 3 + 2
    } else {
        0
    };
    let scroll = selected_end.saturating_sub(inner.height);
    // Only thumbnails wholly inside the scrolled card.
    for (line, url) in thumbs {
        if line >= scroll && line - scroll + THUMB_ROWS <= inner.height {
            spots.push(ImageSpot {
                url: url.to_string(),
                area: Rect {
                    x: inner.x + 1,
                    y: inner.y + line - scroll,
                    width: THUMB_COLS,
                    height: THUMB_ROWS,
                },
            });
        }
    }
    f.render_widget(Paragraph::new(lines).scroll((scroll, 0)), inner);
}

pub(super) fn membership_chip(app: &App, target: &crate::model::MembershipTarget) -> Span<'static> {
    use crate::model::MembershipTarget;
    let membership = app.subscribed(target);
    let (status, color) = match (target, membership) {
        (MembershipTarget::User(_), Some(true)) => ("✓ Following", GREEN),
        (MembershipTarget::User(_), Some(false)) => ("Not following", SUBTEXT),
        (MembershipTarget::User(_), None) => ("Following unknown", OVERLAY),
        (_, Some(true)) => ("✓ Joined", GREEN),
        (_, Some(false)) => ("Not joined", SUBTEXT),
        (_, None) => ("Membership unknown", OVERLAY),
    };
    let label = if app.membership_progress().is_some() {
        status.into()
    } else {
        format!(
            "{status} · {} {}",
            search::list_reading_key(app, "SPC m"),
            membership.map_or_else(
                || "check".into(),
                |joined| target.action(!joined).to_ascii_lowercase()
            )
        )
    };
    chip(label, color, SURFACE0)
}
