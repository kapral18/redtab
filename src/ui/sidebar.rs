use super::*;

pub(super) fn tab_icon<'a>(app: &'a App, tab: &'a Tab) -> Option<&'a str> {
    match &tab.kind {
        TabKind::Subreddit { name, .. } => app.shown_icon(name),
        // Following's avatar first; a known profile's stands in for a missing or failed one.
        TabKind::User { name, icon, .. } if app.thumbnails_on() => {
            app.usable_thumb(icon.as_deref()).or_else(|| {
                app.usable_thumb(
                    app.user_profile(name)
                        .and_then(|profile| profile.icon.as_deref()),
                )
            })
        }
        _ => None,
    }
}

pub(super) enum SideRow {
    Section(&'static str),
    /// A section header whose listed tabs are syncing.
    SyncingSection(&'static str),
    Loading,
    Tab(usize),
    Blank,
}

pub(super) struct SidebarLayout {
    pub(super) area: Rect,
    pub(super) brand: Rect,
    pub(super) list: Rect,
    pub(super) status: Rect,
    pub(super) rows: Vec<SideRow>,
}

pub(super) fn sidebar_layout(app: &App, area: Rect) -> SidebarLayout {
    // One more column on the left keeps the row marker off the focus outline.
    let inner = area.inner(Margin {
        horizontal: PANEL_PAD,
        vertical: PANEL_PAD,
    });
    let inner = Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(1),
        ..inner
    };
    let [brand, list, status] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .areas(inner);

    let fixed = [
        SideRow::Tab(0),
        SideRow::Tab(1),
        SideRow::Tab(crate::app::ALL_SUBS_TAB),
        SideRow::Tab(crate::app::SEARCH_TAB),
    ];
    let fixed_active = fixed
        .iter()
        .position(|row| matches!(row, SideRow::Tab(index) if *index == app.active));
    let mut rows = Vec::new();
    let (pinned, following, opened) = app.sidebar_tabs();
    // Placeholders stand in only before the first listing; a resync never adds rows.
    let first_load = app.all_subs.loading && app.all_subs.freshness.is_none();
    let section = |name, empty| match (app.all_subs.loading, empty) {
        (false, _) => vec![SideRow::Section(name)],
        (true, true) => vec![SideRow::Section(name), SideRow::Loading],
        (true, false) => vec![SideRow::SyncingSection(name)],
    };
    if !pinned.is_empty() || first_load {
        rows.push(SideRow::Blank);
        rows.extend(section("SUBSCRIPTIONS", pinned.is_empty()));
        rows.extend(pinned.into_iter().map(SideRow::Tab));
    }
    if !following.is_empty() || first_load {
        rows.push(SideRow::Blank);
        rows.extend(section("FOLLOWING", following.is_empty()));
        rows.extend(following.into_iter().map(SideRow::Tab));
    }
    if !opened.is_empty() {
        rows.push(SideRow::Blank);
        rows.push(SideRow::Section("OPENED"));
        rows.extend(opened.into_iter().map(SideRow::Tab));
    }

    let height = list.height as usize;
    let fixed_height = fixed
        .len()
        .min(height.saturating_sub(usize::from(fixed_active.is_none())));
    let fixed_start = fixed_active.map_or(0, |index| {
        index.saturating_sub(fixed_height.saturating_sub(1))
    });
    let scroll_height = height.saturating_sub(fixed_height);
    let active_row = rows
        .iter()
        .position(|r| matches!(r, SideRow::Tab(i) if *i == app.active))
        .unwrap_or(0);
    let start = if rows.len() <= scroll_height {
        0
    } else {
        active_row
            .saturating_sub(scroll_height / 2)
            .min(rows.len() - scroll_height)
    };
    let rows = if area.width == 0 {
        Vec::new()
    } else {
        fixed
            .into_iter()
            .skip(fixed_start)
            .take(fixed_height)
            .chain(rows.into_iter().skip(start).take(scroll_height))
            .collect()
    };
    SidebarLayout {
        area,
        brand,
        list,
        status,
        rows,
    }
}

pub(super) fn draw_sidebar(
    f: &mut Frame,
    app: &App,
    layout: &SidebarLayout,
    spots: &mut Vec<ImageSpot>,
) {
    let SidebarLayout {
        area,
        brand,
        list,
        status,
        rows,
    } = layout;
    let (area, brand, list, status) = (*area, *brand, *list, *status);
    let panel = Style::new().bg(MANTLE);
    f.render_widget(Block::default().style(panel), area);
    f.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(" ◆ ", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
                Span::styled("redtab", Style::new().fg(TEXT).add_modifier(Modifier::BOLD)),
                Span::styled(
                    format!("  v{}", env!("CARGO_PKG_VERSION")),
                    Style::new().fg(OVERLAY),
                ),
            ]),
            Line::from(if app.search_editing() {
                vec![
                    key_chip("Esc"),
                    Span::styled(" list keys ", Style::new().fg(SUBTEXT)),
                    key_chip("F1"),
                    Span::styled(" help", Style::new().fg(SUBTEXT)),
                ]
            } else {
                vec![
                    key_chip("F3"),
                    Span::styled(" search ", Style::new().fg(SUBTEXT)),
                    key_chip("SPC"),
                    Span::styled(" keys", Style::new().fg(SUBTEXT)),
                ]
            }),
        ])
        .style(panel),
        brand,
    );

    let width = list.width as usize;
    let lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .map(|(k, row)| match row {
            SideRow::Blank => Line::raw(""),
            SideRow::SyncingSection(name) => Line::from(vec![
                Span::styled(
                    format!("  {name} "),
                    Style::new().fg(OVERLAY).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    loading::glyph(app.loading_frame).to_string(),
                    loading::style(loading::Kind::Background),
                ),
            ]),
            SideRow::Section(name) => Line::styled(
                format!("  {name}"),
                Style::new().fg(OVERLAY).add_modifier(Modifier::BOLD),
            ),
            SideRow::Loading => Line::styled(
                format!("  {} Loading…", loading::glyph(app.loading_frame)),
                loading::style(loading::Kind::Background),
            ),
            SideRow::Tab(i) => {
                // The icon goes where the `#` glyph would: after the bar and the number.
                if let Some(url) = tab_icon(app, &app.tabs[*i]) {
                    spots.push(ImageSpot {
                        url: url.to_string(),
                        area: Rect {
                            x: list.x + 3,
                            y: list.y + k as u16,
                            width: ICON_COLS,
                            height: 1,
                        },
                    });
                }
                sidebar_item(app, *i, width)
            }
        })
        .collect();
    f.render_widget(Paragraph::new(lines).style(panel), list);

    let (dot, color, label) = match app.health {
        Health::SignedOut => (
            "○",
            YELLOW,
            format!("connect: {}", search::reading_key(app, "SPC p a")),
        ),
        Health::Unknown => ("○", OVERLAY, "reddit".into()),
        Health::Up => ("●", GREEN, "reddit".into()),
        Health::Paused => ("◐", YELLOW, "reddit paused".into()),
        Health::Down => ("✗", RED, "reddit down".into()),
    };
    let img = if app.inline_images_on() {
        "img on"
    } else {
        "img off"
    };
    // " ✗ reddit down  ◧ img off" is the label plus 14 columns; narrower sidebars drop the state.
    let label = if width >= label.len() + 14 {
        label.as_str()
    } else {
        "reddit"
    };
    f.render_widget(
        Paragraph::new(vec![
            Line::styled("─".repeat(width), Style::new().fg(SURFACE0)),
            Line::from(vec![
                Span::styled(format!(" {dot} "), Style::new().fg(color)),
                Span::styled(label, Style::new().fg(SUBTEXT)),
                Span::styled(format!("  ◧ {img}"), Style::new().fg(OVERLAY)),
            ]),
        ])
        .style(panel),
        status,
    );
}

pub(super) fn sidebar_item(app: &App, index: usize, width: usize) -> Line<'static> {
    let tab = &app.tabs[index];
    let active = index == app.active;
    let (icon, name) = match &tab.kind {
        TabKind::User { name, .. } => ("@", name.as_str()),
        TabKind::Home => ("⌂", "Home"),
        TabKind::AllSubs => ("▦", "All subs"),
        TabKind::Bookmarks => ("★", "Bookmarks"),
        TabKind::Search => ("⌕", "Search"),
        TabKind::Subreddit { name, .. } => ("#", name.as_str()),
    };
    let badge = match tab.kind {
        TabKind::Bookmarks => UnreadBadge::Count(app.visible_bookmarks().count()),
        _ => app.badge(tab),
    };
    let focused = app.focus == Focus::Sidebar;
    // Without focus the active tab is a place, not a cursor: no row background or accent.
    let base = if active && focused {
        Style::new().bg(SURFACE1)
    } else {
        Style::new()
    };
    let number = if index < 9 {
        format!("{}", index + 1)
    } else {
        " ".into()
    };
    // With thumbnails on, every glyph slot is 4 columns wide so names line up with icons.
    let slot = match tab_icon(app, tab) {
        Some(_) => "    ".to_string(),
        None if app.thumbnails_on() => format!(" {icon}  "),
        None => format!(" {icon} "),
    };
    let mut spans = vec![
        Span::styled(
            match (active, focused) {
                (true, true) => "▌",
                (true, false) => "●",
                _ => " ",
            },
            base.fg(if focused { ACCENT } else { SUBTEXT }),
        ),
        Span::styled(number, base.fg(if active { TEXT } else { OVERLAY })),
        Span::styled(
            slot,
            base.fg(match (active, focused) {
                (true, true) => ACCENT,
                (true, false) => TEXT,
                _ => OVERLAY,
            }),
        ),
    ];
    let badge_text = match (badge, &tab.kind) {
        (UnreadBadge::Count(0) | UnreadBadge::Unavailable, _) => String::new(),
        (UnreadBadge::Unknown, _) => " ? ".into(),
        (UnreadBadge::Count(n), TabKind::Bookmarks) => format!(" {n} "),
        (UnreadBadge::Count(n), _) if n > 99 => " 99+ ".into(),
        (UnreadBadge::Count(n), _) => format!(" {n} "),
    };
    let name_width = width.saturating_sub(line_width(&spans) + badge_text.width() + 1);
    let name_style = if active {
        base.fg(TEXT).add_modifier(Modifier::BOLD)
    } else if matches!(badge, UnreadBadge::Count(n) if n > 0)
        && !matches!(tab.kind, TabKind::Bookmarks)
    {
        base.fg(TEXT)
    } else {
        base.fg(SUBTEXT)
    };
    spans.push(Span::styled(truncate(name, name_width), name_style));
    pad_to(&mut spans, width.saturating_sub(badge_text.width()), base);
    if !badge_text.is_empty() {
        let (fg, bg) = match (badge, &tab.kind) {
            (UnreadBadge::Unknown, _) => (OVERLAY, SURFACE0),
            (_, TabKind::Bookmarks) => (SUBTEXT, SURFACE0),
            _ => (CRUST, ACCENT),
        };
        spans.push(Span::styled(
            badge_text,
            Style::new().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
        ));
    }
    Line::from(spans)
}
