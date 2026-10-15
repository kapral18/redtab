//! Rendering: a vertical tab sidebar on the left, the active view on the
//! right. Takes `&mut App` to keep scroll offsets in sync with what fits and
//! to load the thumbnails it draws.

use std::time::Instant;

use chrono::Utc;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap};
use ratatui_image::picker::ProtocolType;
use ratatui_image::{Image, StatefulImage};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Group, ImageSlot, Overlay, PickerEntry, PostView, RowKind, Tab, TabKind};
use crate::model::{Media, Post, SPOILER_END, SPOILER_START, Sort};
use crate::video;

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
/// A post-row thumbnail, after the selection bar. Its third row is the blank
/// line under the post, so rows keep their height.
const THUMB_COLS: u16 = 6;
const THUMB_ROWS: u16 = 3;
/// Columns a thumbnail slot takes from a row's text: the image and a gap.
const THUMB_RESERVE: usize = THUMB_COLS as usize + 1;
/// A subreddit icon: one line tall.
const ICON_COLS: u16 = 2;

/// A thumbnail or icon to draw once the text is down.
struct ImageSpot {
    url: String,
    area: Rect,
}

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
    let mut spots = Vec::new();
    if sidebar_width > 0 {
        draw_sidebar(f, app, sidebar, &mut spots);
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
            TabKind::Home => draw_home(f, app, body, &mut spots),
            TabKind::Bookmarks => draw_bookmarks(f, app, body, &mut spots),
            TabKind::Subreddit { .. } => draw_feed(f, app, body, &mut spots),
        }
    }
    draw_footer(f, app, footer);
    draw_spots(f, app, &spots);
    if let Some(Overlay::Help { scroll }) = &mut app.overlay {
        draw_help(f, scroll);
        return;
    }
    match &app.overlay {
        Some(Overlay::Picker(p)) => draw_picker(f, app, &p.input, p.selected),
        Some(Overlay::Sort { selected }) => draw_sort(f, *selected),
        Some(Overlay::Search { input }) => draw_search(f, app, input),
        Some(Overlay::Help { .. }) | None => {}
    }
}

/// NO_COLOR: drop every color the frame set, keeping bold and other modifiers.
/// Image placeholder cells keep theirs: kitty reads the image id from the color.
pub fn strip_colors(buf: &mut ratatui::buffer::Buffer) {
    for cell in &mut buf.content {
        if !cell.symbol().contains('\u{10EEEE}') {
            cell.fg = Color::Reset;
            cell.bg = Color::Reset;
        }
    }
}

/// Draw loaded thumbnails and icons; start loading the missing ones.
fn draw_spots(f: &mut Frame, app: &mut App, spots: &[ImageSpot]) {
    // An overlay would cover images drawn under it. kitty gets an image's data
    // only once, in its first cell, so a covered first cell would lose it for good;
    // sixel and iTerm2 would keep stale overlay pixels. Draw them once it closes.
    if app.overlay.is_none() {
        for spot in spots {
            if let Some(ImageSlot::Ready(protocol)) = app.thumbs.get_mut(&spot.url) {
                f.render_stateful_widget(StatefulImage::default(), spot.area, protocol.as_mut());
            }
        }
    }
    let urls: Vec<&str> = spots.iter().map(|s| s.url.as_str()).collect();
    app.thumbs_drawn(&urls);
}

/// The thumbnail a post row shows, when thumbnails are on.
fn row_thumb<'a>(app: &App, post: &'a Post) -> Option<&'a str> {
    // A spoiler's picture is the spoiler: its slot stays empty.
    if !app.thumbnails_on() || post.spoiler {
        return None;
    }
    app.usable_thumb(post.media.list_thumb())
}

/// The thumbnail slot for a list's rows: every row gets it when any post in the list
/// has an image, so scores and titles stay in one column; text-only lists keep full width.
fn thumb_reserve<'a>(app: &App, mut posts: impl Iterator<Item = &'a Post>) -> usize {
    // Only posts that draw a thumbnail count: a spoiler's slot stays empty.
    if app.thumbnails_on() && posts.any(|p| !p.spoiler && p.media.list_thumb().is_some()) {
        THUMB_RESERVE
    } else {
        0
    }
}

/// The icon a subreddit tab shows.
fn tab_icon<'a>(app: &'a App, tab: &Tab) -> Option<&'a str> {
    match &tab.kind {
        TabKind::Subreddit { name, .. } => app.shown_icon(name),
        _ => None,
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

/// Cut `s` to `width` columns with an ellipsis. Works on grapheme clusters, so an
/// emoji sequence or a combining mark is kept whole or dropped whole.
fn truncate(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for g in s.graphemes(true) {
        let w = g.width();
        if used + w + 1 > width {
            break;
        }
        out.push_str(g);
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
    // Rows after word wrapping, so a long hint is not cut at the bottom.
    let cols = area.width.max(1) as usize;
    let h = lines
        .iter()
        .map(|l| {
            let text: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
            textwrap::wrap(&text, cols).len().max(1)
        })
        .sum::<usize>()
        .min(area.height as usize) as u16;
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

fn draw_sidebar(f: &mut Frame, app: &App, area: Rect, spots: &mut Vec<ImageSpot>) {
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
    let (pinned, opened) = app.sidebar_tabs();
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
        .enumerate()
        .map(|(k, row)| match row {
            SideRow::Blank => Line::raw(""),
            SideRow::Section(name) => Line::styled(
                format!("  {name}"),
                Style::new().fg(OVERLAY).add_modifier(Modifier::BOLD),
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

    let (dot, color, down) = match app.redlib_up {
        Some(true) => ("●", GREEN, false),
        Some(false) => ("●", RED, true),
        None => ("○", OVERLAY, false),
    };
    let img = if app.inline_images_on() {
        "img on"
    } else {
        "img off"
    };
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
    // With focus in the sidebar, its row stands out more than the list's selection.
    let base = match (active, app.sidebar_focused) {
        (true, true) => Style::new().bg(SURFACE1),
        (true, false) => Style::new().bg(SURFACE0),
        (false, _) => Style::new(),
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
        Span::styled(if active { "▌" } else { " " }, base.fg(ACCENT)),
        Span::styled(number, base.fg(if active { SUBTEXT } else { SURFACE1 })),
        Span::styled(slot, base.fg(if active { ACCENT } else { OVERLAY })),
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
    /// Columns after the selection bar kept free of text, for a thumbnail.
    reserve: usize,
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
    // The list's selection dims while the sidebar has focus.
    let bar_color = if app.sidebar_focused { OVERLAY } else { ACCENT };
    let bar = Span::styled(if opts.selected { "▌" } else { " " }, bg.fg(bar_color));

    let score = post.score.map_or("•".into(), format_count);
    let slot = Span::styled(" ".repeat(opts.reserve), bg);
    let mut title = vec![
        bar.clone(),
        slot.clone(),
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
        slot,
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
    if post.nsfw {
        meta.push(Span::styled(" ", bg));
        meta.push(chip("nsfw", CRUST, RED));
    }
    if post.spoiler {
        meta.push(Span::styled(" ", bg));
        meta.push(chip("spoiler", CRUST, YELLOW));
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
fn draw_post_list(
    f: &mut Frame,
    app: &App,
    area: Rect,
    spec: &ListSpec<'_>,
    offset: &mut usize,
    spots: &mut Vec<ImageSpot>,
) {
    const ITEM: usize = 3;
    let fits = (area.height as usize / ITEM).max(1);
    if spec.selected < *offset {
        *offset = spec.selected;
    } else if spec.selected >= *offset + fits {
        *offset = spec.selected + 1 - fits;
    }
    let width = area.width.saturating_sub(2) as usize;
    let reserve = thumb_reserve(app, spec.posts.iter().copied());
    let mut lines = Vec::new();
    for (i, post) in spec.posts.iter().enumerate().skip(*offset).take(fits) {
        let y = area.y + lines.len() as u16;
        if let Some(url) = row_thumb(app, post)
            && y + THUMB_ROWS <= area.bottom()
        {
            spots.push(ImageSpot {
                url: url.to_string(),
                area: Rect {
                    x: area.x + 1,
                    y,
                    width: THUMB_COLS,
                    height: THUMB_ROWS,
                },
            });
        }
        let opts = CardOpts {
            selected: i == spec.selected,
            seen_key: spec.seen_key.map(str::to_string),
            show_subreddit: spec.show_subreddit,
            reserve,
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

fn draw_home(f: &mut Frame, app: &mut App, area: Rect, spots: &mut Vec<ImageSpot>) {
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
    let mut shown = 0;
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
            draw_group_card(f, app, g, *cell, spots);
        }
        y += h;
        shown += 1;
    }
    draw_scrollbar(f, body, offset, shown.max(1), rows.len());
}

fn draw_group_card(f: &mut Frame, app: &App, index: usize, area: Rect, spots: &mut Vec<ImageSpot>) {
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
    let icon = app.shown_icon(&group.subreddit);
    // The title starts after the corner: ` ` then the icon in place of `#`.
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
            if icon.is_some() { "    " } else { " # " },
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
    // A failed refresh keeps the posts you can still act on; it only marks the card.
    if group.error.is_some() && !group.posts.is_empty() {
        title.push(chip("✗ refresh failed", RED, SURFACE0));
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
    // (line in the card, URL) for each post row with a thumbnail.
    let mut thumbs: Vec<(u16, &str)> = Vec::new();
    let lines: Vec<Line> = if group.loading && group.posts.is_empty() {
        vec![Line::styled("   ⋯ loading", Style::new().fg(OVERLAY))]
    } else if let Some(e) = group.error.as_ref().filter(|_| group.posts.is_empty()) {
        vec![Line::styled(
            format!("   ✗ {}", truncate(e, width.saturating_sub(5))),
            Style::new().fg(RED),
        )]
    } else if posts.is_empty() {
        vec![Line::styled("   no posts", Style::new().fg(OVERLAY))]
    } else {
        let reserve = thumb_reserve(app, posts.iter().copied());
        let mut lines = Vec::new();
        for (i, p) in posts.iter().enumerate() {
            if i > 0 {
                lines.push(Line::raw(""));
            }
            let opts = CardOpts {
                selected: selected_card && i == app.home.post,
                seen_key: Some(group.subreddit.clone()),
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
    let selected_end = if selected_card {
        app.home.post as u16 * 3 + 2
    } else {
        0
    };
    let scroll = selected_end.saturating_sub(inner.height);
    // Only thumbnails whose rows are all inside the scrolled card; the last post's
    // third row may use the card's bottom padding line.
    for (line, url) in thumbs {
        if line >= scroll && line - scroll + THUMB_ROWS <= inner.height + 1 {
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

// ----- subreddit feed & bookmarks --------------------------------------------

fn draw_feed(f: &mut Frame, app: &mut App, area: Rect, spots: &mut Vec<ImageSpot>) {
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
        chips.push(chip(
            if feed.page_failed {
                "✗ next page failed · L retries"
            } else {
                "✗ reload failed · r retries"
            },
            RED,
            SURFACE0,
        ));
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
    draw_post_list(f, app, body, &spec, &mut offset, spots);
    let active = app.active;
    app.tabs[active].feed.offset = offset;
}

fn draw_bookmarks(f: &mut Frame, app: &mut App, area: Rect, spots: &mut Vec<ImageSpot>) {
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
    draw_post_list(f, app, body, &spec, &mut offset, spots);
    let active = app.active;
    app.tabs[active].feed.offset = offset;
}

// ----- post view -----------------------------------------------------------

/// Text with its `>!…!<` spoilers masked as `▒` (one per column, spaces kept, so
/// the mask is as wide as the text; lines can still break elsewhere, as at hyphens
/// or between CJK characters), or shown with their markers removed once revealed.
fn spoiler_text(text: &str, reveal: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut inside = false;
    let push = |segment: &str, masked: bool, out: &mut String| {
        if !masked || reveal {
            out.push_str(segment);
            return;
        }
        // Whole graphemes, so an emoji sequence masks to the columns it shows in.
        for g in segment.graphemes(true) {
            if g.chars().all(char::is_whitespace) {
                out.push_str(g);
            } else {
                out.extend(std::iter::repeat_n('▒', g.width()));
            }
        }
    };
    let mut rest = text;
    while let Some(at) = rest.find([SPOILER_START, SPOILER_END]) {
        push(&rest[..at], inside, &mut out);
        let marker = rest[at..].chars().next().unwrap_or(SPOILER_END);
        inside = marker == SPOILER_START;
        rest = &rest[at + marker.len_utf8()..];
    }
    push(rest, inside, &mut out);
    out
}

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

    // Inline image (or video) under the header while the document is at the top.
    // Whether it shows depends on where the document scrolls, which depends on how
    // tall the document area is: settle the scroll for the image layout first, and
    // drop the image when the document does not stay at its top. Deciding from the
    // old scroll showed the image for one frame, then the next redraw jumped the text.
    // A video shows its poster until its first frame.
    let video = matches!(&post.media, Media::Video { url: Some(_), .. });
    let image_url = post
        .media
        .shows_inline()
        .then(|| post.media.preview_image().map(str::to_string));
    // Until the post page arrives, a stale list copy may lack the spoiler tag.
    let hide_image = app
        .post_view
        .as_ref()
        .is_some_and(|v| v.spoiler_hidden() || v.page.is_none());
    let image_rows = IMAGE_MAX_HEIGHT.min(rest.height / 2) + 1;
    // A video needs a row of picture above its status line and the blank row.
    let image_url =
        image_url.filter(|_| app.inline_images_on() && !hide_image && (!video || image_rows >= 3));
    let [img_area, with_image] =
        Layout::vertical([Constraint::Length(image_rows), Constraint::Min(1)]).areas(rest);
    let margin = Margin {
        horizontal: 1,
        vertical: 0,
    };
    let (full, with_image) = (rest.inner(margin), with_image.inner(margin));
    // The image changes only the height: the text wraps the same in both layouts.
    let width = full.width.saturating_sub(2) as usize;
    let Some(view) = app.post_view.as_mut() else {
        return;
    };
    let (lines, spans) = post_document(view, width);
    view.row_starts = spans.iter().map(|(s, _)| *s).collect();
    // Replies loaded above the text being read: put it back where it was.
    if let Some((row, offset)) = view.take_reading_anchor()
        && let Some(&(start, _)) = spans.get(row)
    {
        view.scroll = start.saturating_add_signed(offset);
    }
    // `v` while reading: only spoiler lines changed, so the line that was on top
    // is in the new wrapping too. Keys in the same batch move on from it; a jump to
    // the post's top (`g`) stays there.
    if let Some((was, pinned)) = view.rewrapped.take()
        && view.free_scroll
        && view.reveal_spoilers != was
        && (view.scroll != 0 || pinned == 0)
    {
        let now = view.reveal_spoilers;
        view.reveal_spoilers = was;
        let (old_lines, _) = post_document(view, width);
        view.reveal_spoilers = now;
        if let Some(at) = same_line(&old_lines, pinned, &lines) {
            let moved = view.scroll as isize - pinned as isize;
            view.scroll = at.saturating_add_signed(moved);
        }
    }
    // `l` selecting the comment being read keeps its text where it was while that
    // comment stays selected and shows any of it; keys that move the selection end
    // the hold. Rows loading above move the comment, and the view moves with it.
    if view.free_scroll || view.hold_view.is_none_or(|h| h.row != view.cursor) {
        view.hold_view = None;
    }
    // Scrolled away, the image stays away (a short post clamps the scroll back to
    // 0) until a key goes back to the top: no frame shows it and the next not.
    if image_url.is_some() && view.scroll != 0 {
        view.image_away = true;
    }
    let settle = |height: usize| settle_scroll(view, &spans, lines.len(), height);
    let mut image = image_url.filter(|_| view.scroll == 0 && !view.image_away);
    let (mut doc_area, mut settled) = match image {
        Some(_) => (with_image, settle(with_image.height as usize)),
        None => (full, settle(full.height as usize)),
    };
    let mut image_away = false;
    if image.is_some() && settled.0 != 0 {
        image = None;
        image_away = true;
        doc_area = full;
        settled = settle(full.height as usize);
    }
    view.image_away |= image_away;
    view.image_shown = image.is_some();
    let (scroll, held) = settled;
    view.scroll = scroll;
    if !held {
        view.hold_view = None;
    }
    // A hold kept at the top for the image: with no image this frame, hold the text
    // where it is now instead, so later changes above it do not move it.
    if image.is_none()
        && let Some(hold) = &mut view.hold_view
        && hold.top
        && let Some(&(start, _)) = spans.get(hold.row)
    {
        hold.top = false;
        hold.offset = scroll as isize - start as isize;
    }
    view.drawn_cursor = (!view.free_scroll).then_some(view.cursor);
    let height = doc_area.height as usize;
    view.page_height = height;
    let total = lines.len();
    let visible: Vec<Line> = lines.into_iter().skip(scroll).take(height).collect();
    f.render_widget(Paragraph::new(visible), doc_area);
    draw_scrollbar(f, doc_area, scroll, height, total);
    if let Some(still) = image {
        let area = img_area.inner(margin);
        if video {
            draw_video(f, app, still.as_deref(), area);
        } else if let Some(url) = still {
            draw_inline_image(f, app, &url, area);
        }
    }
}

/// Where line `at` of `old` is in `new`: the same line, counted among its equals
/// from the top. None when it changed (a spoiler line that wraps otherwise).
fn same_line(old: &[Line], at: usize, new: &[Line]) -> Option<usize> {
    let line = old.get(at)?;
    let nth = old[..at].iter().filter(|l| *l == line).count();
    new.iter()
        .enumerate()
        .filter(|(_, l)| *l == line)
        .nth(nth)
        .map(|(i, _)| i)
}

/// Where the document scrolls for a doc area `height` lines tall, and whether a
/// hold on the selected comment still applies: the hold keeps the text where it
/// was, otherwise the selected comment is brought into view; never past the end.
fn settle_scroll(
    view: &PostView,
    spans: &[(usize, usize)],
    total: usize,
    height: usize,
) -> (usize, bool) {
    let mut scroll = view.scroll;
    let mut held = false;
    if let Some(hold) = view.hold_view
        && let Some(&(start, end)) = spans.get(view.cursor)
    {
        let at = if hold.top {
            0
        } else {
            start.saturating_add_signed(hold.offset)
        };
        held = at < end && at + height > start;
        if held {
            scroll = at;
        }
    }
    if !view.free_scroll
        && !held
        && let Some(&(start, end)) = spans.get(view.cursor)
    {
        if start < scroll {
            scroll = start;
        } else if end > scroll + height {
            scroll = end.saturating_sub(height).min(start);
        }
    }
    // Stop with the last line at the bottom: scrolling past it would blank the pane.
    (scroll.min(total.saturating_sub(height)), held)
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
    let reveal = view.reveal_spoilers;
    if view.spoiler_hidden() {
        // The comments stay hidden too: they discuss what the post hides.
        let comments = match page.post.comments {
            0 => String::new(),
            1 => " and its comment".into(),
            n => format!(" and its {} comments", format_count(n as i64)),
        };
        lines.push(Line::styled(
            format!("⚠ Spoiler · v reveals the post{comments}"),
            Style::new().fg(YELLOW).add_modifier(Modifier::BOLD),
        ));
        return (lines, Vec::new());
    } else if !page.body.is_empty() {
        let body = spoiler_text(&page.body, reveal);
        wrap_into(&mut lines, &body, width, &[], Style::new().fg(TEXT));
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
                // The bar runs down the whole selected comment, so a selection whose
                // header is above the screen still shows.
                let mut body_prefix = vec![if selected {
                    Span::styled("▌", Style::new().fg(ACCENT))
                } else {
                    Span::raw(" ")
                }];
                body_prefix.extend(thread_prefix(row.depth));
                if *hidden > 0 {
                    let body = spoiler_text(&comment.body, reveal);
                    let first = body
                        .lines()
                        .find(|l| !l.trim().is_empty())
                        .unwrap_or_default()
                        .to_string();
                    let mut l = body_prefix.clone();
                    l.push(Span::styled(
                        truncate(&first, body_width),
                        Style::new().fg(OVERLAY),
                    ));
                    lines.push(Line::from(l));
                } else {
                    wrap_into(
                        &mut lines,
                        &spoiler_text(&comment.body, reveal),
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

/// The post's video under the header: the current frame (the poster until the
/// first one), and under it a line with the time, the sound and the keys.
fn draw_video(f: &mut Frame, app: &mut App, poster: Option<&str>, area: Rect) {
    // The last row stays blank, as under an image.
    let [picture, status, _] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);
    let picture = Rect {
        height: picture.height.min(video::KITTY_MAX_ROWS),
        ..picture
    };
    app.video_area = Some(picture);
    // As for images: nothing drawn under an overlay.
    let covered = app.overlay.is_some();
    let kitty = app.image_picker.protocol_type() == ProtocolType::Kitty;
    let mut shown = false;
    if !covered
        && let Some(player) = &app.video
        && let Some(frame) = player.frame()
    {
        shown = true;
        match &frame.pixels {
            video::Pixels::Zlib(_) if kitty => {
                let cells = video::fit(
                    frame.width,
                    frame.height,
                    picture,
                    app.image_picker.font_size(),
                );
                // A frame goes out once; a draw that only repeats it sends the cells.
                let sent = Some((player.frame_number, cells));
                let transmit = (app.video_sent != sent).then(|| {
                    video::kitty_transmit(
                        video::kitty_id(),
                        frame,
                        cells.width,
                        cells.height,
                        app.image_picker.tmux_detected(),
                    )
                });
                app.video_sent = sent;
                video::kitty_cells(f.buffer_mut(), cells, video::kitty_id(), transmit);
            }
            video::Pixels::Image(protocol) => f.render_widget(Image::new(protocol), picture),
            video::Pixels::Zlib(_) => {}
        }
    }
    if !shown {
        match poster {
            Some(url) => draw_inline_image(f, app, url, picture),
            None if !covered => f.render_widget(
                Paragraph::new("  ▶ video").style(Style::new().fg(OVERLAY)),
                picture,
            ),
            None => {}
        }
    }
    let dim = Style::new().fg(OVERLAY);
    let mut spans = vec![Span::raw(" ")];
    match (
        &app.video,
        app.video.as_ref().and_then(|player| player.error()),
    ) {
        (_, Some(e)) => spans.push(Span::styled(
            format!("✗ video unavailable: {e}"),
            Style::new().fg(RED),
        )),
        (None, _) => spans.push(Span::styled("◌ loading video…", dim)),
        (Some(player), None) => {
            let at = video::clock_text(player.position(Instant::now()));
            let icon = if player.paused() { "⏸" } else { "▶" };
            let time = if player.frame().is_none() {
                "loading video…".to_string()
            } else if let Some(total) = player.duration() {
                format!("{at} / {}", video::clock_text(total))
            } else {
                at
            };
            spans.push(Span::styled(
                format!("{icon} {time}"),
                Style::new().fg(TEXT),
            ));
            // `a` turns the preference over; a video without sound says so.
            let wanted = app.video_sound_wanted();
            spans.push(Span::styled(
                match (player.sound(), wanted) {
                    (true, _) => "   sound on",
                    (false, true) => "   no sound",
                    (false, false) => "   sound off",
                },
                dim,
            ));
            let pause = if player.paused() { "play" } else { "pause" };
            let sound = if wanted { "mute" } else { "sound" };
            for (key, label) in [("P", pause), ("a", sound), (",/.", "seek")] {
                spans.push(Span::raw("   "));
                spans.push(key_chip(key));
                spans.push(Span::styled(format!(" {label}"), dim));
            }
        }
    }
    if !covered {
        f.render_widget(Line::from(spans), status);
    }
}

fn draw_inline_image(f: &mut Frame, app: &mut App, url: &str, area: Rect) {
    // As in `draw_spots`: an image drawn under an overlay is lost (kitty) or leaves
    // stale pixels (sixel, iTerm2), so it waits for the overlay to close.
    let covered = app.overlay.is_some();
    match app.images.get_mut(url) {
        Some(ImageSlot::Ready(_)) if covered => {}
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
        let spoilers = app.post_view.as_ref().is_some_and(PostView::has_spoilers);
        // What `c` does now, if anything: select the comment being read, or on the
        // selected row load more replies, fold or unfold a thread.
        let c = app.post_view.as_ref().and_then(|v| {
            let rows = v.rows();
            if v.free_scroll {
                return (!rows.is_empty()).then_some(("c", "select"));
            }
            match &rows.get(v.cursor)?.kind {
                // Already loading: `c` does nothing until the replies arrive.
                RowKind::More { parent, .. } if v.loading_more.contains(*parent) => None,
                RowKind::More { .. } => Some(("c", "load")),
                RowKind::Comment { comment, .. } if v.collapsed.contains(&comment.id) => {
                    Some(("c", "unfold"))
                }
                RowKind::Comment { comment, .. }
                    if !comment.replies.is_empty() || comment.more.is_some() =>
                {
                    Some(("c", "fold"))
                }
                RowKind::Comment { .. } => None,
            }
        });
        // `m` only with media to open; `b` says which way it goes.
        let shown_post = app
            .post_view
            .as_ref()
            .map(|v| v.page.as_ref().map_or(&v.post, |p| &p.post));
        let media = shown_post
            .is_some_and(|p| p.media.tag().is_some())
            .then_some(("m", "media"));
        let save = if shown_post.is_some_and(|p| app.state.is_bookmarked(&p.id)) {
            ("b", "unsave")
        } else {
            ("b", "save")
        };
        // Pairs that do not fit the width drop from the end.
        let post_hints: Vec<(&str, &str)> = [("j/k", "comments")]
            .into_iter()
            .chain(spoilers.then_some(("v", "spoilers")))
            .chain(c)
            .chain([("space", "page")])
            .chain(media)
            .chain([("o", "browser"), save, ("h", "back")])
            .collect();
        // `x` closes only a tab opened with `p`.
        let closable = matches!(
            app.active_tab().kind,
            TabKind::Subreddit { pinned: false, .. }
        );
        let sidebar_hints: Vec<(&str, &str)> = [
            ("j/k", "tabs"),
            ("g/G", "first/last"),
            ("l/⏎", "list"),
            ("p", "jump"),
        ]
        .into_iter()
        .chain(closable.then_some(("x", "close")))
        .collect();
        // `t` sets the time range of a Top/Controversial listing, not of a search.
        let feed = &app.active_tab().feed;
        let time = (feed.sort.takes_time() && feed.search.is_none()).then_some(("t", "time"));
        let subreddit_hints: Vec<(&str, &str)> = [("j/k", "move"), ("⏎", "open"), ("s", "sort")]
            .into_iter()
            .chain(time)
            .chain([("/", "search"), ("p", "jump")])
            .chain(closable.then_some(("x", "close")))
            .collect();
        let hints: &[(&str, &str)] = if app.post_view.is_some() {
            &post_hints
        } else if app.sidebar_focused {
            &sidebar_hints
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
                TabKind::Subreddit { .. } => &subreddit_hints,
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
        // Keep whole key + label pairs: a key chip without its label means nothing.
        let mut fitted = vec![spans[0].clone()];
        let mut used = spans[0].content.width();
        for pair in spans[1..].chunks(2) {
            let w: usize = pair.iter().map(|s| s.content.width()).sum();
            if used + w > area.width as usize {
                break;
            }
            used += w;
            fitted.extend(pair.iter().cloned());
        }
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
    ("h · l Enter Esc", "focus the sidebar · back to the list"),
    ("j k · g G", "sidebar: next / previous tab · first / last"),
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
    ("Space PgDn · PgUp", "page down · up"),
    ("C-d C-u · C-e C-y", "half page · one line"),
    ("l c Enter", "select the comment read (or left with h)"),
    ("l c Enter again", "fold a thread · load more replies"),
    ("n N", "next / previous top-level comment"),
    ("v", "show / hide spoilers"),
    ("P · a", "video: pause / play · sound on / off"),
    (", .", "video: back / forward 5 seconds"),
    ("g G", "top of post / last comment"),
    ("h", "leave the comment, then back to the list"),
    ("Esc q", "back to the list"),
    ("Anywhere", ""),
    ("o · y", "open on reddit.com · copy link"),
    ("m", "media: image viewer, mpv, or browser"),
    ("b", "bookmark"),
    ("i", "images and videos on / off"),
    ("r · S", "reload · start Redlib"),
    ("q · C-c", "quit"),
];

/// The key list, scrolled with `j`/`k` when the terminal is too short for all of it.
fn draw_help(f: &mut Frame, scroll: &mut usize) {
    let area = popup(f.area(), 68, HELP.len() as u16 + 2);
    let visible = area.height.saturating_sub(2) as usize;
    // j/k keep the popup open even when everything fits, so the title always says so.
    *scroll = (*scroll).min(HELP.len().saturating_sub(visible));
    let lines: Vec<Line> = HELP
        .iter()
        .skip(*scroll)
        .map(|(k, v)| {
            if v.is_empty() {
                Line::styled(
                    k.to_string(),
                    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
                )
            } else {
                Line::from(vec![
                    Span::styled(format!("  {k:<19}"), Style::new().fg(LAVENDER)),
                    Span::styled(v.to_string(), Style::new().fg(SUBTEXT)),
                ])
            }
        })
        .collect();
    let title = "Keys · j/k scroll · any other key closes";
    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new(lines).block(popup_block(title)), area);
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
    // Inside the box's borders: " › " (3) and the caret (1) besides the input.
    let room = (input_area.width as usize).saturating_sub(2 + 3 + 1);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" › ", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(input_tail(input, room), Style::new().fg(TEXT)),
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
    // Besides the query: borders and padding (4 columns), the icon and its space
    // ("🔍 ", 3: the emoji is 2 wide), the caret (1).
    let room = (area.width as usize).saturating_sub(4 + "🔍 ".width() + 1);
    let shown = input_tail(input, room);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("🔍 ", Style::new().fg(SKY)),
            Span::styled(shown, Style::new().fg(TEXT)),
            Span::styled("▏", Style::new().fg(ACCENT)),
        ]))
        .block(popup_block(&title)),
        area,
    );
}

/// Typed input fitted to `room` columns: a longer input shows its end, where you
/// type, after "…".
fn input_tail(input: &str, room: usize) -> String {
    if input.width() <= room {
        return input.to_string();
    }
    let mut tail: Vec<&str> = Vec::new();
    let mut used = 1;
    for g in input.graphemes(true).rev() {
        if used + g.width() > room {
            break;
        }
        used += g.width();
        tail.push(g);
    }
    tail.reverse();
    format!("…{}", tail.concat())
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
            spoiler: false,
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

    /// `open_long_post` with an image and `kitty` graphics.
    fn image_post(body: &str) -> App {
        let (mut app, _) = open_long_post(|_| {});
        app.image_picker = kitty();
        app.show_images = true;
        let image = Media::Image {
            url: "/img/big.jpeg".into(),
        };
        let view = app.post_view.as_mut().unwrap();
        view.post.media = image.clone();
        let page = view.page.as_mut().unwrap();
        page.post.media = image;
        page.body = body.into();
        app
    }

    #[test]
    fn scrolling_a_short_image_post_hides_the_image_until_the_top() {
        let mut app = image_post("short");
        let at_top = screen(&mut app, 100, 40);

        // The text fits without the image: the scroll clamps back to 0.
        press(&mut app, ' ');
        let paged = screen(&mut app, 100, 40);
        let redrawn = screen(&mut app, 100, 40);
        press(&mut app, 'g');
        let top_again = screen(&mut app, 100, 40);
        // Images off and on again at the top show it too.
        press(&mut app, ' ');
        screen(&mut app, 100, 40);
        press(&mut app, 'i');
        press(&mut app, 'i');
        let images_on_again = screen(&mut app, 100, 40);
        // A long post: `k` reading back up to the top line shows it, as C-y does.
        let mut long = image_post("one\n\ntwo");
        screen(&mut long, 100, 40);
        let view = long.post_view.as_mut().unwrap();
        view.page.as_mut().unwrap().body = "para\n\n".repeat(60);
        screen(&mut long, 100, 40);
        long.scroll_post(1);
        screen(&mut long, 100, 40);
        press(&mut long, 'k');
        let k_to_top = screen(&mut long, 100, 40);

        assert!(at_top.contains("loading image"), "{at_top}");
        assert!(!paged.contains("loading image"), "{paged}");
        assert_eq!(paged, redrawn, "a redraw does not bring it back");
        assert!(top_again.contains("loading image"), "{top_again}");
        assert!(
            images_on_again.contains("loading image"),
            "{images_on_again}"
        );
        assert!(k_to_top.contains("loading image"), "{k_to_top}");
    }

    #[test]
    fn the_image_waits_for_the_post_page() {
        let shown = std::cell::RefCell::new(String::new());
        open_long_post(|app| {
            app.image_picker = kitty();
            app.show_images = true;
            app.post_view.as_mut().unwrap().post.media = Media::Image {
                url: "/img/big.jpeg".into(),
            };
            *shown.borrow_mut() = screen(app, 100, 40);
        });
        let loading = shown.into_inner();

        // The list's copy may lack a spoiler tag the page has.
        assert!(!loading.contains("loading image"), "{loading}");
    }

    #[test]
    fn the_footer_says_what_c_does_now() {
        use crate::model::MoreReplies;
        let (mut app, _) = open_long_post(|_| {});
        app.post_view
            .as_mut()
            .unwrap()
            .page
            .as_mut()
            .unwrap()
            .comments[0]
            .more = Some(MoreReplies {
            path: "/r/s/comments/p/x/c".into(),
            count: Some(3),
        });
        let footer = |app: &mut App| {
            app.status = None; // the footer shows a status instead of key hints
            let s = screen(app, 100, 30);
            s.lines().last().unwrap_or_default().to_string()
        };
        let reading = footer(&mut app);
        press(&mut app, 'j');
        let on_comment = footer(&mut app);
        press(&mut app, 'j');
        let on_more = footer(&mut app);
        // Asked for: the row says "loading replies", and `c` does nothing more.
        press(&mut app, 'c');
        let loading = footer(&mut app);
        press(&mut app, 'k');
        press(&mut app, 'c');
        let folded = footer(&mut app);
        // A comment without replies: `c` does nothing there, so no hint.
        let (mut leaf, _) = open_long_post(|_| {});
        press(&mut leaf, 'j');
        let on_leaf = footer(&mut leaf);
        // `x` closes only tabs opened with `p`; r/s is a subscription.
        let mut feed = feed_app(Picker::halfblocks());
        let on_subscription = footer(&mut feed);
        feed.sidebar_focused = true;
        let in_sidebar = footer(&mut feed);

        assert!(reading.contains("select"), "{reading}");
        assert!(on_comment.contains(" fold"), "{on_comment}");
        assert!(on_more.contains("load"), "{on_more}");
        assert!(!loading.contains("load"), "{loading}");
        assert!(folded.contains("unfold"), "{folded}");
        assert!(!on_leaf.contains(" c "), "{on_leaf}");
        assert!(!on_subscription.contains("close"), "{on_subscription}");
        assert!(
            in_sidebar.contains("tabs") && !in_sidebar.contains("close"),
            "{in_sidebar}"
        );
    }

    /// `image_post("short")` with a rewrapping spoiler in its first comment, a
    /// "more replies" row under it and `n` "marker N" comments after; drawn.
    fn image_post_with_comments(n: usize) -> App {
        use crate::model::MoreReplies;
        let mut app = image_post("short");
        let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
        let reply = page.comments[0].clone();
        page.comments[0].body = (31..=36)
            .map(|n| format!("aaaaaaaaaa {SPOILER_START}{}{SPOILER_END}", "ab-".repeat(n)))
            .collect::<Vec<_>>()
            .join("\n\n");
        page.comments[0].more = Some(MoreReplies {
            path: "/r/s/comments/p/x/c".into(),
            count: Some(2),
        });
        for i in 1..=n {
            page.comments.push(Comment {
                id: format!("m{i}"),
                body: format!("marker {i}"),
                ..reply.clone()
            });
        }
        screen(&mut app, 100, 60);
        app
    }

    fn replies(n: usize) -> Vec<Comment> {
        let (app, _) = open_long_post(|_| {});
        let reply = app
            .post_view
            .as_ref()
            .unwrap()
            .page
            .as_ref()
            .unwrap()
            .comments[0]
            .clone();
        (0..n)
            .map(|i| Comment {
                id: format!("r{i}"),
                ..reply.clone()
            })
            .collect()
    }

    #[test]
    fn reading_the_blank_line_under_a_loading_more_row_keeps_the_text() {
        let mut app = image_post_with_comments(37);
        app.post_view
            .as_mut()
            .unwrap()
            .page
            .as_mut()
            .unwrap()
            .comments[0]
            .body = "first".into();
        screen(&mut app, 100, 30);
        // The top line is the blank line under "↳ 2 more replies" (row 1).
        let view = app.post_view.as_mut().unwrap();
        view.scroll = view.row_starts[1] + 1;
        let row_of = |s: &str| s.lines().position(|l| l.contains("marker 1 "));
        let before = row_of(&screen(&mut app, 100, 30));

        let generation = app.post_view.as_ref().unwrap().generation;
        app.on_msg(Msg::MoreReplies {
            generation,
            comment_id: "c".into(),
            result: Ok(replies(2)),
        });
        let after = row_of(&screen(&mut app, 100, 30));

        assert!(before.is_some());
        assert_eq!(after, before);
    }

    #[test]
    fn folding_a_comment_with_only_unloaded_replies_shows_it_folded() {
        use crate::model::MoreReplies;
        let (mut app, _) = open_long_post(|_| {});
        app.post_view
            .as_mut()
            .unwrap()
            .page
            .as_mut()
            .unwrap()
            .comments[0]
            .more = Some(MoreReplies {
            path: "/r/s/comments/p/x/c".into(),
            count: Some(3),
        });
        screen(&mut app, 100, 30);
        press(&mut app, 'j');
        press(&mut app, 'c');
        let s = screen(&mut app, 100, 30);

        assert!(s.contains("+3 folded"), "{s}");
        assert!(!s.contains("more replies"), "{s}");
    }

    #[test]
    fn footer_hints_follow_the_post_and_the_tab() {
        let footer = |app: &mut App| {
            app.status = None;
            let s = screen(app, 160, 30);
            s.lines().last().unwrap_or_default().to_string()
        };
        // No media: no `m`; bookmarked: `b` unsaves.
        let (mut app, _) = open_long_post(|_| {});
        let plain = footer(&mut app);
        // In memory: `b` would also save the state file.
        let post = app.post_view.as_ref().unwrap().post.clone();
        app.state.toggle_bookmark(&post, 0);
        let saved = footer(&mut app);
        // A tab opened with `p`, sorted by top: `t` and `x` both apply.
        let mut feed = feed_app(Picker::halfblocks());
        feed.open_subreddit("foo");
        let active = feed.active;
        feed.tabs[active].feed.sort = crate::model::Sort::Top;
        let opened = footer(&mut feed);
        feed.tabs[active].feed.search = Some("rust".into());
        let searching = footer(&mut feed);

        assert!(!plain.contains("media"), "{plain}");
        assert!(plain.contains(" save"), "{plain}");
        assert!(saved.contains("unsave"), "{saved}");
        assert!(
            opened.contains("time") && opened.contains("close"),
            "{opened}"
        );
        assert!(
            !searching.contains("time") && searching.contains("close"),
            "{searching}"
        );
    }

    #[test]
    fn a_held_selection_at_the_top_keeps_the_image() {
        let mut app = image_post_with_comments(37);
        // Two rewrapping spoiler paragraphs in the first comment, so "marker 1"
        // shows under the image.
        let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
        page.comments[0].body = (31..=32)
            .map(|n| format!("aaaaaaaaaa {SPOILER_START}{}{SPOILER_END}", "ab-".repeat(n)))
            .collect::<Vec<_>>()
            .join("\n\n");
        screen(&mut app, 100, 60);
        // Reveal; select "marker 1" (row 2); `h`, `l` hold it at scroll 0.
        for key in ['v', 'j', 'j', 'j', 'h', 'l'] {
            press(&mut app, key);
            screen(&mut app, 100, 60);
        }
        let view = app.post_view.as_ref().unwrap();
        let held = (view.cursor, view.hold_view.is_some(), view.scroll);

        // Hiding the spoilers again adds lines above the held comment.
        press(&mut app, 'v');
        let s = screen(&mut app, 100, 60);

        assert_eq!(held, (2, true, 0));
        assert!(s.contains("loading image") && s.contains("marker 1"), "{s}");
        assert_eq!(app.post_view.as_ref().unwrap().scroll, 0);
    }

    /// `image_post_with_comments(37)` with a short first comment and "marker 1"
    /// selected under the image (`j j j`), at scroll 0.
    fn marker_selected_under_the_image() -> App {
        let mut app = image_post_with_comments(37);
        app.post_view
            .as_mut()
            .unwrap()
            .page
            .as_mut()
            .unwrap()
            .comments[0]
            .body = "first".into();
        screen(&mut app, 100, 60);
        for key in ['j', 'j', 'j'] {
            press(&mut app, key);
            screen(&mut app, 100, 60);
        }
        assert_eq!(app.post_view.as_ref().unwrap().cursor, 2);
        app
    }

    fn load_two_replies_above(app: &mut App) {
        let generation = app.post_view.as_ref().unwrap().generation;
        app.on_msg(Msg::MoreReplies {
            generation,
            comment_id: "c".into(),
            result: Ok(replies(2)),
        });
    }

    #[test]
    fn a_hold_kept_for_the_image_holds_the_text_once_the_image_is_off() {
        use crate::model::MoreReplies;
        let mut app = marker_selected_under_the_image();
        // "marker 2" gets a "more replies" row of its own, below "marker 1".
        let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
        page.comments[2].more = Some(MoreReplies {
            path: "/r/s/comments/p/x/m2".into(),
            count: Some(1),
        });
        screen(&mut app, 100, 60);
        // `h` `l` hold "marker 1"; a load below it at the top pins the hold there.
        for key in ['h', 'l'] {
            press(&mut app, key);
            screen(&mut app, 100, 60);
        }
        let generation = app.post_view.as_ref().unwrap().generation;
        app.on_msg(Msg::MoreReplies {
            generation,
            comment_id: "m2".into(),
            result: Ok(replies(1)),
        });
        screen(&mut app, 100, 60);
        let top = app
            .post_view
            .as_ref()
            .unwrap()
            .hold_view
            .is_some_and(|h| h.top);
        let row_of = |s: &str| s.lines().position(|l| l.contains("marker 1 "));
        press(&mut app, 'i');
        let before = row_of(&screen(&mut app, 100, 60));

        load_two_replies_above(&mut app);
        let after = row_of(&screen(&mut app, 100, 60));

        assert!(top, "the hold was pinned to the top");
        assert!(before.is_some());
        assert_eq!(after, before, "no image to keep: the text stays");
    }

    #[test]
    fn images_on_and_a_load_in_one_pass_show_the_image() {
        let mut app = marker_selected_under_the_image();
        press(&mut app, 'i');
        screen(&mut app, 100, 60);

        press(&mut app, 'i');
        load_two_replies_above(&mut app);
        let s = screen(&mut app, 100, 60);

        assert!(s.contains("loading image"), "{s}");
        assert_eq!(app.post_view.as_ref().unwrap().scroll, 0);
    }

    #[test]
    fn a_tall_held_comment_at_the_top_stays_there_when_text_below_changes() {
        let mut app = image_post("short");
        let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
        let first = page.comments[0].clone();
        page.comments[0].body = "para\n\n".repeat(40);
        page.comments.push(Comment {
            id: "below".into(),
            body: (31..=36)
                .map(|n| format!("aaaaaaaaaa {SPOILER_START}{}{SPOILER_END}", "ab-".repeat(n)))
                .collect::<Vec<_>>()
                .join("\n\n"),
            ..first
        });
        screen(&mut app, 100, 60);
        // `l` selects and holds the tall first comment under the image.
        press(&mut app, 'l');
        screen(&mut app, 100, 60);

        // A spoiler only below it is revealed.
        press(&mut app, 'v');
        let s = screen(&mut app, 100, 60);

        assert!(s.contains("loading image"), "{s}");
        assert_eq!(app.post_view.as_ref().unwrap().scroll, 0);
    }

    #[test]
    fn a_page_key_in_the_same_pass_as_a_load_keeps_the_reading_position() {
        let first_marker = |load: bool| {
            let mut app = image_post_with_comments(37);
            // A short first comment: a page down puts the top line in the markers,
            // below where the replies load.
            let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
            page.comments[0].body = "first".into();
            screen(&mut app, 100, 60);
            press(&mut app, ' ');
            if load {
                let generation = app.post_view.as_ref().unwrap().generation;
                app.on_msg(Msg::MoreReplies {
                    generation,
                    comment_id: "c".into(),
                    result: Ok(replies(2)),
                });
            }
            let s = screen(&mut app, 100, 60);
            s.lines()
                .enumerate()
                .find_map(|(row, l)| l.split("marker ").nth(1).map(|r| (row, r.to_string())))
        };

        let without = first_marker(false);
        assert!(without.is_some());
        assert_eq!(first_marker(true), without);
    }

    #[test]
    fn changes_above_a_selection_at_the_top_keep_the_image() {
        use crate::model::MoreReplies;
        let mut app = image_post("short");
        let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
        let reply = page.comments[0].clone();
        page.comments[0].more = Some(MoreReplies {
            path: "/r/s/comments/p/x/c".into(),
            count: Some(2),
        });
        page.comments[0].body = format!("so {SPOILER_START}{}{SPOILER_END}", "ab-".repeat(40));
        page.comments.push(Comment {
            id: "c2".into(),
            body: "marker two".into(),
            ..reply.clone()
        });
        // Select c2 under the image, at the top of the post; tall enough that the
        // loaded replies leave it on screen.
        let draw = |app: &mut App| screen(app, 100, 60);
        draw(&mut app);
        for key in ['j', 'j', 'j'] {
            press(&mut app, key);
            draw(&mut app);
        }
        let before = draw(&mut app);
        assert_eq!(app.post_view.as_ref().unwrap().cursor, 2, "c2 selected");

        // Spoilers above it rewrap, then replies load above it.
        press(&mut app, 'v');
        let after_v = draw(&mut app);
        app.on_msg(Msg::MoreReplies {
            generation: app.post_view.as_ref().unwrap().generation,
            comment_id: "c".into(),
            result: Ok((0..2)
                .map(|i| Comment {
                    id: format!("r{i}"),
                    ..reply.clone()
                })
                .collect()),
        });
        let after_load = draw(&mut app);

        assert!(
            before.contains("loading image") && before.contains("marker two"),
            "{before}"
        );
        assert!(after_v.contains("loading image"), "{after_v}");
        assert!(
            after_load.contains("loading image") && after_load.contains("marker two"),
            "{after_load}"
        );
        assert_eq!(app.post_view.as_ref().unwrap().scroll, 0);
    }

    #[test]
    fn selecting_a_comment_below_an_image_post_settles_in_one_frame() {
        let (mut app, _) = open_long_post(|_| {});
        app.image_picker = kitty();
        app.show_images = true;
        let image = Media::Image {
            url: "/img/big.jpeg".into(),
        };
        let view = app.post_view.as_mut().unwrap();
        view.post.media = image.clone();
        view.page.as_mut().unwrap().post.media = image;
        let at_top = screen(&mut app, 100, 40);

        // `j` selects the comment under the 60-paragraph body.
        press(&mut app, 'j');
        let first = screen(&mut app, 100, 40);
        let scroll = app.post_view.as_ref().unwrap().scroll;
        let second = screen(&mut app, 100, 40);

        assert!(at_top.contains("loading image"), "{at_top}");
        assert!(!first.contains("loading image"), "{first}");
        assert_eq!(first, second, "no jump on the next redraw");
        assert_eq!(app.post_view.as_ref().unwrap().scroll, scroll);
    }

    #[test]
    fn replies_loading_above_a_selected_comment_keep_it_in_place() {
        use crate::model::MoreReplies;
        let (mut app, _) = open_long_post(|_| {});
        let generation = app.post_view.as_ref().unwrap().generation;
        let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
        let reply = page.comments[0].clone();
        page.comments[0].more = Some(MoreReplies {
            path: "/r/s/comments/p/x/c".into(),
            count: Some(2),
        });
        page.comments.push(Comment {
            id: "c2".into(),
            body: "marker two".into(),
            ..reply.clone()
        });
        screen(&mut app, 100, 30);
        // Rows c, more(c), c2: select c2 with its header mid-screen.
        let view = app.post_view.as_mut().unwrap();
        view.cursor = 2;
        view.free_scroll = false;
        view.scroll = view.row_starts[2] - 10;
        let row_of = |s: &str| s.lines().position(|l| l.contains("marker two"));
        let before = row_of(&screen(&mut app, 100, 30));

        app.on_msg(Msg::MoreReplies {
            generation,
            comment_id: "c".into(),
            result: Ok((0..2)
                .map(|i| Comment {
                    id: format!("r{i}"),
                    ..reply.clone()
                })
                .collect()),
        });
        let after = row_of(&screen(&mut app, 100, 30));

        assert!(before.is_some());
        assert_eq!(after, before, "the selected comment did not move");
    }

    #[test]
    fn revealing_spoilers_above_keeps_the_text_being_read() {
        let (mut app, _) = open_long_post(|_| {});
        let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
        // Hyphens are break points; the mask has none, so lines break elsewhere.
        page.body = (31..=36)
            .map(|n| format!("aaaaaaaaaa {SPOILER_START}{}{SPOILER_END}", "ab-".repeat(n)))
            .collect::<Vec<_>>()
            .join("\n\n");
        page.comments[0].body = (1..=40)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n\n");
        screen(&mut app, 100, 30);
        let view = app.post_view.as_mut().unwrap();
        let masked_start = view.row_starts[0];
        view.scroll = masked_start + 10;
        let shown = |s: &str| {
            s.lines()
                .enumerate()
                .find_map(|(row, l)| l.split("line ").nth(1).map(|r| (row, r.to_string())))
        };
        let before = shown(&screen(&mut app, 100, 30));

        press(&mut app, 'v');
        let after = shown(&screen(&mut app, 100, 30));
        let revealed_start = app.post_view.as_ref().unwrap().row_starts[0];

        assert_ne!(masked_start, revealed_start, "the reveal rewraps the body");
        assert!(before.is_some());
        assert_eq!(after, before, "the text being read stays");
    }

    #[test]
    fn a_selection_moved_in_the_same_pass_as_a_load_still_shows() {
        use crate::model::MoreReplies;
        let (mut app, _) = open_long_post(|_| {});
        let generation = app.post_view.as_ref().unwrap().generation;
        let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
        let reply = page.comments[0].clone();
        page.comments[0].more = Some(MoreReplies {
            path: "/r/s/comments/p/x/c".into(),
            count: Some(2),
        });
        page.comments.push(Comment {
            id: "tall".into(),
            body: "words\n\n".repeat(40),
            ..reply.clone()
        });
        page.comments.push(Comment {
            id: "b".into(),
            ..reply.clone()
        });
        screen(&mut app, 100, 30);
        // Rows c, more(c), tall, b: select b, then `k` and a load in one pass.
        let view = app.post_view.as_mut().unwrap();
        view.cursor = 3;
        view.free_scroll = false;
        view.scroll = view.row_starts[3] - 10;
        screen(&mut app, 100, 30);
        press(&mut app, 'k');
        app.on_msg(Msg::MoreReplies {
            generation,
            comment_id: "c".into(),
            result: Ok((0..2)
                .map(|i| Comment {
                    id: format!("r{i}"),
                    ..reply.clone()
                })
                .collect()),
        });
        screen(&mut app, 100, 30);
        let view = app.post_view.as_ref().unwrap();

        assert_eq!(
            view.scroll, view.row_starts[view.cursor],
            "the header of `tall` shows"
        );
    }

    /// A post whose body has spoilers that rewrap when revealed, then `body_tail`
    /// lines, then a tall comment; drawn once.
    fn post_with_rewrapping_spoilers(body_tail: usize) -> App {
        let (mut app, _) = open_long_post(|_| {});
        let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
        // Hyphens are break points; the mask has none, so lines break elsewhere.
        let spoilers = (31..=36)
            .map(|n| format!("aaaaaaaaaa {SPOILER_START}{}{SPOILER_END}", "ab-".repeat(n)));
        let tail = (1..=body_tail).map(|i| format!("body {i}"));
        page.body = spoilers.chain(tail).collect::<Vec<_>>().join("\n\n");
        page.comments[0].body = (1..=40)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n\n");
        screen(&mut app, 100, 30);
        app
    }

    #[test]
    fn a_scroll_key_after_v_in_the_same_batch_still_scrolls() {
        // Each batch of keys is handled before one draw, as the main loop does.
        let scroll_after = |batches: &[&[char]]| {
            let mut app = post_with_rewrapping_spoilers(0);
            let view = app.post_view.as_mut().unwrap();
            view.scroll = view.row_starts[0] + 10;
            screen(&mut app, 100, 30);
            for batch in batches {
                for &key in *batch {
                    press(&mut app, key);
                }
                screen(&mut app, 100, 30);
            }
            app.post_view.as_ref().unwrap().scroll
        };

        // Paging after `v` moves on from the kept text, drawn or not in between.
        assert_eq!(
            scroll_after(&[&['v', ' ']]),
            scroll_after(&[&['v'], &[' ']])
        );
        // Hiding again adds lines above; `g` in the same batch still goes to the top.
        assert_eq!(scroll_after(&[&['v'], &['v', 'g']]), 0);
    }

    #[test]
    fn revealing_spoilers_above_the_body_being_read_keeps_it() {
        let mut app = post_with_rewrapping_spoilers(40);
        let view = app.post_view.as_mut().unwrap();
        // Inside the plain body lines below the spoilers.
        view.scroll = view.row_starts[0] - 30;
        let shown = |s: &str| {
            s.lines()
                .enumerate()
                .find_map(|(row, l)| l.split("body ").nth(1).map(|r| (row, r.to_string())))
        };
        let before = shown(&screen(&mut app, 100, 30));

        press(&mut app, 'v');
        let after = shown(&screen(&mut app, 100, 30));

        assert!(before.is_some());
        assert_eq!(after, before);
    }

    #[test]
    fn revealing_spoilers_below_the_body_being_read_leaves_it_alone() {
        let mut app = post_with_rewrapping_spoilers(0);
        let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
        let head = (1..=40).map(|i| format!("body {i}")).collect::<Vec<_>>();
        page.body = format!("{}\n\n{}", head.join("\n\n"), page.body);
        let shown = |s: &str| {
            s.lines()
                .enumerate()
                .find_map(|(row, l)| l.split("body ").nth(1).map(|r| (row, r.to_string())))
        };
        let mut before = Vec::new();
        let mut after = Vec::new();
        for scroll in [0, 10] {
            screen(&mut app, 100, 30);
            app.post_view.as_mut().unwrap().scroll = scroll;
            before.push(shown(&screen(&mut app, 100, 30)));
            press(&mut app, 'v');
            after.push(shown(&screen(&mut app, 100, 30)));
        }

        assert!(before.iter().all(Option::is_some));
        assert_eq!(after, before);
    }

    #[test]
    fn moving_the_selection_in_the_same_pass_as_a_load_keeps_the_text() {
        use crate::model::MoreReplies;
        // Rows c, more(c), a, b, tall; `a` selected, then `j` and a load of 2 replies.
        let run = |draw_between: bool| {
            let (mut app, _) = open_long_post(|_| {});
            let generation = app.post_view.as_ref().unwrap().generation;
            let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
            let reply = page.comments[0].clone();
            page.comments[0].more = Some(MoreReplies {
                path: "/r/s/comments/p/x/c".into(),
                count: Some(2),
            });
            for (id, body) in [("a", "marker a"), ("b", "marker b")] {
                page.comments.push(Comment {
                    id: id.into(),
                    body: body.into(),
                    ..reply.clone()
                });
            }
            page.comments.push(Comment {
                id: "tall".into(),
                body: "words\n\n".repeat(40),
                ..reply.clone()
            });
            screen(&mut app, 100, 30);
            let view = app.post_view.as_mut().unwrap();
            view.cursor = 2;
            view.free_scroll = false;
            view.scroll = view.row_starts[2] - 10;
            screen(&mut app, 100, 30);
            press(&mut app, 'j');
            if draw_between {
                screen(&mut app, 100, 30);
            }
            app.on_msg(Msg::MoreReplies {
                generation,
                comment_id: "c".into(),
                result: Ok((0..2)
                    .map(|i| Comment {
                        id: format!("r{i}"),
                        ..reply.clone()
                    })
                    .collect()),
            });
            let s = screen(&mut app, 100, 30);
            s.lines().position(|l| l.contains("marker b"))
        };

        let same_pass = run(false);
        assert!(same_pass.is_some());
        assert_eq!(same_pass, run(true));
    }

    #[test]
    fn a_scroll_key_after_v_on_a_selected_comment_moves_on_from_its_text() {
        // Returns the first visible "line N" and the scroll.
        let after = |batches: &[&[char]]| {
            let mut app = post_with_rewrapping_spoilers(0);
            let view = app.post_view.as_mut().unwrap();
            view.cursor = 0;
            view.free_scroll = false;
            view.scroll = view.row_starts[0] + 10;
            screen(&mut app, 100, 30);
            for batch in batches {
                for &key in *batch {
                    press(&mut app, key);
                }
                screen(&mut app, 100, 30);
            }
            let s = screen(&mut app, 100, 30);
            let first = s
                .lines()
                .find_map(|l| l.split("line ").nth(1).map(str::to_string));
            (first, app.post_view.as_ref().unwrap().scroll)
        };

        assert_eq!(after(&[&['v', ' ']]).0, after(&[&['v'], &[' ']]).0);
        // Hiding again adds lines above; `g` in the same batch still goes to the top.
        assert_eq!(after(&[&['v'], &['v', 'g']]).1, 0);
    }

    #[test]
    fn spoilers_stay_masked_until_v_reveals_them() {
        use crate::model::{SPOILER_END, SPOILER_START};
        let (mut app, _) = open_long_post(|_| {});
        let view = app.post_view.as_mut().unwrap();
        let page = view.page.as_mut().unwrap();
        page.body = "the butler did it".into();
        page.comments[0].body = format!("so {SPOILER_START}Rosebud{SPOILER_END} was the sled");

        // Untagged post: only the `>!…!<` text is masked.
        let masked = screen(&mut app, 100, 30);
        // Spoiler-tagged post: the body and the comments stay hidden.
        app.post_view.as_mut().unwrap().post.spoiler = true;
        let hidden = screen(&mut app, 100, 30);
        // No comment can be selected or folded while unseen.
        let rows_hidden = app.post_view.as_ref().unwrap().rows().is_empty();
        press(&mut app, 'v');
        let shown = screen(&mut app, 100, 30);

        assert!(masked.contains("the butler did it"), "{masked}");
        assert!(masked.contains("so ▒▒▒▒▒▒▒ was the sled"), "{masked}");
        assert!(
            hidden.contains("v reveals the post and its comment"),
            "{hidden}"
        );
        assert!(
            !hidden.contains("butler") && !hidden.contains("the sled"),
            "{hidden}"
        );
        assert!(!hidden.contains("no comments yet"), "{hidden}");
        assert!(rows_hidden);
        assert!(shown.contains("the butler did it"), "{shown}");
        assert!(shown.contains("so Rosebud was the sled"), "{shown}");
    }

    #[test]
    fn comment_keys_on_a_hidden_spoiler_post_select_nothing() {
        let (mut app, _) = open_long_post(|_| {});
        app.post_view.as_mut().unwrap().post.spoiler = true;
        let reading = |app: &App| app.post_view.as_ref().unwrap().free_scroll;

        screen(&mut app, 100, 30);
        press(&mut app, 'G');
        let after_g = reading(&app);
        // Reveal and draw, then hide and press `j` before the next draw.
        press(&mut app, 'v');
        screen(&mut app, 100, 30);
        press(&mut app, 'v');
        press(&mut app, 'j');
        let after_hide_j = reading(&app);
        // Hidden (positions cleared), then reveal and `G` before the next draw:
        // `G` scrolls down to the last comment, it does not hold the view.
        screen(&mut app, 100, 30);
        press(&mut app, 'v');
        press(&mut app, 'G');
        let g_holds = app.post_view.as_ref().unwrap().hold_view.is_some();

        assert!(after_g, "`G` selects no hidden comment");
        assert!(after_hide_j, "`j` uses no stale comment positions");
        assert!(!g_holds, "`G` trusts no stale comment positions");
    }

    #[test]
    fn selecting_the_comment_being_read_keeps_the_view() {
        let (mut app, _) = open_long_post(|_| {});
        let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
        page.comments[0].body = (1..=40)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n\n");
        // Select the tall comment, then page down inside it: its header leaves.
        for key in ['j', ' ', ' '] {
            press(&mut app, key);
            screen(&mut app, 100, 30);
        }
        let reading = app.post_view.as_ref().unwrap().scroll;

        press(&mut app, 'l');
        screen(&mut app, 100, 30);
        // Later redraws (a status timing out, a health check) keep it too.
        let s = screen(&mut app, 100, 30);
        let (selected, held) = {
            let view = app.post_view.as_ref().unwrap();
            (!view.free_scroll, view.scroll)
        };
        // `j` on the last comment cannot move the selection: the view stays too.
        press(&mut app, 'j');
        screen(&mut app, 100, 30);
        let after_j = app.post_view.as_ref().unwrap().scroll;
        // `G` while reading inside the last comment stays too.
        press(&mut app, 'h');
        press(&mut app, ' ');
        screen(&mut app, 100, 30);
        let paged = app.post_view.as_ref().unwrap().scroll;
        press(&mut app, 'G');
        screen(&mut app, 100, 30);
        let after_g = app.post_view.as_ref().unwrap().scroll;

        assert!(selected, "`l` selected the comment");
        assert_eq!(held, reading, "and left the text where it was");
        assert!(
            s.lines().any(|l| l.contains("▌") && l.contains("line ")),
            "the selection bar shows beside its text: {s}"
        );
        assert_eq!(after_j, reading, "no jump up to the header");
        assert!(paged > reading);
        assert_eq!(after_g, paged, "`G` inside the last comment stays");
    }

    #[test]
    fn more_replies_loading_above_keep_the_text_being_read() {
        use crate::model::MoreReplies;
        let first_line = |s: &str| {
            s.lines()
                .find_map(|l| l.split("line ").nth(1))
                .map(|rest| rest.split_whitespace().next().unwrap_or("").to_string())
        };
        // Read into a tall last comment below two "more replies" rows; then `keys`,
        // with a draw after each only when `draw_between`; then 20 replies load
        // under each comment in `loads`, all before one draw.
        let run = |keys: &[char], draw_between: bool, loads: &[&str]| {
            let (mut app, _) = open_long_post(|_| {});
            let generation = app.post_view.as_ref().unwrap().generation;
            let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
            let reply = page.comments[0].clone();
            let mut tall = reply.clone();
            tall.id = "tall".into();
            tall.body = (1..=40)
                .map(|i| format!("line {i}"))
                .collect::<Vec<_>>()
                .join("\n\n");
            page.comments[0].more = Some(MoreReplies {
                path: "/r/s/comments/p/x/c".into(),
                count: Some(20),
            });
            page.comments.push(Comment {
                id: "c2".into(),
                ..page.comments[0].clone()
            });
            page.comments.push(tall);
            for key in ['G', ' '] {
                press(&mut app, key);
                screen(&mut app, 100, 30);
            }
            let before = first_line(&screen(&mut app, 100, 30));
            for &key in keys {
                press(&mut app, key);
                if draw_between {
                    screen(&mut app, 100, 30);
                }
            }
            for parent in loads {
                let replies = (0..20)
                    .map(|i| Comment {
                        id: format!("{parent}-r{i}"),
                        ..reply.clone()
                    })
                    .collect();
                app.on_msg(Msg::MoreReplies {
                    generation,
                    comment_id: parent.to_string(),
                    result: Ok(replies),
                });
            }
            (before, first_line(&screen(&mut app, 100, 30)))
        };

        let (before, reading) = run(&[], true, &["c"]);
        let (_, selected) = run(&['l'], true, &["c"]);
        // The main loop handles keys, then messages, then draws: `l` and the
        // replies, or two loads, can land before one draw.
        let (_, same_pass) = run(&['l'], false, &["c"]);
        let (_, two_loads) = run(&[], true, &["c", "c2"]);

        assert!(
            before.as_deref().is_some_and(|n| n != "1"),
            "reading inside: {before:?}"
        );
        assert_eq!(reading, before, "while reading");
        assert_eq!(selected, before, "with the comment selected by `l`");
        assert_eq!(same_pass, before, "with `l` in the same pass");
        assert_eq!(two_loads, before, "with two loads in one pass");
    }

    #[test]
    fn reading_at_a_more_row_that_loads_with_one_above_keeps_its_place() {
        use crate::model::MoreReplies;
        let (mut app, _) = open_long_post(|_| {});
        let generation = app.post_view.as_ref().unwrap().generation;
        let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
        let reply = page.comments[0].clone();
        page.comments[0].more = Some(MoreReplies {
            path: "/r/s/comments/p/x/c".into(),
            count: Some(20),
        });
        page.comments.push(Comment {
            id: "c2".into(),
            ..page.comments[0].clone()
        });
        page.comments.push(Comment {
            id: "tall".into(),
            body: "words\n\n".repeat(40),
            ..reply.clone()
        });
        screen(&mut app, 100, 30);
        // Rows c, more(c), c2, more(c2), tall: read with more(c2) on the top line.
        let view = app.post_view.as_mut().unwrap();
        view.scroll = view.row_starts[3];
        screen(&mut app, 100, 30);

        // Both load before one draw: c's replies above, c2's in place of the row.
        for parent in ["c", "c2"] {
            let replies = (0..20)
                .map(|i| Comment {
                    id: format!("{parent}-r{i}"),
                    ..reply.clone()
                })
                .collect();
            app.on_msg(Msg::MoreReplies {
                generation,
                comment_id: parent.into(),
                result: Ok(replies),
            });
        }
        screen(&mut app, 100, 30);
        let view = app.post_view.as_ref().unwrap();
        let first_new = view
            .rows()
            .iter()
            .position(
                |r| matches!(r.kind, RowKind::Comment { comment, .. } if comment.id == "c2-r0"),
            )
            .unwrap();

        assert_eq!(
            view.scroll, view.row_starts[first_new],
            "c2's first new reply on top"
        );
    }

    #[test]
    fn folding_the_comment_being_read_shows_its_header() {
        let (mut app, _) = open_long_post(|_| {});
        let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
        let mut reply = page.comments[0].clone();
        reply.id = "r".into();
        let tall_body = (1..=40)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n\n");
        page.comments[0].body = tall_body.clone();
        page.comments[0].replies = vec![reply.clone()];
        // A tall thread below, so the folded document still scrolls this far.
        page.comments.push(Comment {
            id: "next".into(),
            body: tall_body,
            ..reply
        });
        // Select the comment, scroll its header just off screen, hold it with `l`.
        press(&mut app, 'j');
        screen(&mut app, 100, 30);
        app.scroll_post(1);
        for key in ['l', 'l'] {
            screen(&mut app, 100, 30);
            press(&mut app, key);
        }
        let s = screen(&mut app, 100, 30);

        assert!(s.contains("+1 folded"), "{s}");
    }

    #[test]
    fn folding_two_threads_before_a_draw_keeps_the_comment_left_by_h() {
        let (mut app, _) = open_long_post(|_| {});
        let view = app.post_view.as_mut().unwrap();
        let page = view.page.as_mut().unwrap();
        let template = page.comments[0].clone();
        let comment = |id: &str, body: &str, replies: Vec<Comment>| Comment {
            id: id.into(),
            body: body.into(),
            replies,
            ..template.clone()
        };
        page.body.clear();
        page.comments = vec![
            comment(
                "A",
                &"long line\n".repeat(40),
                vec![comment("a", "reply", vec![])],
            ),
            comment("B", "short", vec![comment("b", "reply", vec![])]),
            comment("C", &"tail line\n".repeat(40), vec![]),
        ];
        view.collapsed.insert("B".into());
        view.cursor = 0;
        view.free_scroll = false;
        screen(&mut app, 100, 30);
        let view = app.post_view.as_mut().unwrap();
        view.scroll = view.row_starts[0];
        screen(&mut app, 100, 30);

        // Replace A's child with B's before a draw: the row count stays four,
        // but their positions change. Reselect only after the draw places them.
        press(&mut app, 'c');
        press(&mut app, 'j');
        press(&mut app, 'c');
        press(&mut app, 'h');
        press(&mut app, 'l');

        assert!(app.post_view.as_ref().unwrap().free_scroll);
        screen(&mut app, 100, 30);
        press(&mut app, 'l');
        let view = app.post_view.as_ref().unwrap();
        assert!(matches!(
            &view.rows()[view.cursor].kind,
            RowKind::Comment { comment, .. } if comment.id == "B"
        ));
    }

    #[test]
    fn upward_keys_in_a_long_body_read_upward() {
        let (mut app, _) = open_long_post(|_| {});
        press(&mut app, ' ');
        screen(&mut app, 100, 30);
        let paged = app.post_view.as_ref().unwrap().scroll;

        press(&mut app, 'k');
        screen(&mut app, 100, 30);
        let after_k = app.post_view.as_ref().unwrap().scroll;
        press(&mut app, 'N');
        screen(&mut app, 100, 30);
        let view = app.post_view.as_ref().unwrap();

        assert!(paged > 0);
        assert_eq!(after_k, paged - 1, "`k` scrolls up a line, not down");
        assert_eq!(view.scroll, 0, "`N` from the body goes to the post's top");
        assert!(view.free_scroll);
    }

    #[test]
    fn l_after_h_goes_back_to_the_same_comment_without_folding_it() {
        let (mut app, _) = open_long_post(|_| {});
        let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
        let mut reply = page.comments[0].clone();
        reply.id = "r".into();
        page.comments[0].replies = vec![reply];
        let mut back_to = |keys: &[char]| {
            for &key in keys.iter().chain(&['h']) {
                press(&mut app, key);
                screen(&mut app, 100, 30);
            }
            press(&mut app, 'l');
            let view = app.post_view.as_ref().unwrap();
            (view.free_scroll, view.cursor, view.collapsed.len())
        };

        // The thread with a reply: selected again, not folded.
        let thread = back_to(&['G', 'k']);
        // The reply under it, both on screen: the reply, not the first comment shown.
        let reply = back_to(&['G']);

        assert_eq!(thread, (false, 0, 0));
        assert_eq!(reply, (false, 1, 0));
    }

    #[test]
    fn no_color_with_half_blocks_says_why_there_are_no_images() {
        let mut app = feed_app(Picker::halfblocks());
        app.no_color = true;

        press(&mut app, 'i');
        let s = screen(&mut app, 100, 20);

        assert!(app.show_images, "i changes nothing it cannot show");
        assert!(
            app.status
                .as_ref()
                .is_some_and(|st| st.text.contains("NO_COLOR")),
            "{:?}",
            app.status.as_ref().map(|st| &st.text)
        );
        assert!(s.contains("img off"), "{s}");
    }

    /// With nothing to show in a thumbnail slot (half-blocks, images off, or every
    /// image post a spoiler), rows keep the full width, as on a text terminal.
    #[test]
    fn lists_with_no_thumbnail_to_show_keep_text_only_rows() {
        let mut spoilers = feed_app(kitty());
        for p in &mut spoilers.tabs[2].feed.posts {
            p.spoiler = true;
        }
        let mut images_off = feed_app(kitty());
        images_off.show_images = false;
        let mut half_blocks = feed_app(Picker::halfblocks());
        let reference = title_run(&screen(&mut half_blocks, 100, 20), "p2");

        for (name, mut app) in [
            ("half-blocks", half_blocks),
            ("images off", images_off),
            ("spoilers", spoilers),
        ] {
            let s = screen(&mut app, 100, 20);
            assert!(app.thumbs.is_empty(), "{name}");
            assert_eq!(title_run(&s, "p1"), reference, "{name}");
            assert_eq!(title_run(&s, "p2"), reference, "{name}");
        }
    }

    #[test]
    fn masked_spoilers_take_the_columns_the_text_shows_in() {
        use crate::model::{SPOILER_END, SPOILER_START};
        let text = format!("a {SPOILER_START}👨‍👩‍👧 ❤️ go 日本{SPOILER_END} b");

        let masked = spoiler_text(&text, false);
        let shown = spoiler_text(&text, true);

        assert_eq!(masked.width(), shown.width(), "{masked:?} vs {shown:?}");
        assert!(!masked.contains("go"));
    }

    #[test]
    fn a_spoiler_post_lists_with_a_chip_and_no_thumbnail() {
        let mut app = feed_app(kitty());
        app.tabs[2].feed.posts[0].spoiler = true;

        let s = screen(&mut app, 100, 20);

        assert!(!app.thumbs.contains_key("/img/p1.jpeg"));
        assert!(s.lines().any(|l| l.contains(" spoiler ")), "{s}");
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
                spoiler: false,
            })
            .collect();

        let top = screen(&mut app, 100, 12);
        app.move_selection(2);
        let bottom = screen(&mut app, 100, 12);

        assert!(top.contains("title 1"), "{top}");
        // The clipped card scrolls so the selected post stays in view.
        assert!(bottom.contains("title 3"), "{bottom}");
    }

    #[allow(deprecated)]
    fn kitty() -> Picker {
        let mut picker = Picker::from_fontsize(ratatui_image::FontSize::new(8, 16));
        picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
        picker
    }

    fn post_with(id: &str, media: Media) -> Post {
        Post {
            id: id.into(),
            subreddit: "s".into(),
            title: format!("{id} {}", "x".repeat(200)),
            author: "a".into(),
            flair: None,
            score: Some(1),
            comments: 0,
            created: None,
            age: String::new(),
            permalink: format!("/r/s/comments/{id}/x/"),
            media,
            stickied: false,
            nsfw: false,
            spoiler: false,
        }
    }

    /// r/s with eight posts; all but `p2` have an image.
    fn feed_app(picker: Picker) -> App {
        let mut app = app(&["s"]);
        app.image_picker = picker;
        app.show_images = true;
        app.switch_tab(2);
        let feed = &mut app.tabs[2].feed;
        feed.posts = (1..=8)
            .map(|i| {
                let media = if i == 2 {
                    Media::None
                } else {
                    Media::Image {
                        url: format!("/img/p{i}.jpeg"),
                    }
                };
                post_with(&format!("p{i}"), media)
            })
            .collect();
        feed.loading = false;
        feed.loaded = true;
        app
    }

    /// Length of the `x` run in the title line of post `id`.
    fn title_run(screen: &str, id: &str) -> usize {
        let line = screen
            .lines()
            .find(|l| l.contains(&format!("{id} x")))
            .unwrap();
        line.chars().filter(|&c| c == 'x').count()
    }

    /// (x, y) of every cell holding a kitty image placeholder.
    fn placeholder_cells(app: &mut App, w: u16, h: u16) -> Vec<(u16, u16)> {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let mut cells = Vec::new();
        for y in 0..h {
            for x in 0..w {
                if buf[(x, y)].symbol().contains('\u{10EEEE}') {
                    cells.push((x, y));
                }
            }
        }
        cells
    }

    #[test]
    fn feed_rows_load_and_place_thumbnails_only_for_visible_image_posts() {
        let mut app = feed_app(kitty());

        let s = screen(&mut app, 100, 20);

        let mut requested: Vec<&str> = app.thumbs.keys().map(String::as_str).collect();
        requested.sort();
        // Five rows fit; p2 has no image and p6-p8 are below the fold.
        assert_eq!(
            requested,
            [
                "/img/p1.jpeg",
                "/img/p3.jpeg",
                "/img/p4.jpeg",
                "/img/p5.jpeg"
            ]
        );
        // Every row gets the slot, so the post without an image stays aligned;
        // the slot costs the titles its columns compared with a text-only list.
        let text_only = screen(&mut feed_app(Picker::halfblocks()), 100, 20);
        assert_eq!(title_run(&s, "p1"), title_run(&s, "p2"));
        assert_eq!(
            title_run(&s, "p2") + THUMB_RESERVE,
            title_run(&text_only, "p2")
        );
        let title_col = |screen: &str, id: &str| {
            let needle = format!("{id} x");
            let line = screen.lines().find(|l| l.contains(&needle)).unwrap();
            line[..line.find(&needle).unwrap()].chars().count()
        };
        assert_eq!(title_col(&s, "p1"), title_col(&s, "p2"));

        let p1_row = s.lines().position(|l| l.contains("p1 x")).unwrap() as u16;
        let image = image::DynamicImage::new_rgb8(256, 256);
        let protocol = app.image_picker.new_resize_protocol(image);
        app.thumbs
            .insert("/img/p1.jpeg".into(), ImageSlot::Ready(Box::new(protocol)));
        let cells = placeholder_cells(&mut app, 100, 20);

        // Sidebar 24 + margin 1: the list starts at x 25 with the selection bar,
        // so the 6×3 thumbnail fills x 26..=31 on the row's two lines and the gap below.
        let expected: Vec<(u16, u16)> = (p1_row..p1_row + 3)
            .flat_map(|y| (26..=31).map(move |x| (x, y)))
            .collect();
        let mut cells = cells;
        cells.sort_by_key(|&(x, y)| (y, x));
        assert_eq!(cells, expected);
    }

    #[test]
    fn subreddit_icon_from_a_listing_shows_in_sidebar_and_home_card() {
        let mut app = app(&["s"]);
        app.image_picker = kitty();
        app.show_images = true;
        app.on_msg(Msg::Group {
            subreddit: "s".into(),
            result: Ok(crate::model::Listing {
                posts: vec![post_with("p1", Media::None)],
                after: None,
                icon: Some("/icon/s.png".into()),
            }),
        });
        // A later listing without an icon (search, error page) keeps the known one.
        app.switch_tab(2);
        let generation = app.tabs[2].feed.generation;
        let id = app.tabs[2].id;
        app.on_msg(Msg::Feed {
            tab: id,
            generation,
            append: false,
            result: Ok(crate::model::Listing {
                posts: vec![post_with("p1", Media::None)],
                after: None,
                icon: None,
            }),
        });
        app.switch_tab(0);

        let s = screen(&mut app, 120, 30);

        assert!(app.thumbs.contains_key("/icon/s.png"));
        let image = image::DynamicImage::new_rgb8(16, 16);
        let protocol = app.image_picker.new_resize_protocol(image);
        app.thumbs
            .insert("/icon/s.png".into(), ImageSlot::Ready(Box::new(protocol)));
        let cells = placeholder_cells(&mut app, 120, 30);

        let sidebar_row = s
            .lines()
            .position(|l| {
                l.chars()
                    .take(30)
                    .collect::<String>()
                    .trim_end()
                    .ends_with(" s")
            })
            .unwrap() as u16;
        let card_row = s.lines().position(|l| l.contains("╭")).unwrap() as u16;
        let card_x = s
            .lines()
            .nth(card_row as usize)
            .unwrap()
            .chars()
            .position(|c| c == '╭')
            .unwrap() as u16;
        // Sidebar: x 1 + bar + number = 3; card title: corner + space = 2.
        let mut expected = vec![
            (4, sidebar_row),
            (5, sidebar_row),
            (card_x + 2, card_row),
            (card_x + 3, card_row),
        ];
        expected.sort_by_key(|&(x, y)| (y, x));
        let mut cells = cells;
        cells.sort_by_key(|&(x, y)| (y, x));
        assert_eq!(cells, expected);
    }

    fn ready(app: &App, w: u32, h: u32) -> ImageSlot {
        let image = image::DynamicImage::new_rgb8(w, h);
        ImageSlot::Ready(Box::new(app.image_picker.new_resize_protocol(image)))
    }

    #[test]
    fn thumbnails_wait_for_an_overlay_to_close() {
        let mut app = feed_app(kitty());
        screen(&mut app, 100, 20);
        let slot = ready(&app, 32, 32);
        app.thumbs.insert("/img/p1.jpeg".into(), slot);

        app.overlay = Some(crate::app::Overlay::Help { scroll: 0 });
        let under_help = placeholder_cells(&mut app, 100, 20);
        app.overlay = None;
        let after = placeholder_cells(&mut app, 100, 20);

        // kitty sends an image once, in its first cell: drawing it under the
        // popup would lose it, so it first draws after the popup closes.
        assert!(under_help.is_empty(), "{under_help:?}");
        assert_eq!(after.len(), 8);
    }

    #[test]
    fn a_failed_thumbnail_keeps_its_row_aligned_until_reload_retries_it() {
        let mut app = feed_app(kitty());
        screen(&mut app, 100, 20);
        app.thumbs
            .insert("/img/p1.jpeg".into(), ImageSlot::Failed("timed out".into()));

        let s = screen(&mut app, 100, 20);

        assert!(matches!(
            app.thumbs.get("/img/p1.jpeg"),
            Some(ImageSlot::Failed(_))
        ));
        assert_eq!(title_run(&s, "p1"), title_run(&s, "p2"));

        app.reload();
        screen(&mut app, 100, 20);

        assert!(matches!(
            app.thumbs.get("/img/p1.jpeg"),
            Some(ImageSlot::Loading)
        ));
    }

    #[test]
    fn a_full_thumbnail_cache_keeps_only_what_is_on_screen() {
        let mut app = feed_app(kitty());
        for i in 0..400 {
            app.thumbs
                .insert(format!("/img/old{i}.jpeg"), ImageSlot::Failed("x".into()));
        }

        screen(&mut app, 100, 20);

        let mut kept: Vec<&str> = app.thumbs.keys().map(String::as_str).collect();
        kept.sort();
        assert_eq!(
            kept,
            [
                "/img/p1.jpeg",
                "/img/p3.jpeg",
                "/img/p4.jpeg",
                "/img/p5.jpeg"
            ]
        );
    }

    #[test]
    fn a_scrolled_home_card_loads_thumbnails_only_for_whole_rows() {
        let mut app = app(&["s"]);
        app.image_picker = kitty();
        app.show_images = true;
        app.home.groups[0].posts = (1..=3)
            .map(|i| {
                post_with(
                    &format!("p{i}"),
                    Media::Image {
                        url: format!("/img/p{i}.jpeg"),
                    },
                )
            })
            .collect();

        // 12 lines leave the card 4 inner lines: p1's two lines fit, p2's do not.
        screen(&mut app, 100, 12);
        let mut top: Vec<String> = app.thumbs.keys().cloned().collect();
        top.sort();
        app.move_selection(2);
        screen(&mut app, 100, 12);

        assert_eq!(top, ["/img/p1.jpeg"]);
        // Scrolled to p3: p2's first line is above the card's top.
        assert!(app.thumbs.contains_key("/img/p3.jpeg"));
        assert!(!app.thumbs.contains_key("/img/p2.jpeg"));
    }

    #[test]
    fn a_tab_opened_by_name_learns_its_icon_from_its_own_feed() {
        let mut app = app(&[]);
        app.image_picker = kitty();
        app.show_images = true;
        app.open_subreddit("Pics");
        let tab = &app.tabs[app.active];
        let (id, generation) = (tab.id, tab.feed.generation);

        app.on_msg(Msg::Feed {
            tab: id,
            generation,
            append: false,
            result: Ok(crate::model::Listing {
                posts: vec![post_with("p1", Media::None)],
                after: None,
                icon: Some("/icon/pics.png".into()),
            }),
        });

        assert_eq!(app.shown_icon("Pics"), Some("/icon/pics.png"));
        assert_eq!(app.shown_icon("pics"), Some("/icon/pics.png"));
        app.show_images = false;
        assert_eq!(app.shown_icon("pics"), None);
    }

    #[test]
    fn a_skipped_thumbnail_loads_again_when_drawn() {
        let mut app = feed_app(kitty());
        screen(&mut app, 100, 20);

        app.on_msg(Msg::ThumbSkipped {
            url: "/img/p1.jpeg".into(),
        });
        let gone = !app.thumbs.contains_key("/img/p1.jpeg");
        screen(&mut app, 100, 20);

        assert!(gone);
        assert!(matches!(
            app.thumbs.get("/img/p1.jpeg"),
            Some(ImageSlot::Loading)
        ));
    }

    fn press(app: &mut App, c: char) {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        crate::keys::handle(app, KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }

    #[test]
    fn h_climbs_from_a_comment_to_the_sidebar_and_l_comes_back() {
        let (mut app, _) = open_long_post(|_| {});
        press(&mut app, 'j');
        assert!(
            !app.post_view.as_ref().unwrap().free_scroll,
            "a comment is selected"
        );

        press(&mut app, 'h');
        assert!(
            app.post_view.as_ref().unwrap().free_scroll,
            "back to reading the post"
        );
        press(&mut app, 'h');
        assert!(app.post_view.is_none(), "back to the list");
        assert!(!app.sidebar_focused);
        press(&mut app, 'h');
        assert!(app.sidebar_focused);
        press(&mut app, 'h');
        assert!(app.sidebar_focused, "the sidebar is the top");

        press(&mut app, 'l');
        assert!(!app.sidebar_focused);
        assert!(
            app.post_view.is_none(),
            "l from the sidebar only returns focus"
        );
    }

    #[test]
    fn sidebar_j_k_switch_tabs_in_sidebar_order_and_stop_at_the_ends() {
        let mut app = app(&["a", "b"]);
        app.open_subreddit("c");
        app.switch_tab(0);
        let name = |app: &App| app.active_tab().title();

        press(&mut app, 'h');
        let mut seen = vec![name(&app)];
        for _ in 0..5 {
            press(&mut app, 'j');
            seen.push(name(&app));
        }
        press(&mut app, 'k');
        let after_k = name(&app);
        press(&mut app, 'g');
        let first = name(&app);

        assert_eq!(seen, ["Home", "Bookmarks", "r/a", "r/b", "r/c", "r/c"]);
        assert_eq!(after_k, "r/b");
        assert_eq!(first, "Home");
        assert!(app.sidebar_focused);
    }

    #[test]
    fn h_shows_a_hidden_sidebar_only_while_it_has_focus() {
        let mut app = app(&["a"]);
        app.toggle_sidebar();

        press(&mut app, 'h');
        let shown = app.show_sidebar;
        press(&mut app, 'j');
        press(&mut app, 'l');

        assert!(shown);
        assert!(!app.show_sidebar);
        assert_eq!(app.active, 1, "the tab picked in the sidebar stays active");
    }

    fn key(app: &mut App, code: crossterm::event::KeyCode, mods: crossterm::event::KeyModifiers) {
        crate::keys::handle(app, crossterm::event::KeyEvent::new(code, mods));
    }

    #[test]
    fn ctrl_letters_do_not_run_plain_letter_commands() {
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut app = app(&[]);
        app.open_subreddit("c");
        let tabs = app.tabs.len();

        key(&mut app, KeyCode::Char('x'), KeyModifiers::CONTROL);
        key(&mut app, KeyCode::Char('q'), KeyModifiers::CONTROL);

        assert_eq!(app.tabs.len(), tabs, "Ctrl+X closed the tab");
        assert!(!app.quit, "Ctrl+Q quit");
    }

    #[test]
    fn esc_then_a_key_arriving_as_alt_key_does_both() {
        use crossterm::event::{KeyCode, KeyModifiers};
        let (mut app, _) = open_long_post(|_| {});

        key(&mut app, KeyCode::Char('?'), KeyModifiers::ALT);

        assert!(app.post_view.is_none(), "Esc closed the post");
        assert!(
            matches!(app.overlay, Some(Overlay::Help { .. })),
            "then ? opened help"
        );
        // Esc then Tab arrives as Alt+Tab: the sort popup closes, then Tab moves on.
        let mut feed = feed_app(Picker::halfblocks());
        let active = feed.active;
        press(&mut feed, 's');
        key(&mut feed, KeyCode::Tab, KeyModifiers::ALT);
        assert!(feed.overlay.is_none(), "Esc closed the sort popup");
        assert_ne!(feed.active, active, "then Tab switched tabs");
    }

    #[test]
    fn text_boxes_show_the_last_typed_letter_and_the_caret() {
        use crate::app::PickerState;
        let mut app = feed_app(Picker::halfblocks());
        let mut cut = Vec::new();
        for n in [30, 40, 45, 46, 47, 51, 52, 53, 54, 70] {
            let input = format!("{}Z", "q".repeat(n));
            let boxes = [
                (
                    "search",
                    Overlay::Search {
                        input: input.clone(),
                    },
                ),
                (
                    "picker",
                    Overlay::Picker(PickerState { input, selected: 0 }),
                ),
            ];
            for (name, overlay) in boxes {
                app.overlay = Some(overlay);
                if !screen(&mut app, 120, 30).contains("Z▏") {
                    cut.push((name, n));
                }
            }
        }

        assert!(cut.is_empty(), "the end was cut: {cut:?}");
    }

    #[test]
    fn alt_letters_in_a_text_box_neither_close_it_nor_run_list_keys() {
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut app = feed_app(Picker::halfblocks());
        let tabs = app.tabs.len();
        press(&mut app, '/');
        for c in "rust".chars() {
            press(&mut app, c);
        }

        // Readline motions: Alt+b, Alt+f; and Alt+x, Alt+q.
        for c in ['b', 'f', 'x', 'q'] {
            key(&mut app, KeyCode::Char(c), KeyModifiers::ALT);
        }

        let search_kept =
            matches!(&app.overlay, Some(Overlay::Search { input }) if input == "rust");
        // Alt+Enter submits the search, as Enter does.
        key(&mut app, KeyCode::Enter, KeyModifiers::ALT);
        let searched =
            app.overlay.is_none() && app.active_tab().feed.search.as_deref() == Some("rust");
        // The picker the same way.
        press(&mut app, 'p');
        for c in "fo".chars() {
            press(&mut app, c);
        }
        for c in ['b', 'f', 'x', 'q'] {
            key(&mut app, KeyCode::Char(c), KeyModifiers::ALT);
        }
        let picker_kept = matches!(&app.overlay, Some(Overlay::Picker(p)) if p.input == "fo");

        assert!(search_kept, "the search box stays open with its query");
        assert!(searched, "Alt+Enter searched");
        assert!(picker_kept, "the picker stays open with its query");
        assert!(app.state.bookmarks().is_empty(), "nothing bookmarked");
        assert_eq!(app.tabs.len(), tabs, "no tab closed");
        assert!(!app.quit);
    }

    #[test]
    fn search_input_takes_ctrl_h_as_backspace_and_pastes_as_text() {
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut app = app(&["s"]);
        app.switch_tab(2);
        let tabs = app.tabs.len();
        app.overlay = Some(Overlay::Search { input: "ab".into() });
        let input = |app: &App| match &app.overlay {
            Some(Overlay::Search { input }) => input.clone(),
            _ => "<closed>".into(),
        };

        key(&mut app, KeyCode::Char('h'), KeyModifiers::CONTROL);
        // A pasted line break keeps the words apart.
        crate::keys::paste(&mut app, "x\nyz");
        let pasted = input(&app);
        // Alt+Backspace deletes the last word, as in readline.
        key(&mut app, KeyCode::Backspace, KeyModifiers::ALT);
        let word_deleted = input(&app);

        assert_eq!(pasted, "ax yz");
        assert_eq!(word_deleted, "ax ");
        assert_eq!(app.tabs.len(), tabs);
    }

    #[test]
    fn space_pages_a_post_by_its_visible_height_and_stops_at_the_end() {
        let (mut app, first) = open_long_post(|_| {});
        let paragraphs = |s: &str| -> Vec<usize> {
            s.lines()
                .filter_map(|l| l.split("paragraph ").nth(1))
                .filter_map(|r| r.split_whitespace().next()?.parse().ok())
                .collect()
        };
        let before = paragraphs(&first);

        press(&mut app, ' ');
        let after = paragraphs(&screen(&mut app, 100, 30));
        for _ in 0..50 {
            press(&mut app, ' ');
        }
        let end = screen(&mut app, 100, 30);

        // One page later, the last paragraph seen is still on screen: nothing skipped.
        assert!(
            after.contains(before.last().unwrap()),
            "{before:?} → {after:?}"
        );
        assert!(
            end.contains("reply"),
            "the last comment stays on screen:\n{end}"
        );
    }

    #[test]
    fn help_scrolls_to_its_last_keys_in_a_short_terminal() {
        let mut app = app(&[]);
        press(&mut app, '?');
        for _ in 0..HELP.len() {
            press(&mut app, 'j');
            screen(&mut app, 80, 24);
        }

        let s = screen(&mut app, 80, 24);

        assert!(s.contains("quit"), "{s}");
        assert!(app.overlay.is_some(), "j scrolls, it does not close help");
    }

    #[test]
    fn every_help_key_is_set_apart_from_its_description() {
        let mut app = app(&[]);
        press(&mut app, '?');

        let s = screen(&mut app, 120, 60);

        for (key, desc) in HELP.iter().filter(|(_, d)| !d.is_empty()) {
            // The key, its padding and the whole description, on one line.
            assert!(
                s.lines()
                    .any(|l| l.contains(key) && l.contains(&format!("  {desc}"))),
                "{key:?} / {desc:?} is not shown whole:\n{s}"
            );
        }
    }

    #[test]
    fn truncate_keeps_grapheme_clusters_whole() {
        let family = "👨‍👩‍👧";
        let cut = truncate(&format!("{family}{family}"), 3);

        assert_eq!(cut, format!("{family}…"));
        assert!(truncate("❤️❤️❤️❤️", 4).width() <= 4);
    }

    const VIDEO: &str = "/hls/v/HLSPlaylist.m3u8";

    /// A loaded video post in a kitty terminal with images on, drawn once.
    fn open_video_post() -> App {
        let (mut app, _) = open_long_post(|_| {});
        app.image_picker = kitty();
        app.show_images = true;
        let view = app.post_view.as_mut().unwrap();
        view.page.as_mut().unwrap().post.media = Media::Video {
            url: Some(VIDEO.into()),
            poster: None,
        };
        screen(&mut app, 100, 30);
        app
    }

    #[test]
    fn the_video_plays_only_while_its_post_shows_it() {
        let mut app = open_video_post();
        let step = |app: &mut App| {
            screen(app, 100, 30);
            app.tick();
            app.video.as_ref().map(|p| p.hidden())
        };

        assert_eq!(step(&mut app), Some(false), "starts with the post");
        press(&mut app, '?');
        assert_eq!(step(&mut app), Some(true), "waits under the help");
        app.overlay = None;
        assert_eq!(step(&mut app), Some(false));
        press(&mut app, ' ');
        assert_eq!(step(&mut app), Some(true), "waits while scrolled away");
        press(&mut app, 'g');
        assert_eq!(step(&mut app), Some(false));
        press(&mut app, 'i');
        assert_eq!(step(&mut app), None, "images off stops it");
        press(&mut app, 'i');
        assert_eq!(step(&mut app), Some(false));
        press(&mut app, 'q');
        assert_eq!(step(&mut app), None, "leaving the post stops it");
    }

    #[test]
    fn a_sets_the_sound_for_later_videos_and_a_missing_ffplay_clears_it() {
        let mut app = open_video_post();
        screen(&mut app, 100, 30);
        app.tick();
        let (url, area) = {
            let player = app.video.as_ref().unwrap();
            (player.url.clone(), player.area)
        };
        app.video = Some(video::Player::silent(&url, area));

        // A video without sound: `a` still turns the setting over; this one stays silent.
        press(&mut app, 'a');
        assert!(app.video_sound_wanted());
        assert!(!app.video.as_ref().unwrap().sound());
        press(&mut app, 'a');
        assert!(!app.video_sound_wanted(), "and back off");

        // ffplay failing, even at a later frame, leaves later videos silent too.
        press(&mut app, 'a');
        app.video.as_mut().unwrap().fail_sound();
        app.tick();
        assert!(!app.video_sound_wanted());
    }

    #[test]
    fn a_resized_video_starts_again_as_it_was_once_the_size_holds() {
        let mut app = open_video_post();
        let step = |app: &mut App, h: u16| {
            screen(app, 100, h);
            app.tick();
        };
        step(&mut app, 30);
        let first = app.video.as_ref().unwrap().area;
        press(&mut app, 'P');

        // Two draws at once at the new size: it has not held still yet.
        step(&mut app, 22);
        step(&mut app, 22);
        assert_eq!(
            app.video.as_ref().unwrap().area,
            first,
            "a size passing by changes nothing"
        );
        std::thread::sleep(std::time::Duration::from_millis(300));
        step(&mut app, 22);
        let player = app.video.as_ref().unwrap();
        assert_ne!(player.area, first, "sized for the new area");
        assert!(player.paused(), "still paused");

        press(&mut app, 'q');
        app.tick();
        assert_eq!(
            app.video_area, None,
            "the next video waits for its own draw"
        );
    }

    #[test]
    fn a_video_frame_goes_to_the_terminal_once() {
        let mut app = open_video_post();
        let frame = || video::Frame {
            width: 16,
            height: 9,
            pixels: video::Pixels::Zlib(vec![1, 2, 3]),
        };
        let url = app.redlib.absolute(VIDEO);
        app.video = Some(video::Player::showing(&url, frame()));
        let sends = |app: &mut App| screen(app, 100, 30).matches("_Ga=T").count();

        assert_eq!(sends(&mut app), 1);
        assert_eq!(sends(&mut app), 0, "a redraw repeats the cells only");
        app.video.as_mut().unwrap().show(frame());
        assert_eq!(sends(&mut app), 1, "the next frame goes out");
    }
}
