//! Rendering: a vertical tab sidebar on the left, the active view on the
//! right. Takes `&mut App` only to keep scroll offsets in sync with what fits.

use chrono::Utc;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap};
use ratatui_image::StatefulImage;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Group, ImageSlot, Overlay, PickerEntry, PostView, RowKind, TabKind};
use crate::model::{Media, Post, Sort};

/// Catppuccin Mocha, with Reddit orange as the accent.
mod theme {
    use ratatui::style::Color;
    pub const ACCENT: Color = Color::Rgb(255, 99, 51);
    pub const TEXT: Color = Color::Rgb(205, 214, 244);
    pub const SUBTEXT: Color = Color::Rgb(166, 173, 200);
    pub const OVERLAY: Color = Color::Rgb(108, 112, 134);
    pub const SURFACE0: Color = Color::Rgb(49, 50, 68);
    pub const SURFACE1: Color = Color::Rgb(69, 71, 90);
    pub const MANTLE: Color = Color::Rgb(24, 24, 37);
    pub const CRUST: Color = Color::Rgb(17, 17, 27);
    pub const BLUE: Color = Color::Rgb(137, 180, 250);
    pub const LAVENDER: Color = Color::Rgb(180, 190, 254);
    pub const MAUVE: Color = Color::Rgb(203, 166, 247);
    pub const PEACH: Color = Color::Rgb(250, 179, 135);
    pub const GREEN: Color = Color::Rgb(166, 227, 161);
    pub const TEAL: Color = Color::Rgb(148, 226, 213);
    pub const YELLOW: Color = Color::Rgb(249, 226, 175);
    pub const RED: Color = Color::Rgb(243, 139, 168);
    pub const SKY: Color = Color::Rgb(137, 220, 235);
    pub const THREADS: [Color; 6] = [BLUE, MAUVE, TEAL, PEACH, GREEN, SKY];
}
use theme::*;

const SIDEBAR_WIDE: u16 = 30;
const SIDEBAR_NARROW: u16 = 24;
const CARD_MIN_WIDTH: u16 = 54;
const IMAGE_MAX_HEIGHT: u16 = 20;

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let sidebar_width = if !app.show_sidebar {
        0
    } else if area.width >= 110 {
        SIDEBAR_WIDE
    } else {
        SIDEBAR_NARROW
    };
    let [sidebar, main] =
        Layout::horizontal([Constraint::Length(sidebar_width), Constraint::Min(20)]).areas(area);
    if sidebar_width > 0 {
        draw_sidebar(f, app, sidebar);
    }
    let [body, footer] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(main);
    let body = body.inner(Margin {
        horizontal: 1,
        vertical: 0,
    });
    if app.post_view.is_some() {
        draw_post(f, app, body);
    } else {
        match app.active_tab().kind {
            TabKind::Home => draw_home(f, app, body),
            TabKind::Bookmarks => draw_bookmarks(f, app, body),
            TabKind::Subreddit { .. } => draw_feed(f, app, body),
        }
    }
    draw_footer(f, app, footer);
    match &app.overlay {
        Some(Overlay::Help) => draw_help(f),
        Some(Overlay::Picker(p)) => draw_picker(f, app, &p.input, p.selected),
        Some(Overlay::Sort { selected }) => draw_sort(f, *selected),
        Some(Overlay::Search { input }) => draw_search(f, app, input),
        None => {}
    }
}

// ----- small helpers -------------------------------------------------------

fn format_count(n: i64) -> String {
    match n.unsigned_abs() {
        0..1_000 => n.to_string(),
        1_000..10_000 => format!("{:.1}k", n as f64 / 1_000.0),
        10_000..1_000_000 => format!("{}k", n / 1_000),
        _ => format!("{:.1}m", n as f64 / 1_000_000.0),
    }
}

fn score_color(score: Option<i64>) -> Color {
    match score {
        Some(s) if s >= 1_000 => ACCENT,
        Some(s) if s >= 100 => PEACH,
        Some(_) => SUBTEXT,
        None => OVERLAY,
    }
}

fn truncate(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}

/// A rounded label: ` text ` on a colored background.
fn chip(text: impl Into<String>, fg: Color, bg: Color) -> Span<'static> {
    Span::styled(format!(" {} ", text.into()), Style::new().fg(fg).bg(bg))
}

fn key_chip(key: &str) -> Span<'static> {
    Span::styled(
        format!(" {key} "),
        Style::new()
            .fg(TEXT)
            .bg(SURFACE1)
            .add_modifier(Modifier::BOLD),
    )
}

fn pad_to(spans: &mut Vec<Span<'static>>, width: usize, style: Style) {
    let used: usize = spans.iter().map(|s| s.content.width()).sum();
    if used < width {
        spans.push(Span::styled(" ".repeat(width - used), style));
    }
}

fn line_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|s| s.content.width()).sum()
}

fn centered(f: &mut Frame, area: Rect, lines: Vec<Line<'static>>) {
    let h = lines.len() as u16;
    let [_, mid, _] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(h),
        Constraint::Fill(1),
    ])
    .areas(area);
    f.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        mid,
    );
}

fn empty_state(f: &mut Frame, area: Rect, icon: &str, title: &str, hint: &str) {
    centered(
        f,
        area,
        vec![
            Line::styled(icon.to_string(), Style::new().fg(OVERLAY)),
            Line::raw(""),
            Line::styled(
                title.to_string(),
                Style::new().fg(SUBTEXT).add_modifier(Modifier::BOLD),
            ),
            Line::styled(hint.to_string(), Style::new().fg(OVERLAY)),
        ],
    );
}

// ----- sidebar -------------------------------------------------------------

enum SideRow {
    Section(&'static str),
    Tab(usize),
    Blank,
}

fn draw_sidebar(f: &mut Frame, app: &App, area: Rect) {
    let panel = Style::new().bg(MANTLE);
    f.render_widget(Block::default().style(panel), area);
    let inner = area.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    let [brand, list, status] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .areas(inner);

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ◆ ", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled("redtab", Style::new().fg(TEXT).add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("  v{}", env!("CARGO_PKG_VERSION")),
                Style::new().fg(OVERLAY),
            ),
        ]))
        .style(panel),
        brand,
    );

    // Build the row model: Home, Bookmarks, subscriptions, then opened tabs.
    let mut rows = vec![SideRow::Tab(0), SideRow::Tab(1)];
    let pinned: Vec<usize> = (0..app.tabs.len())
        .filter(|&i| matches!(app.tabs[i].kind, TabKind::Subreddit { pinned: true, .. }))
        .collect();
    let opened: Vec<usize> = (0..app.tabs.len())
        .filter(|&i| matches!(app.tabs[i].kind, TabKind::Subreddit { pinned: false, .. }))
        .collect();
    if !pinned.is_empty() {
        rows.push(SideRow::Blank);
        rows.push(SideRow::Section("SUBSCRIPTIONS"));
        rows.extend(pinned.into_iter().map(SideRow::Tab));
    }
    if !opened.is_empty() {
        rows.push(SideRow::Blank);
        rows.push(SideRow::Section("OPENED"));
        rows.extend(opened.into_iter().map(SideRow::Tab));
    }

    // Window the list so the active tab stays visible, a little off the edge.
    let height = list.height as usize;
    let active_row = rows
        .iter()
        .position(|r| matches!(r, SideRow::Tab(i) if *i == app.active))
        .unwrap_or(0);
    let start = if rows.len() <= height {
        0
    } else {
        active_row
            .saturating_sub(height / 2)
            .min(rows.len() - height)
    };
    let width = list.width as usize;
    let lines: Vec<Line> = rows
        .iter()
        .skip(start)
        .take(height)
        .map(|row| match row {
            SideRow::Blank => Line::raw(""),
            SideRow::Section(name) => Line::styled(
                format!("  {name}"),
                Style::new().fg(OVERLAY).add_modifier(Modifier::BOLD),
            ),
            SideRow::Tab(i) => sidebar_item(app, *i, width),
        })
        .collect();
    f.render_widget(Paragraph::new(lines).style(panel), list);

    let (dot, color, down) = match app.redlib_up {
        Some(true) => ("●", GREEN, false),
        Some(false) => ("●", RED, true),
        None => ("○", OVERLAY, false),
    };
    let img = if app.show_images { "img on" } else { "img off" };
    // " ● redlib down  ◧ img off" needs 25 columns; narrower sidebars keep the red dot only.
    let label = if down && width >= 25 {
        "redlib down"
    } else {
        "redlib"
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

fn sidebar_item(app: &App, index: usize, width: usize) -> Line<'static> {
    let tab = &app.tabs[index];
    let active = index == app.active;
    let (icon, name) = match &tab.kind {
        TabKind::Home => ("⌂", "Home".to_string()),
        TabKind::Bookmarks => ("★", "Bookmarks".to_string()),
        TabKind::Subreddit { name, .. } => ("#", name.clone()),
    };
    let badge = match tab.kind {
        TabKind::Bookmarks => app.state.bookmarks().len(),
        _ => app.badge(tab),
    };
    let base = if active {
        Style::new().bg(SURFACE0)
    } else {
        Style::new()
    };
    let number = if index < 9 {
        format!("{}", index + 1)
    } else {
        " ".into()
    };
    let mut spans = vec![
        Span::styled(if active { "▌" } else { " " }, base.fg(ACCENT)),
        Span::styled(number, base.fg(if active { SUBTEXT } else { SURFACE1 })),
        Span::styled(
            format!(" {icon} "),
            base.fg(if active { ACCENT } else { OVERLAY }),
        ),
    ];
    let badge_text = match (badge, &tab.kind) {
        (0, _) => String::new(),
        (n, TabKind::Bookmarks) => format!(" {n} "),
        (n, _) if n > 99 => " 99+ ".into(),
        (n, _) => format!(" {n} "),
    };
    let name_width = width.saturating_sub(line_width(&spans) + badge_text.width() + 1);
    let name_style = if active {
        base.fg(TEXT).add_modifier(Modifier::BOLD)
    } else if badge > 0 && !matches!(tab.kind, TabKind::Bookmarks) {
        base.fg(TEXT)
    } else {
        base.fg(SUBTEXT)
    };
    spans.push(Span::styled(truncate(&name, name_width), name_style));
    pad_to(&mut spans, width.saturating_sub(badge_text.width()), base);
    if !badge_text.is_empty() {
        let (fg, bg) = match tab.kind {
            TabKind::Bookmarks => (SUBTEXT, SURFACE0),
            _ => (CRUST, ACCENT),
        };
        spans.push(Span::styled(
            badge_text,
            Style::new().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
        ));
    }
    Line::from(spans)
}

// ----- main header ---------------------------------------------------------

fn view_header(
    f: &mut Frame,
    area: Rect,
    title: &str,
    chips: Vec<Span<'static>>,
    subtitle: Option<String>,
) -> Rect {
    let [head, rest] = Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).areas(area);
    let mut spans = vec![Span::styled(
        format!(" {title} "),
        Style::new()
            .fg(CRUST)
            .bg(ACCENT)
            .add_modifier(Modifier::BOLD),
    )];
    for c in chips {
        spans.push(Span::raw(" "));
        spans.push(c);
    }
    let mut lines = vec![Line::raw(""), Line::from(spans)];
    if let Some(sub) = subtitle {
        lines[0] = Line::styled(sub, Style::new().fg(OVERLAY));
        lines.swap(0, 1);
    }
    f.render_widget(Paragraph::new(lines), head);
    rest
}

fn sort_chip(sort: Sort) -> Span<'static> {
    let (icon, color) = match sort {
        Sort::Hot => ("🔥", PEACH),
        Sort::New => ("✦", GREEN),
        Sort::Top => ("▲", YELLOW),
        Sort::Rising => ("↗", TEAL),
        Sort::Controversial => ("⚡", RED),
    };
    chip(format!("{icon} {}", sort.as_str()), color, SURFACE0)
}

// ----- post cards ----------------------------------------------------------

struct CardOpts {
    selected: bool,
    seen_key: Option<String>,
    show_subreddit: bool,
}

/// Two lines per post: score gutter + title, comment gutter + meta chips.
fn post_card(app: &App, post: &Post, width: usize, opts: &CardOpts) -> [Line<'static>; 2] {
    let now = Utc::now();
    let bg = if opts.selected {
        Style::new().bg(SURFACE0)
    } else {
        Style::new()
    };
    let unseen = opts
        .seen_key
        .as_deref()
        .is_some_and(|k| !app.state.is_seen(k, &post.id));
    let read = app.state.is_read(&post.id);
    let bar = Span::styled(if opts.selected { "▌" } else { " " }, bg.fg(ACCENT));

    let score = post.score.map_or("•".into(), format_count);
    let mut title = vec![
        bar.clone(),
        Span::styled(
            format!("{score:>6} "),
            bg.fg(score_color(post.score)).add_modifier(Modifier::BOLD),
        ),
        Span::styled(if unseen { "● " } else { "  " }, bg.fg(ACCENT)),
    ];
    if app.state.is_bookmarked(&post.id) {
        title.push(Span::styled("★ ", bg.fg(YELLOW)));
    }
    if post.stickied {
        title.push(Span::styled("📌 ", bg));
    }
    let mut title_style = bg.fg(TEXT).add_modifier(Modifier::BOLD);
    if read {
        title_style = bg.fg(OVERLAY);
    }
    if app.config.highlights(post) {
        title_style = title_style.fg(YELLOW);
    }
    let room = width.saturating_sub(line_width(&title));
    title.push(Span::styled(truncate(&post.title, room), title_style));
    pad_to(&mut title, width, bg);

    let comments = format!("💬 {}", format_count(post.comments as i64));
    let mut meta = vec![
        bar,
        Span::styled(
            format!(
                "{}{comments} ",
                " ".repeat(7usize.saturating_sub(comments.width()))
            ),
            bg.fg(BLUE),
        ),
        Span::styled("  ", bg),
    ];
    if opts.show_subreddit {
        meta.push(Span::styled(
            format!("r/{}  ", post.subreddit),
            bg.fg(LAVENDER),
        ));
    }
    meta.push(Span::styled(format!("u/{}", post.author), bg.fg(SUBTEXT)));
    let age = post.age_at(now);
    if !age.is_empty() {
        meta.push(Span::styled(format!(" · {age}"), bg.fg(OVERLAY)));
    }
    if let Some(flair) = &post.flair {
        meta.push(Span::styled(" ", bg));
        meta.push(chip(truncate(flair, 22), MAUVE, SURFACE1));
    }
    if let Some(tag) = post.media.tag() {
        meta.push(Span::styled(" ", bg));
        meta.push(chip(media_label(&post.media, &tag), TEAL, SURFACE1));
    }
    let mut meta_line: Vec<Span<'static>> = Vec::new();
    let mut used = 0;
    for span in meta {
        let w = span.content.width();
        if used + w > width {
            break;
        }
        used += w;
        meta_line.push(span);
    }
    pad_to(&mut meta_line, width, bg);
    [Line::from(title), Line::from(meta_line)]
}

fn media_label(media: &Media, tag: &str) -> String {
    let icon = match media {
        Media::Image { .. } => "▣",
        Media::Video { .. } => "▶",
        Media::Gallery { .. } => "▦",
        Media::Link { .. } => "↗",
        Media::None => "",
    };
    format!("{icon} {}", truncate(tag, 24))
}

struct ListSpec<'a> {
    posts: &'a [&'a Post],
    selected: usize,
    seen_key: Option<&'a str>,
    show_subreddit: bool,
}

/// Scrolling list of post cards separated by a blank line.
fn draw_post_list(f: &mut Frame, app: &App, area: Rect, spec: &ListSpec<'_>, offset: &mut usize) {
    const ITEM: usize = 3;
    let fits = (area.height as usize / ITEM).max(1);
    if spec.selected < *offset {
        *offset = spec.selected;
    } else if spec.selected >= *offset + fits {
        *offset = spec.selected + 1 - fits;
    }
    let width = area.width.saturating_sub(2) as usize;
    let mut lines = Vec::new();
    for (i, post) in spec.posts.iter().enumerate().skip(*offset).take(fits) {
        let opts = CardOpts {
            selected: i == spec.selected,
            seen_key: spec.seen_key.map(str::to_string),
            show_subreddit: spec.show_subreddit,
        };
        let [title, meta] = post_card(app, post, width, &opts);
        lines.push(title);
        lines.push(meta);
        lines.push(Line::raw(""));
    }
    f.render_widget(Paragraph::new(lines), area);
    draw_scrollbar(f, area, *offset, fits, spec.posts.len());
}

fn draw_scrollbar(f: &mut Frame, area: Rect, offset: usize, visible: usize, total: usize) {
    if total <= visible || area.height < 3 {
        return;
    }
    let h = area.height as usize;
    let thumb = (h * visible / total).max(1);
    let top = (h - thumb) * offset / (total - visible).max(1);
    let x = area.x + area.width.saturating_sub(1);
    for y in 0..h {
        let (sym, color) = if y >= top && y < top + thumb {
            ("┃", SURFACE1)
        } else {
            ("│", SURFACE0)
        };
        f.render_widget(
            Span::styled(sym, Style::new().fg(color)),
            Rect {
                x,
                y: area.y + y as u16,
                width: 1,
                height: 1,
            },
        );
    }
}

// ----- home wall -----------------------------------------------------------

fn card_height(app: &App, group: &Group) -> u16 {
    let shown = app.home_group_posts(group).len().max(1) as u16;
    // borders + top/bottom padding + 2 lines per post + a gap between posts
    2 + 2 + shown * 2 + shown.saturating_sub(1)
}

/// Blank line between rows of cards.
const CARD_ROW_GAP: u16 = 1;

fn draw_home(f: &mut Frame, app: &mut App, area: Rect) {
    let total_new: usize = app
        .home
        .groups
        .iter()
        .map(|g| app.state.unseen(&g.subreddit, app.visible(&g.posts)))
        .sum();
    let loading = app.home.groups.iter().filter(|g| g.loading).count();
    let mut chips = vec![chip(
        match app.home.groups.len() {
            1 => "1 subreddit".to_string(),
            n => format!("{n} subreddits"),
        },
        SUBTEXT,
        SURFACE0,
    )];
    if total_new > 0 {
        chips.push(chip(format!("● {total_new} new"), CRUST, ACCENT));
    }
    if loading > 0 {
        chips.push(chip(format!("⋯ loading {loading}"), OVERLAY, SURFACE0));
    }
    let body = view_header(f, area, "⌂ Home", chips, None);
    if app.home.groups.is_empty() {
        empty_state(
            f,
            body,
            "⌂",
            "No subscriptions yet",
            "press p to open a subreddit, or list names in the subscriptions file",
        );
        return;
    }
    let cols = (body.width / CARD_MIN_WIDTH).clamp(1, 3) as usize;
    let rows: Vec<Vec<usize>> = (0..app.home.groups.len())
        .collect::<Vec<_>>()
        .chunks(cols)
        .map(<[usize]>::to_vec)
        .collect();
    let heights: Vec<u16> = rows
        .iter()
        .map(|r| {
            let card = r
                .iter()
                .map(|&g| card_height(app, &app.home.groups[g]))
                .max()
                .unwrap_or(4);
            card + CARD_ROW_GAP
        })
        .collect();
    let selected_row = app.home.group / cols;
    let mut offset = app.home.offset.min(selected_row);
    // The gap after the last visible row may fall outside the body.
    let fits = |rows: &[u16]| rows.iter().sum::<u16>() - CARD_ROW_GAP <= body.height;
    while !fits(&heights[offset..=selected_row]) && offset < selected_row {
        offset += 1;
    }
    app.home.offset = offset;

    let bottom = body.y + body.height;
    let mut y = body.y;
    for (row_index, row) in rows.iter().enumerate().skip(offset) {
        let h = heights[row_index];
        let card = h - CARD_ROW_GAP;
        // Later rows only when whole; the first row always shows, clipped in a short pane.
        if y >= bottom || (row_index > offset && y + card > bottom) {
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
        for (cell, &g) in cells.iter().zip(row) {
            draw_group_card(f, app, g, *cell);
        }
        y += h;
    }
    draw_scrollbar(f, body, offset, 1, rows.len());
}

fn draw_group_card(f: &mut Frame, app: &App, index: usize, area: Rect) {
    let group = &app.home.groups[index];
    let selected_card = index == app.home.group;
    let badge = app
        .state
        .unseen(&group.subreddit, app.visible(&group.posts));
    let border = if selected_card {
        Style::new().fg(ACCENT)
    } else {
        Style::new().fg(SURFACE1)
    };
    let mut title = vec![
        Span::styled(
            " # ",
            Style::new().fg(if selected_card { ACCENT } else { OVERLAY }),
        ),
        Span::styled(
            format!("{} ", group.subreddit),
            Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
        ),
    ];
    if badge > 0 {
        title.push(chip(format!("{badge} new"), CRUST, ACCENT));
        title.push(Span::raw(" "));
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border)
        .title(Line::from(title))
        .padding(Padding {
            left: 0,
            right: 1,
            top: 1,
            bottom: 1,
        });
    let inner = block.inner(area);
    f.render_widget(block, area);
    let width = inner.width as usize;
    let posts = app.home_group_posts(group);
    let lines: Vec<Line> = if group.loading && group.posts.is_empty() {
        vec![Line::styled("   ⋯ loading", Style::new().fg(OVERLAY))]
    } else if let Some(e) = &group.error {
        vec![Line::styled(
            format!("   ✗ {}", truncate(e, width.saturating_sub(5))),
            Style::new().fg(RED),
        )]
    } else if posts.is_empty() {
        vec![Line::styled("   no posts", Style::new().fg(OVERLAY))]
    } else {
        let mut lines = Vec::new();
        for (i, p) in posts.iter().enumerate() {
            if i > 0 {
                lines.push(Line::raw(""));
            }
            let opts = CardOpts {
                selected: selected_card && i == app.home.post,
                seen_key: Some(group.subreddit.clone()),
                show_subreddit: false,
            };
            lines.extend(post_card(app, p, width, &opts));
        }
        lines
    };
    // In a short pane the card is clipped: scroll it so the selected post shows.
    let selected_end = if selected_card {
        app.home.post as u16 * 3 + 2
    } else {
        0
    };
    let scroll = selected_end.saturating_sub(inner.height);
    f.render_widget(Paragraph::new(lines).scroll((scroll, 0)), inner);
}

// ----- subreddit feed & bookmarks --------------------------------------------

fn draw_feed(f: &mut Frame, app: &mut App, area: Rect) {
    let tab = app.active_tab();
    let feed = &tab.feed;
    let mut chips = Vec::new();
    match &feed.search {
        Some(q) => chips.push(chip(format!("🔍 {q}"), SKY, SURFACE0)),
        None => {
            chips.push(sort_chip(feed.sort));
            if feed.sort.takes_time() {
                chips.push(chip(format!("◷ {}", feed.time.as_str()), SUBTEXT, SURFACE0));
            }
        }
    }
    let badge = app.badge(tab);
    if badge > 0 {
        chips.push(chip(format!("● {badge} new"), CRUST, ACCENT));
    }
    if feed.loading {
        chips.push(chip("⋯ loading", OVERLAY, SURFACE0));
    }
    if feed.error.is_some() && !feed.posts.is_empty() {
        chips.push(chip("✗ last load failed · L retries", RED, SURFACE0));
    }
    let title = tab.title();
    let seen_key = tab.subreddit().map(str::to_string);
    let error = feed.error.clone();
    let posts: Vec<Post> = app.visible(&feed.posts).into_iter().cloned().collect();
    let selected = feed.selected;
    let loaded = feed.loaded;
    let mut offset = feed.offset;
    let body = view_header(f, area, &title, chips, None);
    if let Some(e) = error.filter(|_| posts.is_empty()) {
        empty_state(
            f,
            body,
            "✗",
            "Could not load posts",
            &format!("{e} — press r to retry"),
        );
        return;
    }
    if posts.is_empty() {
        if loaded {
            empty_state(
                f,
                body,
                "∅",
                "No posts here",
                "try another sort with s, or a different subreddit with p",
            );
        } else {
            empty_state(f, body, "⋯", "Loading", "");
        }
        return;
    }
    let refs: Vec<&Post> = posts.iter().collect();
    let show_subreddit = matches!(seen_key.as_deref(), Some("all" | "popular"));
    let spec = ListSpec {
        posts: &refs,
        selected,
        seen_key: seen_key.as_deref(),
        show_subreddit,
    };
    draw_post_list(f, app, body, &spec, &mut offset);
    let active = app.active;
    app.tabs[active].feed.offset = offset;
}

fn draw_bookmarks(f: &mut Frame, app: &mut App, area: Rect) {
    let count = app.state.bookmarks().len();
    let body = view_header(
        f,
        area,
        "★ Bookmarks",
        vec![chip(format!("{count} saved"), SUBTEXT, SURFACE0)],
        None,
    );
    let posts: Vec<Post> = app
        .state
        .bookmarks()
        .iter()
        .map(|b| b.post.clone())
        .collect();
    if posts.is_empty() {
        empty_state(
            f,
            body,
            "★",
            "No bookmarks yet",
            "press b on any post to keep it here",
        );
        return;
    }
    let refs: Vec<&Post> = posts.iter().collect();
    let selected = app.active_tab().feed.selected.min(posts.len() - 1);
    let mut offset = app.active_tab().feed.offset;
    let spec = ListSpec {
        posts: &refs,
        selected,
        seen_key: None,
        show_subreddit: true,
    };
    draw_post_list(f, app, body, &spec, &mut offset);
    let active = app.active;
    app.tabs[active].feed.offset = offset;
}

// ----- post view -----------------------------------------------------------

fn wrap_into(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    width: usize,
    prefix: &[Span<'static>],
    style: Style,
) {
    let width = width.max(10);
    for para in text.split('\n') {
        if para.trim().is_empty() {
            lines.push(Line::from(prefix.to_vec()));
            continue;
        }
        let quoted = para.starts_with("│ ");
        let code = para.starts_with("    ");
        let para_style = if quoted {
            style.fg(SUBTEXT).add_modifier(Modifier::ITALIC)
        } else if code {
            style.fg(GREEN)
        } else {
            style
        };
        for piece in textwrap::wrap(para, width) {
            let mut spans = prefix.to_vec();
            spans.push(Span::styled(piece.into_owned(), para_style));
            lines.push(Line::from(spans));
        }
    }
}

fn thread_prefix(depth: usize) -> Vec<Span<'static>> {
    (0..depth)
        .map(|d| Span::styled("│ ", Style::new().fg(THREADS[d % THREADS.len()])))
        .collect()
}

fn draw_post(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(view) = app.post_view.as_ref() else {
        return;
    };
    let post = view.page.as_ref().map_or(&view.post, |p| &p.post).clone();
    let now = Utc::now();

    // Header card: never scrolls.
    let head_width = area.width.saturating_sub(4) as usize;
    let mut header = vec![Line::from({
        let mut s = vec![
            chip(format!("r/{}", post.subreddit), CRUST, ACCENT),
            Span::styled(format!("  u/{}", post.author), Style::new().fg(SUBTEXT)),
            Span::styled(
                Some(post.age_at(now))
                    .filter(|a| !a.is_empty())
                    .map(|a| format!(" · {a}"))
                    .unwrap_or_default(),
                Style::new().fg(OVERLAY),
            ),
        ];
        if let Some(flair) = &post.flair {
            s.push(Span::raw("  "));
            s.push(chip(truncate(flair, 28), MAUVE, SURFACE1));
        }
        if app.state.is_bookmarked(&post.id) {
            s.push(Span::styled("  ★", Style::new().fg(YELLOW)));
        }
        s
    })];
    header.push(Line::raw(""));
    let title_style = if app.config.highlights(&post) {
        Style::new().fg(YELLOW).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(TEXT).add_modifier(Modifier::BOLD)
    };
    wrap_into(&mut header, &post.title, head_width, &[], title_style);
    header.push(Line::raw(""));
    let mut stats = vec![
        Span::styled(
            format!("▲ {}", post.score.map_or("•".into(), format_count)),
            Style::new()
                .fg(score_color(post.score))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("   💬 {}", format_count(post.comments as i64)),
            Style::new().fg(BLUE),
        ),
    ];
    match &post.media {
        Media::Link { url, .. } => {
            stats.push(Span::raw("   "));
            stats.push(Span::styled(
                truncate(&format!("↗ {url}"), head_width.saturating_sub(24)),
                Style::new().fg(SKY).add_modifier(Modifier::UNDERLINED),
            ));
        }
        m => {
            if let Some(tag) = m.tag() {
                stats.push(Span::raw("   "));
                stats.push(chip(media_label(m, &tag), TEAL, SURFACE1));
                stats.push(Span::styled("  m opens", Style::new().fg(OVERLAY)));
            }
        }
    }
    header.push(Line::from(stats));
    let header_height = header.len() as u16 + 2;
    let [head_area, rest] =
        Layout::vertical([Constraint::Length(header_height), Constraint::Min(1)]).areas(area);
    f.render_widget(
        Paragraph::new(header).block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(SURFACE1))
                .padding(Padding::horizontal(1)),
        ),
        head_area,
    );

    // Inline image under the header while the document is at the top.
    let image_url = post.media.preview_image().map(str::to_string);
    let at_top = app.post_view.as_ref().is_some_and(|v| v.scroll == 0);
    let doc_area = match image_url.filter(|_| app.show_images && at_top) {
        Some(url) => {
            let h = IMAGE_MAX_HEIGHT.min(rest.height / 2);
            let [img_area, doc] =
                Layout::vertical([Constraint::Length(h + 1), Constraint::Min(1)]).areas(rest);
            draw_inline_image(
                f,
                app,
                &url,
                img_area.inner(Margin {
                    horizontal: 1,
                    vertical: 0,
                }),
            );
            doc
        }
        None => rest,
    };

    let doc_area = doc_area.inner(Margin {
        horizontal: 1,
        vertical: 0,
    });
    let width = doc_area.width.saturating_sub(2) as usize;
    let Some(view) = app.post_view.as_mut() else {
        return;
    };
    let (lines, spans) = post_document(view, width);
    view.row_starts = spans.iter().map(|(s, _)| *s).collect();
    let height = doc_area.height as usize;
    if !view.free_scroll
        && let Some(&(start, end)) = spans.get(view.cursor)
    {
        if start < view.scroll {
            view.scroll = start;
        } else if end > view.scroll + height {
            view.scroll = end.saturating_sub(height).min(start);
        }
    }
    view.scroll = view.scroll.min(lines.len().saturating_sub(1));
    let total = lines.len();
    let scroll = view.scroll;
    let visible: Vec<Line> = lines.into_iter().skip(scroll).take(height).collect();
    f.render_widget(Paragraph::new(visible), doc_area);
    draw_scrollbar(f, doc_area, scroll, height, total);
}

/// Selftext, then comments. Returns the lines and each comment row's line span.
fn post_document(view: &PostView, width: usize) -> (Vec<Line<'static>>, Vec<(usize, usize)>) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    if let Some(e) = &view.error {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            format!("✗ Could not load the post: {e}"),
            Style::new().fg(RED),
        ));
        lines.push(Line::styled("press r to retry", Style::new().fg(OVERLAY)));
        return (lines, Vec::new());
    }
    let Some(page) = &view.page else {
        lines.push(Line::raw(""));
        lines.push(Line::styled("⋯ loading post", Style::new().fg(OVERLAY)));
        return (lines, Vec::new());
    };
    lines.push(Line::raw(""));
    if !page.body.is_empty() {
        wrap_into(&mut lines, &page.body, width, &[], Style::new().fg(TEXT));
        lines.push(Line::raw(""));
    }
    let label = format!(" 💬 {} comments ", format_count(page.post.comments as i64));
    let rule = width.saturating_sub(label.width()) / 2;
    lines.push(Line::from(vec![
        Span::styled("─".repeat(rule), Style::new().fg(SURFACE1)),
        Span::styled(label, Style::new().fg(SUBTEXT).add_modifier(Modifier::BOLD)),
        Span::styled("─".repeat(rule), Style::new().fg(SURFACE1)),
    ]));
    lines.push(Line::raw(""));
    let rows = view.rows();
    if rows.is_empty() {
        lines.push(Line::styled("   no comments yet", Style::new().fg(OVERLAY)));
    }
    let mut spans = Vec::with_capacity(rows.len());
    for (i, row) in rows.iter().enumerate() {
        let start = lines.len();
        let selected = i == view.cursor && !view.free_scroll;
        let bg = if selected {
            Style::new().bg(SURFACE0)
        } else {
            Style::new()
        };
        let bar = Span::styled(if selected { "▌" } else { " " }, bg.fg(ACCENT));
        let mut prefix = vec![bar.clone()];
        prefix.extend(thread_prefix(row.depth));
        let body_width = width.saturating_sub(row.depth * 2 + 2);
        match &row.kind {
            RowKind::Comment { comment, hidden } => {
                let mut head = prefix.clone();
                let author_color = if comment.is_op {
                    ACCENT
                } else if comment.is_mod {
                    GREEN
                } else {
                    THREADS[row.depth % THREADS.len()]
                };
                head.push(Span::styled(
                    format!("u/{}", comment.author),
                    bg.fg(author_color).add_modifier(Modifier::BOLD),
                ));
                if comment.is_op {
                    head.push(Span::styled(" ", bg));
                    head.push(chip("OP", CRUST, ACCENT));
                }
                if comment.is_mod {
                    head.push(Span::styled(" ", bg));
                    head.push(chip("MOD", CRUST, GREEN));
                }
                if let Some(flair) = &comment.flair {
                    head.push(Span::styled(
                        format!(" {}", truncate(flair, 20)),
                        bg.fg(OVERLAY).add_modifier(Modifier::ITALIC),
                    ));
                }
                head.push(Span::styled(
                    format!("  ▲ {}", comment.score.map_or("•".into(), format_count)),
                    bg.fg(score_color(comment.score)),
                ));
                head.push(Span::styled(format!(" · {}", comment.age), bg.fg(OVERLAY)));
                if *hidden > 0 {
                    head.push(Span::styled("  ", bg));
                    head.push(chip(format!("+{hidden} folded"), YELLOW, SURFACE1));
                }
                if selected {
                    pad_to(&mut head, width, bg);
                }
                lines.push(Line::from(head));
                let mut body_prefix = vec![Span::raw(" ")];
                body_prefix.extend(thread_prefix(row.depth));
                if *hidden > 0 {
                    let first = comment
                        .body
                        .lines()
                        .find(|l| !l.trim().is_empty())
                        .unwrap_or_default();
                    let mut l = body_prefix.clone();
                    l.push(Span::styled(
                        truncate(first, body_width),
                        Style::new().fg(OVERLAY),
                    ));
                    lines.push(Line::from(l));
                } else {
                    wrap_into(
                        &mut lines,
                        &comment.body,
                        body_width,
                        &body_prefix,
                        Style::new().fg(TEXT),
                    );
                }
            }
            RowKind::More { parent, more } => {
                let mut line = prefix.clone();
                let text = if view.loading_more.contains(*parent) {
                    "⋯ loading replies".to_string()
                } else {
                    match more.count {
                        Some(n) => format!(
                            "↳ {n} more {} · Enter",
                            if n == 1 { "reply" } else { "replies" }
                        ),
                        None => "↳ more replies · Enter".to_string(),
                    }
                };
                line.push(Span::styled(text, bg.fg(SKY)));
                if selected {
                    pad_to(&mut line, width, bg);
                }
                lines.push(Line::from(line));
            }
        }
        let end = lines.len();
        lines.push(Line::raw(""));
        spans.push((start, end));
    }
    (lines, spans)
}

fn draw_inline_image(f: &mut Frame, app: &mut App, url: &str, area: Rect) {
    match app.images.get_mut(url) {
        Some(ImageSlot::Ready(protocol)) => {
            f.render_stateful_widget(StatefulImage::default(), area, protocol.as_mut());
        }
        Some(ImageSlot::Failed(e)) => {
            f.render_widget(
                Paragraph::new(format!(
                    "  ▣ image unavailable: {}",
                    truncate(e, area.width as usize)
                ))
                .style(Style::new().fg(OVERLAY)),
                area,
            );
        }
        Some(ImageSlot::Loading) | None => {
            f.render_widget(
                Paragraph::new("  ▣ loading image…").style(Style::new().fg(OVERLAY)),
                area,
            );
        }
    }
}

// ----- footer --------------------------------------------------------------

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let line = if let Some(status) = &app.status {
        let (icon, color) = if status.error {
            ("✗", RED)
        } else {
            ("✓", TEAL)
        };
        Line::from(vec![
            Span::styled(
                format!(" {icon} "),
                Style::new().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(status.text.clone(), Style::new().fg(color)),
        ])
    } else if app.redlib_up == Some(false) {
        let hint = if app.config.redlib_start_command.is_some() {
            "Redlib is not answering · S starts it · r retries"
        } else {
            "Redlib is not answering · start it, then press r"
        };
        Line::from(vec![
            Span::styled(" ✗ ", Style::new().fg(RED)),
            Span::styled(hint, Style::new().fg(RED)),
        ])
    } else {
        let hints: &[(&str, &str)] = if app.post_view.is_some() {
            &[
                ("j/k", "comments"),
                ("c", "fold"),
                ("space", "page"),
                ("m", "media"),
                ("o", "browser"),
                ("b", "save"),
                ("h", "back"),
            ]
        } else {
            match app.active_tab().kind {
                TabKind::Home => &[
                    ("j/k", "posts"),
                    ("J/K", "cards"),
                    ("⏎", "open"),
                    ("space", "subreddit"),
                    ("tab", "next"),
                    ("p", "jump"),
                ],
                TabKind::Bookmarks => &[
                    ("j/k", "move"),
                    ("⏎", "open"),
                    ("b", "remove"),
                    ("o", "browser"),
                    ("p", "jump"),
                ],
                TabKind::Subreddit { .. } if app.active_tab().feed.sort.takes_time() => &[
                    ("j/k", "move"),
                    ("⏎", "open"),
                    ("s", "sort"),
                    ("t", "time"),
                    ("/", "search"),
                    ("p", "jump"),
                ],
                TabKind::Subreddit { .. } => &[
                    ("j/k", "move"),
                    ("⏎", "open"),
                    ("s", "sort"),
                    ("/", "search"),
                    ("p", "jump"),
                    ("x", "close"),
                ],
            }
        };
        let mut spans = vec![Span::raw(" ")];
        for (key, label) in hints.iter().chain(&[("?", "help")]) {
            spans.push(key_chip(key));
            spans.push(Span::styled(
                format!(" {label}  "),
                Style::new().fg(OVERLAY),
            ));
        }
        let mut used = 0;
        let fitted: Vec<Span> = spans
            .into_iter()
            .take_while(|s| {
                used += s.content.width();
                used <= area.width as usize
            })
            .collect();
        Line::from(fitted)
    };
    f.render_widget(line, area);
}

// ----- overlays --------------------------------------------------------------

fn popup(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width.saturating_sub(4));
    let h = height.min(area.height.saturating_sub(2));
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 3,
        width: w,
        height: h,
    }
}

fn popup_block(title: &str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .style(Style::new().bg(MANTLE))
        .padding(Padding::horizontal(1))
        .title(Line::from(vec![
            Span::styled(" ◆ ", Style::new().fg(ACCENT)),
            Span::styled(
                format!("{title} "),
                Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
        ]))
}

const HELP: &[(&str, &str)] = &[
    ("Tabs", ""),
    ("Tab ] / S-Tab [", "next / previous tab"),
    ("1-9 / 0", "jump to a tab / the last tab"),
    ("p", "jump to a tab or open any subreddit"),
    ("x", "close a tab opened with p"),
    ("\\", "hide or show the sidebar"),
    ("Lists", ""),
    ("j k · C-d C-u", "move · page"),
    ("g G", "first / last"),
    ("Enter l", "open the post"),
    ("J K", "Home: next / previous card"),
    ("Space", "Home: open the card's subreddit"),
    ("s · t", "subreddit: sort · time range"),
    ("/ · Esc", "subreddit: search · clear search"),
    ("L", "load the next page (automatic near the end)"),
    ("Post", ""),
    ("j k", "next / previous comment"),
    ("Space C-d C-u", "scroll"),
    ("c Enter", "fold a thread · load more replies"),
    ("n N", "next / previous top-level comment"),
    ("g G", "top of post / last comment"),
    ("h Esc q", "back"),
    ("Anywhere", ""),
    ("o · y", "open on reddit.com · copy link"),
    ("m", "media: image viewer, mpv, or browser"),
    ("b", "bookmark"),
    ("i", "inline images on / off"),
    ("r · S", "reload · start Redlib"),
    ("q · C-c", "quit"),
];

fn draw_help(f: &mut Frame) {
    let area = popup(f.area(), 66, HELP.len() as u16 + 2);
    let lines: Vec<Line> = HELP
        .iter()
        .map(|(k, v)| {
            if v.is_empty() {
                Line::styled(
                    k.to_string(),
                    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
                )
            } else {
                Line::from(vec![
                    Span::styled(format!("  {k:<17}"), Style::new().fg(LAVENDER)),
                    Span::styled(v.to_string(), Style::new().fg(SUBTEXT)),
                ])
            }
        })
        .collect();
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines).block(popup_block("Keys · any key closes")),
        area,
    );
}

fn draw_picker(f: &mut Frame, app: &App, input: &str, selected: usize) {
    let entries = app.picker_entries(input);
    let area = popup(f.area(), 56, 22);
    f.render_widget(Clear, area);
    let block = popup_block("Jump");
    let inner = block.inner(area);
    f.render_widget(block, area);
    let [input_area, list_area, hint_area] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" › ", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(input.to_string(), Style::new().fg(TEXT)),
            Span::styled("▏", Style::new().fg(ACCENT)),
        ]))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(SURFACE1)),
        ),
        input_area,
    );
    let height = list_area.height as usize;
    let start = selected.saturating_sub(height.saturating_sub(1));
    let width = list_area.width as usize;
    let lines: Vec<Line> = entries
        .iter()
        .enumerate()
        .skip(start)
        .take(height)
        .map(|(i, entry)| {
            let bg = if i == selected {
                Style::new().bg(SURFACE0)
            } else {
                Style::new()
            };
            let mut spans = vec![Span::styled(
                if i == selected { "▌" } else { " " },
                bg.fg(ACCENT),
            )];
            match entry {
                PickerEntry::Tab { index, title } => {
                    let icon = match app.tabs[*index].kind {
                        TabKind::Home => "⌂",
                        TabKind::Bookmarks => "★",
                        TabKind::Subreddit { .. } => "#",
                    };
                    spans.push(Span::styled(format!(" {icon} "), bg.fg(OVERLAY)));
                    spans.push(Span::styled(title.clone(), bg.fg(TEXT)));
                    let badge = app.badge(&app.tabs[*index]);
                    if badge > 0 {
                        pad_to(
                            &mut spans,
                            width.saturating_sub(format!(" {badge} ").width()),
                            bg,
                        );
                        spans.push(chip(badge.to_string(), CRUST, ACCENT));
                    }
                }
                PickerEntry::Open(name) => {
                    spans.push(Span::styled(" + ", bg.fg(GREEN)));
                    spans.push(Span::styled(format!("open r/{name}"), bg.fg(GREEN)));
                }
            }
            pad_to(&mut spans, width, bg);
            Line::from(spans)
        })
        .collect();
    f.render_widget(Paragraph::new(lines), list_area);
    f.render_widget(
        Line::from(vec![
            key_chip("⏎"),
            Span::styled(" go  ", Style::new().fg(OVERLAY)),
            key_chip("↑↓"),
            Span::styled(" move  ", Style::new().fg(OVERLAY)),
            key_chip("esc"),
            Span::styled(" close", Style::new().fg(OVERLAY)),
        ]),
        hint_area,
    );
}

fn draw_sort(f: &mut Frame, selected: usize) {
    let area = popup(f.area(), 34, 9);
    f.render_widget(Clear, area);
    let lines: Vec<Line> = Sort::ALL
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let bg = if i == selected {
                Style::new().bg(SURFACE0)
            } else {
                Style::new()
            };
            let name = s.as_str();
            let mut spans = vec![
                Span::styled(if i == selected { "▌" } else { " " }, bg.fg(ACCENT)),
                Span::styled(
                    format!(" {} ", &name[..1]),
                    bg.fg(LAVENDER).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                sort_chip(*s),
            ];
            pad_to(&mut spans, 28, bg);
            Line::from(spans)
        })
        .collect();
    f.render_widget(Paragraph::new(lines).block(popup_block("Sort")), area);
}

fn draw_search(f: &mut Frame, app: &App, input: &str) {
    let area = popup(f.area(), 60, 3);
    f.render_widget(Clear, area);
    let title = format!("Search {}", app.active_tab().title());
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("🔍 ", Style::new().fg(SKY)),
            Span::styled(input.to_string(), Style::new().fg(TEXT)),
            Span::styled("▏", Style::new().fg(ACCENT)),
        ]))
        .block(popup_block(&title)),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::model::{Comment, PostPage};
    use crate::redlib::Redlib;
    use crate::worker::{Msg, Pool};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui_image::picker::Picker;

    fn app(subscriptions: &[&str]) -> App {
        let redlib = Redlib::new("http://127.0.0.1:9", false);
        let (pool, _rx) = Pool::start(redlib.clone(), 3);
        let state = std::env::temp_dir().join(format!(
            "redtab-ui-test-{}-{:?}.json",
            std::process::id(),
            std::thread::current().id()
        ));
        App::new(
            Config {
                image_preview: false,
                ..Config::default()
            },
            subscriptions.iter().map(|s| s.to_string()).collect(),
            pool,
            redlib,
            state,
            Picker::halfblocks(),
        )
    }

    fn screen(app: &mut App, w: u16, h: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn sidebar_lists_tabs_vertically_with_sections() {
        let mut app = app(&["neovim", "rust"]);

        let s = screen(&mut app, 120, 30);
        let lines: Vec<&str> = s.lines().collect();
        // Only the sidebar columns: Home cards also mention subreddit names.
        let row_of = |needle: &str| {
            lines
                .iter()
                .position(|l| l.chars().take(30).collect::<String>().contains(needle))
                .unwrap_or(usize::MAX)
        };

        assert!(row_of("Home") < row_of("Bookmarks"));
        assert!(row_of("Bookmarks") < row_of("SUBSCRIPTIONS"));
        assert!(row_of("SUBSCRIPTIONS") < row_of("neovim"));
        assert!(row_of("neovim") < row_of("rust"));
    }

    /// Opens a 60-paragraph post; `while_loading` runs before the page arrives.
    fn open_long_post(while_loading: impl FnOnce(&mut App)) -> (App, String) {
        let mut app = app(&[]);
        let post = Post {
            id: "p".into(),
            subreddit: "s".into(),
            title: "A long post".into(),
            author: "a".into(),
            flair: None,
            score: Some(1),
            comments: 1,
            created: None,
            age: String::new(),
            permalink: "/r/s/comments/p/x/".into(),
            media: Media::None,
            stickied: false,
            nsfw: false,
        };
        app.home.groups = vec![crate::app::Group {
            subreddit: "s".into(),
            posts: vec![post.clone()],
            ..Default::default()
        }];
        app.open_current_post();
        screen(&mut app, 100, 30);
        while_loading(&mut app);
        let generation = app.post_view.as_ref().unwrap().generation;
        let body = (1..=60)
            .map(|i| format!("paragraph {i}"))
            .collect::<Vec<_>>()
            .join("\n\n");
        let comment = Comment {
            id: "c".into(),
            author: "x".into(),
            flair: None,
            is_op: false,
            is_mod: false,
            score: None,
            age: String::new(),
            body: "reply".into(),
            replies: vec![],
            more: None,
        };
        app.on_msg(Msg::Post {
            generation,
            result: Ok(Box::new(PostPage {
                post,
                body,
                comments: vec![comment],
            })),
        });
        let s = screen(&mut app, 100, 30);
        (app, s)
    }

    #[test]
    fn a_long_post_opens_at_its_top() {
        let (app, s) = open_long_post(|_| {});

        assert!(s.lines().any(|l| l.contains("paragraph 1 ")), "{s}");
        assert_eq!(app.post_view.as_ref().unwrap().scroll, 0);
    }

    #[test]
    fn comment_keys_pressed_while_a_post_loads_keep_it_at_its_top() {
        let (app, s) = open_long_post(|app| {
            app.activate_comment();
            app.move_comment_cursor(1);
            app.jump_top_level(true);
        });

        assert!(s.lines().any(|l| l.contains("paragraph 1 ")), "{s}");
        assert_eq!(app.post_view.as_ref().unwrap().scroll, 0);
    }

    #[test]
    fn home_shows_a_card_in_a_short_pane() {
        let mut app = app(&["s"]);
        app.home.groups[0].posts = (1..=3)
            .map(|i| Post {
                id: format!("p{i}"),
                subreddit: "s".into(),
                title: format!("title {i}"),
                author: "a".into(),
                flair: None,
                score: Some(1),
                comments: 0,
                created: None,
                age: String::new(),
                permalink: format!("/r/s/comments/p{i}/x/"),
                media: Media::None,
                stickied: false,
                nsfw: false,
            })
            .collect();

        let top = screen(&mut app, 100, 12);
        app.move_selection(2);
        let bottom = screen(&mut app, 100, 12);

        assert!(top.contains("title 1"), "{top}");
        // The clipped card scrolls so the selected post stays in view.
        assert!(bottom.contains("title 3"), "{bottom}");
    }
}
