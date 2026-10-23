//! Rendering: tab sidebar, the active list and, when it fits, the open post beside it.
//! Takes `&mut App` to keep scroll offsets in sync and to load the thumbnails it draws.

use std::collections::{HashMap, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
use std::time::Instant;

use chrono::{DateTime, Utc};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect, Size};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap};
use ratatui_image::picker::ProtocolType;
use ratatui_image::{Image, Resize, StatefulImage};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::app::{
    App, CARD_MIN_WIDTH, CardImageProtocol, CardImageSlot, CardScroll, DETAIL_MIN_WIDTH, Focus,
    Group, ImageRefresh, ImageSlot, MAIN_MIN_WIDTH, Overlay, PickerEntry, PostView, RowKind, Tab,
    TabKind,
};
use crate::model::{Media, PICTURE, Post, SPOILER_END, SPOILER_START, Sort};
use crate::reddit::Health;
use crate::state::ListView;
use crate::video;

/// Catppuccin Mocha, with an orange accent close to Reddit's.
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

/// Post-row thumbnail after the selection bar, as tall as the post's two lines.
pub const THUMB_COLS: u16 = 4;
pub const THUMB_ROWS: u16 = 2;
/// Columns a thumbnail slot takes from a row's text: the image and a gap.
const THUMB_RESERVE: usize = THUMB_COLS as usize + 1;
/// A subreddit icon: one line tall.
const ICON_COLS: u16 = 2;
#[cfg(test)]
use crate::card_image::encode_tile;
/// Kitty's Unicode placeholders can address only 297 rows or columns per image.
use crate::card_image::{Key as CardImageKey, TILE_COLS, TILE_ROWS};

/// A thumbnail or icon to draw once the text is down.
struct ImageSpot {
    url: String,
    area: Rect,
}

struct PostCard {
    lines: Vec<Line<'static>>,
    footer: Line<'static>,
    marker: Span<'static>,
    style: Style,
    image: Option<String>,
    image_rows: u16,
    geometry_known: bool,
    pixels: (u32, u32),
}

impl PostCard {
    fn height(&self) -> usize {
        self.lines.len() + usize::from(self.image_rows) + 1
    }

    fn visible(&self, skip: usize, rows: usize) -> Vec<Line<'static>> {
        (skip..skip + rows)
            .map(|line| {
                if let Some(header) = self.lines.get(line) {
                    header.clone()
                } else if line + 1 == self.height() {
                    self.footer.clone()
                } else {
                    Line::from(self.marker.clone()).style(self.style)
                }
            })
            .collect()
    }
}

struct CardPicture {
    url: String,
    area: Rect,
    size: Size,
    skipped: u16,
    pixels: (u32, u32),
}

#[derive(Default)]
struct ListPictures {
    thumbs: Vec<ImageSpot>,
    cards: Vec<CardPicture>,
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    app.resize(area.width);
    let sidebar_width = app.sidebar_width();
    let [sidebar, main] =
        Layout::horizontal([Constraint::Length(sidebar_width), Constraint::Min(20)]).areas(area);
    let mut spots = Vec::new();
    let mut pictures = ListPictures::default();
    if sidebar_width > 0 {
        draw_sidebar(f, app, sidebar, &mut spots);
    }
    let [body, footer] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(main);
    let (main, detail) = if app.post_view.is_some() {
        if app.detail_fits() {
            let main_width = (body.width / 5 * 2).max(MAIN_MIN_WIDTH);
            let [main, detail] = Layout::horizontal([
                Constraint::Length(main_width),
                Constraint::Min(DETAIL_MIN_WIDTH),
            ])
            .areas(body);
            f.render_widget(
                Block::default().borders(Borders::LEFT).border_style(
                    Style::new().fg(if app.post_focused() { ACCENT } else { SURFACE1 }),
                ),
                detail,
            );
            (main, Some(detail))
        } else {
            (Rect::default(), Some(body))
        }
    } else {
        (body, None)
    };
    let margin = Margin {
        horizontal: 1,
        vertical: 0,
    };
    if !main.is_empty() {
        let body = main.inner(margin);
        match app.active_tab().kind {
            TabKind::Home => draw_feed(f, app, body, &mut pictures),
            TabKind::AllSubs => draw_all_subs(f, app, body, &mut spots),
            TabKind::Bookmarks => draw_bookmarks(f, app, body, &mut pictures),
            TabKind::Subreddit { .. } => draw_feed(f, app, body, &mut pictures),
        }
    }
    draw_card_pictures(f, app, &pictures.cards);
    spots.extend(pictures.thumbs);
    if let Some(detail) = detail {
        draw_post(f, app, detail.inner(margin));
    }
    draw_spots(f, app, &spots);
    draw_footer(f, app, footer);
    app.status_drawn();
    if let Some(Overlay::Help { scroll }) = &mut app.overlay {
        draw_help(f, scroll);
        return;
    }
    match &app.overlay {
        Some(Overlay::Auth) => draw_auth(f, app),
        Some(Overlay::Picker(p)) => draw_picker(f, app, &p.input, p.selected),
        Some(Overlay::Sort { selected }) => draw_sort(f, app, *selected),
        Some(Overlay::Search { input }) => draw_search(f, app, input),
        Some(Overlay::Membership { subreddit, join }) => draw_membership(f, subreddit, *join),
        Some(Overlay::Help { .. }) | None => {}
    }
}

/// NO_COLOR: drop colors, keep modifiers. Image placeholder cells keep theirs: kitty
/// reads the image id from the color.
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
    // Nothing under an overlay: kitty sends image data once, in the first cell, and
    // sixel and iTerm2 would leave stale pixels.
    if app.overlay.is_none() {
        for spot in spots {
            if let Some(ImageSlot::Ready { protocol, .. }) = app.thumbs.get_mut(&spot.url) {
                // Cut to the slot's shape on arrival (`fill_slot`): scaled, it fills it.
                let fill = StatefulImage::default()
                    .resize(Resize::Scale(Some(ratatui_image::FilterType::Triangle)));
                f.render_stateful_widget(fill, spot.area, protocol.as_mut());
            } else if !matches!(app.thumbs.get(&spot.url), Some(ImageSlot::Failed(_))) {
                draw_picture_wait(f, spot.area, "◌");
            }
        }
    }
    let urls: Vec<&str> = spots.iter().map(|s| s.url.as_str()).collect();
    app.thumbs_drawn(&urls);
}

fn row_thumb<'a>(app: &App, post: &'a Post) -> Option<&'a str> {
    // A spoiler's picture is the spoiler.
    if !app.thumbnails_on() || post.spoiler {
        return None;
    }
    app.usable_thumb(post.media.list_thumb())
}

/// Thumbnail slot width for a list: every row gets it if any non-spoiler post has a
/// thumbnail, so columns align.
fn thumb_reserve<'a>(app: &App, mut posts: impl Iterator<Item = &'a Post>) -> usize {
    if app.thumbnails_on() && posts.any(|p| !p.spoiler && p.media.list_thumb().is_some()) {
        THUMB_RESERVE
    } else {
        0
    }
}

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

/// Cut `s` to `width` columns with an ellipsis, on grapheme boundaries.
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
    let used = line_width(spans);
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

    let mut rows = vec![
        SideRow::Tab(0),
        SideRow::Tab(1),
        SideRow::Tab(crate::app::ALL_SUBS_TAB),
    ];
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

    let (dot, color, label) = match app.health {
        Health::SignedOut => ("○", YELLOW, "connect · A"),
        Health::Unknown => ("○", OVERLAY, "reddit"),
        Health::Up => ("●", GREEN, "reddit"),
        Health::Paused => ("◐", YELLOW, "reddit paused"),
        Health::Down => ("✗", RED, "reddit down"),
    };
    let img = if app.inline_images_on() {
        "img on"
    } else {
        "img off"
    };
    // " ✗ reddit down  ◧ img off" is the label plus 14 columns; narrower sidebars drop the state.
    let label = if width >= label.len() + 14 {
        label
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

fn sidebar_item(app: &App, index: usize, width: usize) -> Line<'static> {
    let tab = &app.tabs[index];
    let active = index == app.active;
    let (icon, name) = match &tab.kind {
        TabKind::Home => ("⌂", "Home"),
        TabKind::AllSubs => ("▦", "All subs"),
        TabKind::Bookmarks => ("★", "Bookmarks"),
        TabKind::Subreddit { name, .. } => ("#", name.as_str()),
    };
    let badge = match tab.kind {
        TabKind::Bookmarks => app.state.bookmarks().len(),
        _ => app.badge(tab),
    };
    // With focus in the sidebar, its row stands out more than the list's selection.
    let base = match (active, app.focus == Focus::Sidebar) {
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
    spans.push(Span::styled(truncate(name, name_width), name_style));
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

fn view_header(f: &mut Frame, area: Rect, title: &str, chips: Vec<Span<'static>>) -> Rect {
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
    let lines = vec![Line::raw(""), Line::from(spans)];
    f.render_widget(Paragraph::new(lines), head);
    rest
}

fn sort_chip(sort: Sort) -> Span<'static> {
    let (icon, color) = match sort {
        Sort::Best => ("✦", LAVENDER),
        Sort::Hot => ("🔥", PEACH),
        Sort::New => ("✦", GREEN),
        Sort::Top => ("▲", YELLOW),
        Sort::Rising => ("↗", TEAL),
        Sort::Controversial => ("⚡", RED),
    };
    chip(format!("{icon} {}", sort.as_str()), color, SURFACE0)
}

fn list_view_chip(app: &App) -> Span<'static> {
    chip(
        format!("V {}", app.state.list_view().label()),
        TEXT,
        SURFACE0,
    )
}

// ----- post cards ----------------------------------------------------------

struct CardOpts<'a> {
    selected: bool,
    seen_key: Option<&'a str>,
    show_subreddit: bool,
    /// Columns after the selection bar kept free of text, for a thumbnail.
    reserve: usize,
}

/// Two lines per post: score gutter + title, comment gutter + meta chips.
fn post_card(app: &App, post: &Post, width: usize, opts: &CardOpts<'_>) -> [Line<'static>; 2] {
    let bg = post_background(app, post, opts.selected);
    let unseen = opts
        .seen_key
        .is_some_and(|k| !app.state.is_seen(k, &post.id));
    let read = app.state.is_read(&post.id);
    let bar_color = if app.focus == Focus::List {
        ACCENT
    } else {
        OVERLAY
    };
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
    let meta = vec![
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
    [
        Line::from(title),
        post_meta(post, width, opts.show_subreddit, bg, meta),
    ]
}

fn post_background(app: &App, post: &Post, selected: bool) -> Style {
    if selected {
        Style::new().bg(SURFACE0)
    } else if matches!(app.active_tab().kind, TabKind::Home)
        && app.joined(&post.subreddit) == Some(false)
    {
        Style::new().bg(MANTLE)
    } else {
        Style::new()
    }
}

fn post_meta(
    post: &Post,
    width: usize,
    show_subreddit: bool,
    bg: Style,
    mut meta: Vec<Span<'static>>,
) -> Line<'static> {
    if show_subreddit {
        meta.push(Span::styled(
            format!("r/{}  ", post.subreddit),
            bg.fg(LAVENDER),
        ));
    }
    meta.push(Span::styled(format!("u/{}", post.author), bg.fg(SUBTEXT)));
    let age = post.age_at(Utc::now());
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
    Line::from(meta_line).style(bg)
}

fn media_label(media: &Media, tag: &str) -> String {
    let icon = match media {
        Media::Image { .. } if media.video_url().is_some() => "▶",
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
    offset: usize,
    scroll: CardScroll,
    tail: Option<&'static str>,
}

fn card_marker(app: &App, selected: bool, bg: Style) -> Span<'static> {
    Span::styled(
        if selected { "▌ " } else { "  " },
        bg.fg(if app.focus == Focus::List {
            ACCENT
        } else {
            OVERLAY
        }),
    )
}

fn expanded_card(
    app: &App,
    post: &Post,
    width: usize,
    opts: &CardOpts<'_>,
    height: u16,
) -> PostCard {
    let bg = post_background(app, post, opts.selected);
    let marker = card_marker(app, opts.selected, bg);
    let meta = post_meta(post, width, opts.show_subreddit, bg, vec![marker.clone()]);
    let mut lines = card_title(app, post, width, opts, height, bg);
    let image = (!post.spoiler && width >= 8 && app.inline_images_on())
        .then(|| {
            post.media
                .preview_image()
                .or_else(|| post.media.list_thumb())
                .map(str::to_string)
        })
        .flatten();
    let image_size = image.as_deref().and_then(|url| {
        post.preview_size
            .filter(|&(width, height)| width > 0 && height > 0)
            .or_else(|| app.card_image_sizes.get(url).copied())
            .filter(|&(width, height)| width > 0 && height > 0)
    });
    let image_rows = image.as_ref().map_or(0, |_| {
        let (image_width, image_height) = image_size.unwrap_or((1, 1));
        let font = app.image_picker.font_size();
        let pixels_wide = width.saturating_sub(4) as f64 * f64::from(font.width.max(1));
        (pixels_wide * f64::from(image_height)
            / f64::from(image_width)
            / f64::from(font.height.max(1)))
        .ceil()
        .clamp(1.0, f64::from(i16::MAX)) as u16
    });
    lines.insert(0, meta);
    let image = image.filter(|_| image_rows > 0);
    let score = post.score.map_or("•".into(), format_count);
    let footer = Line::from(vec![
        marker.clone(),
        Span::styled(format!("▲ {score}  "), bg.fg(score_color(post.score))),
        Span::styled(
            format!("💬 {} comments", format_count(post.comments as i64)),
            bg.fg(BLUE),
        ),
    ])
    .style(bg);
    let geometry_known = image.is_none()
        || image_size.is_some()
        || image
            .as_deref()
            .is_some_and(|url| matches!(app.card_images.get(url), Some(CardImageSlot::Failed(_))));
    PostCard {
        lines,
        footer,
        marker,
        style: bg,
        geometry_known,
        pixels: image_size.unwrap_or((1, 1)),
        image,
        image_rows,
    }
}

fn card_title(
    app: &App,
    post: &Post,
    width: usize,
    opts: &CardOpts<'_>,
    height: u16,
    bg: Style,
) -> Vec<Line<'static>> {
    let mut style = bg.fg(TEXT).add_modifier(Modifier::BOLD);
    if app.state.is_read(&post.id) {
        style = style.fg(SUBTEXT);
    }
    if app.config.highlights(post) {
        style = style.fg(YELLOW);
    }
    let saved = if app.state.is_bookmarked(&post.id) {
        "★ "
    } else {
        ""
    };
    let pinned = if post.stickied { "📌 " } else { "" };
    let unseen = if opts
        .seen_key
        .is_some_and(|key| !app.state.is_seen(key, &post.id))
    {
        "● "
    } else {
        ""
    };
    let title = format!("{saved}{pinned}{unseen}{}", post.title);
    let title_rows = height.saturating_sub(4).clamp(1, 3) as usize;
    let wrapped = textwrap::wrap(&title, width.saturating_sub(4).max(1));
    let clipped = wrapped.len() > title_rows;
    let mut unseen_pending = !unseen.is_empty();
    wrapped
        .into_iter()
        .take(title_rows)
        .enumerate()
        .map(|(i, text)| {
            let text = if clipped && i + 1 == title_rows {
                truncate(&format!("{text}…"), width.saturating_sub(4))
            } else {
                text.into_owned()
            };
            let mut spans = vec![card_marker(app, opts.selected, bg)];
            if unseen_pending && let Some((before, after)) = text.split_once('●') {
                spans.push(Span::styled(before.to_string(), style));
                spans.push(Span::styled("●", style.fg(ACCENT)));
                spans.push(Span::styled(after.to_string(), style));
                unseen_pending = false;
            } else {
                spans.push(Span::styled(text, style));
            }
            Line::from(spans).style(bg)
        })
        .collect()
}

fn draw_card_list(
    f: &mut Frame,
    app: &App,
    area: Rect,
    spec: &mut ListSpec<'_>,
    pictures: &mut ListPictures,
) {
    if area.is_empty() || spec.posts.is_empty() {
        return;
    }
    let width = area.width.saturating_sub(2) as usize;
    let card_at = |i| {
        expanded_card(
            app,
            spec.posts[i],
            width,
            &CardOpts {
                selected: i == spec.selected,
                seen_key: spec.seen_key,
                show_subreddit: spec.show_subreddit,
                reserve: 0,
            },
            area.height,
        )
    };
    let selected = card_at(spec.selected);
    let post_id = &spec.posts[spec.selected].id;
    let scroll = &mut spec.scroll;
    let offset = &mut spec.offset;
    if scroll.post.as_ref() != Some(post_id) {
        if scroll.post.is_some() {
            scroll.line = 0;
            scroll.height = 0;
            scroll.bottom = false;
        }
        scroll.post = Some(post_id.clone());
    }
    scroll.height = selected.height();
    scroll.viewport = area.height as usize;
    if scroll.bottom {
        scroll.line = scroll.height.saturating_sub(scroll.viewport);
        scroll.bottom = !selected.geometry_known;
    }
    scroll.line = scroll
        .line
        .min(scroll.height.saturating_sub(scroll.viewport));
    *offset = (*offset).min(spec.selected);
    if scroll.height > scroll.viewport {
        *offset = spec.selected;
    }
    let mut used = 0;
    for i in (*offset..=spec.selected).rev() {
        used += card_at(i).height() + 1;
        if used > area.height as usize {
            *offset = (i + 1).min(spec.selected);
            break;
        }
    }
    let mut y = area.y;
    let mut shown = 0;
    for i in *offset..spec.posts.len() {
        let card = card_at(i);
        let skip = if i == *offset { scroll.line } else { 0 };
        let rows = card.height() - skip;
        let at = Rect::new(
            area.x,
            y,
            area.width.saturating_sub(2),
            rows.min(usize::from(area.bottom() - y)) as u16,
        );
        f.render_widget(
            Paragraph::new(card.visible(skip, usize::from(at.height))),
            at,
        );
        if let Some(url) = &card.image {
            let image_start = card.lines.len();
            let image_y = y + image_start.saturating_sub(skip).min(area.height as usize) as u16;
            let skipped = skip.saturating_sub(image_start) as u16;
            if skipped < card.image_rows && image_y < at.bottom() {
                pictures.cards.push(CardPicture {
                    url: url.clone(),
                    area: Rect::new(
                        area.x + 2,
                        image_y,
                        area.width.saturating_sub(6),
                        (card.image_rows - skipped).min(at.bottom() - image_y),
                    ),
                    size: Size::new(area.width.saturating_sub(6), card.image_rows),
                    skipped,
                    pixels: card.pixels,
                });
            }
        }
        shown += 1;
        if i + 1 == spec.posts.len()
            && usize::from(at.height) == rows
            && at.bottom() < area.bottom()
            && let Some(tail) = spec.tail
        {
            draw_list_tail(
                f,
                Rect::new(
                    area.x,
                    at.bottom(),
                    area.width.saturating_sub(2),
                    area.bottom() - at.bottom(),
                ),
                tail,
            );
        }
        y = y.saturating_add(at.height.saturating_add(1));
        if y >= area.bottom() {
            break;
        }
    }
    if scroll.height > scroll.viewport {
        draw_scrollbar(f, area, scroll.line, scroll.viewport, scroll.height);
    } else {
        draw_scrollbar(f, area, *offset, shown, spec.posts.len());
    }
}

fn draw_card_pictures(f: &mut Frame, app: &mut App, spots: &[CardPicture]) {
    let prepare = app.card_preparation_allowed();
    let urls: Vec<&str> = spots.iter().map(|spot| spot.url.as_str()).collect();
    app.card_pictures_drawn(&urls);
    app.card_renderer.begin();
    for spot in spots {
        if app.overlay.is_some() {
            continue;
        }
        let Some(slot) = app.card_images.get_mut(&spot.url) else {
            draw_picture_wait(f, spot.area, "▣ loading image…");
            continue;
        };
        match slot {
            CardImageSlot::Ready { image, protocol }
            | CardImageSlot::Cached {
                image, protocol, ..
            } => {
                if let Err(error) = draw_card_image(
                    f,
                    &app.image_picker,
                    &mut app.card_renderer,
                    image,
                    protocol,
                    spot,
                    prepare,
                ) {
                    *slot = CardImageSlot::Failed(error);
                }
            }
            CardImageSlot::Loading => draw_picture_wait(f, spot.area, "▣ loading image…"),
            CardImageSlot::Failed(error) => f.render_widget(
                Paragraph::new(format!(
                    "▣ image unavailable: {}",
                    truncate(error, spot.area.width as usize)
                ))
                .style(Style::new().fg(OVERLAY)),
                spot.area,
            ),
        }
    }
    app.card_renderer.finish();
}

fn draw_card_image(
    f: &mut Frame,
    picker: &ratatui_image::picker::Picker,
    renderer: &mut crate::card_image::Renderer,
    image: &std::sync::Arc<image::DynamicImage>,
    protocol: &mut Option<CardImageProtocol>,
    spot: &CardPicture,
    prepare: bool,
) -> Result<(), String> {
    if protocol
        .as_ref()
        .is_none_or(|protocol| protocol.size != spot.size || protocol.pixels != spot.pixels)
    {
        *protocol = Some(CardImageProtocol {
            size: spot.size,
            pixels: spot.pixels,
            tiles: HashMap::new(),
        });
    }
    let Some(protocol) = protocol else {
        return Ok(());
    };
    let first = spot.skipped / TILE_ROWS;
    let end = (spot.skipped + spot.area.height).div_ceil(TILE_ROWS);
    let mut visible = Vec::new();
    for row in first..end {
        for column in 0..spot.size.width.div_ceil(TILE_COLS) {
            let start = spot.skipped.max(row * TILE_ROWS);
            let y = spot.area.y + start - spot.skipped;
            let rows = ((row + 1) * TILE_ROWS - start).min(spot.area.bottom() - y);
            let key = (start, column, rows);
            visible.push(key);
            let request = CardImageKey::new(
                &spot.url,
                picker,
                spot.size,
                spot.pixels,
                start,
                rows,
                column,
            );
            if let Some(result) = renderer.tile(request, image, picker, prepare) {
                protocol.tiles.insert(key, result?);
            }
            if let Some(encoded) = protocol.tiles.get(&key) {
                let area = Rect::new(
                    spot.area.x + column * TILE_COLS,
                    y,
                    encoded.size().width,
                    rows,
                );
                f.render_widget(Image::new(encoded.as_ref()), area);
            } else {
                draw_picture_wait(
                    f,
                    Rect::new(
                        spot.area.x + column * TILE_COLS,
                        y,
                        TILE_COLS.min(spot.size.width - column * TILE_COLS),
                        rows,
                    ),
                    "◌ preparing image…",
                );
            }
        }
    }
    protocol.tiles.retain(|key, _| visible.contains(key));
    Ok(())
}

fn draw_picture_wait(f: &mut Frame, area: Rect, label: &str) {
    if area.is_empty() {
        return;
    }
    f.render_widget(
        Paragraph::new(label)
            .alignment(Alignment::Center)
            .style(Style::new().fg(SUBTEXT)),
        Rect::new(area.x, area.y + area.height / 2, area.width, 1),
    );
}

fn draw_list_tail(f: &mut Frame, mut area: Rect, label: &'static str) {
    if area.is_empty() {
        return;
    }
    if area.height > 1 {
        area.y += 1;
        area.height -= 1;
    }
    let mut panel = Block::default().style(Style::new().bg(MANTLE));
    if area.height >= 4 && area.width >= 8 {
        panel = panel
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(SURFACE1));
    }
    let inner = panel.inner(area);
    f.render_widget(panel, area);
    let error = label.starts_with('✗');
    let style = Style::new()
        .fg(if error { RED } else { ACCENT })
        .add_modifier(Modifier::BOLD);
    let mut lines = Vec::new();
    if !error && inner.height >= 3 && usize::from(inner.width) >= label.width() {
        lines.push(Line::styled("⋯", style));
        lines.push(Line::styled(label, style.fg(TEXT)));
    } else {
        lines.push(Line::styled(label, style));
    }
    centered(f, inner, lines);
}

/// Scrolling list of post cards separated by a blank line.
fn draw_post_list(
    f: &mut Frame,
    app: &App,
    area: Rect,
    spec: &mut ListSpec<'_>,
    pictures: &mut ListPictures,
) {
    if app.state.list_view() == ListView::Card && area.height >= 3 {
        draw_card_list(f, app, area, spec, pictures);
        return;
    }
    spec.scroll.viewport = area.height as usize;
    const ITEM: usize = 3;
    let offset = &mut spec.offset;
    let fits = (area.height as usize / ITEM).max(1);
    if spec.selected < *offset {
        *offset = spec.selected;
    } else if spec.selected >= *offset + fits {
        *offset = spec.selected + 1 - fits;
    }
    let width = area.width.saturating_sub(2) as usize;
    let reserve = thumb_reserve(app, spec.posts.iter().copied());
    let mut lines = Vec::new();
    let mut tail_y = None;
    for (i, post) in spec.posts.iter().enumerate().skip(*offset).take(fits) {
        let y = area.y + lines.len() as u16;
        if let Some(url) = row_thumb(app, post)
            && y + THUMB_ROWS <= area.bottom()
        {
            pictures.thumbs.push(ImageSpot {
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
            seen_key: spec.seen_key,
            show_subreddit: spec.show_subreddit,
            reserve,
        };
        let [title, meta] = post_card(app, post, width, &opts);
        lines.push(title);
        lines.push(meta);
        lines.push(Line::raw(""));
        if i + 1 == spec.posts.len() {
            tail_y = Some(y.saturating_add(2));
        }
    }
    f.render_widget(Paragraph::new(lines), area);
    if let (Some(y), Some(tail)) = (tail_y, spec.tail)
        && y < area.bottom()
    {
        draw_list_tail(
            f,
            Rect::new(area.x, y, area.width.saturating_sub(2), area.bottom() - y),
            tail,
        );
    }
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
    let shown = app.group_posts(group).count().max(1) as u16;
    // borders + top/bottom padding + 2 lines per post + a gap between posts
    2 + 2 + shown * 2 + shown.saturating_sub(1)
}

const CARD_ROW_GAP: u16 = 1;

fn draw_all_subs(f: &mut Frame, app: &mut App, area: Rect, spots: &mut Vec<ImageSpot>) {
    let total_new: usize = app
        .all_subs
        .groups
        .iter()
        .map(|g| app.state.unseen(&g.subreddit, app.config.visible(&g.posts)))
        .sum();
    let loading = app.all_subs.groups.iter().filter(|g| g.loading).count();
    let mut chips = vec![chip(
        match app.all_subs.groups.len() {
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
    if app.all_subs.loading {
        chips.push(chip("⋯ syncing subscriptions", OVERLAY, SURFACE0));
    }
    if app.all_subs.error.is_some() {
        chips.push(chip("✗ subscriptions failed · r retries", RED, SURFACE0));
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
            "Press A to connect your account",
        );
        return;
    }
    if app.all_subs.groups.is_empty() {
        let (title, hint) = if let Some(error) = &app.all_subs.error {
            (
                "Could not load subscriptions",
                format!("{error} — press r to retry"),
            )
        } else if app.all_subs.loading {
            ("Loading subscriptions", String::new())
        } else {
            (
                "No joined communities",
                "join communities on Reddit, then press r; p opens any subreddit".to_string(),
            )
        };
        empty_state(f, body, "▦", title, &hint);
        return;
    }
    let cols = (body.width / CARD_MIN_WIDTH).clamp(1, 3) as usize;
    let heights: Vec<u16> = app
        .all_subs
        .groups
        .chunks(cols)
        .map(|row| {
            let card = row
                .iter()
                .map(|group| card_height(app, group))
                .max()
                .unwrap_or(4);
            card + CARD_ROW_GAP
        })
        .collect();
    let selected_row = app.all_subs.group / cols;
    let mut offset = app.all_subs.offset.min(selected_row);
    // The gap after the last visible row may fall outside the body.
    let fits = |rows: &[u16]| rows.iter().sum::<u16>() - CARD_ROW_GAP <= body.height;
    while !fits(&heights[offset..=selected_row]) && offset < selected_row {
        offset += 1;
    }
    app.all_subs.offset = offset;

    let bottom = body.y + body.height;
    let mut y = body.y;
    let mut shown = 0;
    for (row_index, row) in app.all_subs.groups.chunks(cols).enumerate().skip(offset) {
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
        for (column, cell) in cells.iter().take(row.len()).enumerate() {
            draw_group_card(f, app, row_index * cols + column, *cell, spots);
        }
        y += h;
        shown += 1;
    }
    draw_scrollbar(f, body, offset, shown.max(1), heights.len());
}

fn draw_group_card(f: &mut Frame, app: &App, index: usize, area: Rect, spots: &mut Vec<ImageSpot>) {
    let group = &app.all_subs.groups[index];
    let selected_card = index == app.all_subs.group;
    let badge = app
        .state
        .unseen(&group.subreddit, app.config.visible(&group.posts));
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
    if let Some(freshness) = group.freshness {
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
    // A failed refresh keeps the posts; it only marks the card.
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
    let posts: Vec<_> = app.group_posts(group).collect();
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
                seen_key: Some(&group.subreddit),
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

// ----- subreddit feed & bookmarks --------------------------------------------

fn draw_feed(f: &mut Frame, app: &mut App, area: Rect, pictures: &mut ListPictures) {
    let tab = app.active_tab();
    let feed = &tab.feed;
    let mut chips = vec![list_view_chip(app)];
    match &feed.search {
        Some(q) => chips.push(chip(format!("🔍 {q}"), SKY, SURFACE0)),
        None => {
            chips.push(sort_chip(feed.sort));
            if feed.sort.takes_time() {
                chips.push(chip(format!("◷ {}", feed.time.as_str()), SUBTEXT, SURFACE0));
            }
        }
    }
    if let Some(name) = tab
        .subreddit()
        .filter(|name| crate::reddit::is_community(name))
    {
        let (label, color) = match app.joined(name) {
            Some(true) => ("✓ Joined", GREEN),
            Some(false) => ("Not joined", SUBTEXT),
            None => ("Membership unknown", OVERLAY),
        };
        chips.push(chip(label, color, SURFACE0));
    }
    let badge = app.badge(tab);
    if badge > 0 {
        chips.push(chip(format!("● {badge} new"), CRUST, ACCENT));
    }
    if feed.loading() {
        chips.push(chip("⋯ loading", OVERLAY, SURFACE0));
    }
    if feed.error().is_some() && !feed.posts.is_empty() {
        chips.push(chip(
            if feed.page_failed() {
                "✗ next page failed · L retries"
            } else {
                "✗ reload failed · r retries"
            },
            RED,
            SURFACE0,
        ));
    }
    if let Some(freshness) = feed.freshness {
        chips.push(chip(
            freshness.label(
                crate::app::now(),
                feed.loading() || feed.refreshing(),
                feed.error().is_some(),
                app.health == Health::Down,
            ),
            OVERLAY,
            SURFACE0,
        ));
    }
    let title = tab.title();
    let seen_key = tab.subreddit();
    let error = feed.error();
    let posts: Vec<&Post> = app.config.visible(&feed.posts).collect();
    let selected = feed.selected;
    let current = feed.shows_current_query();
    let offset = feed.offset;
    let scroll = feed.cards.clone();
    let body = view_header(f, area, &title, chips);
    if app.reddit.needs_sign_in() && posts.is_empty() {
        empty_state(
            f,
            body,
            "○",
            "Connect Reddit",
            "Press A to connect your account",
        );
        return;
    }
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
        // Until the current sort or search answers, an earlier empty answer says nothing.
        if !current {
            empty_state(f, body, "⋯", "Loading", "");
        } else if !feed.posts.is_empty() {
            empty_state(
                f,
                body,
                "∅",
                "Every loaded post is hidden",
                "show_nsfw and [filters] in config.toml decide what shows",
            );
        } else if feed.search.is_some() {
            empty_state(
                f,
                body,
                "∅",
                "No results",
                "/ edits the search, Esc clears it",
            );
        } else {
            empty_state(
                f,
                body,
                "∅",
                "No posts here",
                "try another sort with s, or a different subreddit with p",
            );
        }
        return;
    }
    let show_subreddit =
        matches!(tab.kind, TabKind::Home) || matches!(seen_key, Some("all" | "popular"));
    let mut spec = ListSpec {
        posts: &posts,
        selected,
        seen_key,
        show_subreddit,
        offset,
        scroll,
        tail: if feed.page_failed() {
            Some("✗ Next page failed · L retries")
        } else {
            feed_activity(feed, !app.reddit.needs_sign_in()).map(|(label, _)| label)
        },
    };
    draw_post_list(f, app, body, &mut spec, pictures);
    let (offset, scroll) = (spec.offset, spec.scroll);
    let active = app.active;
    app.tabs[active].feed.offset = offset;
    app.tabs[active].feed.cards = scroll;
}

fn draw_bookmarks(f: &mut Frame, app: &mut App, area: Rect, pictures: &mut ListPictures) {
    let count = app.state.bookmarks().len();
    let body = view_header(
        f,
        area,
        "★ Bookmarks",
        vec![
            list_view_chip(app),
            chip(format!("{count} saved"), SUBTEXT, SURFACE0),
        ],
    );
    let posts: Vec<&Post> = app.state.bookmarks().iter().map(|b| &b.post).collect();
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
    let selected = app.active_tab().feed.selected.min(posts.len() - 1);
    let mut spec = ListSpec {
        posts: &posts,
        selected,
        seen_key: None,
        show_subreddit: true,
        offset: app.active_tab().feed.offset,
        scroll: app.active_tab().feed.cards.clone(),
        tail: None,
    };
    draw_post_list(f, app, body, &mut spec, pictures);
    let (offset, scroll) = (spec.offset, spec.scroll);
    let active = app.active;
    app.tabs[active].feed.offset = offset;
    app.tabs[active].feed.cards = scroll;
}

// ----- post view -----------------------------------------------------------

/// Masks `>!…!<` spoilers as `▒`, one per column with spaces kept (same width, but break
/// points still differ, e.g. at hyphens or between CJK), or strips the markers once revealed.
fn spoiler_text(text: &str, reveal: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut inside = false;
    let push = |segment: &str, masked: bool, out: &mut String| {
        if !masked || reveal {
            out.push_str(segment);
            return;
        }
        for (i, line) in segment.split('\n').enumerate() {
            if i > 0 {
                out.push('\n');
            }
            // A hidden picture masks as a short bar, not as long as its URL.
            if line.contains(PICTURE) {
                out.push_str("▒▒▒▒▒▒");
                continue;
            }
            // Whole graphemes, so an emoji sequence masks to the columns it shows in.
            for g in line.graphemes(true) {
                if g.chars().all(char::is_whitespace) {
                    out.push_str(g);
                } else {
                    out.extend(std::iter::repeat_n('▒', g.width()));
                }
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

/// Most rows a picture in post or comment text takes.
const TEXT_PICTURE_ROWS: u16 = 12;
/// Text pictures kept loaded before those far from view are dropped: each holds its pixels
/// and their encoding. Dropped ones load again when reached.
const TEXT_PICTURES_KEPT: usize = 24;

/// What the post document needs to lay out pictures in its text: the loaded images (none
/// when images are off), and the most rows one may take, so it fits the text area whole.
struct PictureSizes<'a> {
    images: Option<&'a HashMap<String, ImageSlot>>,
    font: ratatui_image::FontSize,
    rows: u16,
}

/// A picture in the post document: its URL, first line, the columns before it and its
/// cells once it loaded.
struct DocPicture {
    url: String,
    line: usize,
    x: u16,
    size: Option<Size>,
}

/// The URL of a picture line (`PICTURE` then the URL, maybe after a quote bar).
fn picture_line(line: &str) -> Option<&str> {
    line.find(PICTURE)
        .map(|at| &line[at + PICTURE.len_utf8()..])
}

/// `wrap_into`, with each picture line laid out as the picture: rows for it by its shape
/// once loaded (at most `sizes.rows`), else one line saying where it stands.
fn text_into(
    lines: &mut Vec<Line<'static>>,
    pictures: &mut Vec<DocPicture>,
    text: &str,
    width: usize,
    prefix: &[Span<'static>],
    style: Style,
    sizes: &PictureSizes<'_>,
) {
    let x = prefix.iter().map(|s| s.content.width()).sum::<usize>() as u16;
    let mut rest = text;
    while let Some(at) = rest.find(PICTURE) {
        let start = rest[..at].rfind('\n').map_or(0, |n| n + 1);
        let end = rest[at..].find('\n').map_or(rest.len(), |n| at + n);
        if start > 0 {
            wrap_into(lines, &rest[..start - 1], width, prefix, style);
        }
        let url = picture_line(&rest[start..end]).unwrap_or_default();
        // What stands before it on its line, a bullet or quote bar, stays before it: quote
        // bars on every row, the rest (a bullet) on the first.
        let lead = &rest[start..at];
        let quoted = lead.starts_with('│');
        let lead_style = if quoted { style.fg(SUBTEXT) } else { style };
        let bars = lead.len() - lead.trim_start_matches(['│', ' ']).len();
        let lead_rest = format!("{}{}", &lead[..bars], " ".repeat(lead[bars..].width()));
        let first_prefix = [prefix, &[Span::styled(lead.to_string(), lead_style)]].concat();
        let rest_prefix = [prefix, &[Span::styled(lead_rest, lead_style)]].concat();
        let slot = sizes.images.map(|images| images.get(url));
        // A picture shown twice keeps one size, the first one, so it isn't encoded again
        // for each place every frame; where that doesn't fit it is only named.
        let first = pictures
            .iter()
            .find_map(|p| (p.url == url).then_some(p.size).flatten());
        let cols = width.saturating_sub(lead.width()).max(1) as u16;
        let size = match (slot, first) {
            (Some(Some(ImageSlot::Ready { .. })), Some(first)) => {
                (first.width <= cols).then_some(first)
            }
            (Some(Some(ImageSlot::Ready { pixels, .. })), None) => Some(picture_cells(
                pixels.0,
                pixels.1,
                sizes.font,
                cols,
                sizes.rows,
                sizes.rows,
                MAX_ENLARGE,
            )),
            _ => None,
        };
        pictures.push(DocPicture {
            url: url.to_string(),
            line: lines.len(),
            x: x + lead.width() as u16,
            size,
        });
        match size {
            Some(size) => {
                for row in 0..size.height {
                    let prefix = if row == 0 {
                        &first_prefix
                    } else {
                        &rest_prefix
                    };
                    lines.push(Line::from(prefix.clone()));
                }
            }
            None => {
                let label = match slot {
                    None | Some(Some(ImageSlot::Ready { .. })) => "▣ image",
                    Some(Some(ImageSlot::Failed(_))) => "▣ image unavailable",
                    _ => "◌ loading image",
                };
                let mut spans = first_prefix;
                spans.push(Span::styled(label, Style::new().fg(OVERLAY)));
                lines.push(Line::from(spans));
            }
        }
        rest = rest.get(end + 1..).unwrap_or_default();
    }
    if !rest.is_empty() {
        wrap_into(lines, rest, width, prefix, style);
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
    let post = view.shown_post();
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
    let title_style = if app.config.highlights(post) {
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
                .padding(Padding::horizontal(1))
                .title_bottom(Line::styled(
                    view.freshness
                        .map(|freshness| {
                            freshness.label(
                                now.timestamp(),
                                view.loading,
                                view.error.is_some(),
                                app.health == Health::Down,
                            )
                        })
                        .unwrap_or_default(),
                    Style::new().fg(OVERLAY),
                )),
        ),
        head_area,
    );

    // The image or video shows only while the document stays at its top. Settle the
    // scroll for the image layout first: deciding from the old scroll flashed the image
    // for one frame. A video shows its poster until its first frame.
    let video = post.media.video_url().is_some();
    let image_url = post
        .media
        .shows_inline()
        .then(|| post.media.preview_image().map(str::to_string));
    // Until the post page arrives, a stale list copy may lack the spoiler tag.
    let hide_image = app
        .post_view
        .as_ref()
        .is_some_and(|v| v.spoiler_hidden() || v.page.is_none());
    // The picture's rows follow its shape (`picture_cells`): a wide one fills the width
    // while a few lines of text stay in view, a tall one is centred within half the post
    // area. A video adds its status and blank rows, and decodes for its whole box so a
    // frame arriving does not restart it.
    let cols = rest.width.saturating_sub(2);
    let wide_max = rest.height.saturating_sub(TEXT_UNDER_PICTURE).max(1);
    let tall_max = (rest.height / 2).max(1);
    app.video_tall_rows = tall_max;
    let decode_rows = wide_max.min(video::KITTY_MAX_ROWS);
    let still_url = image_url.as_ref().and_then(|u| u.as_deref());
    let picture = |max: u16| {
        picture_size(
            app,
            video,
            still_url,
            cols,
            max.min(wide_max),
            max.min(tall_max),
        )
        .map_or(1, |size| size.height)
    };
    let image_rows = if video {
        picture(video::KITTY_MAX_ROWS) + 2
    } else {
        picture(u16::MAX) + 1
    };
    // A video needs a row of picture above its status line and the blank row, with a
    // line of text under them.
    let image_url =
        image_url.filter(|_| app.inline_images_on() && !hide_image && (!video || rest.height >= 4));
    let [img_area, with_image] =
        Layout::vertical([Constraint::Length(image_rows), Constraint::Min(1)]).areas(rest);
    let margin = Margin {
        horizontal: 1,
        vertical: 0,
    };
    let (full, with_image) = (rest.inner(margin), with_image.inner(margin));
    // The image changes only the height: the text wraps the same in both layouts.
    let width = full.width.saturating_sub(2) as usize;
    let sizes = PictureSizes {
        images: app.inline_images_on().then_some(&app.images),
        font: app.image_picker.font_size(),
        rows: TEXT_PICTURE_ROWS.min(full.height).max(1),
    };
    let Some(view) = app.post_view.as_mut() else {
        return;
    };
    let mut more_lines = std::mem::take(&mut view.rendered_more);
    let PostDocument {
        lines,
        spans,
        pictures,
    } = post_document(view, width, now, Some(&mut more_lines), &sizes);
    view.row_starts = spans.iter().map(|(s, _)| *s).collect();
    let requested_scroll = view.scroll;
    // Replies loaded above the text being read: put it back where it was.
    let reading_anchor = view.take_reading_anchor();
    if let Some((row, offset)) = reading_anchor
        && let Some(&(start, _)) = spans.get(row)
    {
        view.scroll = start.saturating_add_signed(offset);
    }
    // Pictures above the text being read took their rows as they loaded: move with it. On
    // a content change, text matching below, where it applies, places the view instead.
    if reading_anchor.is_none() {
        let grown: isize = pictures
            .iter()
            .zip(&view.rendered_pictures)
            .take_while(|(new, (url, ..))| new.url == *url)
            .filter(|(_, (_, line, _))| *line < requested_scroll)
            // Only its rows above the top count: one wholly above moves the text by all it
            // gained, one across the top by those it now has above the top.
            .map(|(new, (_, line, rows))| {
                let above = requested_scroll - line;
                if *rows <= above {
                    new.rows() as isize - *rows as isize
                } else {
                    new.rows().min(above) as isize - above as isize
                }
            })
            .sum();
        view.scroll = view.scroll.saturating_add_signed(grown);
        // A comment held in place (`l`) is held at the moved text too.
        if grown != 0
            && let Some(hold) = &mut view.hold_view
            && !hold.top
            && let Some(&(start, _)) = spans.get(hold.row)
        {
            hold.offset = view.scroll as isize - start as isize;
        }
    }
    if view.content_changed {
        let handoff = std::mem::take(&mut view.more_handoff);
        if handoff && reading_anchor.is_some() {
            // The semantic row already accounts for spoiler reflow above it.
            view.rewrapped = None;
        }
        let held = view
            .hold_view
            .filter(|hold| !view.free_scroll && hold.row == view.cursor && !hold.top);
        // Text matching must not override a More → first-new-reply handoff by matching a
        // later sibling.
        let anchor_row = if !view.free_scroll {
            Some(view.cursor)
        } else if handoff {
            reading_anchor.map(|(row, _)| row)
        } else {
            None
        };
        let bounds = anchor_row
            .and_then(|row| spans.get(row))
            .map_or(0..lines.len(), |&(start, end)| start..end);
        if requested_scroll != 0
            && (view.free_scroll || held.is_some())
            && let Some((was, target)) = edit_anchor(
                &view.rendered_document,
                requested_scroll,
                &lines,
                view.page_height,
                bounds,
            )
        {
            view.scroll = target.saturating_sub(was - requested_scroll);
            if let Some(hold) = &mut view.hold_view
                && let Some((start, _)) = spans.get(hold.row)
            {
                hold.offset = view.scroll as isize - *start as isize;
            }
            // The retained document already represents the pre-toggle display.
            view.rewrapped = None;
        }
        view.content_changed = false;
    }
    // After `v` only spoiler lines changed, so the old top line exists in the new wrap.
    // Keys in the same batch move on from it; `g` stays at the top.
    if let Some((was, pinned)) = view.rewrapped.take()
        && view.free_scroll
        && view.reveal_spoilers != was
        && (view.scroll != 0 || pinned == 0)
    {
        let shown = view.reveal_spoilers;
        view.reveal_spoilers = was;
        let old_lines = post_document(view, width, now, None, &sizes).lines;
        view.reveal_spoilers = shown;
        if let Some(at) = same_line(&old_lines, pinned, &lines) {
            let moved = view.scroll as isize - pinned as isize;
            view.scroll = at.saturating_add_signed(moved);
        }
    }
    // `l` on the comment being read holds its text in place while it stays selected and
    // visible; moving the selection ends the hold. Loads above move both.
    if view.free_scroll || view.hold_view.is_none_or(|h| h.row != view.cursor) {
        view.hold_view = None;
    }
    // Scrolled away, the image stays away until a key returns to the top, even when a short
    // post clamps the scroll back to 0: otherwise frames alternate with and without it.
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
    // A hold kept at the top for the image: with no image this frame, hold the text where it is.
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
    for (row, line) in lines.iter().skip(scroll).take(height).enumerate() {
        f.buffer_mut()
            .set_line(doc_area.x, doc_area.y + row as u16, line, doc_area.width);
    }
    view.rendered_document = lines;
    view.rendered_more = more_lines;
    view.rendered_pictures = pictures
        .iter()
        .map(|p| (p.url.clone(), p.line, p.rows()))
        .collect();
    draw_scrollbar(f, doc_area, scroll, height, total);
    // Pictures in the text: drawn when wholly in view, loaded from a screen away.
    let near = scroll.saturating_sub(height)..scroll + height * 2;
    let images_on = sizes.images.is_some();
    let mut wanted = Vec::new();
    let mut shown = Vec::new();
    for picture in &pictures {
        if images_on && near.contains(&picture.line) {
            wanted.push(picture.url.clone());
        }
        match picture.size {
            Some(size)
                if picture.line >= scroll
                    && picture.line + usize::from(size.height) <= scroll + height =>
            {
                let at = Rect::new(
                    doc_area.x + picture.x,
                    doc_area.y + (picture.line - scroll) as u16,
                    size.width,
                    size.height,
                );
                shown.push((picture.url.clone(), at));
            }
            _ => {}
        }
    }
    for (url, at) in shown {
        draw_inline_image(f, app, &url, at);
    }
    for url in wanted {
        app.request_text_picture(&url);
    }
    // Far from view: a picture still queued is let go so the ones in view come first; a
    // loaded one only once many are loaded. Either loads again when near. Pictures with no
    // place now (folded, removed, hidden again) are far too.
    let kept = scroll.saturating_sub(height * 3)..scroll + height * 4;
    let post_image = app
        .post_view
        .as_ref()
        .and_then(|view| view.page.as_ref())
        .and_then(|page| page.post.media.preview_image())
        .map(str::to_string);
    let text_pictures = || {
        app.images
            .iter()
            .filter(|(url, _)| post_image.as_deref() != Some(url.as_str()))
    };
    let loaded = text_pictures()
        .filter(|(_, slot)| matches!(slot, ImageSlot::Ready { .. }))
        .count();
    let far: Vec<String> = text_pictures()
        .filter(|(_, slot)| match slot {
            ImageSlot::Loading => true,
            ImageSlot::Ready { .. } => loaded > TEXT_PICTURES_KEPT,
            ImageSlot::Failed(_) => false,
        })
        .filter(|(url, _)| {
            !pictures
                .iter()
                .any(|p| &p.url == *url && kept.contains(&p.line))
        })
        .map(|(url, _)| url.clone())
        .collect();
    for url in &far {
        app.drop_text_picture(url);
    }
    if let Some(still) = image {
        let area = img_area.inner(margin);
        if video {
            let decode_box = Rect {
                height: decode_rows,
                ..area
            };
            draw_video(f, app, still.as_deref(), area, decode_box);
        } else if let Some(url) = still {
            // The last row stays blank.
            let picture = Rect {
                height: area.height.saturating_sub(1).max(1),
                ..area
            };
            draw_inline_image(f, app, &url, picture);
        }
    }
}

impl DocPicture {
    /// Lines the picture takes in the document.
    fn rows(&self) -> usize {
        self.size.map_or(1, |size| usize::from(size.height))
    }
}

/// The post's picture in cells, by `picture_cells`: a video's frame, else the loaded
/// image or poster; none while it loads or after it failed. A frame encoded off the UI
/// thread (sixel, iTerm2, half-blocks) has fixed cells, so the band takes exactly those.
fn picture_size(
    app: &App,
    video: bool,
    still: Option<&str>,
    cols: u16,
    wide_max: u16,
    tall_max: u16,
) -> Option<Size> {
    let font = app.image_picker.font_size();
    let frame = video
        .then(|| app.video.as_ref().and_then(|p| p.frame()))
        .flatten();
    // A frame's pixels are the decoder's, not the source's: only its shape counts.
    let (width, height, enlarge) = match frame {
        Some(frame) => match &frame.pixels {
            video::Pixels::Image(protocol) => return Some(protocol.size()),
            video::Pixels::Zlib(_) => (frame.width, frame.height, f64::INFINITY),
        },
        None => match still.map(|u| app.images.get(u)) {
            Some(Some(ImageSlot::Ready { pixels, .. })) => (pixels.0, pixels.1, MAX_ENLARGE),
            _ => return None,
        },
    };
    Some(picture_cells(
        width, height, font, cols, wide_max, tall_max, enlarge,
    ))
}

/// Lines of the post kept in view under a wide picture (with the blank row after it).
const TEXT_UNDER_PICTURE: u16 = 5;
/// A small picture grows at most this much, so a thumbnail-sized one stays sharp.
const MAX_ENLARGE: f64 = 2.0;

/// Cells a `width`×`height` px picture takes: wider than tall, it fills `cols` unless
/// that is more than `wide_max` rows; otherwise it fits `tall_max` rows (centred by the
/// caller). Never enlarged more than `enlarge` times. Rows round up, so fitting the
/// picture again into the cells it got gives the same cells.
fn picture_cells(
    width: u32,
    height: u32,
    font: ratatui_image::FontSize,
    cols: u16,
    wide_max: u16,
    tall_max: u16,
    enlarge: f64,
) -> Size {
    let (fw, fh) = (f64::from(font.width.max(1)), f64::from(font.height.max(1)));
    let (w, h) = (f64::from(width.max(1)), f64::from(height.max(1)));
    let max_rows = if width > height { wide_max } else { tall_max };
    let scale = (f64::from(cols) * fw / w)
        .min(f64::from(max_rows) * fh / h)
        .min(enlarge);
    let wide = ((w * scale / fw).round() as u16).clamp(1, cols.max(1));
    // Rows follow the width as rounded: a narrow picture rounded down is that much shorter.
    let scale = scale.min(f64::from(wide) * fw / w);
    Size::new(
        wide,
        ((h * scale / fh).ceil() as u16).clamp(1, max_rows.max(1)),
    )
}

/// Text comparison ignores styling and the selected-comment bar.
fn line_text<'a>(line: &'a Line<'_>) -> impl Iterator<Item = char> + 'a {
    line.spans
        .iter()
        .flat_map(|span| span.content.chars())
        .enumerate()
        .map(|(i, c)| if i == 0 && c == '▌' { ' ' } else { c })
}

fn line_hash(line: &Line) -> u64 {
    let mut hasher = DefaultHasher::new();
    for c in line_text(line) {
        c.hash(&mut hasher);
    }
    hasher.finish()
}

/// Find an unambiguous unchanged line within the old viewport, preserving its
/// distance from the top. Repeated punctuation never selects a different passage.
fn edit_anchor(
    old: &[Line],
    at: usize,
    new: &[Line],
    height: usize,
    bounds: std::ops::Range<usize>,
) -> Option<(usize, usize)> {
    let mut occurrences: HashMap<u64, (usize, usize, usize)> = HashMap::with_capacity(old.len());
    for line in old {
        occurrences.entry(line_hash(line)).or_default().0 += 1;
    }
    for (index, line) in new.iter().enumerate() {
        if let Some((_, count, target)) = occurrences.get_mut(&line_hash(line)) {
            *count += 1;
            *target = index;
        }
    }
    old.iter()
        .enumerate()
        .skip(at)
        .take(height)
        .find_map(|(was, line)| {
            if !line_text(line).any(|c| !c.is_whitespace() && c != '│') {
                return None;
            }
            let &(1, 1, target) = occurrences.get(&line_hash(line))? else {
                return None;
            };
            if !bounds.contains(&target) {
                return None;
            }
            line_text(line)
                .eq(line_text(&new[target]))
                .then_some((was, target))
        })
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

/// Scroll for a document area `height` lines tall, and whether the hold on the selected
/// comment still applies; otherwise the selection is brought into view. Never past the end.
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

#[derive(Default)]
struct PostDocument {
    lines: Vec<Line<'static>>,
    spans: Vec<(usize, usize)>,
    pictures: Vec<DocPicture>,
}

/// Selftext, comment spans, pictures and More-row geometry for the displayed document.
fn post_document(
    view: &PostView,
    width: usize,
    now: DateTime<Utc>,
    mut more_lines: Option<&mut Vec<(String, usize)>>,
    sizes: &PictureSizes<'_>,
) -> PostDocument {
    let mut pictures = Vec::new();
    let mut lines: Vec<Line<'static>> = Vec::new();
    // Early-return documents contain no More rows.
    if (view.page.is_none() || view.spoiler_hidden())
        && let Some(rows) = &mut more_lines
    {
        rows.clear();
    }
    if let Some(e) = view.error.as_ref().filter(|_| view.page.is_none()) {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            format!("✗ Could not load the post: {e}"),
            Style::new().fg(RED),
        ));
        lines.push(Line::styled("press r to retry", Style::new().fg(OVERLAY)));
        return PostDocument {
            lines,
            ..PostDocument::default()
        };
    }
    let Some(page) = &view.page else {
        lines.push(Line::raw(""));
        lines.push(Line::styled("⋯ loading post", Style::new().fg(OVERLAY)));
        return PostDocument {
            lines,
            ..PostDocument::default()
        };
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
        return PostDocument {
            lines,
            ..PostDocument::default()
        };
    } else if !page.body.is_empty() {
        let body = spoiler_text(&page.body, reveal);
        let style = Style::new().fg(TEXT);
        text_into(&mut lines, &mut pictures, &body, width, &[], style, sizes);
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
    let mut more_count = 0;
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
                head.push(Span::styled(
                    format!(" · {}", comment.age_at(now)),
                    bg.fg(OVERLAY),
                ));
                if *hidden > 0 {
                    head.push(Span::styled("  ", bg));
                    head.push(chip(format!("+{hidden} folded"), YELLOW, SURFACE1));
                }
                if selected {
                    pad_to(&mut head, width, bg);
                }
                lines.push(Line::from(head));
                // The bar spans the whole selected comment, so it shows with the header off screen.
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
                        .map(|l| {
                            if picture_line(l).is_some() {
                                "▣ image"
                            } else {
                                l
                            }
                        })
                        .unwrap_or_default()
                        .to_string();
                    let mut l = body_prefix.clone();
                    l.push(Span::styled(
                        truncate(&first, body_width),
                        Style::new().fg(OVERLAY),
                    ));
                    lines.push(Line::from(l));
                } else {
                    text_into(
                        &mut lines,
                        &mut pictures,
                        &spoiler_text(&comment.body, reveal),
                        body_width,
                        &body_prefix,
                        Style::new().fg(TEXT),
                        sizes,
                    );
                }
            }
            RowKind::More { parent, more } => {
                if let Some(rows) = &mut more_lines {
                    if let Some((id, line)) = rows.get_mut(more_count) {
                        if id.as_str() != *parent {
                            id.clear();
                            id.push_str(parent);
                        }
                        *line = start;
                    } else {
                        rows.push(((*parent).to_owned(), start));
                    }
                    more_count += 1;
                }
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
    if let Some(rows) = more_lines {
        rows.truncate(more_count);
    }
    PostDocument {
        lines,
        spans,
        pictures,
    }
}

/// The post's video: the current frame (the poster until the first), centred, then a
/// status line. Frames are decoded for `decode_box`, which stays put while the drawn
/// area follows the frame's shape.
fn draw_video(f: &mut Frame, app: &mut App, poster: Option<&str>, area: Rect, decode_box: Rect) {
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
    app.video_area = Some(decode_box);
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
                let fitted = picture_cells(
                    frame.width,
                    frame.height,
                    app.image_picker.font_size(),
                    picture.width,
                    picture.height,
                    picture.height,
                    f64::INFINITY,
                );
                let cells = centred(Rect::new(0, 0, fitted.width, fitted.height), picture);
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
            video::Pixels::Image(protocol) => {
                let size = protocol.size();
                let at = centred(
                    Rect::new(picture.x, picture.y, size.width, size.height),
                    picture,
                );
                f.render_widget(Image::new(protocol), at.intersection(picture));
            }
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
    draw_image_slot(
        f,
        app.images.get_mut(url),
        app.image_picker.font_size(),
        app.overlay.is_some(),
        area,
    );
}

fn draw_image_slot(
    f: &mut Frame,
    slot: Option<&mut ImageSlot>,
    font: ratatui_image::FontSize,
    covered: bool,
    area: Rect,
) {
    // As in `draw_spots`: nothing drawn under an overlay.
    match slot {
        Some(ImageSlot::Ready { .. }) if covered => {}
        Some(ImageSlot::Ready {
            protocol, pixels, ..
        }) => {
            // Sized by its shape as the layout was, centred; a small one may grow.
            let size = picture_cells(
                pixels.0,
                pixels.1,
                font,
                area.width,
                area.height,
                area.height,
                MAX_ENLARGE,
            );
            let at = centred(Rect::new(area.x, area.y, size.width, size.height), area);
            let scaled = StatefulImage::default()
                .resize(Resize::Scale(Some(ratatui_image::FilterType::Triangle)));
            f.render_stateful_widget(scaled, at, protocol.as_mut());
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

/// `inner` moved to the middle of `outer`'s width, at its top.
fn centred(inner: Rect, outer: Rect) -> Rect {
    Rect {
        x: outer.x + outer.width.saturating_sub(inner.width) / 2,
        y: outer.y,
        ..inner
    }
}

// ----- footer --------------------------------------------------------------

fn feed_activity(feed: &crate::app::Feed, can_load: bool) -> Option<(&'static str, &'static str)> {
    if feed.loading_more() {
        Some(("Loading more posts…", "more posts"))
    } else if feed.loading() || (can_load && !feed.loaded && feed.error().is_none()) {
        Some(if feed.posts.is_empty() || !feed.shows_current_query() {
            ("Loading posts…", "posts")
        } else {
            ("Refreshing posts…", "refresh")
        })
    } else if feed.refreshing() {
        Some(("Refreshing posts…", "refresh"))
    } else {
        None
    }
}

fn loading_activity(app: &App) -> Option<&'static str> {
    if app.media_loading() {
        return Some("media");
    }
    if app.post_focused() {
        if let Some(view) = &app.post_view {
            if view.loading || (view.page.is_none() && view.error.is_none()) {
                return Some(if view.page.is_some() {
                    "refresh"
                } else {
                    "post"
                });
            }
            if !view.loading_more.is_empty() {
                return Some("replies");
            }
        }
    } else {
        match app.active_tab().kind {
            TabKind::Home | TabKind::Subreddit { .. } => {
                if let Some((_, activity)) =
                    feed_activity(&app.active_tab().feed, !app.reddit.needs_sign_in())
                {
                    return Some(activity);
                }
            }
            TabKind::AllSubs if app.all_subs.loading => return Some("subs"),
            TabKind::AllSubs if app.all_subs.groups.iter().any(|group| group.loading) => {
                return Some("communities");
            }
            _ => {}
        }
    }
    (app.inline_images_on()
        && (app.card_renderer.pending()
            || app.card_images.values().any(|slot| {
                matches!(
                    slot,
                    CardImageSlot::Loading
                        | CardImageSlot::Cached {
                            refreshing: true,
                            ..
                        }
                )
            })
            || app
                .images
                .values()
                .chain(app.thumbs.values().filter(|_| app.thumbnails_on()))
                .any(|slot| {
                    matches!(
                        slot,
                        ImageSlot::Loading
                            | ImageSlot::Ready {
                                refresh: ImageRefresh::Loading,
                                ..
                            }
                    )
                })))
    .then_some("images")
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let activity = loading_activity(app)
        .map(|activity| Span::styled(format!(" ⋯ {activity}  "), Style::new().fg(YELLOW)));
    let content_width =
        usize::from(area.width).saturating_sub(activity.as_ref().map_or(0, Span::width));
    let mut line = if let Some(status) = &app.status {
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
            Span::styled(status.text.as_str(), Style::new().fg(color)),
        ])
    } else if let Some(progress) = app.membership_progress() {
        Line::from(vec![Span::styled(
            format!(" ⋯ {progress}"),
            Style::new().fg(YELLOW),
        )])
    } else if app.health == Health::SignedOut {
        Line::from(Span::styled(
            " Reddit sign-in required · A connect",
            Style::new().fg(YELLOW),
        ))
    } else if app.health == Health::Down {
        Line::from(vec![
            Span::styled(" ✗ ", Style::new().fg(RED)),
            Span::styled("Reddit is not answering", Style::new().fg(RED)),
        ])
    } else if app.health == Health::Paused {
        Line::from(vec![
            Span::styled(" ◐ ", Style::new().fg(YELLOW)),
            Span::styled(
                "Reddit requests paused · wait before retrying",
                Style::new().fg(YELLOW),
            ),
        ])
    } else {
        // `x` closes only a tab opened with `p`.
        let closable = matches!(
            app.active_tab().kind,
            TabKind::Subreddit { pinned: false, .. }
        );
        let dynamic;
        let hints: &[(&str, &str)] = if app.post_focused() {
            let spoilers = app.post_view.as_ref().is_some_and(PostView::has_spoilers);
            // `c` selects the comment being read, or acts on the selected row.
            let c = app.post_view.as_ref().and_then(|v| {
                let rows = v.rows();
                if v.free_scroll {
                    return (!rows.is_empty()).then_some(("c", "select"));
                }
                match &rows.get(v.cursor)?.kind {
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
            let shown_post = app.post_view.as_ref().map(PostView::shown_post);
            let media = shown_post
                .is_some_and(|p| !matches!(p.media, Media::None))
                .then_some(("m", "media"));
            let save = if shown_post.is_some_and(|p| app.state.is_bookmarked(&p.id)) {
                ("b", "unsave")
            } else {
                ("b", "save")
            };
            dynamic = [("j/k", "comments")]
                .into_iter()
                .chain(spoilers.then_some(("v", "spoilers")))
                .chain(c)
                .chain([("space", "page")])
                .chain([("S", "join/leave")])
                .chain(media)
                .chain([
                    ("o", "browser"),
                    save,
                    ("h", if app.detail_fits() { "list" } else { "back" }),
                ])
                .collect::<Vec<_>>();
            &dynamic
        } else if app.focus == Focus::Sidebar {
            dynamic = [
                ("j/k", "tabs"),
                ("g/G", "first/last"),
                ("l/⏎", "list"),
                ("p", "jump"),
                ("S", "join/leave"),
            ]
            .into_iter()
            .chain(closable.then_some(("x", "close")))
            .collect();
            &dynamic
        } else {
            match app.active_tab().kind {
                TabKind::AllSubs => &[
                    ("j/k", "posts"),
                    ("J/K", "cards"),
                    ("⏎", "open"),
                    ("space", "subreddit"),
                    ("S", "join/leave"),
                    ("tab", "next"),
                    ("p", "jump"),
                ],
                TabKind::Bookmarks => &[
                    ("j/k", "move"),
                    ("⏎", "open"),
                    ("V", "view"),
                    ("PgUp/Dn", "page"),
                    ("space", "sub"),
                    ("S", "join/leave"),
                    ("b", "remove"),
                    ("o", "browser"),
                    ("p", "jump"),
                ],
                TabKind::Home | TabKind::Subreddit { .. } => {
                    let feed = &app.active_tab().feed;
                    let time =
                        (feed.sort.takes_time() && feed.search.is_none()).then_some(("t", "time"));
                    dynamic = [
                        ("j/k", "move"),
                        ("⏎", "open"),
                        ("V", "view"),
                        ("PgUp/Dn", "page"),
                        ("space", "sub"),
                        ("S", "join/leave"),
                        ("s", "sort"),
                    ]
                    .into_iter()
                    .chain(time)
                    .chain(app.active_tab().subreddit().map(|_| ("/", "search")))
                    .chain([("p", "jump")])
                    .chain(closable.then_some(("x", "close")))
                    .collect();
                    &dynamic
                }
            }
        };
        // Keep whole key + label pairs; do not build pairs past the available width.
        let mut fitted = vec![Span::raw(" ")];
        let mut used = 1;
        for (key, label) in hints.iter().chain(&[("?", "help")]) {
            if *key == "PgUp/Dn" && app.state.list_view() != ListView::Card {
                continue;
            }
            let pair = [
                key_chip(key),
                Span::styled(format!(" {label}  "), Style::new().fg(OVERLAY)),
            ];
            let width = line_width(&pair);
            if used + width > content_width {
                break;
            }
            used += width;
            fitted.extend(pair);
        }
        Line::from(fitted)
    };
    if let Some(activity) = activity {
        line.spans.insert(0, activity);
    }
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
    ("A", "saved accounts: switch, add or reconnect"),
    ("Space (list)", "open the selected community"),
    ("S", "join or leave a community on Reddit"),
    ("Tabs", ""),
    ("Tab ] / S-Tab [", "next / previous tab"),
    ("1-9 / 0", "jump to a tab / the last tab"),
    ("p", "jump to a tab or open any subreddit"),
    ("x", "close a tab opened with p"),
    ("\\", "hide or show the sidebar"),
    ("h · l Enter Esc", "focus the sidebar · back to the list"),
    ("j k · g G", "sidebar: next / previous tab · first / last"),
    ("Lists", ""),
    ("V", "Card / Compact view (saved)"),
    (
        "PgDn/PgUp · C-d/u",
        "cards: page / half page; then next post",
    ),
    ("C-e C-y", "cards: scroll down / up one line"),
    ("j k · C-d C-u", "move · page; a post open beside follows"),
    ("g G", "first / last"),
    ("Enter l", "open the post, or resume the one shown"),
    ("J K", "All subs: next / previous card"),
    ("Space", "All subs: open the card's subreddit"),
    ("s · t", "Home/subreddit: sort · time range"),
    ("/ · Esc", "subreddit: search · clear search"),
    ("L", "feed: next page (automatic near end)"),
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
    ("h", "split: list; narrow: back one level"),
    ("Esc q", "back to the list"),
    ("Anywhere", ""),
    ("o · y", "open on reddit.com · copy link"),
    ("m", "media: image viewer, mpv, or browser"),
    ("b", "bookmark"),
    ("i", "images and videos on / off"),
    ("r", "reload"),
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
                        TabKind::AllSubs => "▦",
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

fn draw_sort(f: &mut Frame, app: &App, selected: usize) {
    let sorts = app.active_tab().sorts();
    let home = matches!(app.active_tab().kind, TabKind::Home);
    let area = popup(f.area(), 34, sorts.len() as u16 + 4);
    f.render_widget(Clear, area);
    let lines: Vec<Line> = sorts
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
            if home && *s == Sort::Best {
                spans.push(Span::styled(" recommendations", bg.fg(TEXT)));
            }
            pad_to(&mut spans, 28, bg);
            Line::from(spans)
        })
        .collect();
    f.render_widget(Paragraph::new(lines).block(popup_block("Sort")), area);
}

fn draw_membership(f: &mut Frame, subreddit: &str, join: bool) {
    let action = if join { "Join" } else { "Leave" };
    let lines = vec![
        Line::styled(
            format!("r/{subreddit}"),
            Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
        ),
        Line::raw(format!(
            "Enter {} · Esc cancel",
            action.to_ascii_lowercase()
        )),
        Line::raw("Space open subreddit"),
    ];
    let cols = f.area().width.saturating_sub(8).clamp(1, 50) as usize;
    let rows: usize = lines
        .iter()
        .map(|line| {
            let text: String = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect();
            textwrap::wrap(&text, cols).len().max(1)
        })
        .sum();
    let area = popup(f.area(), 54, rows as u16 + 2);
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .block(popup_block(&format!("{action} on Reddit"))),
        area,
    );
}

fn draw_auth(f: &mut Frame, app: &App) {
    use crate::auth::Phase;
    let auth = &app.auth;
    let area = popup(f.area(), 76, 22);
    f.render_widget(Clear, area);
    let block = popup_block(
        if matches!(auth.phase, Phase::DisconnectConfirm | Phase::Disconnecting) {
            " Remove from redtab "
        } else if matches!(
            auth.phase,
            Phase::Accounts | Phase::LoadingAccounts | Phase::Switching
        ) {
            " Accounts "
        } else {
            " Connect Reddit "
        },
    );
    let inner = block.inner(area);
    f.render_widget(block, area);
    let width = usize::from(inner.width).max(1);
    let wrapped = |text: &str, color| -> Vec<Line<'static>> {
        textwrap::wrap(text, width)
            .into_iter()
            .map(|row| Line::styled(row.into_owned(), Style::new().fg(color)))
            .collect()
    };
    let mut lines = auth
        .message
        .as_deref()
        .map(|message| wrapped(message, YELLOW))
        .unwrap_or_default();
    let mut controls = Vec::new();
    match auth.phase {
        Phase::Accounts => {
            let mut footer = wrapped("Enter switches · j/k choose · a Add account", TEXT);
            footer.extend(wrapped("r Reconnect selected · Esc close", TEXT));
            footer.extend(wrapped("d Remove from redtab", TEXT));
            let height = usize::from(inner.height);
            lines.truncate(height.saturating_sub(footer.len() + 1));
            if lines.is_empty() && height > footer.len() + 1 {
                lines.extend(wrapped("Choose a saved account or add another.", TEXT));
            }
            let visible = height.saturating_sub(lines.len() + footer.len()).max(1);
            let start = auth.selected.saturating_sub(visible.saturating_sub(1));
            for index in (start..auth.choices()).take(visible) {
                let selected = index == auth.selected;
                let label = if let Some(account) = auth.accounts.get(index) {
                    let active = auth.active_id.as_ref() == Some(&account.id);
                    let status = match (active, account.expired) {
                        (true, true) => "active, expired · ",
                        (true, false) => "active · ",
                        (false, true) => "expired · ",
                        (false, false) => "",
                    };
                    format!("{status}{}", account.name)
                } else {
                    "+ Add account".into()
                };
                lines.push(Line::styled(
                    truncate(
                        &format!("{} {label}", if selected { "›" } else { " " }),
                        width,
                    ),
                    Style::new().fg(if selected { ACCENT } else { TEXT }),
                ));
            }
            lines.extend(footer);
        }
        Phase::LoadingAccounts => {
            lines.extend(wrapped("⋯ Loading saved accounts…", TEXT));
            controls = wrapped("Esc cancels", TEXT);
        }
        Phase::Switching => {
            lines.extend(wrapped("⋯ Verifying account…", TEXT));
            lines.extend(wrapped(
                "A failed switch leaves your connection unchanged.",
                SUBTEXT,
            ));
            controls = wrapped("Esc cancels", TEXT);
        }
        Phase::Idle | Phase::Choose => {
            let mut footer = wrapped("Enter connects · j/k choose · r rescan · Esc cancel", TEXT);
            if auth.active_id.is_some() {
                footer.extend(wrapped("d Remove from redtab", TEXT));
            }
            let disclosure = wrapped("Only Reddit cookies; OS access may prompt.", SUBTEXT);
            let height = usize::from(inner.height);
            if lines.len() + footer.len() + disclosure.len() + 2 <= height {
                footer.extend(disclosure);
            }
            // Errors and controls stay visible; labels occupy one row each.
            lines.truncate(height.saturating_sub(footer.len() + 1));
            if lines.is_empty() && height > footer.len() + 1 {
                lines.extend(wrapped("Choose a profile or a new login window.", TEXT));
            }
            let visible = height.saturating_sub(lines.len() + footer.len()).max(1);
            let start = auth.selected.saturating_sub(visible.saturating_sub(1));
            for (index, source) in auth.sources.iter().enumerate().skip(start).take(visible) {
                let selected = index == auth.selected;
                lines.push(Line::from(Span::styled(
                    truncate(
                        &format!("{} {}", if selected { "›" } else { " " }, source.label()),
                        width,
                    ),
                    Style::new().fg(if selected { ACCENT } else { TEXT }),
                )));
            }
            lines.extend(footer);
        }
        Phase::Discovering => {
            lines.extend(wrapped("⋯ Finding browser profiles…", TEXT));
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
            lines.extend(wrapped(
                if window {
                    "⋯ Sign in to Reddit in the separate browser window…"
                } else {
                    "⋯ Reading Reddit session and verifying account…"
                },
                TEXT,
            ));
            lines.extend(wrapped("Allow OS access if prompted. Esc cancels.", TEXT));
            if auth.can_remove() {
                controls = wrapped("d Remove from redtab", TEXT);
            }
        }
        Phase::Confirm => {
            lines.extend(wrapped(
                &format!("Verified u/{}", auth.account_name().unwrap_or_default()),
                GREEN,
            ));
            controls = wrapped(
                "Enter saves this connection · Esc keeps the current account",
                TEXT,
            );
        }
        Phase::Saving => lines.extend(wrapped("⋯ Saving account…", TEXT)),
        Phase::DisconnectConfirm => {
            let who = auth
                .removal_name
                .as_deref()
                .unwrap_or("the current account");
            lines.extend(wrapped(&format!("Remove {who} from redtab?"), YELLOW));
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
        Phase::Disconnecting => lines.extend(wrapped("⋯ Removing saved session…", TEXT)),
    }
    lines.truncate(usize::from(inner.height).saturating_sub(controls.len()));
    lines.extend(controls);
    f.render_widget(Paragraph::new(lines).style(Style::new().fg(TEXT)), inner);
}

fn draw_search(f: &mut Frame, app: &App, input: &str) {
    let area = popup(f.area(), 60, 3);
    f.render_widget(Clear, area);
    let title = format!("Search {}", app.active_tab().title());
    // Borders and padding (4), "🔍 " (3: the emoji is 2 wide), the caret (1).
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

/// Input fitted to `room` columns; a longer input shows its end after "…".
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
pub(crate) mod tests {
    use super::*;
    use crate::config::Config;
    use crate::model::{Comment, PostPage, ReplyPage};
    use crate::reddit::Reddit;
    use crate::worker::{Msg, Pool};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui_image::picker::Picker;

    fn app(subscriptions: &[&str]) -> App {
        let reddit = Reddit::test_at("http://127.0.0.1:9", false);
        let (pool, _rx) = Pool::start(reddit.clone(), 3, None);
        let state = std::env::temp_dir().join(format!(
            "redtab-ui-test-{}-{:?}.json",
            std::process::id(),
            std::thread::current().id()
        ));
        let mut app = App::new(
            Config {
                image_preview: false,
                ..Config::default()
            },
            pool,
            reddit,
            state,
            Picker::halfblocks(),
        );
        app.on_msg(Msg::Subscriptions {
            generation: 0,
            result: Ok(subscriptions.iter().map(|s| s.to_string()).collect()),
        });
        app.active = crate::app::ALL_SUBS_TAB;
        app
    }

    #[test]
    fn when_disconnect_is_chosen_it_should_require_confirmation_and_allow_escape() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = app(&[]);
        app.auth.phase = crate::auth::Phase::Choose;
        app.auth.active_id = Some("reader".into());
        app.auth.active_name = Some("reader".into());
        app.overlay = Some(Overlay::Auth);
        crate::keys::handle(
            &mut app,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        );
        assert_eq!(app.auth.phase, crate::auth::Phase::DisconnectConfirm);
        let actual = screen(&mut app, 60, 12);
        assert!(actual.contains("Remove u/reader from redtab?"), "{actual}");
        assert!(
            actual.contains("Other accounts, bookmarks and history are kept."),
            "{actual}"
        );
        assert!(
            actual.contains("Your browser session is unchanged."),
            "{actual}"
        );
        assert!(actual.contains("Enter removes"), "{actual}");
        crate::keys::handle(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.overlay.is_none());
        assert!(!app.reddit.needs_sign_in());
    }

    #[test]
    fn when_profile_discovery_is_pending_disconnect_should_still_be_available() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = app(&[]);
        app.auth.phase = crate::auth::Phase::Discovering;
        app.auth.active_id = Some("reader".into());
        app.overlay = Some(Overlay::Auth);
        let actual = screen(&mut app, 60, 12);
        assert!(actual.contains("d Remove from redtab"), "{actual}");
        crate::keys::handle(
            &mut app,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        );
        assert_eq!(app.auth.phase, crate::auth::Phase::DisconnectConfirm);
    }

    #[test]
    fn when_disconnect_is_pending_it_should_show_progress_and_prevent_accidental_escape() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = app(&[]);
        app.auth.phase = crate::auth::Phase::Disconnecting;
        app.overlay = Some(Overlay::Auth);
        crate::keys::handle(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        let actual = screen(&mut app, 60, 12);
        assert!(actual.contains("Removing saved session"), "{actual}");
        assert_eq!(app.auth.phase, crate::auth::Phase::Disconnecting);
        assert!(matches!(app.overlay, Some(Overlay::Auth)));
    }

    #[test]
    fn when_disconnected_the_empty_feed_should_invite_connecting_without_loading_activity() {
        let mut app = app(&[]);
        app.reddit = Reddit::disconnected(false).unwrap();
        app.health = Health::SignedOut;
        app.active = 0;
        let actual = screen(&mut app, 90, 24);
        assert!(actual.contains("Connect Reddit"), "{actual}");
        assert!(
            actual.contains("Press A to connect your account"),
            "{actual}"
        );
        assert!(!actual.contains("Loading"), "{actual}");
        assert!(loading_activity(&app).is_none());
    }

    #[test]
    fn when_disconnected_all_subs_should_invite_connecting_instead_of_claiming_no_memberships() {
        let mut app = app(&[]);
        app.reddit = Reddit::disconnected(false).unwrap();
        app.health = Health::SignedOut;
        let actual = screen(&mut app, 90, 24);
        assert!(actual.contains("Connect Reddit"), "{actual}");
        assert!(!actual.contains("No joined communities"), "{actual}");
    }

    #[test]
    fn when_disconnect_cannot_save_state_its_controls_should_stay_visible_with_a_long_error() {
        let mut app = app(&[]);
        app.auth.phase = crate::auth::Phase::DisconnectConfirm;
        app.auth.message =
            Some("Could not save the current reading state; connection unchanged. ".repeat(8));
        app.overlay = Some(Overlay::Auth);
        let actual = screen(&mut app, 60, 12);
        assert!(
            actual.contains("Could not save the current reading state"),
            "{actual}"
        );
        assert!(actual.contains("Enter removes · Esc cancels"), "{actual}");
    }

    #[test]
    fn when_the_connect_dialog_is_open_it_should_show_profile_selection_without_export_instructions()
     {
        let mut app = app(&[]);
        app.auth.phase = crate::auth::Phase::Choose;
        app.auth.sources = vec![crate::auth::Source::Browser {
            browser: "firefox".into(),
            profile: "fixture-profile".into(),
            label: "Firefox · Work".into(),
        }];
        app.overlay = Some(Overlay::Auth);
        let actual = screen(&mut app, 110, 30);
        assert!(actual.contains("Connect Reddit"));
        assert!(actual.contains("Firefox · Work"));
        assert!(actual.contains("Enter connects"));
    }

    fn saved_account(id: &str, expired: bool) -> crate::reddit::SavedAccount {
        crate::reddit::SavedAccount {
            id: id.into(),
            name: format!("u/{id}"),
            expired,
            source: None,
        }
    }

    #[test]
    fn when_saved_accounts_are_shown_it_should_mark_active_expired_and_add_choices() {
        let mut app = app(&[]);
        app.auth.phase = crate::auth::Phase::Accounts;
        app.auth.active_id = Some("first".into());
        app.auth.accounts = vec![saved_account("first", false), saved_account("second", true)];
        app.overlay = Some(Overlay::Auth);

        let actual = screen(&mut app, 90, 24);

        assert!(actual.contains("Accounts"), "{actual}");
        assert!(actual.contains("active · u/first"), "{actual}");
        assert!(actual.contains("expired · u/second"), "{actual}");
        assert!(actual.contains("+ Add account"), "{actual}");
        assert!(actual.contains("Enter switches"), "{actual}");
        assert!(actual.contains("d Remove from redtab"), "{actual}");
    }

    #[test]
    fn when_navigating_saved_accounts_the_arrow_keys_should_reach_accounts_and_add() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = app(&[]);
        app.auth.phase = crate::auth::Phase::Accounts;
        app.auth.accounts = vec![saved_account("first", false)];
        app.overlay = Some(Overlay::Auth);

        crate::keys::handle(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));

        assert_eq!(
            app.auth.selected, 1,
            "Add account must be reachable without browser sources"
        );
        let actual = screen(&mut app, 90, 24);
        assert!(actual.contains("› + Add account"), "{actual}");
    }

    #[test]
    fn when_saved_account_switching_is_pending_it_should_show_progress_and_allow_escape() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = app(&[]);
        app.auth.phase = crate::auth::Phase::Switching;
        app.overlay = Some(Overlay::Auth);
        let actual = screen(&mut app, 60, 12);
        assert!(actual.contains("Verifying account"), "{actual}");
        assert!(actual.contains("Esc cancels"), "{actual}");

        crate::keys::handle(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert!(app.overlay.is_none());
        assert!(!app.reddit.needs_sign_in());
    }

    #[test]
    fn when_account_loading_is_pending_it_should_show_a_loading_indicator() {
        let mut app = app(&[]);
        app.auth.phase = crate::auth::Phase::LoadingAccounts;
        app.overlay = Some(Overlay::Auth);
        let actual = screen(&mut app, 60, 12);
        assert!(actual.contains("Loading saved accounts"), "{actual}");
        assert!(actual.contains("Esc cancels"), "{actual}");
    }

    #[test]
    fn when_saved_account_errors_are_long_the_selected_row_and_controls_should_stay_visible() {
        let mut app = app(&[]);
        app.auth.phase = crate::auth::Phase::Accounts;
        app.auth.accounts = (0..10)
            .map(|n| saved_account(&format!("reader{n}"), false))
            .collect();
        app.auth.selected = 9;
        app.auth.message =
            Some("Could not switch accounts. Reconnect from your browser. ".repeat(8));
        app.overlay = Some(Overlay::Auth);
        let actual = screen(&mut app, 60, 12);
        assert!(actual.contains("Could not switch accounts"), "{actual}");
        assert!(actual.contains("› u/reader9"), "{actual}");
        assert!(actual.contains("Enter switches"), "{actual}");
        assert!(actual.contains("r Reconnect selected"), "{actual}");
        assert!(actual.contains("d Remove from redtab"), "{actual}");
    }

    #[test]
    fn when_removing_an_inactive_saved_account_repeated_delete_keys_should_keep_the_same_confirmation()
     {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = app(&[]);
        app.auth.phase = crate::auth::Phase::Accounts;
        app.auth.active_id = Some("current".into());
        app.auth.accounts = vec![saved_account("expired", true)];
        app.overlay = Some(Overlay::Auth);
        crate::keys::handle(
            &mut app,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        );
        crate::keys::handle(
            &mut app,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        );
        let actual = screen(&mut app, 76, 22);
        assert!(actual.contains("Remove u/expired from redtab?"), "{actual}");
        assert!(
            actual.contains("Remove only this saved session."),
            "{actual}"
        );
        assert!(actual.contains("Enter removes"), "{actual}");
    }

    #[test]
    fn when_import_fails_in_a_small_terminal_it_should_show_the_error_selection_and_controls() {
        let mut app = app(&[]);
        app.auth.phase = crate::auth::Phase::Choose;
        app.auth.sources = (0..10)
            .map(|index| crate::auth::Source::Browser {
                browser: "firefox".into(),
                profile: format!("profile-{index}"),
                label: format!("Firefox {index} · {}", "Wørk ".repeat(30)),
            })
            .collect();
        app.auth.selected = 9;
        app.auth.message = Some(
            "Could not read Reddit cookies. Unlock your browser or choose a login window.".into(),
        );
        app.overlay = Some(Overlay::Auth);
        let actual = screen(&mut app, 60, 12);
        assert!(actual.contains("Could not read Reddit cookies"), "{actual}");
        assert!(actual.contains("login window"), "{actual}");
        assert!(actual.contains("› Firefox 9"), "{actual}");
        assert!(actual.contains("Enter connects"), "{actual}");
        assert!(actual.contains("Esc cancel"), "{actual}");
    }

    #[test]
    fn when_connection_loading_has_no_active_account_it_should_not_offer_disconnect() {
        for phase in [
            crate::auth::Phase::Discovering,
            crate::auth::Phase::Importing,
        ] {
            for connected in [false, true] {
                let mut app = app(&[]);
                app.auth.phase = phase;
                app.auth.active_id = connected.then(|| "current".into());
                app.overlay = Some(Overlay::Auth);

                let actual = screen(&mut app, 76, 22);

                assert_eq!(
                    actual.contains("d Remove from redtab"),
                    connected,
                    "{actual}"
                );
                assert!(actual.contains("Esc cancels"), "{actual}");
            }
        }
    }

    #[test]
    fn when_import_is_pending_it_should_remain_visible_in_a_small_terminal() {
        let mut app = app(&[]);
        app.auth.phase = crate::auth::Phase::Importing;
        app.overlay = Some(Overlay::Auth);
        let actual = screen(&mut app, 60, 12);
        assert!(actual.contains("verifying account"));
        assert!(actual.contains("Esc cancels"));
    }

    fn screen(app: &mut App, w: u16, h: u16) -> String {
        buffer_text(&screen_buffer(app, w, h))
    }

    fn screen_buffer(app: &mut App, w: u16, h: u16) -> ratatui::buffer::Buffer {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        while app.card_renderer.pending() {
            app.card_renderer.wait_for_completion();
            app.card_renderer.poll();
            terminal.draw(|f| draw(f, app)).unwrap();
        }
        terminal.backend().buffer().clone()
    }

    fn first_screen(app: &mut App, w: u16, h: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        buffer_text(terminal.backend().buffer())
    }

    fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn when_home_sort_opens_in_a_short_terminal_it_should_show_every_option() {
        let mut app = app(&[]);
        app.switch_tab(0);
        app.overlay = Some(Overlay::Sort { selected: 5 });

        let shown = screen(&mut app, 80, 10);

        assert!(shown.contains("rising"), "{shown}");
        assert!(shown.contains("controversial"), "{shown}");
        assert!(shown.contains("▌ c"), "{shown}");
    }

    #[test]
    fn when_membership_dialog_opens_it_should_name_the_account_action_in_a_short_terminal() {
        let name = "a_very_long_community_name_with_fifty_characters_123";
        for join in [true, false] {
            for width in [56, 80] {
                let mut app = app(&[]);
                app.overlay = Some(Overlay::Membership {
                    subreddit: name.into(),
                    join,
                });
                let shown = screen(&mut app, width, 10);
                assert!(
                    shown.contains(if join {
                        "Join on Reddit"
                    } else {
                        "Leave on Reddit"
                    }),
                    "{shown}"
                );
                // Wrapping must retain the entire target, including its suffix.
                let unwrapped: String = shown
                    .lines()
                    .filter_map(|line| line.split('│').nth(1))
                    .flat_map(str::chars)
                    .filter(|c| !c.is_whitespace())
                    .collect();
                assert!(unwrapped.contains(&format!("r/{name}")), "{shown}");
                assert!(
                    shown.contains("Enter")
                        && shown.contains("Esc cancel")
                        && shown.contains("Space open subreddit"),
                    "{shown}"
                );
            }
        }
    }

    #[test]
    fn when_a_community_is_open_it_should_show_membership_and_discoverable_controls() {
        let mut app = app(&["rust"]);
        app.open_subreddit("rust");
        let shown = screen(&mut app, 120, 24);
        assert!(
            shown.contains("Joined") && shown.contains("join/leave"),
            "{shown}"
        );
        app.open_subreddit("armenia");
        let shown = screen(&mut app, 120, 24);
        assert!(shown.contains("Not joined"), "{shown}");
        app.set_membership("armenia".into(), true);
        let shown = screen(&mut app, 120, 24);
        assert!(shown.contains("Joining r/armenia"), "{shown}");
    }

    #[test]
    fn when_home_sort_opens_it_should_explain_where_recommendations_appear() {
        let mut app = app(&[]);
        app.switch_tab(0);
        app.overlay = Some(Overlay::Sort { selected: 0 });

        let shown = screen(&mut app, 80, 24);

        assert!(shown.contains("recommendations"), "{shown}");
    }

    #[test]
    fn when_home_is_open_it_should_show_a_mixed_list_and_all_subs_should_keep_islands() {
        let mut app = app(&["rust"]);
        app.switch_tab(0);
        let mut post = post_with("personalized", Media::None);
        post.subreddit = "rust".into();
        app.on_msg(Msg::Feed {
            tab: app.active_tab().id,
            generation: app.active_tab().feed.generation,
            append: false,
            result: Ok(crate::model::Listing {
                posts: vec![post.clone()],
                after: None,
            }),
        });
        let home = screen(&mut app, 120, 30);
        assert!(
            home.contains("best") && home.contains("r/rust") && home.contains("personalized"),
            "{home}"
        );
        assert!(!home.contains("J/K"), "{home}");

        app.switch_tab(crate::app::ALL_SUBS_TAB);
        app.on_msg(Msg::Group {
            generation: app.all_subs.groups[0].generation,
            subreddit: "rust".into(),
            result: Ok(crate::model::Listing {
                posts: vec![post],
                after: None,
            }),
        });
        let islands = screen(&mut app, 120, 30);
        assert!(
            islands.contains("▦ All subs") && islands.contains("personalized"),
            "{islands}"
        );
        assert!(islands.contains("J/K"), "{islands}");
    }

    #[test]
    fn when_subscriptions_are_loading_empty_or_failed_all_subs_should_explain_the_state() {
        let mut app = app(&[]);
        app.all_subs.loading = true;
        assert!(screen(&mut app, 80, 24).contains("Loading subscriptions"));
        app.all_subs.loading = false;
        assert!(screen(&mut app, 80, 24).contains("No joined communities"));
        app.all_subs.error = Some("offline".into());
        let failed = screen(&mut app, 80, 24);
        assert!(
            failed.contains("Could not load subscriptions") && failed.contains("press r"),
            "{failed}"
        );
    }

    #[test]
    fn when_home_sorts_change_best_should_be_available_only_on_home() {
        let mut app = app(&["rust"]);
        app.switch_tab(0);
        press(&mut app, 's');
        assert!(screen(&mut app, 80, 24).contains("best"));
        press(&mut app, 'n');
        assert_eq!(app.active_tab().feed.sort, Sort::New);
        app.open_subreddit("rust");
        press(&mut app, 's');
        assert!(!screen(&mut app, 80, 24).contains("best"));
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

    #[test]
    fn health_shows_in_the_sidebar_footer_and_card_ages() {
        let mut app = app(&["rust"]);
        app.status = None;
        app.all_subs.groups[0].freshness = Some(crate::cache::Freshness {
            fetched_at: crate::app::now(),
            source: crate::cache::Source::default(),
        });
        for (health, sidebar) in [
            (Health::Unknown, "○ reddit "),
            (Health::Up, "● reddit "),
            (Health::Paused, "◐ reddit paused"),
            (Health::Down, "✗ reddit down"),
        ] {
            app.health = health;
            let s = screen(&mut app, 120, 30);
            let footer = s.lines().last().unwrap();

            assert!(
                s.lines()
                    .any(|l| l.chars().take(30).collect::<String>().contains(sidebar)),
                "{health:?}: {s}"
            );
            assert_eq!(footer.contains("requests paused"), health == Health::Paused);
            assert_eq!(footer.contains("not answering"), health == Health::Down);
            assert_eq!(s.contains("offline"), health == Health::Down, "{health:?}");
        }
        // A narrow sidebar keeps the mark and drops the state.
        let s = screen(&mut app, 100, 30);
        assert!(s.contains("✗ reddit ") && !s.contains("reddit down"), "{s}");
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
            permalink: "/r/s/comments/p/x/".into(),
            media: Media::None,
            preview_size: None,
            stickied: false,
            nsfw: false,
            spoiler: false,
        };
        app.all_subs.groups = vec![crate::app::Group {
            subreddit: "s".into(),
            posts: vec![post.clone()],
            ..Default::default()
        }];
        app.open_current_post();
        screen(&mut app, 100, 30);
        while_loading(&mut app);
        let generation = app.post_view.as_ref().unwrap().generation;
        let body = paragraphs("paragraph", 1..=60);
        let comment = reply("c".into());
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

    fn reply(id: String) -> Comment {
        Comment {
            sources: None,
            created: None,
            id,
            author: "x".into(),
            flair: None,
            is_op: false,
            is_mod: false,
            score: None,
            body: "reply".into(),
            replies: vec![],
            more: None,
        }
    }

    fn paragraphs(prefix: &str, range: impl Iterator<Item = usize>) -> String {
        range
            .map(|i| format!("{prefix} {i}"))
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    #[test]
    fn a_picture_in_a_comment_should_load_then_take_rows_by_its_shape() {
        let (mut app, _) = open_long_post(|_| {});
        app.image_picker = kitty();
        app.show_images = true;
        let url = "https://preview.redd.it/c.png";
        let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
        page.body.clear();
        page.comments[0].body = format!("before\n{PICTURE}{url}\nafter");

        let loading = screen(&mut app, 100, 40);
        assert!(loading.contains("◌ loading image"), "{loading}");
        assert!(matches!(app.images.get(url), Some(ImageSlot::Loading)));

        let slot = ready(&app, 400, 200);
        app.images.insert(url.into(), slot);
        let loaded = screen(&mut app, 100, 40);

        let row = |text: &str| loaded.lines().position(|l| l.contains(text)).unwrap();
        // 400×200 grows twice to 800×400 px, 100×25 cells at 8×16: 12 rows at most.
        assert_eq!(row("after") - row("before"), 13, "{loaded}");
        let first = loaded.lines().nth(row("before") + 1).unwrap();
        assert!(first.contains('\u{10EEEE}'), "{loaded}");
    }

    #[test]
    fn when_a_picture_above_the_text_being_read_loads_that_text_should_stay_put() {
        let (mut app, _) = open_long_post(|_| {});
        app.image_picker = kitty();
        app.show_images = true;
        let url = "https://preview.redd.it/c.png";
        let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
        page.body.clear();
        page.comments[0].body = format!("{PICTURE}{url}\n\n{}", paragraphs("line", 1..=60));
        screen(&mut app, 100, 30);
        press(&mut app, ' ');
        let top = |s: &str| {
            s.lines().find_map(|l| {
                let rest = l.split("line ").nth(1)?;
                rest.split_whitespace().next().map(str::to_string)
            })
        };
        let before = top(&screen(&mut app, 100, 30));

        let slot = ready(&app, 400, 200);
        app.images.insert(url.into(), slot);
        let after = top(&screen(&mut app, 100, 30));

        assert!(before.is_some());
        assert_eq!(before, after);
    }

    /// An open post whose only comment is one picture, kitty and images on.
    fn picture_comment(url: &str) -> App {
        let (mut app, _) = open_long_post(|_| {});
        app.image_picker = kitty();
        app.show_images = true;
        let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
        page.body.clear();
        page.comments[0].body = format!("{PICTURE}{url}");
        app
    }

    #[test]
    fn when_a_cached_comment_picture_refresh_fails_retry_should_keep_its_rows_and_update_pixels() {
        let url = "https://preview.redd.it/cached-comment.png";
        let mut app = picture_comment(url);
        screen(&mut app, 100, 40);
        app.on_msg(Msg::Snapshot {
            message: Box::new(Msg::Image {
                url: url.into(),
                result: Ok(image::DynamicImage::new_rgb8(400, 200).into()),
            }),
            fetched_at: crate::app::now() - 8 * 86_400,
            refreshing: true,
            source: crate::cache::Source {
                cached: true,
                ..Default::default()
            },
        });
        screen(&mut app, 100, 40);
        let before = app.post_view.as_ref().unwrap().rendered_pictures.clone();
        app.on_msg(Msg::Image {
            url: url.into(),
            result: Err("offline".into()),
        });

        press(&mut app, 'r');
        let retrying = screen(&mut app, 100, 40);

        assert!(retrying.contains('\u{10EEEE}'), "{retrying}");
        assert!(matches!(
            app.images.get(url),
            Some(ImageSlot::Ready {
                pixels: (400, 200),
                refresh: crate::app::ImageRefresh::Loading,
                ..
            })
        ));
        assert_eq!(app.post_view.as_ref().unwrap().rendered_pictures, before);
        app.on_msg(Msg::Image {
            url: url.into(),
            result: Ok(image::DynamicImage::new_rgb8(800, 400).into()),
        });
        screen(&mut app, 100, 40);
        assert!(matches!(
            app.images.get(url),
            Some(ImageSlot::Ready {
                pixels: (800, 400),
                refresh: crate::app::ImageRefresh::Idle,
                ..
            })
        ));
        assert_eq!(app.post_view.as_ref().unwrap().rendered_pictures, before);
    }

    #[test]
    fn in_a_pane_shorter_than_a_picture_it_should_still_come_wholly_into_view() {
        let url = "https://preview.redd.it/c.png";
        let mut app = picture_comment(url);
        let slot = ready(&app, 400, 200);
        app.images.insert(url.into(), slot);

        // About 10 rows of text: shorter than the 12 a picture may take.
        let drawn = (0..20).any(|_| {
            let shown = screen(&mut app, 100, 18).contains('\u{10EEEE}');
            app.scroll_post(1);
            shown
        });

        assert!(drawn);
    }

    #[test]
    fn after_r_a_failed_comment_picture_should_load_again() {
        let url = "https://preview.redd.it/c.png";
        let mut app = picture_comment(url);
        screen(&mut app, 100, 40);
        app.images
            .insert(url.into(), ImageSlot::Failed("HTTP 429".into()));
        let failed = screen(&mut app, 100, 40);

        press(&mut app, 'r');
        let again = screen(&mut app, 100, 40);

        assert!(failed.contains("▣ image unavailable"), "{failed}");
        assert!(again.contains("◌ loading image"), "{again}");
        assert!(matches!(app.images.get(url), Some(ImageSlot::Loading)));
    }

    /// The number after "line " on the first line that has one.
    fn first_numbered_line(s: &str) -> Option<String> {
        s.lines().find_map(|l| {
            let rest = l.split("line ").nth(1)?;
            rest.split_whitespace().next().map(str::to_string)
        })
    }

    #[test]
    fn a_picture_loading_as_the_page_refreshes_should_not_move_a_short_selected_comment() {
        let url = "https://preview.redd.it/c.png";
        let mut app = picture_comment(url);
        let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
        page.comments[0].body = format!("{PICTURE}{url}\n\n{}", paragraphs("line", 1..=60));
        let mut short = reply("short".into());
        short.body = "short reply".into();
        page.comments.push(short);
        screen(&mut app, 100, 30);
        // The short comment selected and wholly in view, the picture above the top.
        let view = app.post_view.as_mut().unwrap();
        view.scroll = view.row_starts[1] - 5;
        (view.free_scroll, view.cursor, view.hold_view) = (false, 1, None);
        let row = |s: &str| s.lines().position(|l| l.contains("short reply"));
        let before = row(&screen(&mut app, 100, 30));

        app.post_view.as_mut().unwrap().content_changed = true;
        let slot = ready(&app, 400, 200);
        app.images.insert(url.into(), slot);
        let after = row(&screen(&mut app, 100, 30));

        assert!(before.is_some());
        assert_eq!(before, after);
    }

    #[test]
    fn a_held_comment_should_stay_put_when_a_picture_in_it_loads() {
        let url = "https://preview.redd.it/c.png";
        let mut app = picture_comment(url);
        let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
        page.comments[0].body = format!("intro\n{PICTURE}{url}\n\n{}", paragraphs("line", 1..=60));
        screen(&mut app, 100, 30);
        for key in [' ', ' ', ' ', 'l'] {
            press(&mut app, key);
        }
        let before = first_numbered_line(&screen(&mut app, 100, 30));

        let slot = ready(&app, 400, 200);
        app.images.insert(url.into(), slot);
        let after = first_numbered_line(&screen(&mut app, 100, 30));

        assert!(before.is_some());
        assert_eq!(before, after);
    }

    #[test]
    fn a_picture_shown_twice_should_go_to_the_terminal_once_not_every_frame() {
        let url = "https://preview.redd.it/c.png";
        let mut app = picture_comment(url);
        let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
        let mut answer = reply("r".into());
        answer.body = format!("{PICTURE}{url}");
        page.comments[0].replies.push(answer);
        // Wide: each place would size it by its own column.
        let slot = ready(&app, 2000, 100);
        app.images.insert(url.into(), slot);
        let sends = |app: &mut App| screen(app, 100, 40).matches("_Gq=2").count();

        sends(&mut app);

        assert_eq!(sends(&mut app), 0, "a redraw repeats only the cells");
    }

    #[test]
    fn a_queued_picture_scrolled_far_away_should_be_let_go() {
        let urls: Vec<String> = (0..40)
            .map(|i| format!("https://preview.redd.it/q{i}.png"))
            .collect();
        let mut app = picture_comment(&urls[0]);
        let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
        page.comments[0].body = urls
            .iter()
            .map(|url| format!("{PICTURE}{url}\n\n{}", paragraphs("filler", 1..=10)))
            .collect::<Vec<_>>()
            .join("\n\n");
        screen(&mut app, 100, 30);
        assert!(matches!(app.images.get(&urls[0]), Some(ImageSlot::Loading)));

        let view = app.post_view.as_mut().unwrap();
        view.free_scroll = true;
        view.scroll = 600;
        screen(&mut app, 100, 30);

        assert!(
            !app.images.contains_key(&urls[0]),
            "hundreds of lines above"
        );
    }

    #[test]
    fn r_in_the_list_should_retry_the_open_posts_failed_pictures_too() {
        let url = "https://preview.redd.it/c.png";
        let mut app = picture_comment(url);
        app.images
            .insert(url.into(), ImageSlot::Failed("HTTP 429".into()));
        app.focus = crate::app::Focus::List;

        app.reload();

        assert!(!matches!(app.images.get(url), Some(ImageSlot::Failed(_))));
    }

    #[test]
    fn a_quoted_picture_should_keep_its_quote_bar_on_every_row() {
        let url = "https://preview.redd.it/c.png";
        let mut app = picture_comment(url);
        let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
        page.comments[0].body = format!("│ {PICTURE}{url}");
        let slot = ready(&app, 400, 200);
        app.images.insert(url.into(), slot);

        let s = screen(&mut app, 100, 40);

        let rows: Vec<&str> = s.lines().filter(|l| l.contains('\u{10EEEE}')).collect();
        assert!(rows.len() > 1, "{s}");
        // The first row's first cell also carries the image's escape sequence.
        let barred = |l: &&str| {
            let at = l.find(['\u{10EEEE}', '\x1b']).unwrap();
            l[..at].ends_with("│ ")
        };
        assert!(rows.iter().all(barred), "{s}");
    }

    #[test]
    fn a_picture_loading_just_above_the_top_should_keep_the_text_being_read() {
        let url = "https://preview.redd.it/c.png";
        let mut app = picture_comment(url);
        let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
        page.comments[0].body = format!("{PICTURE}{url}\n\n{}", paragraphs("line", 1..=60));
        screen(&mut app, 100, 30);
        // Its one-line label 3 lines above the top, fewer than the 12 rows it will take.
        let view = app.post_view.as_mut().unwrap();
        let line = view.rendered_pictures[0].1;
        (view.free_scroll, view.scroll) = (true, line + 3);
        let before = first_numbered_line(&screen(&mut app, 100, 30));

        let slot = ready(&app, 400, 200);
        app.images.insert(url.into(), slot);
        let after = first_numbered_line(&screen(&mut app, 100, 30));

        assert!(before.is_some());
        assert_eq!(before, after);
    }

    #[test]
    fn a_queued_picture_in_a_folded_reply_should_be_let_go() {
        let url = "https://preview.redd.it/folded.png";
        let mut app = picture_comment("https://preview.redd.it/unused.png");
        let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
        page.comments[0].body = "parent".into();
        let mut answer = reply("r".into());
        answer.body = format!("{PICTURE}{url}");
        page.comments[0].replies.push(answer);
        screen(&mut app, 100, 30);
        assert!(matches!(app.images.get(url), Some(ImageSlot::Loading)));

        let id = app
            .post_view
            .as_ref()
            .unwrap()
            .page
            .as_ref()
            .unwrap()
            .comments[0]
            .id
            .clone();
        app.post_view.as_mut().unwrap().collapsed.insert(id);
        screen(&mut app, 100, 30);

        assert!(!app.images.contains_key(url));
    }

    #[test]
    fn a_bulleted_picture_in_a_quote_should_show_its_bullet_once() {
        let url = "https://preview.redd.it/c.png";
        let mut app = picture_comment(url);
        let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
        page.comments[0].body = format!("│ • {PICTURE}{url}");
        let slot = ready(&app, 400, 200);
        app.images.insert(url.into(), slot);

        let s = screen(&mut app, 100, 40);

        let rows: Vec<&str> = s.lines().filter(|l| l.contains('\u{10EEEE}')).collect();
        assert!(rows.len() > 1, "{s}");
        let bullets = rows.iter().filter(|l| l.contains("• ")).count();
        assert_eq!(bullets, 1, "{s}");
        assert!(rows.iter().all(|l| l.contains("│ ")), "{s}");
    }

    #[test]
    fn a_picture_across_the_top_shrinking_should_keep_the_text_under_it() {
        let url = "https://preview.redd.it/c.png";
        let mut app = picture_comment(url);
        let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
        page.comments[0].body = format!("{PICTURE}{url}\n\n{}", paragraphs("line", 1..=60));
        let slot = ready(&app, 400, 200);
        app.images.insert(url.into(), slot);
        screen(&mut app, 100, 30);
        // Five of the picture's 12 rows above the top, "line 1" a few rows down.
        let view = app.post_view.as_mut().unwrap();
        let line = view.rendered_pictures[0].1;
        (view.free_scroll, view.scroll) = (true, line + 5);
        let before = screen(&mut app, 100, 30);

        app.show_images = false;
        let after = screen(&mut app, 100, 30);

        assert_eq!(
            first_numbered_line(&before).as_deref(),
            Some("1"),
            "{before}"
        );
        assert_eq!(first_numbered_line(&after).as_deref(), Some("1"), "{after}");
    }

    #[test]
    fn many_loaded_pictures_far_from_view_should_be_let_go() {
        let urls: Vec<String> = (0..40)
            .map(|i| format!("https://preview.redd.it/p{i}.png"))
            .collect();
        let mut app = picture_comment(&urls[0]);
        let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
        page.comments[0].body = urls
            .iter()
            .map(|url| format!("{PICTURE}{url}\n\nfiller"))
            .collect::<Vec<_>>()
            .join("\n\n");
        for url in &urls {
            let slot = ready(&app, 400, 400);
            app.images.insert(url.clone(), slot);
        }

        screen(&mut app, 100, 30);

        assert!(app.images.contains_key(&urls[0]), "in view");
        assert!(
            !app.images.contains_key(&urls[39]),
            "hundreds of lines below"
        );
    }

    #[test]
    fn a_long_post_opens_at_its_top() {
        let (app, s) = open_long_post(|_| {});

        assert!(s.lines().any(|l| l.contains("paragraph 1 ")), "{s}");
        assert_eq!(app.post_view.as_ref().unwrap().scroll, 0);
    }

    #[test]
    fn when_a_post_is_open_the_list_should_only_remain_if_both_panes_fit() {
        for (sidebar, width, list_visible) in [
            (true, 158, true),
            (true, 157, false),
            (false, 128, true),
            (false, 127, false),
        ] {
            let (mut app, _) = open_long_post(|_| {});
            app.show_sidebar = sidebar;

            let actual = screen(&mut app, width, 30);
            let content: String = actual
                .lines()
                .map(|line| {
                    line.chars()
                        .skip(if sidebar { 30 } else { 0 })
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n");

            assert_eq!(
                content.contains("▦ All subs"),
                list_visible,
                "sidebar={sidebar}, width={width}\n{actual}"
            );
            assert!(content.contains("paragraph 1 "), "{actual}");
        }
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
            url: "https://i.redd.it/big.jpeg".into(),
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
    fn when_a_post_image_is_small_the_text_should_start_right_below_it() {
        // Reddit's 140×73 link preview grows twice: 146 px is 10 rows of 16 px (rounded
        // up), not the most a wide picture may take.
        let mut app = image_post("first line of the body");
        let slot = ready(&app, 140, 73);
        app.images.insert("https://i.redd.it/big.jpeg".into(), slot);

        let s = screen(&mut app, 100, 50);

        let row = |text: &str| s.lines().position(|l| l.contains(text)).unwrap();
        let gap = row("first line of the body") - row("╰");
        // 10 picture rows plus the 3 rows of spacing every inline picture has.
        assert_eq!(gap, 13, "rows between header and text:\n{s}");
    }

    #[test]
    fn a_post_picture_should_fill_the_width_when_wide_and_stay_within_half_when_tall() {
        // 8×16 px cells, 100 columns; a wide picture may take 40 rows, a tall one 20.
        let font = ratatui_image::FontSize::new(8, 16);
        let cells = |w, h| {
            let size = picture_cells(w, h, font, 100, 40, 20, MAX_ENLARGE);
            (size.width, size.height)
        };

        assert_eq!(cells(1600, 900), (100, 29), "wide: the whole width");
        assert_eq!(
            cells(4000, 400),
            (100, 5),
            "very wide: still the whole width"
        );
        assert_eq!(
            cells(800, 700),
            (91, 40),
            "barely wide: until the wide limit"
        );
        assert_eq!(cells(900, 1600), (23, 20), "tall: within half");
        assert_eq!(cells(1000, 1000), (40, 20), "square counts as tall");
        assert_eq!(cells(140, 73), (35, 10), "small: at most twice its size");
    }

    #[test]
    fn a_picture_fitted_again_into_its_cells_should_keep_them() {
        // The layout sizes a picture, then the draw fits it into those cells: a wide one
        // must still fill the width. 4000×420 is 5.25 rows across 100 columns of 8×16.
        let font = ratatui_image::FontSize::new(8, 16);
        for (w, h) in [
            (4000, 420),
            (400, 3000),
            (275, 1500),
            (1500, 1000),
            (1600, 900),
            (900, 1600),
            (140, 73),
        ] {
            let laid = picture_cells(w, h, font, 100, 40, 20, MAX_ENLARGE);
            let drawn = picture_cells(
                w,
                h,
                font,
                laid.width,
                laid.height,
                laid.height,
                MAX_ENLARGE,
            );
            assert_eq!(drawn, laid, "{w}×{h}");
        }
    }

    #[test]
    fn when_a_video_frame_is_decoded_smaller_it_should_still_be_drawn_at_full_size() {
        // The terminal scales a kitty frame: only its shape decides its cells.
        let placement = |w, h| {
            let mut app = open_video_post();
            let frame = video::Frame {
                width: w,
                height: h,
                pixels: video::Pixels::Zlib(vec![1, 2, 3]),
                same_as_first: false,
            };
            let url = app.reddit.media_url(VIDEO).unwrap();
            app.video = Some(video::Player::showing(&url, frame));
            let s = screen(&mut app, 100, 40);
            let header = s.split("_Ga=T").nth(1).unwrap().to_string();
            let keys: Vec<&str> = header
                .split([',', ';'])
                .filter(|k| k.starts_with("c=") || k.starts_with("r="))
                .take(2)
                .collect();
            keys.join(",")
        };

        assert_eq!(placement(160, 90), placement(1280, 720));
    }

    #[test]
    fn a_wide_video_taller_than_32_rows_should_still_fill_the_width() {
        let placement = |w, h| {
            let mut app = open_video_post();
            let frame = video::Frame {
                width: w,
                height: h,
                pixels: video::Pixels::Zlib(vec![1, 2, 3]),
                same_as_first: false,
            };
            let url = app.reddit.media_url(VIDEO).unwrap();
            app.video = Some(video::Player::showing(&url, frame));
            let s = screen(&mut app, 200, 80);
            let header = s.split("_Ga=T").nth(1).unwrap().to_string();
            let cell = |key: &str| -> u16 {
                header
                    .split([',', ';'])
                    .find_map(|k| k.strip_prefix(key))
                    .unwrap()
                    .parse()
                    .unwrap()
            };
            (cell("c="), cell("r="))
        };

        let (wide_cols, _) = placement(1600, 900);
        // 4:3 across the post's width (over 90 columns of 8 px) needs more than 32 rows.
        let (cols, rows) = placement(800, 600);
        assert!(rows > 32, "{cols}×{rows}");
        // A taller picture may drop the text's scrollbar column.
        assert!(cols.abs_diff(wide_cols) <= 1, "{cols} vs {wide_cols}");
    }

    #[test]
    fn the_image_waits_for_the_post_page() {
        let shown = std::cell::RefCell::new(String::new());
        open_long_post(|app| {
            app.image_picker = kitty();
            app.show_images = true;
            app.post_view.as_mut().unwrap().post.media = Media::Image {
                url: "https://i.redd.it/big.jpeg".into(),
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
            cursor: "c".into(),
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
        feed.focus = Focus::Sidebar;
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
            cursor: "c".into(),
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
        (0..n)
            .map(|i| Comment {
                sources: Some(Default::default()),
                ..reply(format!("r{i}"))
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
            result: Ok(ReplyPage::complete(replies(2))),
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
            cursor: "c".into(),
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
            result: Ok(ReplyPage::complete(replies(2))),
        });
    }

    #[test]
    fn a_hold_kept_for_the_image_holds_the_text_once_the_image_is_off() {
        use crate::model::MoreReplies;
        let mut app = marker_selected_under_the_image();
        // "marker 2" gets a "more replies" row of its own, below "marker 1".
        let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
        page.comments[2].more = Some(MoreReplies {
            cursor: "m2".into(),
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
            result: Ok(ReplyPage::complete(replies(1))),
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
                    result: Ok(ReplyPage::complete(replies(2))),
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
            cursor: "c".into(),
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
            result: Ok(ReplyPage::complete(
                (0..2)
                    .map(|i| Comment {
                        id: format!("r{i}"),
                        ..reply.clone()
                    })
                    .collect(),
            )),
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
            url: "https://i.redd.it/big.jpeg".into(),
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
            cursor: "c".into(),
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
            result: Ok(ReplyPage::complete(
                (0..2)
                    .map(|i| Comment {
                        id: format!("r{i}"),
                        ..reply.clone()
                    })
                    .collect(),
            )),
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
        page.comments[0].body = paragraphs("line", 1..=40);
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
            cursor: "c".into(),
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
            result: Ok(ReplyPage::complete(
                (0..2)
                    .map(|i| Comment {
                        id: format!("r{i}"),
                        ..reply.clone()
                    })
                    .collect(),
            )),
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
        page.comments[0].body = paragraphs("line", 1..=40);
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
                cursor: "c".into(),
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
                result: Ok(ReplyPage::complete(
                    (0..2)
                        .map(|i| Comment {
                            id: format!("r{i}"),
                            ..reply.clone()
                        })
                        .collect(),
                )),
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
        page.comments[0].body = paragraphs("line", 1..=40);
        // Select the tall comment, then page down inside it: its header leaves.
        for key in ['j', ' ', ' '] {
            press(&mut app, key);
            screen(&mut app, 100, 30);
        }
        let reading = app.post_view.as_ref().unwrap().scroll;

        press(&mut app, 'l');
        screen(&mut app, 100, 30);
        // Later redraws (a status timing out, a health change) keep it too.
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
            tall.body = paragraphs("line", 1..=40);
            page.comments[0].more = Some(MoreReplies {
                cursor: "c".into(),
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
                    result: Ok(ReplyPage::complete(replies)),
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
            cursor: "c".into(),
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
                result: Ok(ReplyPage::complete(replies)),
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
        let tall_body = paragraphs("line", 1..=40);
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

    /// With no thumbnail to show, rows keep the full width.
    #[test]
    fn lists_with_no_thumbnail_to_show_keep_text_only_rows() {
        let mut spoilers = feed_app(kitty());
        for p in &mut spoilers.tabs[3].feed.posts {
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
    fn a_hidden_picture_should_mask_as_a_short_bar_not_its_url() {
        let text = format!(
            "see {SPOILER_START}{PICTURE}https://preview.redd.it/abcdefghijklmnop.png{SPOILER_END}"
        );

        assert_eq!(spoiler_text(&text, false), "see ▒▒▒▒▒▒");
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
        app.tabs[3].feed.posts[0].spoiler = true;

        let s = screen(&mut app, 100, 20);

        assert!(!app.thumbs.contains_key("https://i.redd.it/p1.jpeg"));
        assert!(s.lines().any(|l| l.contains(" spoiler ")), "{s}");
    }

    #[test]
    fn home_shows_a_card_in_a_short_pane() {
        let mut app = app(&["s"]);
        app.all_subs.groups[0].posts = (1..=3)
            .map(|i| Post {
                id: format!("p{i}"),
                subreddit: "s".into(),
                title: format!("title {i}"),
                author: "a".into(),
                flair: None,
                score: Some(1),
                comments: 0,
                created: None,
                permalink: format!("/r/s/comments/p{i}/x/"),
                media: Media::None,
                preview_size: None,
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
    pub(crate) fn kitty() -> Picker {
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
            permalink: format!("/r/s/comments/{id}/x/"),
            media,
            preview_size: None,
            stickied: false,
            nsfw: false,
            spoiler: false,
        }
    }

    /// r/s with eight posts; all but `p2` have an image.
    fn card_feed_app(picker: Picker) -> App {
        let mut app = app(&["s"]);
        app.image_picker = picker;
        app.show_images = true;
        app.switch_tab(3);
        let tab = app.tabs[3].id;
        let generation = app.tabs[3].feed.generation;
        let posts = (1..=8)
            .map(|i| {
                let media = if i == 2 {
                    Media::None
                } else {
                    Media::Image {
                        url: format!("https://i.redd.it/p{i}.jpeg"),
                    }
                };
                post_with(&format!("p{i}"), media)
            })
            .collect();
        app.on_msg(Msg::Feed {
            tab,
            generation,
            append: false,
            result: Ok(crate::model::Listing { posts, after: None }),
        });
        app
    }

    fn feed_app(picker: Picker) -> App {
        let mut app = card_feed_app(picker);
        app.state.toggle_list_view();
        app
    }

    fn feed_with_next_page(view: ListView) -> App {
        let mut app = app(&["s"]);
        app.switch_tab(3);
        if app.state.list_view() != view {
            app.state.toggle_list_view();
        }
        let mut post = post_with("existing", Media::None);
        post.title = "Existing post".into();
        app.on_msg(Msg::Feed {
            tab: app.active_tab().id,
            generation: app.active_tab().feed.generation,
            append: false,
            result: Ok(crate::model::Listing {
                posts: vec![post],
                after: Some("next".into()),
            }),
        });
        app
    }

    fn card_with_decoded_image() -> App {
        let mut app = card_feed_app(kitty());
        let url = "https://i.redd.it/p1.jpeg";
        app.card_pictures_drawn(&[url]);
        app.on_msg(Msg::Image {
            url: url.into(),
            result: Ok(image::DynamicImage::new_rgb8(640, 640).into()),
        });
        app
    }

    #[test]
    fn when_sidebar_navigation_shows_decoded_card_pixels_it_should_defer_missing_preparation() {
        let mut app = card_with_decoded_image();
        app.focus_sidebar();
        app.switch_tab(1);
        app.switch_tab(3);

        let actual = first_screen(&mut app, 100, 24);

        assert!(actual.contains("preparing image"), "{actual}");
        assert!(
            !app.card_renderer.pending(),
            "preparation must wait for settling"
        );
    }

    #[test]
    fn when_sidebar_navigation_shows_cached_card_graphics_they_should_stay_visible_immediately() {
        let mut app = card_with_decoded_image();
        screen(&mut app, 100, 24);
        app.focus_sidebar();
        app.switch_tab(1);
        app.switch_tab(3);

        let actual = first_screen(&mut app, 100, 24);

        assert!(actual.contains('\u{10EEEE}'), "{actual}");
        assert!(!actual.contains("preparing image"), "{actual}");
        assert!(!app.card_renderer.pending());
    }

    #[test]
    fn when_sidebar_navigation_settles_missing_card_graphics_should_resume_preparation() {
        let mut app = card_with_decoded_image();
        app.focus_sidebar();
        app.switch_tab(1);
        app.switch_tab(3);
        first_screen(&mut app, 100, 24);

        std::thread::sleep(crate::app::NAV_SETTLE);
        assert!(app.tick());
        let actual = screen(&mut app, 100, 24);

        assert!(actual.contains('\u{10EEEE}'), "{actual}");
        assert!(!actual.contains("preparing image"), "{actual}");
    }

    #[test]
    fn when_a_card_feed_waits_for_more_posts_it_should_label_the_loaded_content_boundary() {
        let mut app = feed_with_next_page(ListView::Card);
        screen(&mut app, 80, 20);
        let geometry = app.active_tab().feed.cards.clone();
        app.load_feed(3, true);

        let actual = screen(&mut app, 80, 20);

        assert!(
            actual.contains("Existing post") && actual.contains("Loading more posts…"),
            "{actual}"
        );
        assert!(
            actual.lines().last().unwrap().contains("⋯ more posts"),
            "{actual}"
        );
        let cards = &app.active_tab().feed.cards;
        assert_eq!(
            (cards.height, cards.viewport, cards.line),
            (geometry.height, geometry.viewport, geometry.line)
        );
    }

    #[test]
    fn when_a_compact_feed_waits_for_more_posts_it_should_label_the_loaded_content_boundary() {
        let mut app = feed_with_next_page(ListView::Compact);
        app.load_feed(3, true);

        let actual = screen(&mut app, 80, 20);

        assert!(
            actual.contains("Existing post") && actual.contains("Loading more posts…"),
            "{actual}"
        );
    }

    #[test]
    fn when_a_notification_is_visible_pagination_activity_should_remain_visible() {
        let mut app = feed_with_next_page(ListView::Card);
        app.load_feed(3, true);
        app.info("Still browsing");

        let actual = screen(&mut app, 80, 20);

        let footer = actual.lines().last().unwrap();
        assert!(
            footer.contains("⋯ more posts") && footer.contains("Still browsing"),
            "{actual}"
        );
    }

    #[test]
    fn when_a_new_query_waits_in_a_narrow_pane_its_activity_should_remain_visible() {
        let mut app = feed_with_next_page(ListView::Card);
        app.search("a query long enough to fill the header".into());

        let actual = screen(&mut app, 50, 12);

        assert!(
            actual.lines().last().unwrap().contains("⋯ posts"),
            "{actual}"
        );
    }

    #[test]
    fn when_a_page_finishes_it_should_remove_the_loading_boundary() {
        let mut app = feed_with_next_page(ListView::Card);
        app.load_feed(3, true);
        app.on_msg(Msg::Feed {
            tab: app.active_tab().id,
            generation: app.active_tab().feed.generation,
            append: true,
            result: Ok(crate::model::Listing {
                posts: vec![post_with("next", Media::None)],
                after: None,
            }),
        });

        let actual = screen(&mut app, 80, 20);

        assert!(actual.contains("next"), "{actual}");
        assert!(
            !actual.contains("Loading more posts") && !actual.contains("⋯ more posts"),
            "{actual}"
        );
    }

    #[test]
    fn when_a_page_fails_it_should_replace_loading_with_the_retry_action() {
        let mut app = feed_with_next_page(ListView::Card);
        app.load_feed(3, true);
        app.on_msg(Msg::Feed {
            tab: app.active_tab().id,
            generation: app.active_tab().feed.generation,
            append: true,
            result: Err("offline".into()),
        });

        let actual = screen(&mut app, 80, 20);

        assert!(actual.contains("Next page failed · L retries"), "{actual}");
        assert!(
            !actual.contains("Loading more posts") && !actual.contains("⋯ more posts"),
            "{actual}"
        );
    }

    #[test]
    fn when_cached_posts_are_refreshing_the_footer_should_keep_the_activity_visible() {
        let mut app = feed_with_next_page(ListView::Card);
        app.load_feed(3, false);
        app.on_msg(Msg::Snapshot {
            message: Box::new(Msg::Feed {
                tab: app.active_tab().id,
                generation: app.active_tab().feed.generation,
                append: false,
                result: Ok(crate::model::Listing {
                    posts: app.active_tab().feed.posts.clone(),
                    after: Some("next".into()),
                }),
            }),
            fetched_at: crate::app::now() - 600,
            refreshing: true,
            source: crate::cache::Source {
                cached: true,
                ..Default::default()
            },
        });
        assert!(!app.active_tab().feed.loading() && app.active_tab().feed.refreshing());

        let actual = screen(&mut app, 80, 20);

        assert!(
            actual.contains("Existing post") && actual.contains("Refreshing posts…"),
            "{actual}"
        );
        assert!(
            actual.lines().last().unwrap().contains("⋯ refresh"),
            "{actual}"
        );
    }

    #[test]
    fn when_navigation_defers_a_card_image_its_reserved_space_should_show_loading() {
        let mut app = card_feed_app(kitty());
        app.move_selection(1);
        app.move_selection(1);

        let actual = first_screen(&mut app, 100, 24);

        assert!(actual.contains("loading image"), "{actual}");
        assert!(
            !app.card_images.contains_key("https://i.redd.it/p3.jpeg"),
            "navigation must still defer the request"
        );
    }

    #[test]
    fn when_card_pixels_wait_for_graphics_preparation_the_first_frame_should_explain_the_wait() {
        let mut app = card_with_decoded_image();

        let actual = first_screen(&mut app, 100, 24);

        assert!(actual.contains("preparing image"), "{actual}");
    }

    #[test]
    fn when_card_graphics_finish_they_should_replace_the_preparation_indicator() {
        let mut app = card_with_decoded_image();

        let actual = screen(&mut app, 100, 24);

        assert!(actual.contains('\u{10EEEE}'), "{actual}");
        assert!(!actual.contains("preparing image"), "{actual}");
    }

    #[test]
    fn when_a_card_refreshes_its_image_it_should_show_activity_while_preserving_pixels() {
        let mut app = card_with_decoded_image();
        screen(&mut app, 100, 24);
        let url = "https://i.redd.it/p1.jpeg";
        let Some(CardImageSlot::Ready { image, protocol }) = app.card_images.remove(url) else {
            panic!("loaded card image")
        };
        app.card_images.insert(
            url.into(),
            CardImageSlot::Cached {
                image,
                protocol,
                refreshing: true,
            },
        );

        let actual = first_screen(&mut app, 100, 24);

        assert!(actual.contains('\u{10EEEE}'), "{actual}");
        assert!(!actual.contains("preparing image"), "{actual}");
        assert!(
            actual.lines().last().unwrap().contains("⋯ images"),
            "{actual}"
        );
    }

    #[test]
    fn when_compact_thumbnails_are_pending_their_reserved_slots_should_show_activity() {
        let mut app = feed_app(kitty());

        let actual = first_screen(&mut app, 100, 20);

        assert!(actual.contains('◌'), "{actual}");
    }

    fn refreshing_thumbnail_app() -> App {
        let mut app = feed_app(kitty());
        app.tabs[3].feed.posts.truncate(1);
        app.thumbs_drawn(&["https://i.redd.it/p1.jpeg"]);
        app.on_msg(Msg::Snapshot {
            message: Box::new(Msg::Thumb {
                url: "https://i.redd.it/p1.jpeg".into(),
                result: Ok(image::DynamicImage::new_rgb8(32, 32)),
            }),
            fetched_at: crate::app::now() - 600,
            refreshing: true,
            source: crate::cache::Source {
                cached: true,
                ..Default::default()
            },
        });
        app
    }

    #[test]
    fn when_a_cached_thumbnail_is_refreshing_the_footer_should_show_activity_and_keep_pixels() {
        let mut app = refreshing_thumbnail_app();

        let actual = first_screen(&mut app, 100, 20);

        assert!(actual.contains('\u{10EEEE}'), "{actual}");
        assert!(
            actual.lines().last().unwrap().contains("⋯ images"),
            "{actual}"
        );
    }

    #[test]
    fn when_a_skipped_thumbnail_returns_its_first_frame_should_show_resumed_refresh_activity() {
        let mut app = refreshing_thumbnail_app();
        app.on_msg(Msg::ThumbSkipped {
            url: "https://i.redd.it/p1.jpeg".into(),
        });

        let actual = first_screen(&mut app, 100, 20);

        assert!(actual.contains('\u{10EEEE}'), "{actual}");
        assert!(
            actual.lines().last().unwrap().contains("⋯ images"),
            "{actual}"
        );
    }

    #[test]
    fn when_a_skipped_thumbnail_stays_offscreen_the_footer_should_not_show_refresh_activity() {
        let mut app = refreshing_thumbnail_app();
        app.on_msg(Msg::ThumbSkipped {
            url: "https://i.redd.it/p1.jpeg".into(),
        });
        app.tabs[3].feed.posts.clear();

        let actual = first_screen(&mut app, 100, 20);

        assert!(!actual.contains('\u{10EEEE}'), "{actual}");
        assert!(
            !actual.lines().last().unwrap().contains("⋯ images"),
            "{actual}"
        );
    }

    #[test]
    fn when_a_cached_thumbnail_refresh_finishes_the_footer_should_stop_activity() {
        let mut app = refreshing_thumbnail_app();
        app.on_msg(Msg::Thumb {
            url: "https://i.redd.it/p1.jpeg".into(),
            result: Ok(image::DynamicImage::new_rgb8(32, 32)),
        });

        let actual = first_screen(&mut app, 100, 20);

        assert!(actual.contains('\u{10EEEE}'), "{actual}");
        assert!(
            !actual.lines().last().unwrap().contains("⋯ images"),
            "{actual}"
        );
    }

    #[test]
    fn when_a_cached_thumbnail_refresh_fails_the_footer_should_stop_activity_and_keep_pixels() {
        let mut app = refreshing_thumbnail_app();
        app.on_msg(Msg::Thumb {
            url: "https://i.redd.it/p1.jpeg".into(),
            result: Err("offline".into()),
        });

        let actual = first_screen(&mut app, 100, 20);

        assert!(actual.contains('\u{10EEEE}'), "{actual}");
        assert!(
            !actual.lines().last().unwrap().contains("⋯ images"),
            "{actual}"
        );
    }

    #[test]
    fn when_images_are_disabled_thumbnail_refresh_should_not_show_activity() {
        let mut app = refreshing_thumbnail_app();
        app.show_images = false;

        let actual = first_screen(&mut app, 100, 20);

        assert!(!actual.contains('\u{10EEEE}'), "{actual}");
        assert!(
            !actual.lines().last().unwrap().contains("⋯ images"),
            "{actual}"
        );
    }

    #[test]
    fn when_thumbnails_are_unsupported_their_refresh_should_not_show_activity() {
        let mut app = refreshing_thumbnail_app();
        app.image_picker = Picker::halfblocks();

        let actual = first_screen(&mut app, 100, 20);

        assert!(!actual.contains('\u{10EEEE}'), "{actual}");
        assert!(
            !actual.lines().last().unwrap().contains("⋯ images"),
            "{actual}"
        );
    }

    #[test]
    fn when_media_opening_outlasts_its_notification_the_footer_should_keep_showing_activity() {
        let mut app = feed_with_next_page(ListView::Card);
        app.tabs[3].feed.posts[0].media = Media::Video {
            url: None,
            poster: None,
        };
        app.open_media();
        app.status.as_mut().unwrap().at = Instant::now() - std::time::Duration::from_secs(7);
        app.tick();
        assert!(app.status.is_none());

        let actual = screen(&mut app, 80, 20);

        assert!(
            actual.lines().last().unwrap().contains("⋯ media"),
            "{actual}"
        );
    }

    #[test]
    fn when_media_loading_fails_the_footer_should_show_the_error_and_stop_activity() {
        let mut app = feed_with_next_page(ListView::Card);
        app.tabs[3].feed.posts[0].media = Media::Video {
            url: None,
            poster: None,
        };
        app.open_media();
        app.on_msg(Msg::MediaResolved {
            permalink: app.tabs[3].feed.posts[0].permalink.clone(),
            result: Err("offline".into()),
        });

        let actual = screen(&mut app, 80, 20);

        let footer = actual.lines().last().unwrap();
        assert!(
            footer.contains("media: offline") && !footer.contains("⋯ media"),
            "{actual}"
        );
    }

    #[test]
    fn when_a_feed_opens_it_should_default_to_cards_with_wrapped_titles() {
        let mut app = card_feed_app(Picker::halfblocks());
        app.show_images = false;
        app.tabs[3].feed.posts[0].title =
            "First line of the title followed by a second line ending in 東京".into();

        let actual = screen(&mut app, 70, 20);

        assert!(actual.contains("V Card"), "{actual}");
        assert!(actual.contains("First line of the title"), "{actual}");
        assert!(actual.contains("ending in 東"), "{actual}");
        assert!(actual.contains('京'), "{actual}");
        assert!(actual.contains("comments"), "{actual}");
    }

    #[test]
    fn when_view_is_toggled_it_should_keep_the_selected_post_and_restore_cards() {
        let mut app = card_feed_app(Picker::halfblocks());
        app.move_selection(4);
        let state_path = std::env::temp_dir().join(format!(
            "redtab-ui-test-{}-{:?}.json",
            std::process::id(),
            std::thread::current().id()
        ));
        let before = app.current_post().unwrap().id;

        press(&mut app, 'V');
        let compact = screen(&mut app, 100, 20);
        let (saved, warning) = crate::state::State::load(&state_path);
        assert!(warning.is_none());
        assert_eq!(saved.list_view(), ListView::Compact);
        assert!(compact.contains("V Compact"), "{compact}");
        assert_eq!(app.current_post().unwrap().id, before);

        press(&mut app, 'V');
        let cards = screen(&mut app, 100, 20);
        std::fs::remove_file(&state_path).unwrap();
        std::fs::remove_file(state_path.with_extension("json.lock")).unwrap();
        assert!(cards.contains("V Card"), "{cards}");
        assert!(cards.contains("p5"), "{cards}");
        assert!(!cards.contains("……"), "{cards}");
        assert_eq!(app.current_post().unwrap().id, before);
    }

    #[test]
    fn when_cards_show_media_they_should_load_only_visible_full_size_previews() {
        let mut app = card_feed_app(kitty());

        screen(&mut app, 100, 20);
        app.on_msg(Msg::Image {
            url: "https://i.redd.it/p1.jpeg".into(),
            result: Ok(image::DynamicImage::new_rgb8(640, 320).into()),
        });
        let cells = placeholder_cells(&mut app, 100, 20);

        assert_eq!(app.card_images.len(), 1);
        assert!(matches!(
            app.card_images.get("https://i.redd.it/p1.jpeg"),
            Some(CardImageSlot::Ready { image, .. }) if (image.width(), image.height()) == (640, 320)
        ));
        assert!(app.thumbs.is_empty());
        assert!(
            cells.len() > 8,
            "card preview should exceed the compact 4×2 thumbnail"
        );
    }

    fn card_with_image(width: u32, height: u32) -> App {
        let mut app = card_feed_app(kitty());
        app.tabs[3].feed.posts.truncate(1);
        app.tabs[3].feed.posts[0].title = "Full width image".into();
        screen(&mut app, 100, 20);
        app.on_msg(Msg::Image {
            url: "https://i.redd.it/p1.jpeg".into(),
            result: Ok(image::DynamicImage::new_rgb8(width, height).into()),
        });
        app
    }

    #[test]
    fn when_card_style_marks_new_posts_the_dot_should_stay_orange_in_a_highlighted_title() {
        let mut app = feed_with_next_page(ListView::Card);
        let mut post = post_with("fresh", Media::None);
        post.title = "Target ● body".into();
        post.stickied = true;
        app.state.toggle_bookmark(&post, 1);
        app.tabs[3].feed.posts[0] = post;
        app.config.highlight.keywords = vec!["Target".into()];

        let actual = screen_buffer(&mut app, 100, 20);

        let dots: Vec<_> = (25..98)
            .map(|x| &actual[(x, 4)])
            .filter(|cell| cell.symbol() == "●")
            .map(|cell| cell.fg)
            .collect();
        assert_eq!(dots, vec![ACCENT, YELLOW]);
    }

    #[test]
    fn when_card_style_wraps_bookmark_and_pin_prefixes_the_new_post_dot_should_stay_orange() {
        let mut app = feed_with_next_page(ListView::Card);
        app.show_sidebar = false;
        let mut post = post_with("fresh", Media::None);
        post.title = "New title".into();
        post.stickied = true;
        app.state.toggle_bookmark(&post, 1);
        app.tabs[3].feed.posts[0] = post;

        let actual = screen_buffer(&mut app, 14, 20);

        let dots: Vec<_> = (4..7)
            .flat_map(|y| (1..12).map(move |x| (x, y)))
            .map(|at| &actual[at])
            .filter(|cell| cell.symbol() == "●")
            .map(|cell| cell.fg)
            .collect();
        assert_eq!(dots, vec![ACCENT]);
    }

    #[test]
    fn when_card_style_dims_read_posts_it_should_not_add_a_new_post_dot_or_recolor_body_dots() {
        let mut app = feed_with_next_page(ListView::Card);
        let mut post = post_with("fresh", Media::None);
        post.title = "Read ● body".into();
        app.state.mark_read(&post, 1);
        app.tabs[3].feed.posts[0] = post;

        let actual = screen_buffer(&mut app, 100, 20);

        let dots: Vec<_> = (25..98)
            .map(|x| &actual[(x, 4)])
            .filter(|cell| cell.symbol() == "●")
            .map(|cell| cell.fg)
            .collect();
        assert_eq!(dots, vec![SUBTEXT]);
    }

    #[test]
    fn when_card_style_selects_an_image_the_bar_should_cover_every_card_row() {
        let mut app = card_with_image(640, 320);

        let actual = screen_buffer(&mut app, 100, 40);

        assert!(
            (3..23).all(|y| actual[(25, y)].symbol() == "▌" && actual[(25, y)].fg == ACCENT),
            "{}",
            buffer_text(&actual)
        );
        assert!(actual[(27, 5)].symbol().contains('\u{10EEEE}'));
        assert_eq!(actual[(25, 23)].symbol(), " ", "inter-card gap stays clear");
    }

    #[test]
    fn when_card_style_leaves_the_list_unfocused_the_whole_bar_should_use_its_muted_color() {
        let mut app = card_with_image(640, 320);
        app.focus = Focus::Sidebar;

        let actual = screen_buffer(&mut app, 100, 40);

        assert!(
            (3..23).all(|y| actual[(25, y)].symbol() == "▌" && actual[(25, y)].fg == OVERLAY),
            "{}",
            buffer_text(&actual)
        );
    }

    #[test]
    fn when_card_style_scrolls_into_an_image_the_bar_should_remain_visible_with_native_pixels() {
        let mut app = card_with_image(320, 640);
        app.scroll_card(16);

        let actual = screen_buffer(&mut app, 100, 20);

        assert!(
            (3..19).all(|y| actual[(25, y)].symbol() == "▌" && actual[(25, y)].fg == ACCENT),
            "{}",
            buffer_text(&actual)
        );
        assert!(actual[(27, 3)].symbol().contains('\u{10EEEE}'));
        assert_eq!(app.active_tab().feed.cards.line, 16);
    }

    fn assert_centered_page_loading(actual: &ratatui::buffer::Buffer) {
        let text = buffer_text(actual);
        let y = text
            .lines()
            .position(|line| line.contains("Loading more posts…"))
            .unwrap() as u16;
        let x = (25..78).find(|&x| actual[(x, y)].symbol() == "L").unwrap();
        assert!((12..=22).contains(&y), "{text}");
        assert!((2 * x + 19).abs_diff(102) <= 1, "{text}");
        assert_eq!(actual[(x, y)].bg, MANTLE);
        assert!(actual[(x, y)].modifier.contains(Modifier::BOLD));
        assert_eq!(actual[(25, 8)].bg, MANTLE);
        assert_eq!(actual[(25, 28)].bg, MANTLE);
    }

    #[test]
    fn when_card_style_waits_for_more_posts_the_loading_panel_should_fill_and_center_in_empty_space()
     {
        let mut app = feed_with_next_page(ListView::Card);
        app.load_feed(3, true);

        let actual = screen_buffer(&mut app, 80, 30);

        assert_centered_page_loading(&actual);
        assert_eq!(app.active_tab().feed.cards.height, 3);
        assert_eq!(app.active_tab().feed.cards.line, 0);
    }

    #[test]
    fn when_compact_style_waits_for_more_posts_the_loading_panel_should_fill_and_center_in_empty_space()
     {
        let mut app = feed_with_next_page(ListView::Compact);
        app.load_feed(3, true);

        let actual = screen_buffer(&mut app, 80, 30);

        assert_centered_page_loading(&actual);
        assert_eq!(app.active_tab().feed.offset, 0);
    }

    #[test]
    fn when_card_style_has_no_room_for_a_panel_pagination_should_stay_visible_in_the_footer() {
        let mut app = feed_with_next_page(ListView::Card);
        app.load_feed(3, true);

        let actual = screen(&mut app, 50, 5);

        assert!(
            actual.lines().last().unwrap().contains("⋯ more posts"),
            "{actual}"
        );
        assert!(actual.contains("Existing po"), "{actual}");
    }

    #[test]
    fn when_a_large_card_image_arrives_it_should_leave_the_ui_thread_responsive() {
        let mut app = card_feed_app(kitty());
        app.tabs[3].feed.posts.truncate(1);
        screen(&mut app, 100, 20);
        let message = Msg::Image {
            url: "https://i.redd.it/p1.jpeg".into(),
            result: Ok(image::DynamicImage::new_rgb8(1600, 1600).into()),
        };
        let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();

        let started = Instant::now();
        app.on_msg(message);
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        app.scroll_card(1);
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();

        assert!(
            started.elapsed() < std::time::Duration::from_millis(200),
            "image preparation blocked input for {:?}",
            started.elapsed()
        );
        assert_eq!(app.active_tab().feed.cards.line, 1);
    }

    #[test]
    fn when_revisiting_a_card_after_pixel_eviction_its_geometry_should_stay_reserved() {
        let mut app = card_with_image(320, 640);
        app.tabs[3].feed.posts.push(post_with("p2", Media::None));
        screen(&mut app, 100, 20);
        app.card_images.clear();
        app.move_selection(1);
        screen(&mut app, 100, 20);
        app.move_selection(-1);

        let actual = screen(&mut app, 100, 20);

        assert_eq!(app.active_tab().feed.cards.height, 71, "{actual}");
        assert!(!actual.contains("p2"), "{actual}");
    }

    #[test]
    fn when_returning_to_card_media_after_eviction_its_preview_should_stay_visible() {
        for video in [false, true] {
            let mut app = card_with_image(320, 640);
            if video {
                app.tabs[3].feed.posts[0].media = Media::Video {
                    url: Some("https://v.redd.it/example/HLSPlaylist.m3u8".into()),
                    poster: Some("https://i.redd.it/p1.jpeg".into()),
                };
            }
            screen(&mut app, 100, 20);
            for number in 0..9 {
                let url = format!("https://i.redd.it/pressure-{number}.png");
                app.card_pictures_drawn(&[&url]);
                app.on_msg(Msg::Image {
                    url,
                    result: Ok(image::DynamicImage::new_rgb8(1, 1).into()),
                });
            }
            app.card_pictures_drawn(&[]);
            assert!(!app.card_images.contains_key("https://i.redd.it/p1.jpeg"));
            app.move_selection(0);
            app.move_selection(0);

            let actual = screen(&mut app, 100, 20);

            assert!(
                actual.contains('\u{10EEEE}'),
                "retained image/poster must render immediately: {actual}"
            );
            assert!(!actual.contains("loading image"), "{actual}");
            assert_eq!(app.active_tab().feed.cards.height, 71);
            std::thread::sleep(crate::app::NAV_SETTLE);
            app.tick();
            screen(&mut app, 100, 20);
            app.on_msg(Msg::Image {
                url: "https://i.redd.it/p1.jpeg".into(),
                result: Err("offline".into()),
            });
            let failed_upgrade = screen(&mut app, 100, 20);
            assert!(failed_upgrade.contains('\u{10EEEE}'), "{failed_upgrade}");
            assert!(
                !failed_upgrade.contains("loading image")
                    && !failed_upgrade.contains("image unavailable"),
                "{failed_upgrade}"
            );
            app.reload();
            screen(&mut app, 100, 20);
            app.on_msg(Msg::Image {
                url: "https://i.redd.it/p1.jpeg".into(),
                result: Ok(image::DynamicImage::new_rgb8(320, 640).into()),
            });
            screen(&mut app, 100, 20);
            assert!(matches!(
                app.card_images.get("https://i.redd.it/p1.jpeg"),
                Some(CardImageSlot::Ready { .. })
            ));
            assert_eq!(app.active_tab().feed.cards.height, 71);
        }
    }

    #[test]
    fn when_a_card_has_listing_dimensions_loading_and_failure_should_keep_its_height() {
        let mut app = card_feed_app(kitty());
        app.tabs[3].feed.posts.truncate(1);
        app.tabs[3].feed.posts[0].title = "Reserved portrait".into();
        app.tabs[3].feed.posts[0].preview_size = Some((320, 640));
        screen(&mut app, 100, 20);
        assert_eq!(app.active_tab().feed.cards.height, 71);
        app.scroll_card(16);

        app.on_msg(Msg::Image {
            url: "https://i.redd.it/p1.jpeg".into(),
            result: Err("offline".into()),
        });
        let actual = screen(&mut app, 100, 20);

        assert_eq!(app.active_tab().feed.cards.height, 71, "{actual}");
        assert_eq!(app.active_tab().feed.cards.line, 16);
        assert!(actual.contains("image unavailable"), "{actual}");
    }

    #[test]
    fn when_a_reserved_image_finishes_loading_the_selected_card_should_not_move() {
        let mut app = card_feed_app(kitty());
        app.tabs[3].feed.posts.truncate(1);
        app.tabs[3].feed.posts[0].title = "Reserved landscape".into();
        app.tabs[3].feed.posts[0].preview_size = Some((640, 320));
        let before = screen(&mut app, 100, 40);
        let row = |screen: &str| screen.lines().position(|line| line.contains("comments"));
        assert_eq!(app.active_tab().feed.cards.height, 20);

        app.on_msg(Msg::Image {
            url: "https://i.redd.it/p1.jpeg".into(),
            result: Ok(image::DynamicImage::new_rgb8(320, 160).into()),
        });
        let actual = screen(&mut app, 100, 40);

        assert_eq!(app.active_tab().feed.cards.height, 20);
        assert_eq!(row(&before), row(&actual));
        assert!(row(&actual).is_some());
    }

    #[test]
    fn when_neighboring_images_load_navigating_back_should_keep_the_same_card_positions() {
        let mut app = card_feed_app(kitty());
        app.tabs[3].feed.posts.iter_mut().for_each(|post| {
            post.title = post.id.clone();
            post.preview_size = Some((640, 320));
        });
        screen(&mut app, 100, 60);
        app.move_selection(1);
        let before = screen(&mut app, 100, 60);
        let row = |screen: &str| screen.lines().position(|line| line.contains("p2"));
        assert!(matches!(
            app.card_images.get("https://i.redd.it/p1.jpeg"),
            Some(CardImageSlot::Loading)
        ));
        app.on_msg(Msg::Image {
            url: "https://i.redd.it/p1.jpeg".into(),
            result: Ok(image::DynamicImage::new_rgb8(640, 320).into()),
        });
        screen(&mut app, 100, 60);
        app.move_selection(-1);
        screen(&mut app, 100, 60);
        app.move_selection(1);

        let actual = screen(&mut app, 100, 60);

        assert!(matches!(
            app.card_images.get("https://i.redd.it/p1.jpeg"),
            Some(CardImageSlot::Ready { .. })
        ));
        assert_eq!(row(&before), row(&actual));
        assert!(row(&actual).is_some());
    }

    #[test]
    fn when_scrolling_a_placeholder_by_hand_it_should_cancel_pending_bottom_restoration() {
        let mut app = card_feed_app(kitty());
        app.tabs[3].feed.posts.truncate(1);
        app.tabs[3].feed.posts[0].title = "Loading image".into();
        app.tabs[3].feed.cards.bottom = true;
        screen(&mut app, 100, 20);
        assert_eq!(app.active_tab().feed.cards.line, 21);

        app.scroll_card(-1);
        screen(&mut app, 100, 20);

        assert_eq!(app.active_tab().feed.cards.line, 20);
        assert!(!app.active_tab().feed.cards.bottom);
    }

    #[test]
    fn when_paging_a_bookmark_before_its_image_loads_it_should_scroll_reserved_space() {
        let mut app = card_feed_app(kitty());
        app.tabs[3].feed.posts[0].title = "Reserved bookmark".into();
        app.tabs[3].feed.posts[0].preview_size = Some((320, 640));
        app.state.toggle_bookmark(&app.current_post().unwrap(), 1);
        app.switch_tab(1);
        screen(&mut app, 100, 20);

        app.page_list(1, 1);
        screen(&mut app, 100, 20);

        assert_eq!(app.active_tab().feed.cards.line, 16);
        assert_eq!(app.active_tab().feed.cards.height, 71);
        assert_eq!(app.current_post().unwrap().id, "p1");
    }

    #[test]
    fn when_a_landscape_card_image_loads_it_should_fill_the_available_width() {
        let mut app = card_with_image(640, 320);

        let cells = placeholder_cells(&mut app, 100, 20);

        assert_eq!(cells.iter().map(|cell| cell.0).min(), Some(27));
        assert_eq!(cells.iter().map(|cell| cell.0).max(), Some(94));
        assert!(matches!(app.card_images.get("https://i.redd.it/p1.jpeg"),
            Some(CardImageSlot::Ready { protocol: Some(protocol), .. })
                if protocol.size == Size::new(68, 17)));
    }

    #[test]
    fn when_a_portrait_card_image_loads_it_should_fill_width_and_extend_beyond_the_viewport() {
        let mut app = card_with_image(320, 640);

        let cells = placeholder_cells(&mut app, 100, 20);

        assert_eq!(cells.iter().map(|cell| cell.0).min(), Some(27));
        assert_eq!(cells.iter().map(|cell| cell.0).max(), Some(94));
        assert!(matches!(app.card_images.get("https://i.redd.it/p1.jpeg"),
            Some(CardImageSlot::Ready { protocol: Some(protocol), .. })
                if protocol.size == Size::new(68, 68)));
    }

    #[test]
    fn when_a_small_card_image_loads_it_should_fill_width_without_the_detail_upscaling_limit() {
        let mut app = card_with_image(16, 8);

        let cells = placeholder_cells(&mut app, 100, 20);

        assert_eq!(cells.iter().map(|cell| cell.0).min(), Some(27));
        assert_eq!(cells.iter().map(|cell| cell.0).max(), Some(94));
    }

    fn card_png(protocol: &ratatui_image::protocol::Protocol) -> image::RgbImage {
        use base64::Engine;
        let ratatui_image::protocol::Protocol::ITerm2(protocol) = protocol else {
            panic!("iTerm2 card protocol");
        };
        let png = protocol.data.split("doNotMoveCursor=1:").nth(1).unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(png.split('\u{7}').next().unwrap())
            .unwrap();
        image::load_from_memory(&bytes).unwrap().to_rgb8()
    }

    #[test]
    fn when_scrolling_an_upscaled_card_its_pixels_should_keep_the_same_scale() {
        let mut picker = kitty();
        picker.set_protocol_type(ProtocolType::Iterm2);
        let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(2, 2, |x, y| {
            image::Rgb([(x * 255) as u8, (y * 255) as u8, 0])
        }));
        let tile = |start| {
            let protocol = encode_tile(
                &picker,
                &image,
                None,
                (Size::new(68, 34), (2, 2)),
                start,
                16,
                0,
            )
            .unwrap();
            card_png(&protocol)
        };

        let first = tile(14);
        let next = tile(15);

        // A square 2×2 source fills 544×544 pixels, even when only a slice is visible.
        assert_eq!(first.dimensions(), (544, 256));
        assert!((82..=84).contains(&first.get_pixel(272, 0)[1]));
        assert_ne!(first.get_pixel(272, 0), next.get_pixel(272, 0));
        assert_eq!(
            image::imageops::crop_imm(&first, 0, 16, 544, 240).to_image(),
            image::imageops::crop_imm(&next, 0, 0, 544, 240).to_image()
        );
    }

    #[test]
    fn when_a_retained_preview_is_rounded_to_one_pixel_wide_it_should_keep_the_original_aspect() {
        let mut picker = kitty();
        picker.set_protocol_type(ProtocolType::Iterm2);
        let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(1, 256, |_, y| {
            image::Rgb([y as u8, 0, 0])
        }));

        let encoded = encode_tile(
            &picker,
            &image,
            None,
            (Size::new(68, 32767), (2, 1600)),
            16384,
            1,
            0,
        )
        .unwrap();
        let actual = card_png(&encoded);

        assert_eq!(actual.dimensions(), (544, 16));
        assert!((153..=154).contains(&actual.get_pixel(272, 0)[0]));
    }

    #[test]
    fn when_a_card_shrinks_fine_detail_it_should_filter_all_contributing_pixels() {
        let mut app = card_with_image(2176, 64);
        app.image_picker.set_protocol_type(ProtocolType::Iterm2);
        let CardImageSlot::Ready { image, .. } = app
            .card_images
            .get_mut("https://i.redd.it/p1.jpeg")
            .unwrap()
        else {
            panic!("card image");
        };
        *image = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(2176, 64, |x, _| {
            let value = if matches!(x % 4, 1 | 2) { 255 } else { 0 };
            image::Rgb([value; 3])
        }))
        .into();

        screen(&mut app, 100, 20);

        let CardImageSlot::Ready {
            protocol: Some(protocol),
            ..
        } = app.card_images.get("https://i.redd.it/p1.jpeg").unwrap()
        else {
            panic!("card protocol");
        };
        let pixels = card_png(&protocol.tiles[&(0, 0, 1)]);
        assert_eq!(pixels.dimensions(), (544, 16));
        assert!((120..=136).contains(&pixels.get_pixel(272, 8)[0]));
    }

    #[test]
    fn when_paging_inside_a_tall_card_it_should_keep_the_selected_post() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = card_with_image(320, 640);
        screen(&mut app, 100, 20);

        crate::keys::handle(
            &mut app,
            KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
        );
        let cells = placeholder_cells(&mut app, 100, 20);

        assert_eq!(app.current_post().unwrap().id, "p1");
        assert_eq!(app.active_tab().feed.cards.line, 16);
        assert_eq!(cells.iter().map(|cell| cell.0).max(), Some(94));
    }

    #[test]
    fn when_paging_to_a_tall_cards_bottom_it_should_expose_the_footer_and_keep_the_last_post() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = card_with_image(320, 640);
        screen(&mut app, 100, 20);
        app.scroll_card(1000);
        screen(&mut app, 100, 20);

        crate::keys::handle(
            &mut app,
            KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
        );
        let actual = screen(&mut app, 100, 20);

        assert!(actual.contains("comments"), "{actual}");
        assert_eq!(app.active_tab().feed.cards.line, 55);
        assert_eq!(app.current_post().unwrap().id, "p1");
    }

    #[test]
    fn when_scrolling_up_a_card_by_control_y_it_should_move_one_line_without_copying() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = card_with_image(320, 640);
        screen(&mut app, 100, 20);
        app.scroll_card(5);

        crate::keys::handle(
            &mut app,
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL),
        );

        assert_eq!(app.active_tab().feed.cards.line, 4);
        assert!(app.status.is_none(), "Ctrl+y must not copy a link");
    }

    #[test]
    fn when_pressing_g_inside_the_first_card_it_should_return_to_its_top() {
        let mut app = card_with_image(320, 640);
        screen(&mut app, 100, 20);
        app.scroll_card(5);

        press(&mut app, 'g');
        let actual = screen(&mut app, 100, 20);

        assert!(actual.contains("Full width image"), "{actual}");
        assert_eq!(app.active_tab().feed.cards.line, 0);
    }

    #[test]
    fn when_resizing_a_card_it_should_refit_the_image_to_the_new_width() {
        let mut app = card_with_image(640, 320);
        placeholder_cells(&mut app, 100, 20);

        let cells = placeholder_cells(&mut app, 80, 20);

        assert_eq!(cells.iter().map(|cell| cell.0).min(), Some(27));
        assert_eq!(cells.iter().map(|cell| cell.0).max(), Some(74));
        assert!(matches!(app.card_images.get("https://i.redd.it/p1.jpeg"),
            Some(CardImageSlot::Ready { protocol: Some(protocol), .. })
                if protocol.size == Size::new(48, 12)));
    }

    #[test]
    fn when_a_compact_list_pages_it_should_still_move_ten_posts() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = feed_app(kitty());

        crate::keys::handle(
            &mut app,
            KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
        );

        assert_eq!(app.current_post().unwrap().id, "p8");
    }

    #[test]
    fn when_card_mode_uses_compact_rows_in_a_tiny_terminal_paging_should_still_work() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = card_feed_app(kitty());
        screen(&mut app, 100, 6);

        crate::keys::handle(
            &mut app,
            KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
        );

        assert_eq!(app.current_post().unwrap().id, "p8");
    }

    #[test]
    fn when_returning_from_fullscreen_detail_a_reloading_card_should_keep_its_scroll() {
        let mut app = card_with_image(320, 640);
        screen(&mut app, 100, 20);
        app.scroll_card(16);
        screen(&mut app, 100, 20);
        app.open_current_post();
        screen(&mut app, 100, 20);
        app.close_post();
        screen(&mut app, 100, 20);
        assert_eq!(app.active_tab().feed.cards.line, 16);

        app.on_msg(Msg::Image {
            url: "https://i.redd.it/p1.jpeg".into(),
            result: Ok(image::DynamicImage::new_rgb8(320, 640).into()),
        });
        placeholder_cells(&mut app, 100, 20);

        assert_eq!(app.active_tab().feed.cards.line, 16);
        assert_eq!(app.current_post().unwrap().id, "p1");
    }

    #[test]
    fn when_scrolling_below_kittys_row_limit_image_tiles_should_use_local_coordinates() {
        let mut app = card_with_image(100, 1000);
        screen(&mut app, 100, 20);
        app.scroll_card(300);
        screen(&mut app, 100, 20);

        let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();

        let CardImageSlot::Ready {
            protocol: Some(protocol),
            ..
        } = app.card_images.get("https://i.redd.it/p1.jpeg").unwrap()
        else {
            panic!("card protocol");
        };
        assert_eq!(protocol.size, Size::new(68, 340));
        assert!(protocol.tiles.contains_key(&(298, 0, 6)));
        assert_eq!(protocol.tiles.len(), 2);
        assert_eq!(protocol.tiles[&(298, 0, 6)].size(), Size::new(68, 6));
        assert_eq!(protocol.tiles[&(304, 0, 10)].size(), Size::new(68, 10));
        let row = terminal.backend().buffer()[(27, 3)]
            .symbol()
            .split_once('\u{10EEEE}')
            .unwrap()
            .1
            .chars()
            .next();
        assert_eq!(row, Some('\u{305}'));
    }

    #[test]
    fn when_a_card_is_wider_than_kittys_column_limit_it_should_render_its_right_edge() {
        let mut app = card_with_image(640, 320);

        let cells = placeholder_cells(&mut app, 340, 20);

        assert_eq!(cells.iter().map(|cell| cell.0).min(), Some(33));
        assert_eq!(cells.iter().map(|cell| cell.0).max(), Some(334));
    }

    #[test]
    fn when_scrolling_an_iterm2_card_it_should_encode_only_the_visible_image_rows() {
        let mut app = card_with_image(320, 640);
        app.image_picker.set_protocol_type(ProtocolType::Iterm2);
        screen(&mut app, 100, 20);
        app.scroll_card(16);

        screen(&mut app, 100, 20);

        let CardImageSlot::Ready {
            protocol: Some(protocol),
            ..
        } = app.card_images.get("https://i.redd.it/p1.jpeg").unwrap()
        else {
            panic!("card protocol");
        };
        assert_eq!(protocol.tiles.len(), 2);
        assert_eq!(protocol.tiles[&(14, 0, 2)].size(), Size::new(68, 2));
        assert_eq!(protocol.tiles[&(16, 0, 14)].size(), Size::new(68, 14));
        assert!(matches!(
            *protocol.tiles[&(14, 0, 2)],
            ratatui_image::protocol::Protocol::ITerm2(_)
        ));
    }

    #[test]
    fn when_scrolling_a_sixel_card_it_should_encode_only_the_visible_image_rows() {
        let mut app = card_with_image(320, 640);
        app.image_picker.set_protocol_type(ProtocolType::Sixel);
        screen(&mut app, 100, 20);
        app.scroll_card(16);

        screen(&mut app, 100, 20);

        let CardImageSlot::Ready {
            protocol: Some(protocol),
            ..
        } = app.card_images.get("https://i.redd.it/p1.jpeg").unwrap()
        else {
            panic!("card protocol");
        };
        assert_eq!(protocol.tiles.len(), 2);
        assert_eq!(protocol.tiles[&(14, 0, 2)].size(), Size::new(68, 2));
        assert_eq!(protocol.tiles[&(16, 0, 14)].size(), Size::new(68, 14));
        assert!(matches!(
            *protocol.tiles[&(14, 0, 2)],
            ratatui_image::protocol::Protocol::Sixel(_)
        ));
    }

    #[test]
    fn when_bookmarks_page_a_tall_card_the_saved_post_should_stay_selected() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = card_with_image(320, 640);
        app.state.toggle_bookmark(&app.current_post().unwrap(), 1);
        app.switch_tab(1);
        screen(&mut app, 100, 20);

        crate::keys::handle(
            &mut app,
            KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
        );
        screen(&mut app, 100, 20);

        assert_eq!(app.current_post().unwrap().id, "p1");
        assert_eq!(app.active_tab().feed.cards.line, 16);
    }

    #[test]
    fn when_paging_back_to_a_card_that_reloads_it_should_reach_its_bottom_after_loading() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = card_with_image(320, 640);
        app.tabs[3].feed.posts.push(post_with("p2", Media::None));
        screen(&mut app, 100, 20);
        app.move_selection(1);
        screen(&mut app, 100, 20);
        crate::keys::handle(&mut app, KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
        screen(&mut app, 100, 20);
        std::thread::sleep(crate::app::NAV_SETTLE);
        app.tick();
        screen(&mut app, 100, 20);

        app.on_msg(Msg::Image {
            url: "https://i.redd.it/p1.jpeg".into(),
            result: Ok(image::DynamicImage::new_rgb8(320, 640).into()),
        });
        let actual = screen(&mut app, 100, 20);

        assert!(actual.contains("comments"), "{actual}");
        assert_eq!(app.active_tab().feed.cards.line, 55);
        assert_eq!(app.current_post().unwrap().id, "p1");
    }

    #[test]
    fn when_a_card_is_a_spoiler_it_should_show_the_tag_without_loading_its_image() {
        let mut app = card_feed_app(kitty());
        app.tabs[3].feed.posts[0].spoiler = true;

        let actual = screen(&mut app, 100, 20);

        assert!(actual.contains("spoiler"), "{actual}");
        assert!(!app.card_images.contains_key("https://i.redd.it/p1.jpeg"));
    }

    #[test]
    fn when_cards_scroll_in_a_short_pane_the_selected_post_should_stay_visible() {
        let mut app = card_feed_app(Picker::halfblocks());
        app.move_selection(7);

        let actual = screen(&mut app, 60, 9);

        assert!(actual.contains("p8"), "{actual}");
        assert_eq!(app.current_post().unwrap().id, "p8");
    }

    #[test]
    fn when_card_images_are_switched_off_their_previews_should_be_released() {
        let mut app = card_feed_app(kitty());
        screen(&mut app, 100, 20);
        assert!(!app.card_images.is_empty());

        press(&mut app, 'i');
        screen(&mut app, 100, 20);

        assert!(app.card_images.is_empty());
    }

    #[test]
    fn when_a_card_is_a_gallery_it_should_preview_the_first_image_instead_of_its_thumbnail() {
        let mut app = card_feed_app(kitty());
        app.tabs[3].feed.posts[0].media = Media::Gallery {
            images: vec!["https://i.redd.it/full.jpeg".into()],
            thumb: Some("https://i.redd.it/small.jpeg".into()),
        };

        screen(&mut app, 100, 20);

        assert!(app.card_images.contains_key("https://i.redd.it/full.jpeg"));
        assert!(!app.card_images.contains_key("https://i.redd.it/small.jpeg"));
    }

    #[test]
    fn when_a_bookmark_list_opens_it_should_use_the_card_layout() {
        let mut app = card_feed_app(Picker::halfblocks());
        app.state
            .toggle_bookmark(&post_with("saved", Media::None), 1);
        app.switch_tab(1);

        let actual = screen(&mut app, 100, 20);

        assert!(actual.contains("V Card"), "{actual}");
        assert!(actual.contains("saved"), "{actual}");
        assert!(actual.contains("comments"), "{actual}");
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
        while app.card_renderer.pending() {
            app.card_renderer.wait_for_completion();
            app.card_renderer.poll();
            terminal.draw(|f| draw(f, app)).unwrap();
        }
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
    fn an_empty_feed_says_whether_the_config_hid_the_posts_or_a_search_found_none() {
        let mut app = feed_app(Picker::halfblocks());
        let answer = |app: &mut App, posts: Vec<Post>| {
            let tab = app.tabs[3].id;
            let generation = app.tabs[3].feed.generation;
            app.on_msg(Msg::Feed {
                tab,
                generation,
                append: false,
                result: Ok(crate::model::Listing { posts, after: None }),
            });
        };

        app.search("first".into());
        answer(&mut app, Vec::new());
        let no_results = screen(&mut app, 100, 20);
        app.search("second".into());
        let waiting = screen(&mut app, 100, 20);
        let mut nsfw = post_with("n1", Media::None);
        nsfw.nsfw = true;
        answer(&mut app, vec![nsfw]);
        let hidden = screen(&mut app, 100, 20);

        assert!(no_results.contains("No results"), "{no_results}");
        assert!(!no_results.contains("try another sort"), "{no_results}");
        // The first search's empty answer says nothing about the second one.
        assert!(waiting.contains("Loading"), "{waiting}");
        assert!(!waiting.contains("No results"), "{waiting}");
        assert!(hidden.contains("Every loaded post is hidden"), "{hidden}");
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
                "https://i.redd.it/p1.jpeg",
                "https://i.redd.it/p3.jpeg",
                "https://i.redd.it/p4.jpeg",
                "https://i.redd.it/p5.jpeg"
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
        app.thumbs.insert(
            "https://i.redd.it/p1.jpeg".into(),
            ImageSlot::Ready {
                protocol: Box::new(protocol),
                refresh: crate::app::ImageRefresh::Idle,
                pixels: (256, 256),
            },
        );
        let cells = placeholder_cells(&mut app, 100, 20);

        // Sidebar 24 + margin 1: the list starts at x 25 with the selection bar,
        // so the 4×2 thumbnail fills x 26..=29 on the row's two lines only.
        let expected: Vec<(u16, u16)> = (p1_row..p1_row + 2)
            .flat_map(|y| (26..=29).map(move |x| (x, y)))
            .collect();
        let mut cells = cells;
        cells.sort_by_key(|&(x, y)| (y, x));
        assert_eq!(cells, expected);
    }

    #[test]
    fn a_subreddit_icon_shows_in_sidebar_and_home_card() {
        let mut app = app(&["s"]);
        app.image_picker = kitty();
        app.show_images = true;
        app.on_msg(Msg::Group {
            generation: 0,
            subreddit: "s".into(),
            result: Ok(crate::model::Listing {
                posts: vec![post_with("p1", Media::None)],
                after: None,
            }),
        });
        app.on_msg(Msg::Icon {
            subreddit: "s".into(),
            result: Ok(Some("/icon/s.png".into())),
        });
        // A later load of the subreddit keeps the known icon.
        app.switch_tab(3);
        let generation = app.tabs[3].feed.generation;
        let id = app.tabs[3].id;
        app.on_msg(Msg::Feed {
            tab: id,
            generation,
            append: false,
            result: Ok(crate::model::Listing {
                posts: vec![post_with("p1", Media::None)],
                after: None,
            }),
        });
        app.switch_tab(crate::app::ALL_SUBS_TAB);
        // Two quick tab switches hold new icons until they settle.
        std::thread::sleep(crate::app::NAV_SETTLE);

        let s = screen(&mut app, 120, 30);

        assert!(app.thumbs.contains_key("/icon/s.png"));
        let image = image::DynamicImage::new_rgb8(16, 16);
        let protocol = app.image_picker.new_resize_protocol(image);
        app.thumbs.insert(
            "/icon/s.png".into(),
            ImageSlot::Ready {
                protocol: Box::new(protocol),
                refresh: crate::app::ImageRefresh::Idle,
                pixels: (16, 16),
            },
        );
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
        ImageSlot::Ready {
            protocol: Box::new(app.image_picker.new_resize_protocol(image)),
            refresh: crate::app::ImageRefresh::Idle,
            pixels: (w, h),
        }
    }

    #[test]
    fn thumbnails_wait_for_an_overlay_to_close() {
        let mut app = feed_app(kitty());
        screen(&mut app, 100, 20);
        let slot = ready(&app, 32, 32);
        app.thumbs.insert("https://i.redd.it/p1.jpeg".into(), slot);

        app.overlay = Some(crate::app::Overlay::Help { scroll: 0 });
        let under_help = placeholder_cells(&mut app, 100, 20);
        app.overlay = None;
        let after = placeholder_cells(&mut app, 100, 20);

        // kitty sends an image once, in its first cell: drawing it under the
        // popup would lose it, so it first draws after the popup closes.
        assert!(under_help.is_empty(), "{under_help:?}");
        assert_eq!(after.len(), 8, "the whole 4×2 slot");
    }

    #[test]
    fn a_failed_thumbnail_keeps_its_row_aligned_until_reload_retries_it() {
        let mut app = feed_app(kitty());
        screen(&mut app, 100, 20);
        app.thumbs.insert(
            "https://i.redd.it/p1.jpeg".into(),
            ImageSlot::Failed("timed out".into()),
        );

        let s = screen(&mut app, 100, 20);

        assert!(matches!(
            app.thumbs.get("https://i.redd.it/p1.jpeg"),
            Some(ImageSlot::Failed(_))
        ));
        assert_eq!(title_run(&s, "p1"), title_run(&s, "p2"));

        app.reload();
        screen(&mut app, 100, 20);

        assert!(matches!(
            app.thumbs.get("https://i.redd.it/p1.jpeg"),
            Some(ImageSlot::Loading)
        ));
    }

    #[test]
    fn a_full_thumbnail_cache_keeps_only_what_is_on_screen() {
        let mut app = feed_app(kitty());
        for i in 0..400 {
            app.thumbs.insert(
                format!("https://i.redd.it/old{i}.jpeg"),
                ImageSlot::Failed("x".into()),
            );
        }

        screen(&mut app, 100, 20);

        let mut kept: Vec<&str> = app.thumbs.keys().map(String::as_str).collect();
        kept.sort();
        assert_eq!(
            kept,
            [
                "https://i.redd.it/p1.jpeg",
                "https://i.redd.it/p3.jpeg",
                "https://i.redd.it/p4.jpeg",
                "https://i.redd.it/p5.jpeg"
            ]
        );
    }

    #[test]
    fn a_scrolled_home_card_loads_thumbnails_only_for_whole_rows() {
        let mut app = app(&["s"]);
        app.image_picker = kitty();
        app.show_images = true;
        app.all_subs.groups[0].posts = (1..=3)
            .map(|i| {
                post_with(
                    &format!("p{i}"),
                    Media::Image {
                        url: format!("https://i.redd.it/p{i}.jpeg"),
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

        assert_eq!(top, ["https://i.redd.it/p1.jpeg"]);
        // Scrolled to p3: p2's first line is above the card's top.
        assert!(app.thumbs.contains_key("https://i.redd.it/p3.jpeg"));
        assert!(!app.thumbs.contains_key("https://i.redd.it/p2.jpeg"));
    }

    #[test]
    fn a_tab_opened_by_name_shows_its_icon_whatever_the_letter_case() {
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
            }),
        });
        app.on_msg(Msg::Icon {
            subreddit: "pics".into(),
            result: Ok(Some("/icon/pics.png".into())),
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
            url: "https://i.redd.it/p1.jpeg".into(),
        });
        let gone = !app.thumbs.contains_key("https://i.redd.it/p1.jpeg");
        screen(&mut app, 100, 20);

        assert!(gone);
        assert!(matches!(
            app.thumbs.get("https://i.redd.it/p1.jpeg"),
            Some(ImageSlot::Loading)
        ));
    }

    #[test]
    fn a_skipped_refresh_keeps_a_loaded_thumbnail() {
        let mut app = feed_app(kitty());
        screen(&mut app, 100, 20);
        app.on_msg(Msg::Thumb {
            url: "https://i.redd.it/p1.jpeg".into(),
            result: Ok(image::DynamicImage::new_rgb8(4, 4)),
        });

        app.on_msg(Msg::ThumbSkipped {
            url: "https://i.redd.it/p1.jpeg".into(),
        });

        assert!(matches!(
            app.thumbs.get("https://i.redd.it/p1.jpeg"),
            Some(ImageSlot::Ready { .. })
        ));
    }

    #[test]
    fn a_cache_warning_is_repeated_until_a_draw_shows_it() {
        let mut app = app(&[]);
        let warning = || Msg::CacheWarning("Could not save cache: disk full".into());
        // Replaced by a load error before any draw: never seen, so it comes back.
        app.on_msg(warning());
        app.error("r/x: connection refused");
        screen(&mut app, 100, 20);
        app.on_msg(warning());
        let shown = screen(&mut app, 100, 20);
        assert!(shown.contains("disk full"), "{shown}");

        app.status = None;
        app.on_msg(warning());

        assert!(app.status.is_none());
    }

    #[test]
    fn comment_age_counts_from_its_creation_time() {
        let (mut app, _) = open_long_post(|_| {});
        let comment = &mut app
            .post_view
            .as_mut()
            .unwrap()
            .page
            .as_mut()
            .unwrap()
            .comments[0];
        comment.created = Some(Utc::now() - chrono::Duration::days(3));
        let view = app.post_view.as_mut().unwrap();
        view.scroll = usize::MAX / 2;

        let shown = screen(&mut app, 100, 30);
        let header = shown
            .lines()
            .find(|l| l.contains("u/x"))
            .unwrap_or_default()
            .to_owned();

        assert!(header.contains("3d ago"), "{header}");
    }

    fn press(app: &mut App, c: char) {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        crate::keys::handle(app, KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }

    #[test]
    fn when_detail_is_split_h_should_focus_the_list_without_leaving_the_comment() {
        let (mut app, _) = open_long_post(|_| {});
        let mut other = app.all_subs.groups[0].posts[0].clone();
        other.id = "other".into();
        other.permalink = "/r/s/comments/other/x/".into();
        app.all_subs.groups[0].posts.push(other);
        screen(&mut app, 180, 30);
        press(&mut app, 'j');
        screen(&mut app, 180, 30);
        let view = app.post_view.as_ref().unwrap();
        let expected = (view.generation, view.cursor, view.scroll, view.free_scroll);
        assert!(view.scroll > 0, "reading the comment, not the post's top");

        press(&mut app, 'h');
        screen(&mut app, 180, 30);
        let view = app.post_view.as_ref().unwrap();
        let actual = (view.generation, view.cursor, view.scroll, view.free_scroll);
        press(&mut app, 'j');

        assert_eq!(actual, expected, "h must not leave or reset the comment");
        assert_eq!(app.all_subs.post, 1, "j now moves the list selection");
        assert_eq!(app.current_post().unwrap().id, "other");
    }

    #[test]
    fn when_the_list_has_retained_detail_q_should_close_it_before_quitting() {
        let (mut app, _) = open_long_post(|_| {});
        screen(&mut app, 180, 30);
        press(&mut app, 'h');

        press(&mut app, 'q');

        assert!(!app.quit, "the first q only closes the retained post");
        assert!(app.post_view.is_none());
        press(&mut app, 'q');
        assert!(app.quit, "q quits once no post remains open");
    }

    #[test]
    fn when_a_resize_hides_the_list_the_next_key_should_target_detail() {
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
        let (mut app, _) = open_long_post(|_| {});
        let mut other = app.all_subs.groups[0].posts[0].clone();
        other.id = "other".into();
        other.permalink = "/r/s/comments/other/x/".into();
        app.all_subs.groups[0].posts.push(other);
        screen(&mut app, 180, 30);
        press(&mut app, 'h');
        press(&mut app, 'j');

        crate::handle_event(&mut app, Event::Resize(157, 30));
        crate::handle_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE)),
        );

        assert_eq!(app.post_view.as_ref().unwrap().post.id, "other");
        assert_eq!(app.all_subs.post, 1, "the hidden list selection stays put");
    }

    #[test]
    fn when_a_resize_shows_the_list_h_should_focus_it_before_the_next_key() {
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
        let (mut app, _) = open_long_post(|_| {});
        let mut other = app.all_subs.groups[0].posts[0].clone();
        other.id = "other".into();
        other.permalink = "/r/s/comments/other/x/".into();
        app.all_subs.groups[0].posts.push(other);
        press(&mut app, 'j');
        screen(&mut app, 100, 30);

        crate::handle_event(&mut app, Event::Resize(180, 30));
        crate::handle_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE)),
        );
        assert!(
            !app.post_view.as_ref().unwrap().free_scroll,
            "h kept the comment selected"
        );
        press(&mut app, 'j');

        assert_eq!(app.all_subs.post, 1);
    }

    #[test]
    fn when_a_list_search_is_cleared_escape_should_keep_the_retained_detail() {
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut app = feed_app(Picker::halfblocks());
        app.open_current_post();
        screen(&mut app, 180, 30);
        press(&mut app, 'h');
        app.search("query".into());
        let expected_generation = app.post_view.as_ref().unwrap().generation;

        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);

        assert!(app.active_tab().feed.search.is_none());
        assert_eq!(
            app.post_view.as_ref().unwrap().generation,
            expected_generation
        );
    }

    #[test]
    fn when_the_list_becomes_empty_l_should_resume_the_retained_detail() {
        let (mut app, _) = open_long_post(|_| {});
        screen(&mut app, 180, 30);
        press(&mut app, 'h');
        app.all_subs.groups[0].posts.clear();
        screen(&mut app, 180, 30);

        press(&mut app, 'l');
        press(&mut app, ' ');
        screen(&mut app, 180, 30);

        let view = app
            .post_view
            .as_ref()
            .expect("still reading the opened post");
        assert!(
            view.scroll > 0,
            "Space pages detail instead of opening a tab"
        );
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
        assert_eq!(app.focus, Focus::List);
        press(&mut app, 'h');
        assert_eq!(app.focus, Focus::Sidebar);
        press(&mut app, 'h');
        assert_eq!(app.focus, Focus::Sidebar, "the sidebar is the top");

        press(&mut app, 'l');
        assert_eq!(app.focus, Focus::List);
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
        for _ in 0..6 {
            press(&mut app, 'j');
            seen.push(name(&app));
        }
        press(&mut app, 'k');
        let after_k = name(&app);
        press(&mut app, 'g');
        let first = name(&app);

        assert_eq!(
            seen,
            ["Home", "Bookmarks", "All subs", "r/a", "r/b", "r/c", "r/c"]
        );
        assert_eq!(after_k, "r/b");
        assert_eq!(first, "Home");
        assert_eq!(app.focus, Focus::Sidebar);
    }

    #[test]
    fn h_shows_a_hidden_sidebar_only_while_it_has_focus() {
        let mut app = app(&["a"]);
        app.switch_tab(0);
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
        app.switch_tab(3);
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

    const VIDEO: &str = "https://v.redd.it/v/HLSPlaylist.m3u8";

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
    fn when_a_frame_is_encoded_for_fixed_cells_the_video_band_should_be_that_tall() {
        let mut app = open_video_post();
        let mut picker = kitty();
        picker.set_protocol_type(ratatui_image::picker::ProtocolType::Sixel);
        app.image_picker = picker.clone();
        // A 160×90 frame encoded unscaled is 20×6 cells; the rule would grow it twice.
        let image = image::DynamicImage::new_rgb8(160, 90);
        let protocol = picker
            .new_protocol(image, Size::new(98, 30), Resize::Fit(None))
            .unwrap();
        let cells = protocol.size();
        let frame = video::Frame {
            width: 160,
            height: 90,
            pixels: video::Pixels::Image(protocol),
            same_as_first: false,
        };
        let url = app.reddit.media_url(VIDEO).unwrap();
        app.video = Some(video::Player::showing(&url, frame));

        let s = screen(&mut app, 100, 40);

        let row = |text: &str| s.lines().position(|l| l.contains(text)).unwrap();
        assert_eq!(cells.height, 6);
        // Six picture rows right under the header, then the status line.
        assert_eq!(row("pause") - row("╰"), 6 + 1, "no empty band:\n{s}");
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
    fn when_a_gif_post_opens_it_should_play_inline_without_an_image_download() {
        let (mut app, _) = open_long_post(|_| {});
        app.image_picker = kitty();
        app.show_images = true;
        let view = app.post_view.as_mut().unwrap();
        let generation = view.generation;
        let mut page = view.page.take().unwrap();
        page.post.media = Media::Image {
            url: "https://i.redd.it/f5ndv6yotovh1.gif".into(),
        };

        app.on_msg(Msg::Post {
            generation,
            result: Ok(Box::new(page)),
        });
        screen(&mut app, 100, 30);
        app.tick();

        let player = app
            .video
            .as_ref()
            .expect("GIF should start the video player");
        assert_eq!(
            player.url,
            app.reddit
                .media_url("https://i.redd.it/f5ndv6yotovh1.gif")
                .unwrap()
        );
        assert!(!player.hidden(), "the GIF should have a visible video area");
        assert!(
            !app.images
                .contains_key("https://i.redd.it/f5ndv6yotovh1.gif"),
            "GIFs must not enter the full-image download queue"
        );
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
    fn when_a_frame_shrinks_the_drawn_video_the_decoder_should_not_restart() {
        let mut app = open_video_post();
        let step = |app: &mut App| {
            screen(app, 100, 30);
            app.tick();
        };
        step(&mut app);
        let (url, decoding) = {
            let player = app.video.as_ref().unwrap();
            (player.url.clone(), player.area)
        };
        // A very wide frame takes far fewer rows than the box it was decoded for.
        let frame = video::Frame {
            width: 160,
            height: 9,
            pixels: video::Pixels::Zlib(vec![1, 2, 3]),
            same_as_first: true,
        };
        let mut player = video::Player::showing(&url, frame);
        player.area = decoding;
        app.video = Some(player);

        step(&mut app);
        std::thread::sleep(std::time::Duration::from_millis(300));
        step(&mut app);

        let player = app.video.as_ref().unwrap();
        assert!(
            player.frame().is_some(),
            "the same player, not a restarted one"
        );
        assert_eq!(player.area, decoding);
    }

    #[test]
    fn a_video_frame_goes_to_the_terminal_once() {
        let mut app = open_video_post();
        let frame = || video::Frame {
            width: 16,
            height: 9,
            pixels: video::Pixels::Zlib(vec![1, 2, 3]),
            same_as_first: false,
        };
        let url = app.reddit.media_url(VIDEO).unwrap();
        app.video = Some(video::Player::showing(&url, frame()));
        let sends = |app: &mut App| screen(app, 100, 30).matches("_Ga=T").count();

        assert_eq!(sends(&mut app), 1);
        assert_eq!(sends(&mut app), 0, "a redraw repeats the cells only");
        app.video.as_mut().unwrap().show(frame());
        assert_eq!(sends(&mut app), 1, "the next frame goes out");
    }

    #[test]
    fn edited_selftext_keeps_the_unchanged_passage_being_read() {
        let (mut app, _) = open_long_post(|_| {});
        let original = paragraphs("body passage", 1..=60);
        app.post_view.as_mut().unwrap().page.as_mut().unwrap().body = original.clone();
        screen(&mut app, 100, 30);
        app.post_view.as_mut().unwrap().scroll = 26; // A blank line before unchanged text.
        let passage = |text: String| {
            text.lines().enumerate().find_map(|(row, line)| {
                line.split("body passage ")
                    .nth(1)
                    .and_then(|rest| rest.split_whitespace().next())
                    .map(|rest| (row, rest.to_owned()))
            })
        };
        let before = passage(screen(&mut app, 100, 30));
        assert!(before.is_some());
        for body in [
            format!("inserted one\n\ninserted two\n\n{original}"),
            original,
        ] {
            let view = app.post_view.as_ref().unwrap();
            let generation = view.generation;
            let mut page = view.page.as_ref().unwrap().clone();
            page.body = body;
            app.on_msg(Msg::Post {
                generation,
                result: Ok(Box::new(page)),
            });
            assert_eq!(passage(screen(&mut app, 100, 30)), before);
        }
    }

    #[test]
    fn edited_selftext_anchor_respects_batched_spoiler_and_scroll_keys() {
        let run = |edit_body: bool, draw_between: bool, before: &[char], after: &[char]| {
            let mut app = post_with_rewrapping_spoilers(40);
            let view = app.post_view.as_mut().unwrap();
            view.scroll = view.row_starts[0] - 30;
            screen(&mut app, 100, 30);
            for &key in before {
                press(&mut app, key);
                if draw_between {
                    screen(&mut app, 100, 30);
                }
            }
            if edit_body {
                let view = app.post_view.as_ref().unwrap();
                let generation = view.generation;
                let mut page = view.page.as_ref().unwrap().clone();
                page.body = format!("inserted one\n\ninserted two\n\n{}", page.body);
                app.on_msg(Msg::Post {
                    generation,
                    result: Ok(Box::new(page)),
                });
            }
            if draw_between {
                screen(&mut app, 100, 30);
            }
            for &key in after {
                press(&mut app, key);
            }
            let shown = screen(&mut app, 100, 30);
            let passage = shown.lines().enumerate().find_map(|(row, line)| {
                line.split("body ")
                    .nth(1)
                    .and_then(|rest| rest.split_whitespace().next())
                    .map(|rest| (row, rest.to_owned()))
            });
            (app.post_view.as_ref().unwrap().scroll, passage)
        };
        for (before, after) in [
            (&[][..], &['v', ' '][..]),
            (&['v'][..], &[][..]),
            (&[][..], &['g'][..]),
        ] {
            let batched = run(true, false, before, after);
            assert_eq!(batched, run(true, true, before, after));
            if after.contains(&'g') {
                assert_eq!(batched.0, 0);
            } else {
                let unchanged = run(false, false, before, after);
                assert!(unchanged.1.is_some());
                assert_eq!(batched.1, unchanged.1);
            }
        }
    }

    #[test]
    fn edited_comments_keep_the_passage_read_or_held_across_post_and_more_updates() {
        for more in [false, true] {
            for held in [0, 1, 2] {
                let (mut app, _) = open_long_post(|_| {});
                let original = paragraphs("comment passage", 1..=60);
                let view = app.post_view.as_mut().unwrap();
                let page = view.page.as_mut().unwrap();
                page.body.clear();
                page.comments[0].body = original.clone();
                if more {
                    let child = page.comments.remove(0);
                    let mut parent = child.clone();
                    parent.id = "parent".into();
                    parent.body = "parent".into();
                    parent.replies = vec![child];
                    page.comments.push(parent);
                }
                screen(&mut app, 100, 30);
                let view = app.post_view.as_mut().unwrap();
                view.scroll = view.row_starts[usize::from(more)] + 26;
                screen(&mut app, 100, 30);
                let passage = |text: String| {
                    text.lines().enumerate().find_map(|(row, line)| {
                        line.split("comment passage ")
                            .nth(1)
                            .and_then(|text| text.split_whitespace().next())
                            .map(|text| (row, text.to_owned()))
                    })
                };
                let before = passage(screen(&mut app, 100, 30));
                assert!(before.is_some());
                if held != 0 {
                    press(&mut app, 'l');
                    if held == 1 {
                        screen(&mut app, 100, 30);
                    }
                }
                for body in [
                    format!("new paragraph\n\nanother paragraph\n\n{original}"),
                    original,
                ] {
                    let view = app.post_view.as_ref().unwrap();
                    let generation = view.generation;
                    let mut page = view.page.as_ref().unwrap().clone();
                    let child = if more {
                        &mut page.comments[0].replies[0]
                    } else {
                        &mut page.comments[0]
                    };
                    child.body = body;
                    let msg = if more {
                        Msg::MoreReplies {
                            generation,
                            comment_id: "parent".into(),
                            result: Ok(ReplyPage::complete(std::mem::take(
                                &mut page.comments[0].replies,
                            ))),
                        }
                    } else {
                        Msg::Post {
                            generation,
                            result: Ok(Box::new(page)),
                        }
                    };
                    app.on_msg(msg);
                    assert_eq!(
                        passage(screen(&mut app, 100, 30)),
                        before,
                        "more={more} held={held}"
                    );
                }
            }
        }
    }

    #[test]
    fn spoiler_then_comment_refresh_in_one_batch_keeps_the_old_passage() {
        for more in [false, true] {
            let (mut app, _) = open_long_post(|_| {});
            let original = paragraphs("unchanged comment passage", 1..=60);
            let original = format!("{original}\n\n{SPOILER_START}short spoiler{SPOILER_END}");
            let view = app.post_view.as_mut().unwrap();
            let page = view.page.as_mut().unwrap();
            page.body.clear();
            page.comments[0].body = original;
            if more {
                let child = page.comments.remove(0);
                let mut parent = child.clone();
                parent.id = "parent".into();
                parent.body = "parent".into();
                parent.replies = vec![child];
                page.comments.push(parent);
            }
            screen(&mut app, 100, 30);
            let view = app.post_view.as_mut().unwrap();
            view.scroll = view.row_starts[usize::from(more)] + 26;
            let passage = |text: String| {
                text.lines().enumerate().find_map(|(row, line)| {
                    line.split("unchanged comment passage ")
                        .nth(1)
                        .and_then(|rest| rest.split_whitespace().next())
                        .map(|rest| (row, rest.to_owned()))
                })
            };
            let before = passage(screen(&mut app, 100, 30));
            assert!(before.is_some());
            press(&mut app, 'v');
            let view = app.post_view.as_ref().unwrap();
            let generation = view.generation;
            let mut page = view.page.as_ref().unwrap().clone();
            let child = if more {
                &mut page.comments[0].replies[0]
            } else {
                &mut page.comments[0]
            };
            child.body = format!("inserted\n\nalso inserted\n\n{}", child.body);
            app.on_msg(if more {
                Msg::MoreReplies {
                    generation,
                    comment_id: "parent".into(),
                    result: Ok(ReplyPage::complete(std::mem::take(
                        &mut page.comments[0].replies,
                    ))),
                }
            } else {
                Msg::Post {
                    generation,
                    result: Ok(Box::new(page)),
                }
            });
            assert_eq!(passage(screen(&mut app, 100, 30)), before, "more={more}");
        }
    }

    #[test]
    fn a_top_jump_discards_the_old_comment_hold_before_an_edit() {
        let (mut app, _) = open_long_post(|_| {});
        let view = app.post_view.as_mut().unwrap();
        let page = view.page.as_mut().unwrap();
        page.body.clear();
        page.comments[0].body = paragraphs("passage", 1..=60);
        screen(&mut app, 100, 30);
        let view = app.post_view.as_mut().unwrap();
        view.scroll = view.row_starts[0] + 26;
        screen(&mut app, 100, 30);
        press(&mut app, 'l');
        screen(&mut app, 100, 30);
        press(&mut app, 'g');
        let view = app.post_view.as_ref().unwrap();
        let generation = view.generation;
        let mut page = view.page.as_ref().unwrap().clone();
        page.comments[0].body = format!("inserted\n\n{}", page.comments[0].body);
        app.on_msg(Msg::Post {
            generation,
            result: Ok(Box::new(page)),
        });
        screen(&mut app, 100, 30);
        assert_eq!(app.post_view.as_ref().unwrap().scroll, 0);
    }

    #[test]
    fn a_new_hold_takes_precedence_over_the_previously_drawn_comment() {
        let run = |edited: bool| {
            let (mut app, _) = open_long_post(|_| {});
            let view = app.post_view.as_mut().unwrap();
            let page = view.page.as_mut().unwrap();
            page.body.clear();
            page.comments[0].body = "short first comment".into();
            let mut next = page.comments[0].clone();
            next.id = "next".into();
            next.body = paragraphs("second passage", 1..=60);
            page.comments.push(next);
            screen(&mut app, 100, 30);
            press(&mut app, 'j');
            screen(&mut app, 100, 30);
            for key in ['h', ' ', 'l'] {
                press(&mut app, key);
            }
            if edited {
                let view = app.post_view.as_ref().unwrap();
                let generation = view.generation;
                let mut page = view.page.as_ref().unwrap().clone();
                page.comments[1].body =
                    format!("inserted\n\nalso inserted\n\n{}", page.comments[1].body);
                app.on_msg(Msg::Post {
                    generation,
                    result: Ok(Box::new(page)),
                });
            }
            screen(&mut app, 100, 30)
                .lines()
                .enumerate()
                .find_map(|(row, line)| {
                    line.split("second passage ")
                        .nth(1)
                        .and_then(|rest| rest.split_whitespace().next())
                        .map(|rest| (row, rest.to_owned()))
                })
        };
        let before = run(false);
        assert!(before.is_some());
        assert_eq!(run(true), before);
    }

    #[test]
    fn repeated_braces_do_not_move_an_edit_anchor_to_another_passage() {
        for comment in [false, true] {
            let (mut app, _) = open_long_post(|_| {});
            let original = format!(
                "opening\n\n}}\n\nFIRST unique passage\n\n}}\n\nSECOND unique passage\n\n{}",
                paragraphs("tail", 1..=40)
            );
            let view = app.post_view.as_mut().unwrap();
            let page = view.page.as_mut().unwrap();
            if comment {
                page.body.clear();
                page.comments[0].body = original.clone();
            } else {
                page.body = original.clone();
            }
            screen(&mut app, 100, 30);
            let view = app.post_view.as_mut().unwrap();
            view.scroll = if comment { view.row_starts[0] + 7 } else { 7 };
            let second = |shown: String| {
                shown
                    .lines()
                    .position(|line| line.contains("SECOND unique passage"))
            };
            let before = second(screen(&mut app, 100, 30));
            assert!(before.is_some());
            let view = app.post_view.as_ref().unwrap();
            let generation = view.generation;
            let mut page = view.page.as_ref().unwrap().clone();
            if comment {
                page.comments[0].body = format!("}}\n\n{original}");
            } else {
                page.body = format!("}}\n\n{original}");
            }
            app.on_msg(Msg::Post {
                generation,
                result: Ok(Box::new(page)),
            });
            assert_eq!(
                second(screen(&mut app, 100, 30)),
                before,
                "comment={comment}"
            );
        }
    }

    #[test]
    fn spoiler_then_selftext_edit_keeps_the_unchanged_comment_passage() {
        let (mut app, _) = open_long_post(|_| {});
        let view = app.post_view.as_mut().unwrap();
        let page = view.page.as_mut().unwrap();
        page.body = "short selftext".into();
        page.comments[0].body = format!(
            "{}\n\n{SPOILER_START}short spoiler{SPOILER_END}",
            paragraphs("stable passage", 1..=60)
        );
        screen(&mut app, 100, 30);
        let view = app.post_view.as_mut().unwrap();
        view.scroll = view.row_starts[0] + 26;
        let passage = |shown: String| {
            shown.lines().enumerate().find_map(|(row, line)| {
                line.split("stable passage ")
                    .nth(1)
                    .and_then(|rest| rest.split_whitespace().next())
                    .map(|rest| (row, rest.to_owned()))
            })
        };
        let before = passage(screen(&mut app, 100, 30));
        assert!(before.is_some());
        press(&mut app, 'v');
        let view = app.post_view.as_ref().unwrap();
        let generation = view.generation;
        let mut page = view.page.as_ref().unwrap().clone();
        page.body = format!("inserted\n\nalso inserted\n\n{}", page.body);
        app.on_msg(Msg::Post {
            generation,
            result: Ok(Box::new(page)),
        });
        assert_eq!(passage(screen(&mut app, 100, 30)), before);
    }

    #[test]
    fn spoiler_then_paging_anchors_the_comment_actually_being_read() {
        let run = |edited: bool, held: bool| {
            let (mut app, _) = open_long_post(|_| {});
            let view = app.post_view.as_mut().unwrap();
            let page = view.page.as_mut().unwrap();
            page.body.clear();
            page.comments[0].body = format!(
                "{}\n{SPOILER_START}short spoiler{SPOILER_END}",
                paragraphs("first passage", 1..=10)
            );
            let mut next = page.comments[0].clone();
            next.id = "next".into();
            next.body = paragraphs("next passage", 1..=60);
            page.comments.push(next);
            screen(&mut app, 100, 30);
            let view = app.post_view.as_mut().unwrap();
            view.scroll = view.row_starts[0] + 8;
            screen(&mut app, 100, 30);
            if held {
                press(&mut app, 'l');
                screen(&mut app, 100, 30);
            }
            press(&mut app, 'v');
            press(&mut app, ' ');
            if edited {
                let view = app.post_view.as_ref().unwrap();
                let generation = view.generation;
                let mut page = view.page.as_ref().unwrap().clone();
                page.comments[1].body =
                    format!("inserted\n\nalso inserted\n\n{}", page.comments[1].body);
                app.on_msg(Msg::Post {
                    generation,
                    result: Ok(Box::new(page)),
                });
            }
            screen(&mut app, 100, 30)
                .lines()
                .enumerate()
                .find_map(|(row, line)| {
                    line.split("next passage ")
                        .nth(1)
                        .and_then(|rest| rest.split_whitespace().next())
                        .map(|rest| (row, rest.to_owned()))
                })
        };
        for held in [false, true] {
            let before = run(false, held);
            assert!(before.is_some());
            assert_eq!(run(true, held), before, "held={held}");
        }
    }

    #[test]
    fn reading_a_more_slot_follows_new_replies_not_a_following_sibling() {
        let mut wrong = Vec::new();
        for (toggle, preceding_blank, twice) in [
            (false, false, false),
            (true, false, false),
            (false, true, false),
            (false, false, true),
        ] {
            let (mut app, _) = open_long_post(|_| {});
            let view = app.post_view.as_mut().unwrap();
            let page = view.page.as_mut().unwrap();
            page.body.clear();
            page.comments[0].body = format!("parent {SPOILER_START}short spoiler{SPOILER_END}");
            page.comments[0].more = Some(crate::model::MoreReplies {
                cursor: "/more".into(),
                count: Some(10),
            });
            let mut sibling = page.comments[0].clone();
            sibling.id = "sibling".into();
            sibling.more = None;
            sibling.body = paragraphs("unique sibling passage", 1..=30);
            page.comments.push(sibling);
            screen(&mut app, 100, 30);
            let view = app.post_view.as_mut().unwrap();
            view.cursor = 1;
            view.free_scroll = false;
            view.scroll = view.row_starts[1] - usize::from(preceding_blank);
            screen(&mut app, 100, 30);
            app.activate_comment();
            press(&mut app, 'h');
            screen(&mut app, 100, 30);
            if toggle {
                press(&mut app, 'v');
            }
            let view = app.post_view.as_ref().unwrap();
            let generation = view.generation;
            let mut reply = view.page.as_ref().unwrap().comments[0].clone();
            reply.more = None;
            let replies: Vec<_> = (0..10)
                .map(|i| Comment {
                    id: format!("new-{i}"),
                    body: format!("new reply {i}"),
                    ..reply.clone()
                })
                .collect();
            for _ in 0..if twice { 2 } else { 1 } {
                app.on_msg(Msg::MoreReplies {
                    generation,
                    comment_id: "c".into(),
                    result: Ok(ReplyPage::complete(replies.clone())),
                });
            }
            screen(&mut app, 100, 30);
            let view = app.post_view.as_ref().unwrap();
            let expected = view.row_starts[1] - usize::from(preceding_blank);
            if view.scroll != expected {
                wrong.push((toggle, preceding_blank, twice, view.scroll, expected));
            }
        }
        assert!(
            wrong.is_empty(),
            "toggle/blank/twice/actual/expected: {wrong:?}"
        );
    }
    #[test]
    fn removed_parent_more_follows_a_recovered_descendant_before_drawing() {
        let (mut app, _) = open_long_post(|_| {});
        let source = |sequence, cached| crate::cache::Source {
            order: Some(crate::cache::RequestOrder {
                generation: [1; 16],
                sequence,
            }),
            observed: Some((sequence, sequence)),
            cached,
        };
        let view = app.post_view.as_mut().unwrap();
        let page = view.page.as_mut().unwrap();
        page.body.clear();
        let mut child = page.comments[0].clone();
        child.id = "child".into();
        child.more = Some(crate::model::MoreReplies {
            cursor: "/child".into(),
            count: None,
        });
        child.sources = Some(crate::model::CommentSources {
            content: source(1, true),
            more: source(1, true),
            parent: Some(crate::model::ParentMembership {
                source: source(1, true),
                present: true,
            }),
        });
        let mut parent = child.clone();
        parent.id = "parent".into();
        parent.more.as_mut().unwrap().cursor = "/parent".into();
        parent.replies = vec![child.clone()];
        let mut sibling = child.clone();
        sibling.id = "sibling".into();
        sibling.more = None;
        sibling.body = paragraphs("unique following passage", 0..30);
        page.comments = vec![parent, sibling];
        screen(&mut app, 100, 30);
        let view = app.post_view.as_mut().unwrap();
        let more = view
            .rows()
            .iter()
            .position(|row| {
                matches!(
                    row.kind,
                    RowKind::More {
                        parent: "parent",
                        ..
                    }
                )
            })
            .unwrap();
        view.cursor = more;
        view.free_scroll = false;
        view.scroll = view.row_starts[more];
        screen(&mut app, 100, 30);
        app.activate_comment();
        press(&mut app, 'h');
        screen(&mut app, 100, 30);
        let view = app.post_view.as_ref().unwrap();
        let generation = view.generation;
        let mut fresh = view.page.as_ref().unwrap().clone();
        fresh.comments[0].more = None;
        app.on_msg(Msg::Snapshot {
            message: Box::new(Msg::Post {
                generation,
                result: Ok(Box::new(fresh)),
            }),
            fetched_at: 1000,
            source: source(3, false),
            refreshing: false,
        });
        let mut recovered = child.clone();
        recovered.id = "recovered".into();
        recovered.more = None;
        child.replies.push(recovered);
        app.on_msg(Msg::Snapshot {
            message: Box::new(Msg::MoreReplies {
                generation,
                comment_id: "parent".into(),
                result: Ok(ReplyPage::complete(vec![child])),
            }),
            fetched_at: 1000,
            source: source(2, true),
            refreshing: false,
        });
        screen(&mut app, 100, 30);
        let view = app.post_view.as_ref().unwrap();
        let reply = view.rows().iter().position(|row|
            matches!(row.kind, RowKind::Comment { comment, .. } if comment.id == "recovered")).unwrap();
        assert_eq!(
            view.scroll, view.row_starts[reply],
            "follow the reply, not its sibling"
        );
    }
}
