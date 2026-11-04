//! Rendering: tab sidebar, the active list and, when it fits, the open post beside it.
//! Takes `&mut App` to keep scroll offsets in sync and to load the thumbnails it draws.

mod experience;
mod help;
mod keymap;
mod loading;
mod rich_text;
mod search;
mod settings;

use std::collections::{HashMap, HashSet, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
use std::time::Instant;

use chrono::{DateTime, Utc};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect, Size};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap};
#[cfg(test)]
use ratatui_image::picker::ProtocolType;
use ratatui_image::{Resize, StatefulImage};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::app::{
    App, CARD_MIN_WIDTH, CardImageProtocol, CardImageSlot, CardScroll, DETAIL_MIN_WIDTH, Focus,
    ImageSlot, MAIN_MIN_WIDTH, Overlay, PickerEntry, PostView, RowKind, Tab, TabKind, UnreadBadge,
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
    pub const OVERLAY: Color = Color::Rgb(154, 160, 185);
    pub const BASE: Color = Color::Rgb(30, 30, 46);
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
const PANEL_PAD: u16 = 1;
const PANEL_GAP: u16 = 1;
/// The list and detail panes' inset, which leaves room for the focus outline.
const PANE_MARGIN: Margin = Margin {
    horizontal: PANEL_PAD + 1,
    vertical: 1,
};
const POPUP_OUTER_X: u16 = 2;
const POPUP_OUTER_Y: u16 = 1;
const NSFW_EMPTY_HINT: &str = "NSFW is off. Press F2 and enable Show NSFW in Settings.";
#[cfg(test)]
use crate::card_image::encode_tile;
/// Kitty's Unicode placeholders can address only 297 rows or columns per image.
use crate::card_image::{
    Indicator, Key as CardImageKey, Overlay as ImageOverlay, Preparation, TILE_COLS, TILE_ROWS,
};

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
    image_size: Size,
    image_at: ratatui::layout::Position,
    pixels: (u32, u32),
}

impl PostCard {
    fn height(&self) -> usize {
        self.lines
            .len()
            .max(usize::from(self.image_at.y) + usize::from(self.image_size.height))
            + 1
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

#[derive(Clone)]
struct CardPicture {
    url: Option<String>,
    area: Rect,
    size: Size,
    skipped: u16,
    pixels: (u32, u32),
    indicator: Option<Indicator>,
    selected: bool,
    detail: bool,
    post_id: Option<String>,
    video_url: Option<String>,
    video_frame: bool,
}

impl CardPicture {
    fn overlay(&self) -> Option<ImageOverlay> {
        self.indicator.clone().map(|indicator| ImageOverlay {
            indicator,
            top: 0,
            rows: self.size.height,
        })
    }

    fn status(&self, label: &str) -> String {
        let prefix = match &self.indicator {
            Some(Indicator::Video { gif: true, .. }) => "▶ GIF ·",
            Some(Indicator::Video { .. }) => "▶ video ·",
            Some(Indicator::Link) => "↗ link ·",
            Some(Indicator::Gallery(_)) => "▦ gallery ·",
            None => "▣",
        };
        format!("{prefix} {label}")
    }
}

#[derive(Default)]
struct ListPictures {
    thumbs: Vec<ImageSpot>,
    cards: Vec<CardPicture>,
    inline: Vec<ImageSpot>,
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    f.render_widget(Block::default().style(Style::new().fg(TEXT).bg(BASE)), area);
    app.resize(area.width);
    let sidebar_width = app.sidebar_width();
    let [sidebar, main] =
        Layout::horizontal([Constraint::Length(sidebar_width), Constraint::Min(20)]).areas(area);
    let sidebar_layout = sidebar_layout(app, sidebar);
    let sidebar_icons: Vec<String> = sidebar_layout
        .rows
        .iter()
        .filter_map(|row| match row {
            SideRow::Tab(index) => app.tabs[*index].subreddit().map(str::to_string),
            _ => None,
        })
        .collect();
    let mut spots = Vec::new();
    let mut pictures = ListPictures::default();
    let [body, footer] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(main);
    let search_feed = app.search_feed_open() && app.post_view.is_none();
    let body = if (app.active == crate::app::SEARCH_TAB || search_feed) && !body.is_empty() {
        search::draw_bar(
            f,
            app,
            body,
            Margin {
                horizontal: PANEL_PAD,
                vertical: 0,
            },
        )
    } else {
        body
    };
    let wants_preview =
        app.post_view.is_none() && app.active == crate::app::SEARCH_TAB && app.search.shows_rows();
    let preview_beside = wants_preview && app.detail_fits();
    let search_feed_beside = search_feed && app.detail_fits();
    if app.active == crate::app::SEARCH_TAB {
        app.search_preview_drawn(preview_beside);
    }
    let (main, detail) = if preview_beside || search_feed_beside {
        let main_width = (body.width / 5 * 2).max(MAIN_MIN_WIDTH);
        let [main, detail] = Layout::horizontal([
            Constraint::Length(main_width),
            Constraint::Min(DETAIL_MIN_WIDTH),
        ])
        .areas(body);
        (main, Some(detail))
    } else if app.post_view.is_some() {
        if app.detail_fits() {
            let main_width = (body.width / 5 * 2).max(MAIN_MIN_WIDTH);
            let [main, detail] = Layout::horizontal([
                Constraint::Length(main_width),
                Constraint::Min(DETAIL_MIN_WIDTH),
            ])
            .areas(body);
            (main, Some(detail))
        } else {
            (Rect::default(), Some(body))
        }
    } else {
        (body, None)
    };
    let beside = if preview_beside {
        Beside::Preview
    } else if search_feed_beside {
        Beside::Feed
    } else {
        Beside::Post
    };
    let panes = panes(app, sidebar, main, detail, beside);
    // A lone pane needs no outline, so it keeps the cells the outline would take.
    let margin = if panes.len() >= 2 {
        PANE_MARGIN
    } else {
        Margin {
            horizontal: PANEL_PAD,
            vertical: 0,
        }
    };
    if main.is_empty() || !matches!(app.active_tab().kind, TabKind::AllSubs) {
        let mut icons = sidebar_icons.clone();
        if (app.active == crate::app::SEARCH_TAB || search_feed_beside) && !main.is_empty() {
            icons.extend(search::shown_communities(app, main.inner(margin)));
        }
        app.groups_drawn(&[], &icons, &[]);
    }
    if let Some(name) = app.active_tab().subreddit().map(str::to_string) {
        app.demand_icon(&name);
    }
    let mut list_loader = None;
    if !main.is_empty() {
        let body = main.inner(margin);
        if search_feed_beside {
            list_loader = search::draw(f, app, body, &mut pictures);
        } else {
            match app.active_tab().kind {
                TabKind::Home => list_loader = draw_feed(f, app, body, &mut pictures),
                TabKind::AllSubs => {
                    list_loader = draw_all_subs(f, app, body, &mut spots, &sidebar_icons)
                }
                TabKind::Bookmarks => draw_bookmarks(f, app, body, &mut pictures),
                TabKind::Search => list_loader = search::draw(f, app, body, &mut pictures),
                TabKind::Subreddit { .. } | TabKind::User { .. } => {
                    list_loader = draw_feed(f, app, body, &mut pictures)
                }
            }
        }
    }
    let post_loader = detail.and_then(|detail| {
        if search_feed_beside {
            list_loader = draw_feed(f, app, detail.inner(margin), &mut pictures);
            None
        } else if preview_beside {
            search::draw_preview(f, app, detail.inner(margin), &mut pictures)
        } else {
            draw_post(f, app, detail.inner(margin), &mut pictures)
        }
    });
    if sidebar_width > 0 {
        draw_sidebar(f, app, &sidebar_layout, &mut spots);
    }
    outline_focused(f, &panes);
    spots.extend(pictures.thumbs);
    let covered = draw_overlay(f, app);
    draw_card_pictures(f, app, &pictures.cards, covered);
    draw_spots(f, app, &spots, covered);
    for spot in &pictures.inline {
        draw_inline_image(f, app, &spot.url, spot.area, covered);
    }
    let visible = |loader: Option<Rect>| {
        loader.is_some_and(|area| covered.is_none_or(|covered| !area.intersects(covered)))
    };
    let loaders = VisibleLoaders {
        list: visible(list_loader),
        post: visible(post_loader),
    };
    app.loading_drawn(app.loading_active());
    draw_footer(f, app, footer, &loaders, &panes);
    // On very short terminals a dialog may also cover the underlying footer.
    if covered.is_some_and(|area| area.intersects(footer)) {
        draw_overlay(f, app);
    }
    app.status_drawn();
}

fn draw_overlay(f: &mut Frame, app: &mut App) -> Option<Rect> {
    crate::keys::map::sync(app);
    if app.help_scroll.is_some() {
        return help::draw(f, app);
    }
    if app.keymap.hint_visible() {
        return keymap::draw(f, app);
    }
    match &app.overlay {
        Some(Overlay::Settings) => Some(settings::draw(f, app)),
        Some(Overlay::Experience) => experience::draw(f, app),
        Some(Overlay::Auth) => Some(draw_auth(f, app)),
        Some(Overlay::Picker(p)) => Some(draw_picker(f, app, &p.input, p.selected)),
        Some(Overlay::Sort { selected }) => Some(draw_sort(f, app, *selected)),
        Some(Overlay::Search { input }) => Some(draw_search(f, app, input)),
        Some(Overlay::Membership { target, join }) => Some(draw_membership(f, target, *join)),
        None => None,
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
fn draw_spots(f: &mut Frame, app: &mut App, spots: &[ImageSpot], covered: Option<Rect>) {
    for spot in spots {
        if covered.is_some_and(|area| area.intersects(spot.area)) {
            continue;
        }
        if let Some(ImageSlot::Ready { protocol, .. }) = app.thumbs.get_mut(&spot.url) {
            let fill = StatefulImage::default()
                .resize(Resize::Scale(Some(ratatui_image::FilterType::Triangle)));
            f.render_stateful_widget(fill, spot.area, protocol.as_mut());
        } else if !matches!(app.thumbs.get(&spot.url), Some(ImageSlot::Failed(_))) {
            loading::draw(f, spot.area, loading::Kind::Media, "", app.loading_frame);
        }
    }
    let urls: Vec<&str> = spots.iter().map(|s| s.url.as_str()).collect();
    app.thumbs_drawn(&urls);
}

fn row_thumb<'a>(app: &App, post: &'a Post) -> Option<std::borrow::Cow<'a, str>> {
    // A spoiler's picture is the spoiler.
    if !app.thumbnails_on() || post.spoiler {
        return None;
    }
    post.media
        .list_thumb()
        .filter(|url| app.usable_thumb(Some(url.as_ref())).is_some())
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

fn tab_icon<'a>(app: &'a App, tab: &'a Tab) -> Option<&'a str> {
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
    if width == 0 {
        return String::new();
    }
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

fn choice_style(selected: bool) -> Style {
    if selected {
        Style::new().fg(TEXT).bg(SURFACE0).bold()
    } else {
        Style::new().fg(SUBTEXT)
    }
}

/// The focusable panes on screen, left to right, with their footer names and whether
/// each has focus. Beside Search the right column holds either a preview, which is
/// never focused, or the opened feed, which owns list focus while the Search rows
/// beside it wait.
fn panes(
    app: &App,
    sidebar: Rect,
    main: Rect,
    detail: Option<Rect>,
    beside: Beside,
) -> Vec<(Rect, &'static str, bool)> {
    let (list, post) = match beside {
        Beside::Preview => (main, None),
        Beside::Feed => (detail.unwrap_or_default(), None),
        Beside::Post => (main, detail.filter(|_| app.post_view.is_some())),
    };
    [
        (sidebar, "sidebar", app.focus == Focus::Sidebar),
        (list, "list", app.focus == Focus::List),
        (post.unwrap_or_default(), "post", app.post_focused()),
    ]
    .into_iter()
    .filter(|(area, ..)| !area.is_empty())
    .collect()
}

enum Beside {
    Preview,
    Feed,
    Post,
}

/// With two or more panes on screen, outlines the focused one in accent. Each pane then
/// keeps a one-cell inset on every side for it, so moving focus never moves content.
fn outline_focused(f: &mut Frame, panes: &[(Rect, &'static str, bool)]) {
    if panes.len() < 2 {
        return;
    }
    if let Some(&(area, ..)) = panes.iter().find(|pane| pane.2) {
        f.render_widget(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(ACCENT)),
            area,
        );
    }
}

fn selection_marker(selected: bool, focused: bool) -> &'static str {
    match (selected, focused) {
        (true, true) => "▌",
        (true, false) => "│",
        _ => " ",
    }
}

fn fit_spans(spans: Vec<Span<'static>>, width: usize) -> Vec<Span<'static>> {
    let mut output = Vec::new();
    let mut remaining = width;
    for mut span in spans {
        if remaining == 0 {
            break;
        }
        if span.width() > remaining {
            span.content = truncate(&span.content, remaining).into();
        }
        remaining = remaining.saturating_sub(span.width());
        output.push(span);
    }
    output
}

fn wrap_spans(
    spans: Vec<Span<'static>>,
    width: usize,
    prefix: &[Span<'static>],
) -> Vec<Line<'static>> {
    let room = width.saturating_sub(line_width(prefix)).max(1);
    let mut lines = Vec::new();
    let mut row = prefix.to_vec();
    let mut used = 0;
    let mut gap = 0;
    for mut span in spans {
        if span.content.trim().is_empty() {
            if used > 0 {
                gap += span.width();
            }
            continue;
        }
        if used > 0 && used + gap + span.width() > room {
            lines.push(Line::from(std::mem::replace(&mut row, prefix.to_vec())));
            used = 0;
            gap = 0;
        }
        if used == 0 {
            span.content = span.content.trim_start().to_string().into();
        }
        if gap > 0 {
            row.push(Span::raw(" ".repeat(gap)));
            used += gap;
        }
        gap = 0;
        span.content = truncate(&span.content, room.saturating_sub(used)).into();
        used += span.width();
        row.push(span);
    }
    if used > 0 {
        lines.push(Line::from(row));
    }
    lines
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

fn empty_content_state(f: &mut Frame, area: Rect, title: &str, hint: &str, show_nsfw: bool) {
    if !show_nsfw
        && 2 + wrapped_rows(title, area.width) + wrapped_rows(hint, area.width)
            > usize::from(area.height)
    {
        centered(
            f,
            area,
            vec![
                Line::styled(title.to_string(), Style::new().fg(SUBTEXT).bold()),
                Line::styled("NSFW is off", Style::new().fg(OVERLAY)),
                Line::styled("F2: enable Show NSFW", Style::new().fg(OVERLAY)),
            ],
        );
    } else {
        empty_state(f, area, "∅", title, hint);
    }
}

fn unread_chip(badge: UnreadBadge) -> Option<Span<'static>> {
    match badge {
        UnreadBadge::Unknown => Some(chip("? unread", OVERLAY, SURFACE0)),
        UnreadBadge::Count(n) if n > 0 => Some(chip(format!("● {n} unread"), CRUST, ACCENT)),
        _ => None,
    }
}

// ----- sidebar -------------------------------------------------------------

enum SideRow {
    Section(&'static str),
    /// A section header whose listed tabs are syncing.
    SyncingSection(&'static str),
    Loading,
    Tab(usize),
    Blank,
}

struct SidebarLayout {
    area: Rect,
    brand: Rect,
    list: Rect,
    status: Rect,
    rows: Vec<SideRow>,
}

fn sidebar_layout(app: &App, area: Rect) -> SidebarLayout {
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

fn draw_sidebar(f: &mut Frame, app: &App, layout: &SidebarLayout, spots: &mut Vec<ImageSpot>) {
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

fn sidebar_item(app: &App, index: usize, width: usize) -> Line<'static> {
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

// ----- main header ---------------------------------------------------------

fn view_header(f: &mut Frame, area: Rect, title: &str, chips: Vec<Span<'static>>) -> Rect {
    let width = usize::from(area.width);
    let maximum = area.height.saturating_sub(1).min(3);
    let gap = if area.width >= 100 && maximum >= 3 {
        PANEL_GAP
    } else {
        0
    };
    let chip_rows = maximum.saturating_sub(1 + gap) as usize;
    let mut lines = vec![Line::from(fit_spans(
        vec![chip(title, CRUST, ACCENT)],
        width,
    ))];
    let chips = chips
        .into_iter()
        .enumerate()
        .flat_map(|(index, chip)| {
            if index == 0 {
                vec![chip]
            } else {
                vec![Span::raw(" "), chip]
            }
        })
        .collect();
    lines.extend(wrap_spans(chips, width, &[]).into_iter().take(chip_rows));
    // Request activity must not move the list under the selection.
    let height = maximum;
    let [head, rest] =
        Layout::vertical([Constraint::Length(height), Constraint::Min(1)]).areas(area);
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
    let key = if matches!(
        crate::keys::map::context(app),
        crate::keys::map::Context::List | crate::keys::map::Context::Sidebar
    ) && app.help_scroll.is_none()
    {
        "zc "
    } else {
        ""
    };
    chip(
        format!("{key}{}", app.state.list_view().label()),
        TEXT,
        SURFACE0,
    )
}

// ----- post cards ----------------------------------------------------------

struct CardOpts {
    selected: bool,
    show_subreddit: bool,
    /// Columns after the selection bar kept free of text, for a thumbnail.
    reserve: usize,
}

/// Two lines per post: score gutter + title, comment gutter + meta chips.
fn post_card(app: &App, post: &Post, width: usize, opts: &CardOpts) -> [Line<'static>; 2] {
    let bg = post_background(app, post, opts.selected);
    let unseen = !app.state.is_read(&post.id);
    let read = app.state.is_read(&post.id);
    let bar_color = if app.focus == Focus::List {
        ACCENT
    } else {
        OVERLAY
    };
    let bar = Span::styled(
        selection_marker(opts.selected, app.focus == Focus::List),
        bg.fg(bar_color),
    );

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
        post_meta(app, post, width, opts.show_subreddit, bg, meta),
    ]
}

fn post_background(app: &App, post: &Post, selected: bool) -> Style {
    if selected {
        Style::new().bg(SURFACE0)
    } else if matches!(app.active_tab().kind, TabKind::Home | TabKind::User { .. })
        && app.joined(&post.subreddit) == Some(false)
    {
        Style::new().bg(MANTLE)
    } else {
        Style::new()
    }
}

fn post_meta(
    app: &App,
    post: &Post,
    width: usize,
    show_subreddit: bool,
    bg: Style,
    mut meta: Vec<Span<'static>>,
) -> Line<'static> {
    if post.nsfw {
        meta.push(Span::styled(" ", bg));
        meta.push(chip("nsfw", CRUST, RED));
    }
    if post.spoiler {
        meta.push(Span::styled(" ", bg));
        meta.push(chip("spoiler", CRUST, YELLOW));
    }
    if post.original_deleted || app.state.original_deleted(&post.id) {
        meta.push(Span::styled(" ", bg));
        meta.push(chip("Original post deleted", YELLOW, SURFACE0));
    }
    if show_subreddit {
        meta.push(Span::styled(
            format!("{}  ", post.source_label()),
            bg.fg(LAVENDER),
        ));
    }
    meta.push(Span::styled(format!("u/{}", post.author), bg.fg(SUBTEXT)));
    let age = post.age_at(Utc::now());
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
    let mut meta_line = fit_spans(meta, width);
    pad_to(&mut meta_line, width, bg);
    Line::from(meta_line).style(bg)
}

fn media_label(media: &Media, tag: &str) -> String {
    let icon = match media {
        Media::Image { .. } if media.video_url().is_some() => "▶",
        Media::Image { .. } => "▣",
        Media::Video { .. } => "▶",
        Media::Link { .. } if media.youtube_url().is_some() => "▶",
        Media::Gallery { .. } => "▦",
        Media::Link { .. } => "↗",
        Media::None => "",
    };
    format!("{icon} {}", truncate(tag, 24))
}

struct ListSpec<'a> {
    posts: &'a [&'a Post],
    selected: usize,
    /// The search preview lists posts without choosing one.
    show_selection: bool,
    show_subreddit: bool,
    offset: usize,
    scroll: CardScroll,
    append_area: Option<Rect>,
}

fn card_marker(app: &App, selected: bool, bg: Style) -> Span<'static> {
    Span::styled(
        format!("{} ", selection_marker(selected, app.focus == Focus::List)),
        bg.fg(if app.focus == Focus::List {
            ACCENT
        } else {
            OVERLAY
        }),
    )
}

fn fit_picture(
    pixels: (u32, u32),
    font: ratatui_image::FontSize,
    bounds: Size,
    viewport_height: u16,
) -> Size {
    let bounds = Size::new(
        bounds.width,
        bounds
            .height
            .min((u32::from(viewport_height) * 3 / 5) as u16),
    );
    if bounds.width == 0 || bounds.height == 0 {
        return Size::default();
    }
    picture_cells(
        pixels.0,
        pixels.1,
        font,
        bounds.width,
        bounds.height,
        bounds.height,
        f64::INFINITY,
    )
}

fn failed_video_without_picture(app: &App, post: &Post, still: Option<&str>) -> bool {
    app.video_failed_without_frame(post)
        && still.is_none_or(|url| {
            post.media.video_url() == Some(url)
                || (!matches!(
                    app.card_images.get(url),
                    Some(CardImageSlot::Ready { .. } | CardImageSlot::Cached { .. })
                ) && !matches!(app.images.get(url), Some(ImageSlot::Ready { .. }))
                    && (matches!(app.card_images.get(url), Some(CardImageSlot::Failed(_)))
                        || matches!(app.images.get(url), Some(ImageSlot::Failed(_)))))
        })
}

fn expanded_card(app: &App, post: &Post, width: usize, opts: &CardOpts, height: u16) -> PostCard {
    let bg = post_background(app, post, opts.selected);
    let marker = card_marker(app, opts.selected, bg);
    let article = matches!(post.media, Media::Link { .. }) && post.media.video_url().is_none();
    let image = (!post.spoiler && width >= 8 && app.inline_images_on())
        .then(|| {
            post.media
                .preview_image()
                .or_else(|| post.media.list_thumb())
                .map(std::borrow::Cow::into_owned)
        })
        .flatten();
    let decoded_size = image
        .as_deref()
        .and_then(|url| app.card_image_metadata.get(url).map(|m| m.size));
    let image_size = if article {
        decoded_size.or(post.preview_size)
    } else {
        post.preview_size.or(decoded_size)
    }
    .filter(|&(width, height)| width > 0 && height > 0);
    let (article_size, beside) = if article {
        article_picture(
            image_size,
            app.image_picker.font_size(),
            width as u16,
            height,
        )
    } else {
        (Size::default(), false)
    };
    let reserve = if article && image.is_none() && width >= 24 {
        6
    } else if article && beside {
        usize::from(article_size.width) + 2
    } else {
        0
    };
    let text_width = width.saturating_sub(reserve);
    let meta = post_meta(
        app,
        post,
        text_width,
        opts.show_subreddit,
        bg,
        vec![marker.clone()],
    );
    let mut lines = card_title(app, post, text_width, opts, height, bg);
    lines.insert(0, meta);
    if reserve > 0 {
        let monogram = if image.is_none() {
            post.media
                .tag()
                .unwrap_or_default()
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .take(2)
                .collect::<String>()
                .to_ascii_uppercase()
        } else {
            String::new()
        };
        for (index, line) in lines.iter_mut().enumerate() {
            let mut padding = vec![Span::styled(" ".repeat(reserve), bg)];
            if index == 0 && !monogram.is_empty() {
                padding = vec![
                    Span::styled(
                        format!(" {monogram:<2} "),
                        Style::new().fg(TEXT).bg(SURFACE1),
                    ),
                    Span::styled("  ", bg),
                ];
            }
            line.spans.splice(1..1, padding);
        }
    }
    let failed_without_picture = failed_video_without_picture(app, post, image.as_deref());
    if failed_without_picture {
        lines.push(Line::from(vec![
            marker.clone(),
            Span::styled("▶ video unavailable · O opens media", bg.fg(OVERLAY)),
        ]));
    }
    let preview = !failed_without_picture
        && !post.spoiler
        && width >= 8
        && app.inline_images_on()
        && (image.is_some() || post.media.video_url().is_some());
    let fitted_size = if preview {
        if article {
            article_size
        } else {
            fit_picture(
                image_size.unwrap_or(if post.media.video_url().is_some() {
                    (16, 9)
                } else {
                    (1, 1)
                }),
                app.image_picker.font_size(),
                Size::new(
                    width.saturating_sub(4) as u16,
                    height.saturating_sub(lines.len() as u16 + 1),
                ),
                height,
            )
        }
    } else {
        Size::default()
    };
    let image = image.filter(|_| fitted_size.height > 0);
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
    let lines_len = lines.len();
    PostCard {
        lines,
        footer,
        marker,
        style: bg,
        pixels: image_size.unwrap_or(if post.media.video_url().is_some() {
            (16, 9)
        } else {
            (1, 1)
        }),
        image,
        image_size: fitted_size,
        image_at: ratatui::layout::Position::new(
            if article && beside {
                2
            } else {
                2 + (width as u16).saturating_sub(4 + fitted_size.width) / 2
            },
            if article && beside {
                0
            } else {
                lines_len as u16
            },
        ),
    }
}

fn article_picture(
    pixels: Option<(u32, u32)>,
    font: ratatui_image::FontSize,
    width: u16,
    height: u16,
) -> (Size, bool) {
    let cols = width.saturating_sub(4);
    let rows = (height.saturating_sub(3)).min((u32::from(height) * 3 / 10) as u16);
    let beside = width >= 48
        && pixels.is_none_or(|(w, _)| {
            f64::from(w) * 1.5 < f64::from(cols) * f64::from(font.width.max(1)) * 0.6
        });
    if cols == 0 || rows == 0 {
        return (Size::default(), beside);
    }
    let (w, h) = pixels.unwrap_or((160, 90));
    let natural_cols = ((f64::from(w) * 1.5 / f64::from(font.width.max(1))).floor() as u16).max(1);
    (
        picture_cells(
            w,
            h,
            font,
            cols.min(natural_cols)
                .min(if beside { (width / 3).min(32) } else { cols }),
            rows.min(if beside { 8 } else { rows }),
            rows.min(if beside { 8 } else { rows }),
            1.5,
        ),
        beside,
    )
}

fn card_title(
    app: &App,
    post: &Post,
    width: usize,
    opts: &CardOpts,
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
    let unseen = if !app.state.is_read(&post.id) {
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

fn shift_card_rows(
    mut index: usize,
    mut line: usize,
    delta: isize,
    count: usize,
    height: impl Fn(usize) -> usize,
) -> (usize, usize) {
    if delta < 0 {
        let mut remaining = delta.unsigned_abs();
        while remaining > line && index > 0 {
            remaining -= line + 1;
            index -= 1;
            line = height(index);
        }
        line = line.saturating_sub(remaining);
    } else {
        line = line.saturating_add(delta as usize);
    }
    while index + 1 < count && line > height(index) {
        line -= height(index) + 1;
        index += 1;
    }
    (index, line.min(height(index)))
}

/// Row of card `target` when card `index` is drawn first with `line` rows hidden.
fn card_row(index: usize, line: usize, target: usize, height: impl Fn(usize) -> usize) -> isize {
    let rows = |cards: std::ops::Range<usize>| cards.map(|i| height(i) as isize + 1).sum::<isize>();
    if target >= index {
        rows(index..target) - line as isize
    } else {
        -rows(target..index) - line as isize
    }
}

fn card_viewport_end(
    index: usize,
    line: usize,
    viewport: usize,
    count: usize,
    height: impl Fn(usize) -> usize,
) -> (usize, usize) {
    let mut used = 0;
    let mut last = index;
    for i in index..count {
        let rows = height(i).saturating_sub(if i == index { line } else { 0 });
        if rows > 0 {
            last = i;
            used += rows;
        }
        if i + 1 < count && used < viewport {
            used += 1;
        }
        if used >= viewport {
            break;
        }
    }
    (used.min(viewport), last)
}

fn draw_card_list(
    f: &mut Frame,
    app: &App,
    area: Rect,
    spec: &mut ListSpec<'_>,
    pictures: &mut ListPictures,
) -> Option<usize> {
    if area.is_empty() || spec.posts.is_empty() {
        return None;
    }
    let width = area.width.saturating_sub(2) as usize;
    let show_selection = spec.show_selection;
    let card_at = |i, selected| {
        expanded_card(
            app,
            spec.posts[i],
            width,
            &CardOpts {
                selected: show_selection && i == selected,

                show_subreddit: spec.show_subreddit,
                reserve: 0,
            },
            area.height,
        )
    };
    let scroll = &mut spec.scroll;
    let offset = &mut spec.offset;
    let pending = std::mem::take(&mut scroll.pending);
    let mut anchor = scroll
        .post
        .as_ref()
        .and_then(|id| spec.posts.iter().position(|post| &post.id == id));
    let height = |i| card_at(i, spec.selected).height();
    let (first_visible, last_visible) = loop {
        if let Some(index) = anchor {
            *offset = index;
        } else {
            scroll.line = 0;
            *offset = (*offset).min(spec.selected);
            if height(spec.selected) > usize::from(area.height) {
                *offset = spec.selected;
            }
            let mut used = 0;
            for i in (*offset..=spec.selected).rev() {
                used += height(i) + 1;
                if used > usize::from(area.height) {
                    *offset = (i + 1).min(spec.selected);
                    break;
                }
            }
        }
        scroll.viewport = area.height as usize;
        let anchor_height = height(*offset);
        scroll.line = scroll.line.min(if scroll.height != anchor_height {
            anchor_height.saturating_sub(1)
        } else {
            anchor_height
        });
        let mut delta = pending;
        if pending == 0
            && spec.selected.abs_diff(*offset) <= scroll.viewport
            && let Some((id, row, fitted)) = &scroll.selected
            && (0..scroll.viewport as isize).contains(row)
            && spec.posts[spec.selected].id == *id
        {
            // A card that fitted and no longer fits below its row moves up until it does.
            let fits = scroll.viewport as isize - height(spec.selected) as isize;
            let target = if *fitted {
                (*row).min(fits.max(0))
            } else {
                *row
            };
            delta = card_row(*offset, scroll.line, spec.selected, height) - target;
        }
        (*offset, scroll.line) =
            shift_card_rows(*offset, scroll.line, delta, spec.posts.len(), height);
        let (available, _) = card_viewport_end(
            *offset,
            scroll.line,
            scroll.viewport,
            spec.posts.len(),
            height,
        );
        if available < scroll.viewport {
            let last = spec.posts.len() - 1;
            (*offset, scroll.line) = shift_card_rows(
                last,
                height(last),
                -(scroll.viewport as isize),
                spec.posts.len(),
                height,
            );
        }
        let (_, last_visible) = card_viewport_end(
            *offset,
            scroll.line,
            scroll.viewport,
            spec.posts.len(),
            height,
        );
        let first_visible =
            (*offset + usize::from(scroll.line == height(*offset))).min(last_visible);
        // Only scrolling moves the selection; data, picture and size changes move the view
        // to the selected post instead.
        if anchor.is_some()
            && pending == 0
            && !(first_visible..=last_visible).contains(&spec.selected)
        {
            anchor = None;
            continue;
        }
        break (first_visible, last_visible);
    };
    spec.selected = spec.selected.clamp(first_visible, last_visible);
    scroll.height = card_at(*offset, spec.selected).height();
    scroll.post = Some(spec.posts[*offset].id.clone());
    let selected_height = card_at(spec.selected, spec.selected).height() as isize;
    let row = card_row(*offset, scroll.line, spec.selected, |i| {
        card_at(i, spec.selected).height()
    });
    scroll.selected = Some((
        spec.posts[spec.selected].id.clone(),
        row,
        row >= 0 && row + selected_height <= scroll.viewport as isize,
    ));
    let mut y = area.y;
    let mut shown = 0;
    for i in *offset..spec.posts.len() {
        let card = card_at(i, spec.selected);
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
        if card.image_size.height > 0 {
            let image_start = usize::from(card.image_at.y);
            let image_y = y + image_start.saturating_sub(skip).min(area.height as usize) as u16;
            let skipped = skip.saturating_sub(image_start) as u16;
            if skipped < card.image_size.height && image_y < at.bottom() {
                pictures.cards.push(CardPicture {
                    url: card.image.clone(),
                    area: Rect::new(
                        area.x + card.image_at.x,
                        image_y,
                        card.image_size.width,
                        (card.image_size.height - skipped).min(at.bottom() - image_y),
                    ),
                    size: card.image_size,
                    skipped,
                    pixels: card.pixels,
                    indicator: match &spec.posts[i].media {
                        Media::Video { gif, url, .. } => Some(Indicator::Video {
                            gif: *gif,
                            play: url.is_some(),
                        }),
                        Media::Image { .. } if spec.posts[i].media.video_url().is_some() => {
                            Some(Indicator::Video {
                                gif: true,
                                play: true,
                            })
                        }
                        Media::Link { .. } if spec.posts[i].media.youtube_url().is_some() => {
                            Some(Indicator::Video {
                                gif: false,
                                play: true,
                            })
                        }
                        Media::Link { .. } => Some(Indicator::Link),
                        Media::Gallery { images, .. } if !images.is_empty() => {
                            Some(Indicator::Gallery(images.len()))
                        }
                        _ => None,
                    },
                    selected: spec.show_selection && i == spec.selected,
                    detail: false,
                    post_id: Some(spec.posts[i].id.clone()),
                    video_url: spec.posts[i].media.video_url().map(str::to_string),
                    video_frame: false,
                });
            }
        }
        shown += 1;
        if i + 1 == spec.posts.len()
            && usize::from(at.height) == rows
            && at.bottom() < area.bottom()
        {
            let tail_area = Rect::new(
                area.x,
                at.bottom(),
                area.width.saturating_sub(2),
                area.bottom() - at.bottom(),
            );
            spec.append_area = Some(tail_area);
        }
        y = y.saturating_add(at.height.saturating_add(1));
        if y >= area.bottom() {
            break;
        }
    }
    if scroll.height > scroll.viewport {
        draw_scrollbar(
            f,
            area,
            scroll
                .line
                .min(scroll.height.saturating_sub(scroll.viewport)),
            scroll.viewport,
            scroll.height,
        );
    } else {
        draw_scrollbar(f, area, *offset, shown, spec.posts.len());
    }
    Some(last_visible)
}

fn draw_card_pictures(f: &mut Frame, app: &mut App, spots: &[CardPicture], covered: Option<Rect>) {
    let prepare = app.card_preparation_allowed();
    app.card_video_drawn(
        spots
            .iter()
            .find(|spot| spot.selected)
            .map(|spot| spot.size),
    );
    let mut spots = spots.to_vec();
    let mut loading_rows = Vec::new();
    if let Some(label) = app.video_loading() {
        for spot in spots.iter_mut().filter(|spot| {
            spot.selected && (spot.detail || app.post_view.is_none()) && !spot.area.is_empty()
        }) {
            loading_rows.push((
                Rect::new(spot.area.x, spot.area.bottom() - 1, spot.area.width, 1),
                label,
            ));
            spot.area.height -= 1;
        }
    }
    let urls: Vec<&str> = spots
        .iter()
        .filter_map(|spot| {
            spot.url
                .as_deref()
                .filter(|url| !spot.selected || !app.video_uses_playback(url))
        })
        .collect();
    app.card_pictures_drawn(&urls);
    app.card_renderer.begin();
    for spot in &spots {
        if spot.area.is_empty() {
            continue;
        }
        let preview = if spot.selected {
            app.video_preview(!spot.detail)
        } else {
            spot.post_id
                .as_deref()
                .zip(spot.video_url.as_deref())
                .and_then(|(post, source)| app.inactive_card_frame(post, source))
                .map(|frame| (frame, false))
        };
        if let Some((
            video::Frame {
                pixels: video::Pixels::Raw(image),
                ..
            },
            playing,
        )) = preview
        {
            let image = image.clone();
            let mut spot = spot.clone();
            spot.video_frame = true;
            if playing && let Some(Indicator::Video { play, .. }) = &mut spot.indicator {
                *play = false;
            }
            if let Err(error) = draw_card_image(
                f,
                &image,
                &mut None,
                &spot,
                CardRender {
                    picker: &app.image_picker,
                    renderer: &mut app.card_renderer,
                    prepare: Preparation::Refresh,
                    fingerprint: None,
                    covered,
                    phase: app.loading_frame,
                },
            ) {
                draw_visible_status(
                    f,
                    spot.area,
                    covered,
                    &format!("▶ video unavailable: {error}"),
                );
            }
            continue;
        }
        let Some(url) = spot
            .url
            .as_deref()
            .filter(|url| !spot.selected || !app.video_uses_playback(url))
        else {
            let label = if !spot.selected {
                spot.status("select to preview")
            } else if !spot.detail && app.post_view.is_some() {
                spot.status("open to preview")
            } else if !app.video_controls_available() && !app.config.media.video_autoplay {
                spot.status("l return to preview")
            } else if app
                .selected_video()
                .is_some_and(|player| player.error().is_some())
            {
                spot.status("unavailable · O opens media")
            } else if app.video_loading().is_some() {
                continue;
            } else {
                spot.status("p play/pause")
            };
            draw_visible_status(f, spot.area, covered, &label);
            continue;
        };
        let preparation = if !prepare {
            Preparation::Hold
        } else if app.card_image_is_retained(url) {
            Preparation::Missing
        } else {
            Preparation::Refresh
        };
        let Some(slot) = app.card_images.get_mut(url) else {
            draw_visible_loading(
                f,
                spot.area,
                covered,
                &spot.status("loading image…"),
                app.loading_frame,
            );
            continue;
        };
        match slot {
            CardImageSlot::Ready { image, protocol }
            | CardImageSlot::Cached {
                image, protocol, ..
            } => {
                if let Err(error) = draw_card_image(
                    f,
                    image,
                    protocol,
                    spot,
                    CardRender {
                        picker: &app.image_picker,
                        renderer: &mut app.card_renderer,
                        prepare: preparation,
                        fingerprint: app
                            .card_image_metadata
                            .get(url)
                            .map(|metadata| metadata.fingerprint),
                        covered,
                        phase: app.loading_frame,
                    },
                ) {
                    draw_visible_status(
                        f,
                        spot.area,
                        covered,
                        &spot.status(&format!("image unavailable: {error}")),
                    );
                    *slot = CardImageSlot::Failed(error);
                }
            }
            CardImageSlot::Loading => draw_visible_loading(
                f,
                spot.area,
                covered,
                &spot.status("loading image…"),
                app.loading_frame,
            ),
            CardImageSlot::Failed(error) => draw_visible_status(
                f,
                spot.area,
                covered,
                &spot.status(&format!("image unavailable: {error}")),
            ),
        }
    }
    app.card_renderer.finish();
    for (area, label) in loading_rows {
        draw_visible_loading(f, area, covered, label, app.loading_frame);
    }
}

struct CardRender<'a> {
    picker: &'a ratatui_image::picker::Picker,
    renderer: &'a mut crate::card_image::Renderer,
    prepare: Preparation,
    fingerprint: Option<[u8; 32]>,
    covered: Option<Rect>,
    phase: usize,
}

fn draw_card_image(
    f: &mut Frame,
    image: &std::sync::Arc<image::DynamicImage>,
    protocol: &mut Option<CardImageProtocol>,
    spot: &CardPicture,
    render: CardRender<'_>,
) -> Result<(), String> {
    let CardRender {
        picker,
        renderer,
        prepare,
        fingerprint,
        covered,
        phase,
    } = render;
    if protocol.as_ref().is_none_or(|protocol| {
        protocol.size != spot.size
            || protocol.pixels != spot.pixels
            || protocol.overlay != spot.overlay()
    }) {
        *protocol = Some(CardImageProtocol {
            size: spot.size,
            pixels: spot.pixels,
            overlay: spot.overlay(),
            tiles: HashMap::new(),
        });
    }
    let Some(protocol) = protocol else {
        return Ok(());
    };
    let native = picker.protocol_type() == ratatui_image::picker::ProtocolType::Kitty;
    let request_key = |top, rows, left, cols| {
        let mut request = CardImageKey::new(
            spot.url.as_deref().unwrap_or("card-video"),
            picker,
            spot.size,
            spot.pixels,
            top,
            rows,
            left / TILE_COLS,
        );
        request.left = left;
        request.cols = cols;
        request.overlay = spot.overlay();
        request.post_id = spot.video_frame.then(|| spot.post_id.clone()).flatten();
        request.video = spot.video_frame;
        request.fingerprint = (!spot.video_frame).then_some(fingerprint).flatten();
        request
    };
    let preupload_at = visible_rects(spot.area, covered)
        .first()
        .map(|area| ratatui::layout::Position::new(area.x, area.y));
    let mut prepared = Vec::new();
    // A fitted still is bounded by the viewport. Finish its canvas while any part
    // is visible so scrolling into the other strips does not restart preparation.
    if native && !spot.video_frame && preupload_at.is_some() {
        for top in (0..spot.size.height).step_by(usize::from(TILE_ROWS)) {
            for left in (0..spot.size.width).step_by(usize::from(TILE_COLS)) {
                let request = request_key(
                    top,
                    TILE_ROWS.min(spot.size.height - top),
                    left,
                    TILE_COLS.min(spot.size.width - left),
                );
                if let Some(Ok(tile)) = renderer.tile(request, image, picker, prepare) {
                    prepared.push(tile);
                }
            }
        }
    }
    let first = spot.skipped / TILE_ROWS;
    let end = (spot.skipped + spot.area.height).div_ceil(TILE_ROWS);
    let mut visible = Vec::new();
    for row in first..end {
        for column in 0..spot.size.width.div_ceil(TILE_COLS) {
            let start = spot.skipped.max(row * TILE_ROWS);
            let y = spot.area.y + start - spot.skipped;
            let rows = ((row + 1) * TILE_ROWS - start).min(spot.area.bottom() - y);
            let cols = TILE_COLS.min(spot.size.width - column * TILE_COLS);
            let tile_area = Rect::new(spot.area.x + column * TILE_COLS, y, cols, rows);
            for area in visible_rects(tile_area, covered) {
                let top = start + area.y - y;
                let left = column * TILE_COLS + area.x - tile_area.x;
                let key = (top, left, area.height, area.width);
                visible.push(key);
                let source_top = if native { row * TILE_ROWS } else { top };
                let source_left = if native { column * TILE_COLS } else { left };
                let request = request_key(
                    source_top,
                    if native {
                        TILE_ROWS.min(spot.size.height - source_top)
                    } else {
                        area.height
                    },
                    source_left,
                    if native { cols } else { area.width },
                );
                if let Some(result) = renderer.tile(request, image, picker, prepare) {
                    protocol.tiles.insert(key, result?);
                }
                if let Some(encoded) = protocol.tiles.get(&key) {
                    encoded.render_region(
                        area,
                        ratatui::layout::Position::new(left - source_left, top - source_top),
                        f.buffer_mut(),
                    );
                } else {
                    loading::draw(
                        f,
                        area,
                        loading::Kind::Media,
                        &spot.status("preparing image…"),
                        phase,
                    );
                }
            }
        }
    }
    if let Some(position) = preupload_at {
        let mut uploads = String::new();
        for tile in prepared {
            if let Some(upload) = tile.take_upload() {
                uploads.push_str(&upload);
            }
        }
        if !uploads.is_empty()
            && let Some(cell) = f.buffer_mut().cell_mut(position)
        {
            uploads.push_str(cell.symbol());
            cell.set_symbol(&uploads).set_diff_option(
                ratatui::buffer::CellDiffOption::ForcedWidth(std::num::NonZeroU16::MIN),
            );
        }
    }
    protocol.tiles.retain(|key, _| visible.contains(key));
    Ok(())
}

/// The uncovered top, bottom, left and right strips retain their source coordinates.
fn visible_rects(area: Rect, covered: Option<Rect>) -> Vec<Rect> {
    let Some(covered) = covered.filter(|covered| area.intersects(*covered)) else {
        return (!area.is_empty()).then_some(area).into_iter().collect();
    };
    let cut = area.intersection(covered);
    [
        Rect::new(area.x, area.y, area.width, cut.y - area.y),
        Rect::new(
            area.x,
            cut.bottom(),
            area.width,
            area.bottom() - cut.bottom(),
        ),
        Rect::new(area.x, cut.y, cut.x - area.x, cut.height),
        Rect::new(cut.right(), cut.y, area.right() - cut.right(), cut.height),
    ]
    .into_iter()
    .filter(|rect| !rect.is_empty())
    .collect()
}

fn draw_visible_status(f: &mut Frame, area: Rect, covered: Option<Rect>, label: &str) {
    for visible in visible_rects(area, covered) {
        draw_picture_status(f, visible, label);
    }
}

fn draw_visible_loading(
    f: &mut Frame,
    area: Rect,
    covered: Option<Rect>,
    label: &str,
    phase: usize,
) {
    for visible in visible_rects(area, covered) {
        loading::draw(f, visible, loading::Kind::Media, label, phase);
    }
}

fn draw_picture_status(f: &mut Frame, area: Rect, label: &str) {
    if area.is_empty() {
        return;
    }
    let lines: Vec<Line> = textwrap::wrap(label, usize::from(area.width))
        .into_iter()
        .take(usize::from(area.height))
        .map(|line| Line::raw(line.into_owned()))
        .collect();
    let rows = lines.len() as u16;
    f.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .style(Style::new().fg(SUBTEXT)),
        Rect::new(area.x, area.y + (area.height - rows) / 2, area.width, rows),
    );
}

fn draw_pagination_error(f: &mut Frame, mut area: Rect, label: &str) {
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
    centered(
        f,
        inner,
        vec![Line::styled(label.to_string(), Style::new().fg(RED).bold())],
    );
}

/// Scrolling list of post cards separated by a blank line.
fn draw_post_list(
    f: &mut Frame,
    app: &App,
    area: Rect,
    spec: &mut ListSpec<'_>,
    pictures: &mut ListPictures,
) -> Option<usize> {
    if app.state.list_view() == ListView::Card && area.height >= 3 {
        return draw_card_list(f, app, area, spec, pictures);
    }
    spec.scroll.viewport = area.height as usize;
    const ITEM: usize = 3;
    let offset = &mut spec.offset;
    let fits = ((area.height as usize + 1) / ITEM).max(1);
    if spec.selected < *offset {
        *offset = spec.selected;
    } else if spec.selected >= *offset + fits {
        *offset = spec.selected + 1 - fits;
    }
    let shown = spec
        .posts
        .len()
        .saturating_sub(*offset)
        .min((area.height as usize).div_ceil(ITEM));
    let width = area.width.saturating_sub(2) as usize;
    let reserve = thumb_reserve(app, spec.posts.iter().copied());
    let mut lines = Vec::new();
    let mut tail_y = None;
    for (i, post) in spec.posts.iter().enumerate().skip(*offset).take(shown) {
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
            selected: spec.show_selection && i == spec.selected,

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
    if let Some(y) = tail_y.filter(|y| *y < area.bottom()) {
        let tail_area = Rect::new(area.x, y, area.width.saturating_sub(2), area.bottom() - y);
        spec.append_area = Some(tail_area);
    }
    draw_scrollbar(f, area, *offset, shown, spec.posts.len());
    (shown > 0).then(|| *offset + shown - 1)
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
            ("┃", OVERLAY)
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

fn new_posts_chip(app: &App) -> Span<'static> {
    let label = if app.post_focused() {
        "New posts".to_string()
    } else {
        format!("New posts · {} refresh", search::reading_key(app, "r"))
    };
    chip(label, ACCENT, SURFACE0)
}

/// Every island reserves room for all the posts it may show, so loading never moves the grid.
fn card_height(app: &App) -> u16 {
    let shown = app.config.all_subs_posts_per_group.max(1) as u16;
    // borders + top/bottom padding + 2 lines per post + a gap between posts
    2 + 2 + shown * 2 + shown.saturating_sub(1)
}

const CARD_ROW_GAP: u16 = 1;

fn draw_all_subs(
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

fn draw_group_card(f: &mut Frame, app: &App, index: usize, area: Rect, spots: &mut Vec<ImageSpot>) {
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

// ----- subreddit feed & bookmarks --------------------------------------------

fn membership_chip(app: &App, target: &crate::model::MembershipTarget) -> Span<'static> {
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

/// A user feed's profile block: the karma line and up to two description lines.
const USER_PROFILE_ROWS: usize = 3;

/// A user's profile page summary above their posts: the avatar beside the karma line,
/// then the description, wrapped to at most a third of the pane.
fn draw_profile(
    f: &mut Frame,
    app: &App,
    area: Rect,
    profile: &crate::model::UserProfile,
    pictures: &mut ListPictures,
    fixed: Option<usize>,
) -> Rect {
    const TEXT_START: usize = ICON_COLS as usize + 3;
    let text_width = usize::from(area.width).saturating_sub(TEXT_START).max(1);
    let limit = usize::from(area.height / 3).min(fixed.unwrap_or(usize::MAX));
    let mut wrapped: Vec<String> = profile
        .lines
        .iter()
        .flat_map(|line| {
            textwrap::wrap(
                line,
                textwrap::Options::new(text_width)
                    .word_splitter(textwrap::WordSplitter::NoHyphenation),
            )
            .into_iter()
            .map(|line| line.into_owned())
            .collect::<Vec<_>>()
        })
        .collect();
    if wrapped.is_empty() || limit == 0 {
        return area;
    }
    if wrapped.len() > limit {
        wrapped.truncate(limit);
        if let Some(last) = wrapped.last_mut() {
            *last = truncate(&format!("{last}…"), text_width);
        }
    }
    let lines: Vec<Line> = wrapped
        .into_iter()
        .enumerate()
        .map(|(index, text)| {
            Line::from(vec![
                Span::styled(
                    if index == 0 {
                        format!("  {:width$} ", "@", width = usize::from(ICON_COLS))
                    } else {
                        " ".repeat(TEXT_START)
                    },
                    Style::new().fg(OVERLAY),
                ),
                Span::styled(
                    text,
                    if index == 0 {
                        Style::new().fg(TEXT).add_modifier(Modifier::BOLD)
                    } else {
                        Style::new().fg(SUBTEXT)
                    },
                ),
            ])
        })
        .collect();
    let height = fixed.map_or(lines.len(), |_| limit) as u16;
    let [head, _, rest] = Layout::vertical([
        Constraint::Length(height),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(area);
    if app.thumbnails_on()
        && let Some(url) = app.usable_thumb(profile.icon.as_deref())
    {
        pictures.thumbs.push(ImageSpot {
            url: url.to_string(),
            area: Rect {
                x: head.x + 2,
                y: head.y,
                width: ICON_COLS,
                height: 1,
            },
        });
    }
    f.render_widget(Paragraph::new(lines), head);
    rest
}

fn draw_feed(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    pictures: &mut ListPictures,
) -> Option<Rect> {
    let waiting = app
        .reddit
        .pause()
        .map(|pause| format!("Waiting for Reddit · {}s", pause.seconds));
    let tab = app.active_tab();
    let feed = &tab.feed;
    let access = tab.subreddit().and_then(|_| feed.access_denied());
    let mut chips = vec![list_view_chip(app)];
    if access.is_none() && feed.has_new_posts(&app.config) {
        chips.insert(0, new_posts_chip(app));
    }
    let search_tab = matches!(tab.kind, TabKind::Search);
    let retry = search::list_reading_key(app, "r");
    match &feed.search {
        Some(q) => {
            if !search_tab {
                chips.push(chip(format!("🔍 {q}"), SKY, SURFACE0));
            }
            chips.push(chip(
                format!("⇅ {}", feed.search_sort.as_str()),
                SUBTEXT,
                SURFACE0,
            ));
            chips.push(chip(
                format!("◷ {}", feed.search_time.as_str()),
                SUBTEXT,
                SURFACE0,
            ));
        }
        None => {
            chips.push(sort_chip(feed.sort));
            if feed.sort.takes_time() {
                chips.push(chip(format!("◷ {}", feed.time.as_str()), SUBTEXT, SURFACE0));
            }
        }
    }
    if let Some(name) = tab
        .subreddit()
        .filter(|name| crate::reddit::is_community(name) && access.is_none())
    {
        chips.push(membership_chip(
            app,
            &crate::model::MembershipTarget::Community(name.into()),
        ));
    }
    if let TabKind::User { name, .. } = &tab.kind {
        chips.push(membership_chip(
            app,
            &crate::model::MembershipTarget::User(name.clone()),
        ));
    }
    let badge = app.badge(tab);
    if access.is_none()
        && let Some(badge) = unread_chip(badge)
    {
        chips.push(badge);
    }
    if feed.loading() && !feed.loading_more() && !feed.receiving() && !feed.posts.is_empty() {
        chips.push(chip(
            format!("{} loading", loading::glyph(app.loading_frame)),
            OVERLAY,
            SURFACE0,
        ));
    }
    if access.is_some() {
        chips.insert(0, chip("Private community", YELLOW, SURFACE0));
    }
    if access.is_none() && feed.error().is_some() && !feed.posts.is_empty() {
        let label = if feed.page_failed() {
            "✗ next page failed"
        } else {
            "✗ reload failed"
        };
        chips.splice(
            0..0,
            [
                chip(label, RED, SURFACE0),
                chip(format!("{retry} retry"), RED, SURFACE0),
            ],
        );
    }
    if let Some(freshness) = feed.freshness.filter(|_| access.is_none()) {
        chips.push(chip(
            freshness.label(
                crate::app::now(),
                (feed.loading() || feed.refreshing()) && !feed.loading_more(),
                feed.error().is_some(),
                app.health == Health::Down,
            ),
            OVERLAY,
            SURFACE0,
        ));
    }
    let title = tab.title();
    let community = tab.subreddit();
    let error = feed.error();
    let posts: Vec<&Post> = app.config.visible(&feed.posts).collect();
    let selected = feed.selected;
    let current = feed.shows_current_query();
    let offset = feed.offset;
    let scroll = feed.cards.clone();
    let mut body = if search_tab {
        search::feed_header(f, app, area, chips)
    } else {
        view_header(f, area, &title, chips)
    };
    // A fixed block, loading or not, keeps the list still when the profile arrives.
    if let TabKind::User { name, .. } = &tab.kind {
        let status;
        let profile = match app.user_profile(name) {
            Some(profile) => profile,
            None => {
                status = crate::model::UserProfile::new(
                    None,
                    &if app.profile_failed(name) {
                        format!("Profile unavailable — press {retry} to retry")
                    } else {
                        "Loading profile…".into()
                    },
                );
                &status
            }
        };
        body = draw_profile(f, app, body, profile, pictures, Some(USER_PROFILE_ROWS));
    }
    if app.reddit.needs_sign_in() && posts.is_empty() {
        empty_state(
            f,
            body,
            "○",
            "Connect Reddit",
            &format!(
                "Press {} to connect your account",
                search::reading_key(app, "SPC p a")
            ),
        );
        return None;
    }
    if access.is_some() {
        let compact = body.width < 36 || body.height < 8;
        let open = search::list_reading_key(app, "o");
        let guidance = if compact {
            format!("Approved members only · {open} Reddit")
        } else {
            format!(
                "Only approved members can view this community. Press {open} to open Reddit and check access."
            )
        };
        empty_state(f, body, "🔒", "Private community", &guidance);
        return None;
    }
    if let Some(e) = error.filter(|_| posts.is_empty()) {
        let title = if feed.page_failed() {
            "Could not load more posts"
        } else {
            "Could not load posts"
        };
        empty_state(
            f,
            body,
            "✗",
            title,
            &format!("{e} — press {retry} to retry"),
        );
        return None;
    }
    if posts.is_empty() {
        // Until the current sort or search answers, an earlier empty answer says nothing.
        if !current || feed.receiving() {
            return loading::draw(
                f,
                body,
                loading::Kind::Initial,
                waiting.as_deref().unwrap_or("Loading posts"),
                app.loading_frame,
            );
        } else if !feed.posts.is_empty() {
            empty_state(
                f,
                body,
                "∅",
                "Every loaded post is hidden",
                "F2 Settings: check Show NSFW and hidden keywords, flairs or authors",
            );
        } else if search_tab {
            empty_content_state(
                f,
                body,
                "No results",
                &search::empty_hint(app),
                app.config.show_nsfw,
            );
        } else if feed.search.is_some() {
            let edit = search::list_reading_key(app, "/");
            let clear = search::list_reading_key(app, "Esc");
            let action = format!("{edit} edits the search, {clear} clears it");
            let hint = if app.config.show_nsfw {
                action
            } else {
                format!("{NSFW_EMPTY_HINT} {action}.")
            };
            empty_content_state(f, body, "No results", &hint, app.config.show_nsfw);
        } else if !app.config.show_nsfw {
            empty_content_state(f, body, "No visible posts", NSFW_EMPTY_HINT, false);
        } else {
            let sort = search::list_reading_key(app, "SPC f s");
            let source = search::list_reading_key(app, "SPC o c");
            empty_state(
                f,
                body,
                "∅",
                "No posts here",
                &if matches!(tab.kind, TabKind::User { .. }) {
                    format!("{sort} sorts · {source} opens this profile")
                } else {
                    format!("{sort} changes sort; SPC b b opens a subreddit")
                },
            );
        }
        return None;
    }
    let show_subreddit = matches!(
        tab.kind,
        TabKind::Home | TabKind::User { .. } | TabKind::Search
    ) || matches!(community, Some("all" | "popular"));
    let mut spec = ListSpec {
        posts: &posts,
        selected,
        show_selection: true,
        show_subreddit,
        offset,
        scroll,
        append_area: None,
    };
    let last_visible = draw_post_list(f, app, body, &mut spec, pictures);
    let (selected, offset, scroll, append_area) =
        (spec.selected, spec.offset, spec.scroll, spec.append_area);
    app.set_list_viewport(selected, offset, scroll, last_visible);
    let append_area = append_area?;
    let feed = &app.active_tab().feed;
    if feed.loading_more() || feed.receiving() {
        loading::draw(
            f,
            append_area,
            loading::Kind::More,
            waiting.as_deref().unwrap_or("Loading posts"),
            app.loading_frame,
        )
    } else if feed.page_failed() {
        draw_pagination_error(
            f,
            append_area,
            &format!("✗ Next page failed · {retry} retries"),
        );
        None
    } else if let Some((label, _)) = feed_activity(feed, !app.reddit.needs_sign_in()) {
        let kind = if !feed.shows_current_query() {
            loading::Kind::Initial
        } else {
            loading::Kind::Background
        };
        loading::draw(
            f,
            append_area,
            kind,
            waiting.as_deref().unwrap_or(label),
            app.loading_frame,
        )
    } else {
        None
    }
}

fn draw_bookmarks(f: &mut Frame, app: &mut App, area: Rect, pictures: &mut ListPictures) {
    let count = app.visible_bookmarks().count();
    let body = view_header(
        f,
        area,
        "★ Bookmarks",
        vec![
            list_view_chip(app),
            chip(
                format!("{count} bookmark{}", if count == 1 { "" } else { "s" }),
                SUBTEXT,
                SURFACE0,
            ),
        ],
    );
    let posts: Vec<&Post> = app.visible_bookmarks().map(|b| &b.post).collect();
    if posts.is_empty() {
        empty_state(
            f,
            body,
            "★",
            if count == 0 && !app.state.bookmarks().is_empty() {
                "No visible bookmarks"
            } else {
                "No bookmarks yet"
            },
            if count == 0 && !app.state.bookmarks().is_empty() {
                NSFW_EMPTY_HINT
            } else {
                "press b on any post to keep it here"
            },
        );
        return;
    }
    let selected = app.active_tab().feed.selected.min(posts.len() - 1);
    let mut spec = ListSpec {
        posts: &posts,
        selected,
        show_selection: true,
        show_subreddit: true,
        offset: app.active_tab().feed.offset,
        scroll: app.active_tab().feed.cards.clone(),
        append_area: None,
    };
    let last_visible = draw_post_list(f, app, body, &mut spec, pictures);
    let (selected, offset, scroll) = (spec.selected, spec.offset, spec.scroll);
    app.set_list_viewport(selected, offset, scroll, last_visible);
}

// ----- post view -----------------------------------------------------------

/// Masks `>!…!<` spoilers as `▒`, one per column with spaces kept (same width, but break
/// points still differ, e.g. at hyphens or between CJK), or strips the markers once revealed.
fn spoiler_text(text: &str, reveal: bool) -> String {
    let text = crate::model::without_link_targets(text);
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
            for part in line.split_inclusive(crate::model::text_format) {
                let marker = part
                    .chars()
                    .last()
                    .filter(|c| crate::model::text_format(*c));
                let content = marker.map_or(part, |c| &part[..part.len() - c.len_utf8()]);
                for g in content.graphemes(true) {
                    if g.chars().all(char::is_whitespace) {
                        out.push_str(g);
                    } else {
                        out.extend(std::iter::repeat_n('▒', g.width()));
                    }
                }
                if let Some(marker) = marker {
                    out.push(marker);
                }
            }
        }
    };
    let mut rest = text.as_ref();
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
    for mut line in rich_text::wrap(text, width, style) {
        line.spans.splice(0..0, prefix.iter().cloned());
        lines.push(line);
    }
}

/// Most rows a picture in post or comment text takes.
const TEXT_PICTURE_ROWS: u16 = 12;
/// Text pictures kept loaded before those far from view are dropped: each holds its pixels
/// and their encoding. Dropped ones load again when reached.
const TEXT_PICTURES_KEPT: usize = 24;

/// What the post document needs to lay out pictures in its text: the loaded images (none
/// when images are off), and the most rows one may take, so it fits the text area whole.
struct DocumentContext<'a> {
    focused: bool,
    phase: usize,
    images: Option<&'a HashMap<String, ImageSlot>>,
    font: ratatui_image::FontSize,
    rows: u16,
}

/// A picture in the post document: its URL, first line, the columns before it and its
/// cells once it loaded.
#[derive(Clone, Debug)]
struct DocPicture {
    url: String,
    line: usize,
    x: u16,
    size: Option<Size>,
    main: bool,
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
    sizes: &DocumentContext<'_>,
) {
    let x = prefix.iter().map(|s| s.content.width()).sum::<usize>() as u16;
    let mut rest = text;
    while let Some(at) = rest.find(PICTURE) {
        let start = rest[..at].rfind('\n').map_or(0, |n| n + 1);
        let end = rest[at..].find('\n').map_or(rest.len(), |n| at + n);
        if start > 0 {
            wrap_into(lines, &rest[..start - 1], width, prefix, style);
        }
        let picture_text = crate::model::unformatted(&rest[start..end]);
        let url = picture_line(&picture_text).unwrap_or_default();
        // What stands before it on its line, a bullet or quote bar, stays before it: quote
        // bars on every row, the rest (a bullet) on the first.
        let lead = crate::model::unformatted(&rest[start..at]);
        let lead = lead.as_ref();
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
            .find_map(|p| (!p.main && p.url == url).then_some(p.size).flatten());
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
            main: false,
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
                    _ => "loading image",
                };
                let mut spans = first_prefix;
                if label == "loading image" {
                    spans.extend(loading::caption(loading::Kind::Media, label, sizes.phase).spans);
                } else {
                    spans.push(Span::styled(label, Style::new().fg(OVERLAY)));
                }
                lines.push(Line::from(spans));
            }
        }
        rest = rest.get(end + 1..).unwrap_or_default();
    }
    if !rest.is_empty() {
        wrap_into(lines, rest, width, prefix, style);
    }
}

fn thread_prefix(depth: usize, visible: usize) -> Vec<Span<'static>> {
    let mut rails: Vec<_> = (0..visible)
        .map(|d| {
            Span::styled(
                "│ ",
                Style::new().fg(THREADS[(depth - visible + d) % THREADS.len()]),
            )
        })
        .collect();
    if depth > visible {
        if let Some(first) = rails.first_mut() {
            first.content = "⋮ ".into();
        } else {
            rails.push(Span::styled("↳", Style::new().fg(OVERLAY)));
        }
    }
    rails
}

fn draw_post(f: &mut Frame, app: &mut App, area: Rect, media: &mut ListPictures) -> Option<Rect> {
    let controls = crate::keys::map::context(app) == crate::keys::map::Context::Post
        && app.help_scroll.is_none();
    let gallery_controls =
        controls && crate::keys::map::available(app, crate::keys::map::Action::NextImage);
    let view = app.post_view.as_ref()?;
    let post = view.shown_post();
    let now = Utc::now();

    // Header card: never scrolls.
    let head_width = area.width.saturating_sub(4) as usize;
    let mut header = vec![Line::from({
        let mut s = vec![
            chip(post.source_label(), CRUST, ACCENT),
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
        let mut flags = Vec::new();
        if post.nsfw {
            flags.push(chip("NSFW", CRUST, RED));
            flags.push(Span::raw(" "));
        }
        if post.spoiler {
            flags.push(chip("spoiler", CRUST, YELLOW));
            flags.push(Span::raw(" "));
        }
        if post.original_deleted || app.state.original_deleted(&post.id) {
            flags.push(chip("Original post deleted", YELLOW, SURFACE0));
            flags.push(Span::raw(" "));
        }
        flags.extend(s);
        fit_spans(flags, head_width)
    })];
    let title_style = if app.config.highlights(post) {
        Style::new().fg(YELLOW).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(TEXT).add_modifier(Modifier::BOLD)
    };
    let mut title_lines = Vec::new();
    wrap_into(&mut title_lines, &post.title, head_width, &[], title_style);
    let title_limit = area
        .height
        .saturating_div(3)
        .max(5)
        .saturating_sub(4)
        .clamp(1, 3) as usize;
    let overflow_title = (title_lines.len() > title_limit).then_some(title_style);
    if overflow_title.is_some() {
        title_lines.truncate(title_limit);
        if let Some(last) = title_lines.last_mut() {
            *last = Line::styled(truncate(&format!("{}…", last), head_width), title_style);
        }
    }
    header.extend(title_lines);
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
        Media::Gallery { images, .. } if !images.is_empty() => {
            stats.push(Span::styled(
                format!(
                    "   image {}/{}{}",
                    view.gallery_index % images.len() + 1,
                    images.len(),
                    if gallery_controls {
                        " · [i ]i browse"
                    } else {
                        ""
                    }
                ),
                Style::new().fg(TEAL),
            ));
        }
        Media::Link { url, .. } if post.media.youtube_url().is_none() => {
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
                if controls {
                    stats.push(Span::styled("  O opens", Style::new().fg(OVERLAY)));
                }
            }
        }
    }
    header.push(Line::from(fit_spans(stats, head_width)));
    let header_height = header.len() as u16 + 2;
    let [head_area, rest] =
        Layout::vertical([Constraint::Length(header_height), Constraint::Min(1)]).areas(area);
    f.render_widget(
        Paragraph::new(header).block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(SURFACE1))
                .padding(Padding::horizontal(PANEL_PAD))
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

    let video = post.media.video_url().is_some();
    let image_url = post.media.shows_inline().then(|| {
        app.post_view
            .as_ref()
            .and_then(PostView::preview_image)
            .map(std::borrow::Cow::into_owned)
    });
    // Until the post page arrives, a stale list copy may lack the spoiler tag.
    let hide_image = app
        .post_view
        .as_ref()
        .is_some_and(|v| v.spoiler_hidden() || v.page.is_none());
    let margin = Margin {
        horizontal: 1,
        vertical: 0,
    };
    let full = rest.inner(margin);
    let width = full.width.saturating_sub(2) as usize;
    let still_url = image_url.as_ref().and_then(|u| u.as_deref());
    let main_pixels = post
        .preview_size
        .filter(|_| video)
        .or_else(|| {
            still_url.and_then(|url| {
                match app.images.get(url) {
                    Some(ImageSlot::Ready { pixels, .. }) => Some(*pixels),
                    _ => app.card_image_metadata.get(url).map(|m| m.size),
                }
                .or_else(|| {
                    (!matches!(post.media, Media::Gallery { .. }) || view.gallery_index == 0)
                        .then_some(post.preview_size)
                        .flatten()
                })
            })
        })
        .filter(|&(w, h)| w > 0 && h > 0)
        .or_else(|| {
            video
                .then(|| {
                    app.video_preview(false)
                        .map(|(frame, _)| (frame.width, frame.height))
                })
                .flatten()
        })
        .or_else(|| video.then_some((16, 9)));
    let failed_without_picture = video && failed_video_without_picture(app, post, still_url);
    let main_size = main_pixels
        .filter(|_| !failed_without_picture)
        .map(|pixels| {
            fit_picture(
                pixels,
                app.image_picker.font_size(),
                Size::new(width as u16, full.height.saturating_sub(2)),
                full.height,
            )
        });
    let video_id = video.then(|| post.id.clone());
    let video_url = post.media.video_url().map(str::to_string);
    let main_url = still_url.map(str::to_string).or_else(|| video_id.clone());
    let main_image = main_url.as_deref().filter(|_| !hide_image).map(|url| {
        let size = if app.inline_images_on() && !failed_without_picture {
            main_size
        } else {
            Some(Size::new(width as u16, 0))
        };
        (url, size)
    });
    if video && app.inline_images_on() && !hide_image {
        app.video_area = main_size
            .filter(|size| size.width > 0 && size.height > 0)
            .map(|size| Rect::new(0, 0, size.width, size.height));
    }
    let video_status = video.then(|| video_status(app));
    let sizes = DocumentContext {
        focused: controls,
        phase: app.loading_frame,
        images: app.inline_images_on().then_some(&app.images),
        font: app.image_picker.font_size(),
        rows: TEXT_PICTURE_ROWS.min(full.height).max(1),
    };
    let view = app.post_view.as_mut()?;
    if view.title_in_document != overflow_title.is_some() {
        view.title_in_document = overflow_title.is_some();
        view.content_changed = true;
    }
    let mut more_lines = std::mem::take(&mut view.rendered_more);
    let key = document_key(view, width, now, &sizes, main_image, overflow_title);
    // Relative ages can wrap a header to another line count: that moves the text below
    // like an edit does. A spinner frame advancing in the same draw changes no layout.
    let clock_only = view.page.document_cache.as_ref().is_some_and(|cache| {
        cache.key.second != key.second
            && DocumentKey {
                second: key.second,
                phase: key.phase,
                ..cache.key.clone()
            } == key
    });
    let cached = view
        .page
        .document_cache
        .take()
        .filter(|cache| cache.key == key && !view.content_changed && view.rewrapped.is_none());
    let PostDocument {
        mut lines,
        spans,
        pictures,
    } = if let Some(cache) = cached {
        PostDocument {
            lines: std::mem::take(&mut view.rendered_document),
            spans: cache.spans,
            pictures: cache.pictures,
        }
    } else {
        post_document(
            view,
            width,
            now,
            Some(&mut more_lines),
            &sizes,
            main_image,
            overflow_title,
        )
    };
    // A header rewrapping as its age changes moves the rows after it, even when another
    // rewrap keeps the document's length.
    if clock_only
        && !view.rendered_document.is_empty()
        && (lines.len() != view.rendered_document.len()
            || spans
                .iter()
                .map(|(start, _)| *start)
                .ne(view.row_starts.iter().copied()))
    {
        view.content_changed = true;
    }
    view.page.document_cache = Some(CachedDocument {
        key,
        spans: spans.clone(),
        pictures: pictures.clone(),
    });
    if let Some(status) = video_status
        && let Some(picture) = pictures.iter().find(|p| p.main)
        && picture.rows() > 0
    {
        lines[picture.line + picture.rows()] = status;
    }
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
        let old_lines =
            post_document(view, width, now, None, &sizes, main_image, overflow_title).lines;
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
    let (scroll, held) = settle_scroll(view, &spans, lines.len(), full.height as usize);
    let doc_area = full;
    let main_visible = pictures.iter().any(|p| {
        p.main
            && p.rows() > 0
            && p.line < scroll + full.height as usize
            && p.line + p.rows() > scroll
    });
    view.image_shown = main_visible;
    view.scroll = scroll;
    if !held {
        view.hold_view = None;
    }
    // A hold kept at the top for the image: with no image this frame, hold the text where it is.
    if !view.image_shown
        && let Some(hold) = &mut view.hold_view
        && hold.top
        && let Some(&(start, _)) = spans.get(hold.row)
    {
        hold.top = false;
        hold.offset = scroll as isize - start as isize;
    }
    view.drawn_cursor = (!view.free_scroll).then_some(view.cursor);
    view.record_drawn_anchor();
    let height = doc_area.height as usize;
    view.page_height = height;
    let total = lines.len();
    for (row, line) in lines.iter().skip(scroll).take(height).enumerate() {
        f.buffer_mut()
            .set_line(doc_area.x, doc_area.y + row as u16, line, doc_area.width);
    }
    let primary_loader = if view.page.is_none() && view.error.is_none() {
        let occupied = total.saturating_sub(1).saturating_sub(scroll).min(height) as u16;
        loading::draw(
            f,
            Rect::new(
                doc_area.x,
                doc_area.y + occupied,
                doc_area.width,
                doc_area.height - occupied,
            ),
            loading::Kind::Initial,
            "Loading post",
            sizes.phase,
        )
    } else {
        more_lines
            .iter()
            .find(|(id, line)| {
                view.loading_more.contains(id) && *line >= scroll && *line < scroll + height
            })
            .map(|(_, line)| {
                Rect::new(
                    doc_area.x,
                    doc_area.y + (*line - scroll) as u16,
                    doc_area.width,
                    1,
                )
            })
    };
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
        if picture.main {
            let start = picture.line.max(scroll);
            let end = (picture.line + picture.rows()).min(scroll + height);
            if start < end {
                let size = picture.size.unwrap_or(Size::new(width as u16, 1));
                media.cards.push(CardPicture {
                    url: still_url.map(str::to_string),
                    area: Rect::new(
                        doc_area.x + picture.x,
                        doc_area.y + (start - scroll) as u16,
                        size.width,
                        (end - start) as u16,
                    ),
                    size,
                    skipped: (start - picture.line) as u16,
                    pixels: main_pixels.unwrap_or((1, 1)),
                    indicator: None,
                    selected: video,
                    detail: true,
                    post_id: video_id.clone(),
                    video_url: video_url.clone(),
                    video_frame: false,
                });
            }
            continue;
        }
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
    media
        .inline
        .extend(shown.into_iter().map(|(url, area)| ImageSpot { url, area }));
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
        .and_then(PostView::preview_image)
        .map(std::borrow::Cow::into_owned);
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
    app.post_drawn(!doc_area.is_empty());
    primary_loader
}

impl DocPicture {
    /// Lines the picture takes in the document.
    fn rows(&self) -> usize {
        self.size.map_or(1, |size| usize::from(size.height))
    }
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
enum DocumentImage {
    Loading,
    Ready(u32, u32),
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DocumentKey {
    focused: bool,
    width: usize,
    second: i64,
    selected: Option<usize>,
    collapsed: HashSet<String>,
    reveal: bool,
    hidden: bool,
    phase: usize,
    font: (u16, u16),
    rows: u16,
    images: Option<Vec<(String, DocumentImage)>>,
    main: Option<(String, Option<Size>)>,
    title: Option<Style>,
    error: Option<String>,
    loading_more: HashSet<String>,
}

#[derive(Debug)]
pub(crate) struct CachedDocument {
    key: DocumentKey,
    spans: Vec<(usize, usize)>,
    pictures: Vec<DocPicture>,
}

fn document_key(
    view: &PostView,
    width: usize,
    now: DateTime<Utc>,
    sizes: &DocumentContext<'_>,
    main: Option<(&str, Option<Size>)>,
    title: Option<Style>,
) -> DocumentKey {
    let images = sizes.images.map(|images| {
        let mut state: Vec<_> = images
            .iter()
            .map(|(url, slot)| match slot {
                ImageSlot::Ready { pixels, .. } => {
                    (url.clone(), DocumentImage::Ready(pixels.0, pixels.1))
                }
                ImageSlot::Failed(_) => (url.clone(), DocumentImage::Failed),
                ImageSlot::Loading => (url.clone(), DocumentImage::Loading),
            })
            .collect();
        state.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        state
    });
    DocumentKey {
        focused: sizes.focused,
        width,
        second: now.timestamp(),
        selected: (!view.free_scroll).then_some(view.cursor),
        collapsed: view.collapsed.clone(),
        reveal: view.reveal_spoilers,
        hidden: view.spoiler_hidden(),
        phase: sizes.phase,
        font: (sizes.font.width, sizes.font.height),
        rows: sizes.rows,
        images,
        main: main.map(|(url, size)| (url.into(), size)),
        title,
        error: view.error.clone(),
        loading_more: view.loading_more.clone(),
    }
}

/// Selftext, comment spans, pictures and More-row geometry for the displayed document.
fn post_document(
    view: &PostView,
    width: usize,
    now: DateTime<Utc>,
    mut more_lines: Option<&mut Vec<(String, usize)>>,
    sizes: &DocumentContext<'_>,
    main_image: Option<(&str, Option<Size>)>,
    overflow_title: Option<Style>,
) -> PostDocument {
    let mut pictures = Vec::new();
    let mut lines: Vec<Line<'static>> = Vec::new();
    if let Some(style) = overflow_title {
        wrap_into(&mut lines, &view.shown_post().title, width, &[], style);
        lines.push(Line::default());
    }
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
        if sizes.focused {
            lines.push(Line::styled("press r to retry", Style::new().fg(OVERLAY)));
        }
        return PostDocument {
            lines,
            ..PostDocument::default()
        };
    }
    let Some(page) = view.page.as_ref() else {
        lines.push(Line::raw(""));

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
            if sizes.focused {
                format!("⚠ Spoiler · zs reveals the post{comments}")
            } else {
                format!("⚠ Spoiler · post{comments} hidden")
            },
            Style::new().fg(YELLOW).add_modifier(Modifier::BOLD),
        ));
        return PostDocument {
            lines,
            ..PostDocument::default()
        };
    }
    if let Some((url, size)) = main_image {
        let picture = DocPicture {
            url: url.to_string(),
            line: lines.len(),
            x: size.map_or(0, |size| (width as u16).saturating_sub(size.width) / 2),
            size,
            main: true,
        };
        lines.extend(std::iter::repeat_n(Line::raw(""), picture.rows() + 1));
        pictures.push(picture);
    }
    if !page.body.is_empty() {
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
        let bar = Span::styled(
            selection_marker(selected, sizes.focused),
            bg.fg(if sizes.focused { ACCENT } else { OVERLAY }),
        );
        let mut prefix = vec![bar.clone()];
        let visible_depth = row.depth.min(width.saturating_sub(18) / 2);
        prefix.extend(thread_prefix(row.depth, visible_depth));
        let body_width = width.saturating_sub(visible_depth * 2 + 2).max(1);
        match &row.kind {
            RowKind::Comment { comment, hidden } => {
                let mut head = prefix.clone();
                let author_color = if comment.is_op {
                    PEACH
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
                let age = comment.age_at(now);
                if !age.is_empty() {
                    head.push(Span::styled(format!(" · {age}"), bg.fg(OVERLAY)));
                }
                if *hidden > 0 {
                    head.push(Span::styled("  ", bg));
                    head.push(chip(format!("+{hidden} folded"), YELLOW, SURFACE1));
                }
                if selected {
                    pad_to(&mut head, width, bg);
                }
                let content = head.split_off(prefix.len());
                let mut header = wrap_spans(content, width, &prefix);
                if selected {
                    for line in &mut header {
                        pad_to(&mut line.spans, width, bg);
                    }
                }
                lines.extend(header);
                // The bar spans the whole selected comment, so it shows with the header off screen.
                let mut body_prefix = vec![if selected {
                    Span::styled("▌", Style::new().fg(ACCENT))
                } else {
                    Span::raw(" ")
                }];
                body_prefix.extend(thread_prefix(row.depth, visible_depth));
                if *hidden > 0 {
                    let body = crate::model::unformatted(&spoiler_text(&comment.body, reveal))
                        .into_owned();
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
                    let mut progress = loading::compact(
                        loading::Kind::More,
                        "Loading replies",
                        body_width,
                        sizes.phase,
                    );
                    for span in &mut progress.spans {
                        span.style = span.style.patch(bg);
                    }
                    line.extend(progress.spans);
                    if selected {
                        pad_to(&mut line, width, bg);
                    }
                    lines.push(Line::from(fit_spans(line, width)));
                    let end = lines.len();
                    lines.push(Line::raw(""));
                    spans.push((start, end));
                    continue;
                } else {
                    match more.count {
                        Some(n) => {
                            format!("↳ {n} more {}", if n == 1 { "reply" } else { "replies" })
                        }
                        None => "↳ more replies".to_string(),
                    }
                };
                let text = if sizes.focused {
                    format!("{text} · Enter")
                } else {
                    text
                };
                line.push(Span::styled(text, bg.fg(SKY)));
                if selected {
                    pad_to(&mut line, width, bg);
                }
                lines.push(Line::from(fit_spans(line, width)));
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

fn video_status(app: &App) -> Line<'static> {
    use crate::keys::map::{self, Action};
    let dim = Style::new().fg(OVERLAY);
    let mut spans = vec![Span::raw(" ")];
    let controls = map::available(app, Action::Pause);
    if !controls {
        spans.push(key_chip("l"));
        spans.push(Span::styled(" return · ", dim));
    }
    let player = app.selected_video();
    match (player, player.and_then(|player| player.error())) {
        (_, Some(e)) => spans.push(Span::styled(
            format!("✗ video unavailable: {e}"),
            Style::new().fg(RED),
        )),
        (None, _) if !app.config.media.video_autoplay && controls => {
            spans.push(key_chip("p"));
            spans.push(Span::styled(" play · autoplay off", dim));
        }
        (None, _) if !app.config.media.video_autoplay => {
            spans.push(Span::styled("autoplay off", dim))
        }
        (None, _) => spans.extend(
            loading::caption(loading::Kind::Media, "loading video…", app.loading_frame).spans,
        ),
        (Some(player), None) => {
            let at = video::clock_text(player.position(Instant::now()));
            let icon = if player.paused() { "⏸" } else { "▶" };
            let loading = app.video_loading();
            let time = if let Some(label) = loading {
                label.to_string()
            } else if let Some(total) = player.duration() {
                format!("{at} / {}", video::clock_text(total))
            } else {
                at
            };
            if loading.is_some() {
                spans
                    .extend(loading::caption(loading::Kind::Media, &time, app.loading_frame).spans);
            } else {
                spans.push(Span::styled(
                    format!("{icon} {time}"),
                    Style::new().fg(TEXT),
                ));
            }
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
            for (_, key, label) in [
                (Action::Pause, "p", pause),
                (Action::Mute, "m", sound),
                (Action::SeekForward, "H/L", "seek"),
            ]
            .into_iter()
            .filter(|(action, _, _)| map::available(app, *action))
            {
                spans.push(Span::raw("   "));
                spans.push(key_chip(key));
                spans.push(Span::styled(format!(" {label}"), dim));
            }
        }
    }
    Line::from(spans)
}

fn draw_inline_image(f: &mut Frame, app: &mut App, url: &str, area: Rect, covered: Option<Rect>) {
    draw_image_slot(
        f,
        app.images.get_mut(url),
        app.image_picker.font_size(),
        covered.is_some_and(|popup| popup.intersects(area)),
        area,
        app.loading_frame,
    );
}

fn draw_image_slot(
    f: &mut Frame,
    slot: Option<&mut ImageSlot>,
    font: ratatui_image::FontSize,
    covered: bool,
    area: Rect,
    phase: usize,
) {
    // Stateful protocols cannot crop arbitrary holes; only intersecting slots wait.
    if covered {
        return;
    }
    match slot {
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
            draw_picture_status(f, area, &format!("▣ image unavailable: {e}"))
        }
        Some(ImageSlot::Loading) | None => {
            loading::draw(f, area, loading::Kind::Media, "loading image…", phase);
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
    if feed.loading_more() || feed.receiving() {
        Some(("Loading more posts…", "Loading more posts…"))
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

#[derive(Default)]
struct VisibleLoaders {
    list: bool,
    post: bool,
}

fn loading_activity(app: &App, loaders: &VisibleLoaders) -> Option<&'static str> {
    if app.media_loading() {
        return Some("media");
    }
    if app.video_loading().is_some() {
        return Some("video");
    }
    let post = app
        .post_view
        .as_ref()
        .filter(|_| !loaders.post)
        .and_then(|view| {
            if view.loading || (view.page.is_none() && view.error.is_none()) {
                Some(if view.page.is_some() {
                    "refresh"
                } else {
                    "post"
                })
            } else {
                (!view.loading_more.is_empty()).then_some("replies")
            }
        });
    let list = if !loaders.list && (app.post_view.is_none() || app.detail_fits()) {
        match app.active_tab().kind {
            TabKind::Home | TabKind::Subreddit { .. } | TabKind::User { .. } => {
                feed_activity(&app.active_tab().feed, !app.reddit.needs_sign_in())
                    .map(|(_, activity)| activity)
            }
            TabKind::Search if !app.search.shows_rows() => {
                feed_activity(&app.active_tab().feed, !app.reddit.needs_sign_in())
                    .map(|(_, activity)| activity)
            }
            TabKind::AllSubs if app.all_subs.loading => Some("subs"),
            TabKind::AllSubs if app.all_subs.groups.iter().any(|group| group.loading) => {
                Some("communities")
            }
            _ => None,
        }
    } else {
        None
    };
    let active = if app.post_focused() {
        post.or(list)
    } else {
        list.or(post)
    };
    active.or_else(|| app.images_loading().then_some("images"))
}

fn draw_footer(
    f: &mut Frame,
    app: &App,
    area: Rect,
    loaders: &VisibleLoaders,
    panes: &[(Rect, &'static str, bool)],
) {
    let progress = app.membership_progress();
    let cooldown = app
        .reddit
        .pause()
        .map(|pause| format!("Reddit paused {}s · {}", pause.seconds, pause.cause));
    let playback_error = app
        .selected_video()
        .filter(|_| app.video_controls_available())
        .and_then(|player| player.error());
    let feedback = app.status.is_some()
        || progress.is_some()
        || playback_error.is_some()
        || matches!(
            app.health,
            Health::SignedOut | Health::Down | Health::Paused
        );
    let help_width = if playback_error.is_some() && area.width < 40 {
        area.width.min(2)
    } else if area.width >= 180
        || (area.width >= 64
            && app.settings.store.persistence == crate::config::store::Persistence::Unsaved)
    {
        26
    } else if area.width >= 20 {
        10
    } else {
        2.min(area.width)
    };
    let available = usize::from(area.width.saturating_sub(help_width));
    // Reserve progress space even while idle, so hints never reflow as work changes.
    // Errors and connection/membership feedback take the full content width.
    let activity_width = if !feedback || app.status.as_ref().is_some_and(|status| !status.error) {
        (available / 3).min(24)
    } else {
        0
    };
    let content_width = available.saturating_sub(activity_width);
    let mut line = if let Some(status) = &app.status {
        let (icon, color) = if status.error {
            ("✗", RED)
        } else {
            if app.media_loading() {
                (loading::glyph(app.loading_frame), YELLOW)
            } else {
                ("✓", TEAL)
            }
        };
        Line::from(vec![
            Span::styled(
                format!(" {icon} "),
                Style::new().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if status.error
                    && (status.text.contains("Reddit rate limit reached")
                        || status.text.contains("Reddit requests paused"))
                {
                    cooldown.clone().unwrap_or_else(|| status.text.clone())
                } else {
                    status.text.clone()
                },
                Style::new().fg(color),
            ),
        ])
    } else if let Some(progress) = progress {
        Line::from(vec![Span::styled(
            format!(" {} {progress}", loading::glyph(app.loading_frame)),
            loading::style(loading::Kind::Action),
        )])
    } else if let Some(error) = playback_error {
        let handoff = if app
            .current_post()
            .is_some_and(|post| post.media.youtube_url().is_some())
        {
            "O browser"
        } else {
            "O media"
        };
        let label = if content_width < 28 {
            "✗ p retry".into()
        } else {
            format!("✗ p retry · {error} · {handoff}")
        };
        Line::from(Span::styled(label, Style::new().fg(RED)))
    } else if app.health == Health::SignedOut {
        Line::from(Span::styled(
            format!(
                " Reddit sign-in required · {} connect",
                search::reading_key(app, "SPC p a")
            ),
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
                cooldown.unwrap_or_else(|| "Reddit requests paused".into()),
                Style::new().fg(YELLOW),
            ),
        ])
    } else {
        let owned = crate::keys::footer_hints(app);
        let hints: Vec<_> = owned
            .iter()
            .map(|(key, label)| (key.as_str(), label.as_str()))
            .collect();
        // Keep whole key + label pairs; do not build pairs past the available width.
        let mut fitted = vec![Span::raw(" ")];
        let mut used = 1;
        for (key, label) in &hints {
            let pair = [
                key_chip(key),
                Span::styled(format!(" {label}  "), Style::new().fg(OVERLAY)),
            ];
            let width = line_width(&pair);
            if used + width > content_width {
                continue;
            }
            used += width;
            fitted.extend(pair);
        }
        Line::from(fitted)
    };
    line.spans = fit_spans(line.spans, content_width);
    f.render_widget(
        line,
        Rect::new(area.x, area.y, content_width as u16, area.height),
    );
    let activity = loading_activity(app, loaders).filter(|_| activity_width > 0);
    // With two or more panes visible, the idle activity slot names the focused pane.
    if activity.is_none()
        && activity_width > 0
        && panes.len() >= 2
        && let Some(&(_, name, _)) = panes.iter().find(|pane| pane.2)
    {
        f.render_widget(
            Line::from(fit_spans(
                vec![chip(name.to_uppercase(), CRUST, ACCENT)],
                activity_width,
            ))
            .alignment(Alignment::Right),
            Rect::new(
                area.x + content_width as u16,
                area.y,
                activity_width.saturating_sub(1) as u16,
                area.height,
            ),
        );
    }
    if let Some(activity) = activity {
        let kind = match activity {
            "Loading more posts…" | "replies" => loading::Kind::More,
            "images" => loading::Kind::Media,
            "media" => loading::Kind::Action,
            "refresh" | "communities" => loading::Kind::Background,
            _ => loading::Kind::Initial,
        };
        let label = match activity {
            _ if activity_width < 7 => "",
            "Loading more posts…" if activity_width < 13 => "more",
            "Loading more posts…" if activity_width < 22 => "more posts",
            _ => activity,
        };
        let activity = Span::styled(
            if activity_width < 3 {
                loading::glyph(app.loading_frame).to_string()
            } else {
                format!(" {} {label}", loading::glyph(app.loading_frame))
            },
            loading::style(kind),
        );
        f.render_widget(
            Line::from(fit_spans(vec![activity], activity_width)),
            Rect::new(
                area.x + content_width as u16,
                area.y,
                activity_width as u16,
                area.height,
            ),
        );
    }
    let help = if help_width >= 26 {
        Line::from(vec![
            key_chip("F2"),
            Span::styled(
                if app.settings.store.persistence == crate::config::store::Persistence::Unsaved {
                    " unsaved  "
                } else {
                    " settings "
                },
                Style::new().fg(SUBTEXT),
            ),
            key_chip("F1"),
            Span::styled(" help", Style::new().fg(SUBTEXT)),
        ])
    } else if help_width >= 10 {
        Line::from(vec![
            key_chip("F1"),
            Span::styled(" help", Style::new().fg(SUBTEXT)),
        ])
    } else {
        Line::from("F1")
    };
    f.render_widget(
        help,
        Rect::new(
            area.right().saturating_sub(help_width),
            area.y,
            help_width,
            area.height,
        ),
    );
}

// ----- overlays --------------------------------------------------------------

fn popup(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width.saturating_sub(if area.width >= 8 {
        POPUP_OUTER_X * 2
    } else {
        0
    }));
    let h = height.min(area.height.saturating_sub(if area.height >= 6 {
        POPUP_OUTER_Y * 2
    } else {
        0
    }));
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

fn wrapped_rows(text: &str, width: u16) -> usize {
    text.lines()
        .map(|line| textwrap::wrap(line, usize::from(width).max(1)).len().max(1))
        .sum()
}

fn fitting_label<'a>(labels: &[&'a str], width: usize) -> &'a str {
    labels
        .iter()
        .copied()
        .find(|label| label.width() <= width)
        .unwrap_or("")
}

fn enter_escape(width: usize) -> &'static str {
    fitting_label(&["Enter · Esc", "⏎ · Esc", "⏎⎋", "⎋"], width)
}

fn dialog_controls(action: &str, width: usize) -> String {
    let full = format!("Enter {action} · Esc cancel");
    if full.width() <= width {
        full
    } else {
        enter_escape(width).into()
    }
}

fn identity_lines(text: &str, width: usize, color: Color) -> Vec<Line<'static>> {
    let options = textwrap::Options::new(width.max(1))
        .word_separator(textwrap::WordSeparator::AsciiSpace)
        .word_splitter(textwrap::WordSplitter::NoHyphenation);
    textwrap::wrap(text, options)
        .into_iter()
        .map(|row| Line::styled(row.into_owned(), Style::new().fg(color)))
        .collect()
}

fn popup_block(title: &str, width: u16) -> Block<'static> {
    let title = truncate(title, usize::from(width.saturating_sub(4)));
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .style(Style::new().bg(MANTLE).fg(TEXT))
        .padding(Padding::horizontal(PANEL_PAD))
        .title(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                format!("{title} "),
                Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
        ]))
}

fn draw_picker(f: &mut Frame, app: &App, input: &str, selected: usize) -> Rect {
    let entries = app.picker_entries(input);
    let mut area = popup(f.area(), 56, entries.len().clamp(1, 12) as u16 + 6);
    let compact = area.height.saturating_sub(2) < 5;
    if compact {
        area = popup(f.area(), 56, entries.len().clamp(1, 12) as u16 + 3);
    }
    f.render_widget(Clear, area);
    let mut block = popup_block("Jump", area.width);
    if compact {
        let keys = if entries.is_empty() {
            "Esc close"
        } else {
            enter_escape(usize::from(area.width.saturating_sub(4)))
        };
        block = block.title_bottom(Line::styled(format!(" {keys} "), Style::new().fg(SUBTEXT)));
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    let [input_area, list_area, hint_area] = Layout::vertical([
        Constraint::Length(if compact { 1 } else { 3 }),
        Constraint::Min(0),
        Constraint::Length(if compact { 0 } else { 1 }),
    ])
    .areas(inner);
    let room = usize::from(input_area.width).saturating_sub(if compact { 4 } else { 6 });
    let mut input = Paragraph::new(Line::from(vec![
        Span::styled(" › ", Style::new().fg(ACCENT).bold()),
        Span::styled(input_tail(input, room), Style::new().fg(TEXT)),
        Span::styled("▏", Style::new().fg(ACCENT)),
    ]))
    .style(Style::new().bg(SURFACE0));
    if !compact {
        input = input.block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(SURFACE1)),
        );
    }
    f.render_widget(input, input_area);
    let height = list_area.height as usize;
    let start = selected.saturating_sub(height.saturating_sub(1));
    let width = list_area.width as usize;
    let lines: Vec<Line> = if entries.is_empty() {
        vec![Line::styled(
            truncate("No matching tab or valid subreddit", width),
            Style::new().fg(OVERLAY),
        )]
    } else {
        entries
            .iter()
            .enumerate()
            .skip(start)
            .take(height)
            .map(|(i, entry)| {
                let bg = choice_style(i == selected);
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
                            TabKind::Search => "⌕",
                            TabKind::Subreddit { .. } => "#",
                            TabKind::User { .. } => "@",
                        };
                        spans.push(Span::styled(format!(" {icon} "), bg.fg(OVERLAY)));
                        let badge = app.badge(&app.tabs[*index]);
                        let label = match badge {
                            UnreadBadge::Unknown => "?".into(),
                            UnreadBadge::Count(n) if n > 0 => n.to_string(),
                            _ => String::new(),
                        };
                        let badge_width = if label.is_empty() {
                            0
                        } else {
                            label.width() + 2
                        };
                        spans.push(Span::styled(
                            truncate(
                                title,
                                width.saturating_sub(line_width(&spans) + badge_width),
                            ),
                            bg.fg(TEXT),
                        ));
                        if !label.is_empty() {
                            pad_to(&mut spans, width.saturating_sub(badge_width), bg);
                            spans.push(match badge {
                                UnreadBadge::Unknown => chip(label, OVERLAY, SURFACE0),
                                _ => chip(label, CRUST, ACCENT),
                            });
                        }
                    }
                    PickerEntry::OpenUser(name) => {
                        spans.push(Span::styled(format!(" @ open u/{name}"), bg.fg(SKY)));
                    }
                    PickerEntry::Open(name) => {
                        spans.push(Span::styled(" + ", bg.fg(GREEN)));
                        spans.push(Span::styled(format!("open r/{name}"), bg.fg(GREEN)));
                    }
                }
                let mut spans = fit_spans(spans, width);
                pad_to(&mut spans, width, bg);
                Line::from(spans)
            })
            .collect()
    };
    f.render_widget(Paragraph::new(lines), list_area);
    let mut controls = Vec::new();
    for (key, label) in [("⏎", "open"), ("Esc", "close"), ("↑↓", "move")] {
        if entries.is_empty() && key != "Esc" {
            continue;
        }
        let pair = [
            key_chip(key),
            Span::styled(format!(" {label}  "), Style::new().fg(OVERLAY)),
        ];
        if line_width(&controls) + line_width(&pair) <= hint_area.width as usize {
            controls.extend(pair);
        }
    }
    let controls = if hint_area.width < 20 {
        Line::from(if entries.is_empty() {
            "Esc close"
        } else {
            enter_escape(usize::from(hint_area.width))
        })
    } else {
        Line::from(controls)
    };
    f.render_widget(controls, hint_area);
    area
}

fn draw_sort(f: &mut Frame, app: &App, selected: usize) -> Rect {
    let sorts = app.active_tab().sorts();
    let home = matches!(app.active_tab().kind, TabKind::Home);
    let area = popup(f.area(), 34, sorts.len() as u16 + 2);
    let visible = area.height.saturating_sub(2) as usize;
    let offset = selected.saturating_sub(visible.saturating_sub(1));
    f.render_widget(Clear, area);
    let lines: Vec<Line> = sorts
        .iter()
        .enumerate()
        .skip(offset)
        .take(visible)
        .map(|(i, s)| {
            let bg = choice_style(i == selected);
            let name = s.as_str();
            let mut spans = vec![
                Span::styled(if i == selected { "▌" } else { " " }, bg.fg(ACCENT)),
                Span::styled(
                    format!(" {} ", if *s == Sort::Hot { "H" } else { &name[..1] }),
                    bg.fg(LAVENDER).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                sort_chip(*s),
            ];
            if home && *s == Sort::Best {
                spans.push(Span::styled(" recommendations", bg.fg(TEXT)));
            }
            let mut spans = fit_spans(spans, area.width.saturating_sub(4) as usize);
            pad_to(&mut spans, area.width.saturating_sub(4) as usize, bg);
            Line::from(spans)
        })
        .collect();
    let block = popup_block("Sort", area.width).title_bottom(Line::styled(
        format!(
            " {} ",
            dialog_controls("apply", usize::from(area.width.saturating_sub(4)))
        ),
        Style::new().fg(SUBTEXT),
    ));
    f.render_widget(Paragraph::new(lines).block(block), area);
    area
}

fn draw_membership(f: &mut Frame, target: &crate::model::MembershipTarget, join: bool) -> Rect {
    let action = target.action(join);
    let available = popup(f.area(), 54, u16::MAX);
    let width = usize::from(available.width.saturating_sub(4)).max(1);
    let mut lines = identity_lines(&target.label(), width, TEXT);
    for line in &mut lines {
        line.style = line.style.bold();
    }
    let hint = fitting_label(
        &[
            if matches!(target, crate::model::MembershipTarget::User(_)) {
                "o open profile"
            } else {
                "o open subreddit"
            },
            "o open",
        ],
        width,
    );
    let mut area = popup(
        f.area(),
        54,
        (lines.len() + usize::from(!hint.is_empty()) + 2).min(u16::MAX as usize) as u16,
    );
    let minimum = lines.len().saturating_add(2).min(u16::MAX as usize) as u16;
    if area.height < minimum {
        area.height = f.area().height.min(minimum);
        area.y = f.area().y + (f.area().height - area.height) / 2;
    }
    let block = popup_block(&format!("{action} on Reddit"), area.width).title_bottom(Line::styled(
        format!(" {} ", dialog_controls(&action.to_ascii_lowercase(), width)),
        Style::new().fg(SUBTEXT),
    ));
    let inner = block.inner(area);
    if lines.len() > usize::from(inner.height) {
        lines.truncate(usize::from(inner.height));
        if let Some(last) = lines.last_mut() {
            *last = Line::styled(truncate(&format!("{last}…"), width), last.style);
        }
    }
    if lines.len() < usize::from(inner.height) && !hint.is_empty() {
        lines.push(Line::styled(hint, Style::new().fg(SUBTEXT)));
    }
    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new(lines).block(block), area);
    area
}

fn draw_auth(f: &mut Frame, app: &App) -> Rect {
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

fn auth_lines(app: &App, width: usize, height: usize) -> Vec<Line<'static>> {
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

fn draw_search(f: &mut Frame, app: &App, input: &str) -> Rect {
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
        .block(popup_block(&title, area.width).title_bottom(Line::styled(
            format!(
                " {} ",
                dialog_controls("search", usize::from(area.width.saturating_sub(4)))
            ),
            Style::new().fg(SUBTEXT),
        ))),
        area,
    );
    area
}

/// Input fitted to `room` columns; a longer input shows its end after "…".
fn input_tail(input: &str, room: usize) -> String {
    if room == 0 {
        return String::new();
    }
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
    use crate::reddit::{AccessDenied, Reddit};
    use crate::worker::{Msg, Pool};
    use crossterm::event::{KeyCode, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui_image::picker::Picker;

    #[test]
    fn unchanged_redraws_reuse_document_text_and_content_updates_replace_it() {
        let mut app = image_post("Stable layout marker");
        screen(&mut app, 100, 40);
        let marker_pointer = |app: &App| {
            app.post_view
                .as_ref()
                .unwrap()
                .rendered_document
                .iter()
                .flat_map(|line| &line.spans)
                .find(|span| span.content.contains("Stable layout marker"))
                .unwrap()
                .content
                .as_ptr()
        };
        let pointer = marker_pointer(&app);
        screen(&mut app, 100, 40);
        assert_eq!(
            marker_pointer(&app),
            pointer,
            "unchanged text should not be allocated and wrapped on every media draw"
        );
        let view = app.post_view.as_ref().unwrap();
        let generation = view.generation;
        let mut page = view.page.clone().unwrap();
        page.body = "Changed layout marker".into();
        app.on_msg(Msg::Post {
            generation,
            result: Ok(Box::new(page)),
        });
        assert!(screen(&mut app, 100, 40).contains("Changed layout marker"));
    }

    fn app(subscriptions: &[&str]) -> App {
        let reddit = Reddit::test_at("http://127.0.0.1:9", false);
        app_with_client(subscriptions, reddit)
    }

    fn app_with_client(subscriptions: &[&str], reddit: Reddit) -> App {
        let (pool, _rx) = Pool::start(reddit.clone(), 3, None);
        let state = crate::test_support::state_path();
        let mut app = App::new(
            Config {
                media: crate::config::MediaSettings {
                    image_preview: false,
                    ..Default::default()
                },
                ..Config::default()
            },
            pool,
            reddit,
            state,
            Picker::halfblocks(),
        );
        app.on_msg(Msg::Subscriptions {
            generation: 0,
            result: Ok((subscriptions
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<String>>())
            .into_iter()
            .map(|name| crate::model::Subscription::Community {
                name,
                icon: crate::model::SubscriptionIcon::Unknown,
            })
            .collect()),
        });
        app.config.select_account(Some("t"));
        app.settings.store.select_account(Some("t"));
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
            actual.contains("Press SPC p a to connect your account"),
            "{actual}"
        );
        assert!(!actual.contains("Loading"), "{actual}");
        assert!(loading_activity(&app, &VisibleLoaders::default()).is_none());
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
        assert!(
            actual
                .lines()
                .any(|line| line.contains("u/first") && line.contains("[active]")),
            "{actual}"
        );
        assert!(
            actual
                .lines()
                .any(|line| line.contains("u/second") && line.contains("[expired]")),
            "{actual}"
        );
        assert!(actual.contains("+ Add account"), "{actual}");
        assert!(actual.contains("Enter selects"), "{actual}");
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
        assert!(actual.contains("▌ + Add account"), "{actual}");
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
        assert!(actual.contains("▌ u/reader9"), "{actual}");
        assert!(actual.contains("Enter selects"), "{actual}");
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
        assert!(actual.contains("▌ Firefox 9"), "{actual}");
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

    fn popup_snapshot(app: &mut App, width: u16, height: u16) -> (Rect, String) {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut area = None;
        terminal.draw(|f| area = draw_overlay(f, app)).unwrap();
        (area.unwrap(), buffer_text(terminal.backend().buffer()))
    }

    #[test]
    fn popup_navigation_should_keep_collection_bounds_and_preview_divider_stable() {
        use crate::app::experience::Screen;
        use crate::reddit::experience::{Entry, Page, Query, UserFeed};
        let mut app = app(&[]);
        app.overlay = Some(Overlay::Experience);
        for query in [
            Query::User {
                name: "reader".into(),
                feed: UserFeed::Overview,
            },
            Query::Thread("post".into()),
        ] {
            app.experience.screens = vec![Screen::Browse {
                pagination: Default::default(),
                query,
                page: Page {
                    title: "u/reader · Overview".into(),
                    text: String::new(),
                    entries: (0..20)
                        .map(|index| Entry {
                            icon: Default::default(),
                            title: format!("Entry {index}"),
                            body: match index {
                                0 => "Short preview".into(),
                                1 => "Long preview with enough text to wrap.\n".repeat(12),
                                _ => String::new(),
                            },
                            target: None,
                            query: None,
                            post: None,
                        })
                        .collect(),
                    after: None,
                    url: "/user/reader".into(),
                },
                selected: 0,
                scroll: 0,
            }];
            for (width, height) in [(120, 60), (40, 20), (24, 10)] {
                let mut layouts = Vec::new();
                for selected in [0, 1, 2, 19, 0] {
                    let Screen::Browse {
                        selected: current, ..
                    } = app.experience.screens.last_mut().unwrap()
                    else {
                        unreachable!()
                    };
                    *current = selected;
                    let (area, shown) = popup_snapshot(&mut app, width, height);
                    let divider = shown
                        .lines()
                        .enumerate()
                        .filter(|(y, line)| {
                            *y > usize::from(area.y)
                                && *y + 1 < usize::from(area.bottom())
                                && line.contains("──")
                        })
                        .map(|(y, _)| y)
                        .collect::<Vec<_>>();
                    layouts.push((area, divider));
                    assert!(shown.contains(&format!("Entry {selected}")), "{shown}");
                    assert!(shown.contains("Esc"), "{shown}");
                }
                assert!(
                    layouts.windows(2).all(|pair| pair[0] == pair[1]),
                    "{width}x{height}: {layouts:?}"
                );
            }
        }
    }

    #[test]
    fn popup_navigation_should_keep_settings_list_and_description_regions_stable() {
        use crate::config::schema::SETTINGS;
        let mut app = app(&[]);
        app.show_settings();
        for (width, height) in [(100, 40), (40, 40), (40, 16)] {
            let mut layouts = Vec::new();
            for (selected, field) in SETTINGS.iter().enumerate() {
                app.settings.selected = selected;
                let (area, shown) = popup_snapshot(&mut app, width, height);
                let description = shown
                    .lines()
                    .position(|line| line.contains(&field.file_key(&app.config)))
                    .unwrap();
                layouts.push((area, description));
                assert!(shown.contains(field.label), "{shown}");
                assert!(shown.contains("Esc"), "{shown}");
            }
            assert!(
                layouts.windows(2).all(|pair| pair[0] == pair[1]),
                "{width}x{height}: {layouts:?}"
            );
        }
    }

    #[test]
    fn popup_navigation_should_keep_account_list_and_controls_stable_including_add() {
        let mut app = app(&[]);
        app.overlay = Some(Overlay::Auth);
        app.auth.phase = crate::auth::Phase::Accounts;
        app.auth.accounts = vec![
            saved_account("reader", false),
            saved_account("abcdefghijklmnopqrst", true),
        ];
        app.auth.active_id = Some("abcdefghijklmnopqrst".into());
        for (width, height) in [(90, 30), (46, 24), (24, 12), (20, 8)] {
            let mut layouts = Vec::new();
            for selected in [0, 1, 2, 0] {
                app.auth.selected = selected;
                let (area, shown) = popup_snapshot(&mut app, width, height);
                let controls = shown
                    .lines()
                    .position(|line| line.contains("Enter") || line.contains('⏎'))
                    .unwrap();
                layouts.push((area, controls));
                assert!(shown.contains("▌"), "{shown}");
                assert!(shown.contains("Esc"), "{shown}");
            }
            assert!(
                layouts.windows(2).all(|pair| pair[0] == pair[1]),
                "{width}x{height}: {layouts:?}"
            );
        }
    }

    #[test]
    fn popup_navigation_should_keep_form_fields_stable_at_caret_wrap_boundaries() {
        use crate::app::experience::Screen;
        for (width, height, value_width) in [(120, 50, 66), (40, 30, 30)] {
            let mut app = form_app("Create text post");
            let Screen::Input(form) = app.experience.screens.last_mut().unwrap() else {
                unreachable!()
            };
            for field in form.fields.iter_mut().take(2) {
                field.value = "é".repeat(value_width);
                field.cursor = field.value.len();
            }
            let mut layouts = Vec::new();
            for active in [0, 1, 2, 0] {
                let Screen::Input(form) = app.experience.screens.last_mut().unwrap() else {
                    unreachable!()
                };
                form.active = active;
                let (area, shown) = popup_snapshot(&mut app, width, height);
                let labels = shown
                    .lines()
                    .enumerate()
                    .filter(|(_, line)| {
                        line.contains("Subreddit")
                            || line.contains("Title")
                            || line.contains("Body (Markdown)")
                    })
                    .map(|(y, _)| y)
                    .collect::<Vec<_>>();
                layouts.push((area, labels));
                assert_eq!(shown.matches('▏').count(), 1, "{shown}");
            }
            assert!(
                layouts.windows(2).all(|pair| pair[0] == pair[1]),
                "{width}x{height}: {layouts:?}"
            );
        }
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

    fn description_popup(
        query: crate::reddit::experience::Query,
        summary: &str,
        bodies: &[&str],
    ) -> App {
        use crate::app::experience::Screen;
        use crate::reddit::experience::{Entry, Page};
        let mut app = app(&[]);
        app.overlay = Some(Overlay::Experience);
        app.experience.screens = vec![Screen::Browse {
            pagination: Default::default(),
            query,
            page: Page {
                title: "Collection".into(),
                text: summary.into(),
                entries: bodies
                    .iter()
                    .enumerate()
                    .map(|(index, body)| Entry {
                        icon: Default::default(),
                        title: format!("Entry {index}"),
                        body: (*body).into(),
                        target: None,
                        query: Some(crate::reddit::experience::Query::Profile(format!(
                            "reader{index}"
                        ))),
                        post: None,
                    })
                    .collect(),
                after: None,
                url: "/user/reader".into(),
            },
            selected: 0,
            scroll: 0,
        }];
        app
    }

    #[test]
    fn popup_description_footer_should_prioritize_vim_scroll_keys_at_each_width() {
        use crate::reddit::experience::Query;
        for (query, summary, bodies, scrolls) in [
            (
                Query::Profile("reader".into()),
                "",
                vec!["Description\n"; 2],
                true,
            ),
            (
                Query::Profile("reader".into()),
                "Summary\n",
                vec![""; 2],
                true,
            ),
            (Query::Rules("rust".into()), "Rules\n", vec![], true),
            (
                Query::Thread("post".into()),
                "Ignored thread summary",
                vec![""; 2],
                false,
            ),
            (Query::Profile("reader".into()), "", vec![""; 2], false),
        ] {
            let mut app = description_popup(query, summary, &bodies);
            for (width, height) in [(120, 40), (40, 16), (24, 12)] {
                let (_, shown) = popup_snapshot(&mut app, width, height);
                assert_eq!(
                    shown.contains("Ctrl+d/u")
                        || shown.contains("C-d/u")
                        || shown.contains("C-e/y"),
                    scrolls,
                    "{width}: {shown}"
                );
                assert!(shown.contains("Esc"), "{shown}");
                assert_eq!(
                    shown.contains("Enter") || shown.contains('⏎'),
                    !bodies.is_empty(),
                    "{shown}"
                );
            }
        }
    }

    #[test]
    fn popup_description_vim_keys_should_scroll_without_selecting_or_opening_feed_actions() {
        use crate::app::experience::Screen;
        use crate::reddit::experience::Query;
        let body = (0..40)
            .map(|row| format!("Line {row:02}\n"))
            .collect::<String>();
        let mut app = description_popup(
            Query::Multi {
                owner: "reader".into(),
                name: "feed".into(),
            },
            "",
            &[&body, &body],
        );
        let (area, before) = popup_snapshot(&mut app, 100, 40);
        assert!(before.contains("Line 00"));

        key(&mut app, KeyCode::Char('d'), KeyModifiers::CONTROL);
        assert!(matches!(
            app.experience.screens.last(),
            Some(Screen::Browse {
                selected: 0,
                scroll: 5,
                ..
            })
        ));
        let (after_area, after) = popup_snapshot(&mut app, 100, 40);
        assert_eq!(after_area, area);
        assert!(after.contains("Line 05"), "{after}");
        assert!(!after.contains("Line 00"), "{after}");
        assert!(app.experience.pending.is_none());

        key(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
        key(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
        assert!(matches!(
            app.experience.screens.last(),
            Some(Screen::Browse {
                selected: 0,
                scroll: 0,
                ..
            })
        ));
        key(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
        assert!(matches!(
            app.experience.screens.last(),
            Some(Screen::Browse { scroll: 10, .. })
        ));
        key(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        assert!(matches!(
            app.experience.screens.last(),
            Some(Screen::Browse {
                selected: 1,
                scroll: 0,
                ..
            })
        ));
    }

    #[test]
    fn popup_description_help_should_name_vim_scrolling_and_pending_reads_should_block_it() {
        use crate::app::experience::Screen;
        use crate::reddit::experience::{Query, Request};
        let query = Query::Profile("reader".into());
        let mut app = description_popup(query.clone(), "", &["Description"]);
        key(&mut app, KeyCode::F(1), KeyModifiers::NONE);
        let sections = help::entries(&app);
        assert!(
            sections
                .iter()
                .any(|(keys, action)| keys == "C-d / C-u" && action == "Half page down / up")
        );
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        app.experience.pending = Some((1, Request::Read { query, after: None }));

        key(&mut app, KeyCode::Char('d'), KeyModifiers::CONTROL);
        assert!(matches!(
            app.experience.screens.last(),
            Some(Screen::Browse { scroll: 0, .. })
        ));
        let (_, shown) = popup_snapshot(&mut app, 40, 16);
        assert!(
            !shown.contains("C-d/u") && !shown.contains("C-e/y"),
            "{shown}"
        );
        assert!(shown.contains("Esc"), "{shown}");
    }

    #[test]
    fn popup_description_line_keys_should_reach_text_skipped_by_large_steps_in_short_panes() {
        use crate::reddit::experience::Query;
        let body = (0..40)
            .map(|row| format!("Line {row:02}\n"))
            .collect::<String>();
        let mut app = description_popup(Query::Profile("reader".into()), "", &[&body, &body]);
        let (area, before) = popup_snapshot(&mut app, 24, 12);
        assert!(before.contains("Line 00"));
        assert!(!before.contains("Line 04"));

        key(&mut app, KeyCode::Char('e'), KeyModifiers::CONTROL);
        let (after_area, shown) = popup_snapshot(&mut app, 24, 12);
        assert_eq!(area, after_area);
        assert!(shown.contains("Line 04"), "{shown}");
        assert!(!shown.contains("Line 00"), "{shown}");
        assert!(shown.contains("C-e/y"), "{shown}");
        key(&mut app, KeyCode::Char('y'), KeyModifiers::CONTROL);
        let (_, restored) = popup_snapshot(&mut app, 24, 12);
        assert!(restored.contains("Line 00"), "{restored}");
    }

    #[test]
    fn popup_description_should_keep_scrollable_text_visible_beside_crowded_short_lists() {
        use crate::reddit::experience::Query;
        let body = (0..20)
            .map(|row| format!("Line {row:02}\n"))
            .collect::<String>();
        for (width, height, summary, bodies) in [
            (20, 8, body.as_str(), vec![""; 3]),
            (24, 10, "", vec![body.as_str(); 8]),
            (20, 7, "", vec![body.as_str(); 3]),
        ] {
            let mut app = description_popup(Query::Profile("reader".into()), summary, &bodies);
            let (area, before) = popup_snapshot(&mut app, width, height);
            assert!(before.contains("Entry 0"), "{before}");
            assert!(before.contains("Line 00"), "{width}x{height}: {before}");
            key(&mut app, KeyCode::Char('e'), KeyModifiers::CONTROL);
            let (after_area, after) = popup_snapshot(&mut app, width, height);
            assert_eq!(area, after_area);
            assert!(after.contains("Line 01"), "{after}");
            key(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
            let (selected_area, selected) = popup_snapshot(&mut app, width, height);
            assert_eq!(area, selected_area);
            assert!(selected.contains("Entry 1"), "{selected}");
            assert!(selected.contains("Line 00"), "{selected}");
        }
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
    fn short_picker_and_account_dialogs_should_fit_their_content() {
        let app = app(&[]);
        for accounts in [false, true] {
            let mut terminal = Terminal::new(TestBackend::new(120, 60)).unwrap();
            terminal
                .draw(|f| {
                    if accounts {
                        draw_auth(f, &app);
                    } else {
                        draw_picker(f, &app, "", 0);
                    }
                })
                .unwrap();
            let rows: Vec<_> = terminal
                .backend()
                .buffer()
                .content()
                .chunks(120)
                .enumerate()
                .filter(|(_, row)| row.iter().any(|cell| cell.symbol() != " "))
                .map(|(index, _)| index)
                .collect();
            assert!(rows.last().unwrap() - rows.first().unwrap() < 14);
        }
    }

    #[test]
    fn review_regression_wrapped_selected_menu_labels_should_stay_wholly_visible() {
        let mut app = app(&[]);
        app.reddit_menu();
        let Some(crate::app::experience::Screen::Menu {
            choices, selected, ..
        }) = app.experience.screens.last_mut()
        else {
            panic!("menu missing")
        };
        *selected = choices
            .iter()
            .position(|choice| choice.label.starts_with("Privacy, blocked users"))
            .unwrap();
        let actual = screen(&mut app, 40, 12);
        assert!(actual.contains("profile settings"), "{actual}");
        assert!(actual.contains("Privacy, blocked"), "{actual}");
        assert!(actual.contains("Esc back"), "{actual}");
    }

    #[test]
    fn when_membership_dialog_opens_it_should_name_the_account_action_in_a_short_terminal() {
        let name = "a_very_long_community_name_with_fifty_characters_123";
        for join in [true, false] {
            for width in [56, 80] {
                let mut app = app(&[]);
                app.overlay = Some(Overlay::Membership {
                    target: crate::model::MembershipTarget::Community(name.into()),
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
                        && shown.contains("o open subreddit"),
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
            shown.contains("Joined") && shown.contains("SPC m leave"),
            "{shown}"
        );
        app.open_subreddit("armenia");
        let shown = screen(&mut app, 120, 24);
        assert!(shown.contains("Not joined"), "{shown}");
        app.set_membership(
            crate::model::MembershipTarget::Community("armenia".into()),
            true,
        );
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
        assert!(home.contains("]s"), "{home}");

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
        assert!(islands.contains("]s"), "{islands}");
    }

    #[test]
    fn distant_island_selection_should_fit_after_more_than_u16_max_rows() {
        let mut app = app(&[]);
        app.config.all_subs_posts_per_group = 10;
        app.all_subs.groups = (0..6000)
            .map(|index| crate::app::Group {
                subreddit: format!("community{index}"),
                loading: true,
                ..Default::default()
            })
            .collect();
        app.all_subs.group = 5999;
        let mut terminal = Terminal::new(TestBackend::new(60, 40)).unwrap();

        terminal
            .draw(|f| {
                draw_all_subs(f, &mut app, f.area(), &mut Vec::new(), &[]);
            })
            .unwrap();

        assert_eq!(app.all_subs.offset, 5999);
        assert!(buffer_text(terminal.backend().buffer()).contains("community5999"));
    }

    #[test]
    fn island_lookahead_should_load_only_the_visible_and_following_grid_rows() {
        for (width, selected, remaining, visible, expected) in [
            (60, 0, 0, vec![0], vec![0, 1]),
            (180, 0, 0, vec![0, 1, 2], vec![0, 1, 2, 3, 4, 5]),
            (180, 11, 0, vec![9, 10, 11], vec![9, 10, 11]),
            (60, 0, 5, vec![0, 1], vec![0, 1, 2]),
            (
                180,
                0,
                5,
                vec![0, 1, 2, 3, 4, 5],
                vec![0, 1, 2, 3, 4, 5, 6, 7, 8],
            ),
        ] {
            let mut app = app(&["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l"]);
            app.active = 2;
            app.all_subs.group = selected;
            let height = card_height(&app) + 4 + remaining;
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();

            terminal
                .draw(|f| {
                    draw_all_subs(f, &mut app, f.area(), &mut Vec::new(), &[]);
                })
                .unwrap();

            let loading = |app: &App| {
                app.all_subs
                    .groups
                    .iter()
                    .enumerate()
                    .filter_map(|(i, group)| group.loading.then_some(i))
                    .collect::<Vec<_>>()
            };
            assert_eq!(loading(&app), visible);
            for &index in &visible {
                let name = app.all_subs.groups[index].subreddit.clone();
                let mut post = post_with(&format!("stream{index}"), Media::None);
                post.subreddit = name.clone();
                app.on_msg(Msg::GroupProgress {
                    subreddit: name,
                    generation: app.all_subs.groups[index].generation,
                    post: Box::new(post),
                    source: Default::default(),
                });
            }
            terminal
                .draw(|f| {
                    draw_all_subs(f, &mut app, f.area(), &mut Vec::new(), &[]);
                })
                .unwrap();
            let text = buffer_text(terminal.backend().buffer());
            for index in visible {
                assert!(text.contains(&format!("stream{index}")), "{text}");
            }
            let actual: Vec<_> = app
                .all_subs
                .groups
                .iter()
                .enumerate()
                .filter_map(|(i, group)| group.loading.then_some(i))
                .collect();
            assert_eq!(actual, expected);
            assert!(!app.all_subs.has_new_posts(&app.config));
        }
    }

    #[test]
    fn partial_bottom_islands_should_render_and_load_then_scroll_fully_into_selection() {
        for (width, columns) in [(60, 1), (180, 3)] {
            for remaining in [1, 5] {
                let mut app = app(&["a", "b", "c", "d", "e", "f", "g", "h", "i"]);
                for group in &mut app.all_subs.groups {
                    let mut post = post_with(&format!("post_{}", group.subreddit), Media::None);
                    post.subreddit = group.subreddit.clone();
                    group.posts = vec![post];
                }
                let height = 3 + card_height(&app) + CARD_ROW_GAP + remaining;
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();

                terminal
                    .draw(|f| {
                        draw_all_subs(f, &mut app, f.area(), &mut Vec::new(), &[]);
                    })
                    .unwrap();

                let text = buffer_text(terminal.backend().buffer());
                let name = &app.all_subs.groups[columns].subreddit;
                assert!(
                    text.lines()
                        .nth((height - remaining) as usize)
                        .unwrap()
                        .contains(&format!("#  {name}")),
                    "{text}"
                );
                if remaining == 5 {
                    assert!(text.contains(&format!("post_{name}")), "{text}");
                }
                let loading: Vec<_> = app
                    .all_subs
                    .groups
                    .iter()
                    .enumerate()
                    .filter_map(|(index, group)| group.loading.then_some(index))
                    .collect();
                assert_eq!(loading, (0..columns * 2).collect::<Vec<_>>());

                app.move_all_subs_group(columns as isize);
                terminal
                    .draw(|f| {
                        draw_all_subs(f, &mut app, f.area(), &mut Vec::new(), &[]);
                    })
                    .unwrap();

                let selected = &app.all_subs.groups[columns].subreddit;
                assert_eq!(app.all_subs.offset, 1);
                assert!(
                    buffer_text(terminal.backend().buffer()).contains(&format!("post_{selected}"))
                );
            }
        }
    }

    #[test]
    fn islands_loading_or_joining_above_keep_the_selected_island_in_place() {
        let mut app = app(&["go", "rust", "vim"]);
        app.switch_tab(crate::app::ALL_SUBS_TAB);
        let titled = |id: &str, sub: &str| {
            let mut post = post_with(id, Media::None);
            post.subreddit = sub.into();
            post.title = format!("Title {id}");
            post
        };
        let answer = |app: &App, sub: &str, ids: &[&str]| Msg::Group {
            generation: app
                .all_subs
                .groups
                .iter()
                .find(|group| group.subreddit == sub)
                .unwrap()
                .generation,
            subreddit: sub.into(),
            result: Ok(crate::model::Listing {
                posts: ids.iter().map(|id| titled(id, sub)).collect(),
                after: None,
            }),
        };
        let row = |shown: &str| shown.lines().position(|line| line.contains("Title v1"));
        app.on_msg(answer(&app, "go", &["g1"]));
        app.on_msg(answer(&app, "rust", &["r1"]));
        app.on_msg(answer(&app, "vim", &["v1", "v2", "v3"]));
        screen(&mut app, 100, 60);
        app.move_selection(2);
        assert_eq!(app.current_post().unwrap().id, "v1");
        let before = row(&screen(&mut app, 100, 60));
        assert!(before.is_some());

        app.on_msg(answer(&app, "go", &["g1", "g2", "g3"]));
        assert_eq!(row(&screen(&mut app, 100, 60)), before);

        app.on_msg(Msg::Subscriptions {
            generation: 0,
            result: Ok(["ada", "go", "rust", "vim"]
                .into_iter()
                .map(|name| crate::model::Subscription::Community {
                    name: name.into(),
                    icon: crate::model::SubscriptionIcon::Unknown,
                })
                .collect()),
        });
        let shown = screen(&mut app, 100, 60);

        assert_eq!(app.current_post().unwrap().id, "v1");
        assert_eq!(row(&shown), before, "{shown}");
    }

    #[test]
    fn the_notice_offers_r_only_while_r_would_apply_it() {
        let mut app = app(&["rust", "vim"]);
        app.switch_tab(crate::app::ALL_SUBS_TAB);
        let titled = |id: &str, sub: &str| {
            let mut post = post_with(id, Media::None);
            post.subreddit = sub.into();
            post.title = format!("Title {id}");
            post
        };
        let answer = |app: &App, sub: &str, ids: &[&str]| Msg::Group {
            generation: app
                .all_subs
                .groups
                .iter()
                .find(|group| group.subreddit == sub)
                .unwrap()
                .generation,
            subreddit: sub.into(),
            result: Ok(crate::model::Listing {
                posts: ids.iter().map(|id| titled(id, sub)).collect(),
                after: None,
            }),
        };
        app.on_msg(answer(&app, "rust", &["r1", "r2"]));
        app.on_msg(answer(&app, "vim", &["v1"]));
        app.on_msg(answer(&app, "rust", &["fresh", "r1", "r2"]));
        screen(&mut app, 200, 40);

        press_keys(&mut app, "l");
        assert!(app.post_focused());
        let reading = screen(&mut app, 200, 40);

        assert!(reading.contains("New posts"), "{reading}");
        assert!(!reading.contains("r refresh"), "{reading}");
        app.focus = Focus::List;
        assert!(screen(&mut app, 200, 40).contains("New posts · r refresh"));
    }

    #[test]
    fn island_arrivals_wait_behind_the_notice_and_r_shows_them_from_the_top() {
        let mut app = app(&["rust", "vim"]);
        app.switch_tab(crate::app::ALL_SUBS_TAB);
        let titled = |id: &str, sub: &str| {
            let mut post = post_with(id, Media::None);
            post.subreddit = sub.into();
            post.title = format!("Title {id}");
            post
        };
        let answer = |app: &App, sub: &str, posts: Vec<Post>| Msg::Group {
            generation: app
                .all_subs
                .groups
                .iter()
                .find(|group| group.subreddit == sub)
                .unwrap()
                .generation,
            subreddit: sub.into(),
            result: Ok(crate::model::Listing { posts, after: None }),
        };
        let rust = vec![titled("r1", "rust"), titled("r2", "rust")];
        app.on_msg(answer(&app, "rust", rust.clone()));
        app.on_msg(answer(&app, "vim", vec![titled("v1", "vim")]));
        screen(&mut app, 120, 40);
        app.move_selection(2);
        assert_eq!(app.current_post().unwrap().id, "v1");

        let mut refreshed = vec![titled("fresh", "rust")];
        refreshed.extend(rust);
        refreshed[1].score = Some(99);
        app.on_msg(answer(&app, "rust", refreshed));
        let pending = screen(&mut app, 120, 40);

        assert!(pending.contains("New posts · r refresh"), "{pending}");
        assert!(!pending.contains("Title fresh"), "{pending}");
        assert!(pending.contains("99"), "{pending}");
        assert_eq!(app.current_post().unwrap().id, "v1");

        press_keys(&mut app, "r");
        let applied = screen(&mut app, 120, 40);

        assert!(!applied.contains("r refresh"), "{applied}");
        assert!(applied.contains("Title fresh"), "{applied}");
        assert_eq!(app.current_post().unwrap().id, "fresh");
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
        press_keys(&mut app, " fs");
        assert!(screen(&mut app, 80, 24).contains("best"));
        press(&mut app, 'n');
        assert_eq!(app.active_tab().feed.sort, Sort::New);
        app.open_subreddit("rust");
        press_keys(&mut app, " fs");
        assert!(!screen(&mut app, 80, 24).contains("best"));
    }

    #[test]
    fn sidebar_keeps_global_navigation_fixed_while_feed_sections_scroll() {
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
        let globals = ["Home", "Bookmarks", "All subs", "Search"];
        let expected_rows = globals.map(row_of);
        assert!(!expected_rows.contains(&usize::MAX));
        app.on_msg(Msg::Subscriptions {
            generation: 0,
            result: Ok((0..24)
                .map(|i| crate::model::Subscription::Community {
                    name: format!("sub{i:02}"),
                    icon: crate::model::SubscriptionIcon::Unknown,
                })
                .collect()),
        });
        app.open_subreddit("opened");
        let sidebar = |app: &mut App, height| {
            screen(app, 120, height)
                .lines()
                .map(|line| line.chars().take(30).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n")
        };
        for height in [30, 11] {
            let actual = sidebar(&mut app, height);
            let actual_rows = globals.map(|name| {
                actual
                    .lines()
                    .position(|line| line.contains(name))
                    .unwrap_or(usize::MAX)
            });
            assert_eq!(actual_rows, expected_rows, "{actual}");
            assert!(actual.contains("opened"), "{actual}");
        }
        let actual = sidebar(&mut app, 8);
        assert!(actual.contains("opened"), "{actual}");
        app.switch_tab(crate::app::SEARCH_TAB);
        let actual = sidebar(&mut app, 8);
        assert!(actual.contains("Search"), "{actual}");
    }

    #[test]
    fn when_body_markup_wraps_it_should_style_visible_text_without_changing_column_width() {
        use crate::model::{
            LINK_TARGET, LINK_TARGET_END, TEXT_BOLD, TEXT_END, TEXT_ITALIC, TEXT_LINK,
        };
        let mut lines = Vec::new();
        wrap_into(
            &mut lines,
            &format!(
                "plain {TEXT_BOLD}bold {TEXT_ITALIC}both{TEXT_END} end{TEXT_END} {TEXT_LINK}{LINK_TARGET}https://example.org/a-long-destination{LINK_TARGET_END}link{TEXT_END}"
            ),
            12,
            &[Span::raw("│ ")],
            Style::new().fg(TEXT),
        );
        let actual: String = lines
            .iter()
            .flat_map(|line| line.spans.iter().map(|span| span.content.as_ref()))
            .collect();
        assert!(!actual.contains(crate::model::text_format), "{actual:?}");
        assert!(!actual.contains("example.org"), "{actual:?}");
        assert!(lines.iter().all(|line| line.width() <= 14), "{lines:?}");
        let both = lines
            .iter()
            .flat_map(|line| &line.spans)
            .find(|span| span.content.contains("both"))
            .unwrap();
        assert!(
            both.style
                .add_modifier
                .contains(Modifier::BOLD | Modifier::ITALIC),
            "{both:?}"
        );
        let link = lines
            .iter()
            .flat_map(|line| &line.spans)
            .find(|span| span.content.contains("link"))
            .unwrap();
        assert!(
            link.style.add_modifier.contains(Modifier::UNDERLINED),
            "{link:?}"
        );
    }

    #[test]
    fn when_account_lists_load_the_sidebar_should_show_progress_and_retain_refreshed_entries() {
        let mut app = app(&[]);
        app.active = 0;
        app.tabs[0].feed.loaded = true;
        app.all_subs.freshness = None;
        app.all_subs.loading = true;
        let sidebar = |app: &mut App| {
            screen(app, 120, 30)
                .lines()
                .map(|line| line.chars().take(28).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n")
        };

        let actual = sidebar(&mut app);
        assert!(actual.contains("SUBSCRIPTIONS"), "{actual}");
        assert!(actual.contains("FOLLOWING"), "{actual}");
        assert_eq!(actual.matches("⠋ Loading").count(), 2, "{actual}");

        app.on_msg(Msg::Subscriptions {
            generation: 0,
            result: Ok(vec![
                crate::model::Subscription::Community {
                    name: "rust".into(),
                    icon: crate::model::SubscriptionIcon::Unknown,
                },
                crate::model::Subscription::User {
                    name: "reader".into(),
                    icon: None,
                },
            ]),
        });
        let actual = sidebar(&mut app);
        assert!(
            actual.contains("rust") && actual.contains("reader"),
            "{actual}"
        );
        assert!(!actual.contains("Loading"), "{actual}");

        let rows = |actual: &str| {
            actual
                .lines()
                .position(|line| line.contains("rust"))
                .zip(actual.lines().position(|line| line.contains("reader")))
        };
        let settled = rows(&actual);
        app.all_subs.loading = true;
        let actual = sidebar(&mut app);
        assert_eq!(rows(&actual), settled, "{actual}");
        assert!(!actual.contains("Loading"), "{actual}");
        assert!(actual.contains("SUBSCRIPTIONS ⠋"), "{actual}");
        assert!(actual.contains("FOLLOWING ⠋"), "{actual}");

        app.on_msg(Msg::Subscriptions {
            generation: 0,
            result: Err("offline".into()),
        });
        let actual = sidebar(&mut app);
        assert!(
            actual.contains("rust") && actual.contains("reader"),
            "{actual}"
        );
        assert!(!actual.contains("Loading"), "{actual}");

        // An emptied section does not reappear as a placeholder while syncing.
        app.on_msg(Msg::Subscriptions {
            generation: 0,
            result: Ok(vec![crate::model::Subscription::Community {
                name: "rust".into(),
                icon: crate::model::SubscriptionIcon::Unknown,
            }]),
        });
        let settled = sidebar(&mut app);
        assert!(!settled.contains("FOLLOWING"), "{settled}");
        app.all_subs.loading = true;
        let actual = sidebar(&mut app);
        assert!(!actual.contains("Loading"), "{actual}");
        assert_eq!(rows(&actual), rows(&settled), "{actual}");
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
            original_deleted: false,
        };
        app.all_subs.groups = vec![crate::app::Group {
            subreddit: "s".into(),
            posts: vec![post.clone()],
            freshness: Some(crate::cache::Freshness {
                fetched_at: crate::app::now(),
                source: Default::default(),
            }),
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
            stickied: false,
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
        let mut source: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/post.json")).unwrap();
        source[1]["data"]["children"][0]["data"]["body_html"] = serde_json::json!(format!(
            "before<a href='https://giphy.com/gifs/x'><img src='{url}'></a>after"
        ));
        let parsed = crate::reddit::parse::parse_post_page(&source.to_string()).unwrap();
        page.comments[0].body = parsed.comments[0].replies[0].body.clone();

        let loading = screen(&mut app, 100, 40);
        assert!(loading.contains("⠋ loading image"), "{loading}");
        assert!(!loading.contains("giphy.com"), "{loading}");
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
        key(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
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
        assert!(again.contains("⠋ loading image"), "{again}");
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
        for keys in ["\u{6}", "\u{6}", "\u{6}", "l"] {
            press_keys(&mut app, keys);
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
    fn private_community_worker_result_should_show_access_guidance_in_the_feed() {
        use crate::reddit::tests::{Fixture, Reply};
        let fixture = Fixture::new(|request| {
            assert!(request.url.starts_with("/r/doit/"));
            Reply {
                status: 403,
                body: serde_json::json!({"error":403,"reason":"private","message":"Forbidden"})
                    .to_string(),
                headers: vec![],
            }
        });
        let reddit = fixture.reddit();
        let (pool, messages) = Pool::start(reddit.clone(), 3, None);
        let state = crate::test_support::state_path();
        let mut app = App::new(
            Config {
                media: crate::config::MediaSettings {
                    image_preview: false,
                    ..Default::default()
                },
                ..Config::default()
            },
            pool,
            reddit,
            state,
            Picker::halfblocks(),
        );
        app.open_subreddit("doit");
        app.on_msg(
            messages
                .recv_timeout(std::time::Duration::from_secs(30))
                .unwrap(),
        );

        let actual = screen(&mut app, 100, 24);

        assert!(actual.contains("Private community"), "{actual}");
        assert!(actual.contains("approved members"), "{actual}");
        assert!(!actual.contains("Could not load posts"), "{actual}");
        assert!(!actual.contains("press r to retry"), "{actual}");
    }

    #[test]
    fn private_community_cached_feed_should_hide_unavailable_posts_and_keep_recovery_visible() {
        for (width, height) in [(24, 10), (40, 12), (100, 24)] {
            let mut app = denied_community(crate::reddit::AccessDenied::PrivateCommunity);
            app.show_sidebar = false;
            let actual = screen(&mut app, width, height);
            let words = actual.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(words.contains("Private community"), "{actual}");
            assert!(
                words.contains("o Reddit") || words.contains("o to open Reddit"),
                "{actual}"
            );
            assert!(
                !actual.contains("failed") && !actual.contains("retry"),
                "{actual}"
            );
            assert!(!app.active_tab().feed.posts.is_empty(), "cache is retained");
            assert!(
                app.current_post().is_none(),
                "hidden cached posts cannot be acted on"
            );
        }
    }

    fn denied_community(access: AccessDenied) -> App {
        let mut app = card_feed_app(Picker::halfblocks());
        app.on_msg(Msg::Feed {
            tab: app.active_tab().id,
            generation: app.active_tab().feed.generation,
            append: false,
            result: Err(crate::worker::FeedError::Access(access)),
        });
        app
    }

    #[test]
    fn private_community_home_denial_should_keep_the_generic_home_failure() {
        let mut app = app(&[]);
        app.switch_tab(0);
        app.on_msg(Msg::Feed {
            tab: app.active_tab().id,
            generation: app.active_tab().feed.generation,
            append: false,
            result: Err(crate::worker::FeedError::Access(AccessDenied::Forbidden)),
        });

        assert!(app.status.as_ref().is_some_and(|status| status.error));
        let actual = screen(&mut app, 100, 24);
        assert!(actual.contains("Could not load posts"), "{actual}");
        assert!(!actual.contains("Private community"), "{actual}");
    }

    #[test]
    fn unclassified_request_denial_should_keep_cached_posts_and_offer_retry_when_empty() {
        let mut app = denied_community(AccessDenied::Forbidden);
        let actual = screen(&mut app, 100, 24);
        assert!(app.current_post().is_some(), "{actual}");
        assert!(
            !actual.contains("Access restricted") && !actual.contains("Private community"),
            "{actual}"
        );
        let current = app.active;
        app.tabs[current].feed.posts.clear();
        let actual = screen(&mut app, 100, 24);
        assert!(
            actual.contains("Could not load posts") && actual.contains("retry"),
            "{actual}"
        );
        assert!(
            !actual.contains("Access restricted") && !actual.contains("Private community"),
            "{actual}"
        );
    }

    #[test]
    fn private_community_island_should_show_approval_guidance_with_cached_posts() {
        let mut app = app(&["doit"]);
        app.all_subs.groups[0].posts = (0..3)
            .map(|i| post_with(&format!("cached{i}"), Media::None))
            .collect();
        app.all_subs.post = 2;
        app.on_msg(Msg::Group {
            subreddit: "doit".into(),
            generation: app.all_subs.groups[0].generation,
            result: Err(crate::worker::FeedError::Access(
                AccessDenied::PrivateCommunity,
            )),
        });
        let actual = screen(&mut app, 100, 24);
        assert!(
            actual.contains("Private community") && actual.contains("Approved members only"),
            "{actual}"
        );
        assert!(
            !actual.contains("r retry") && !actual.contains("cached"),
            "{actual}"
        );
        assert!(app.current_post().is_none());
    }

    #[test]
    fn private_community_help_should_prioritize_access_actions() {
        let mut app = denied_community(AccessDenied::PrivateCommunity);
        press(&mut app, '?');
        let actual = screen(&mut app, 100, 14);
        assert!(actual.contains("Feed"), "{actual}");
        assert!(
            actual.contains("Open the current item on Reddit"),
            "{actual}"
        );
        assert!(actual.contains("Reload / retry"), "{actual}");
    }

    #[cfg(unix)]
    #[test]
    fn private_community_browser_key_should_open_the_community_without_a_selected_post() {
        let mut app = denied_community(AccessDenied::PrivateCommunity);
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("opened-url");
        app.config.commands.browser = vec![
            "/bin/sh".into(),
            "-c".into(),
            "printf '%s' \"$1\" > \"$0\"".into(),
            file.to_string_lossy().into_owned(),
        ];
        press_keys(&mut app, "o");
        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        let actual = loop {
            if let Ok(text) = std::fs::read_to_string(&file)
                && !text.is_empty()
            {
                break text;
            }
            assert!(
                Instant::now() < deadline,
                "native browser probe did not receive a URL"
            );
            std::thread::yield_now();
        };
        assert_eq!(actual, "https://www.reddit.com/r/s/");
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
            preview: None,
        };
        let view = app.post_view.as_mut().unwrap();
        view.post.media = image.clone();
        let page = view.page.as_mut().unwrap();
        page.post.media = image;
        page.body = body.into();
        app
    }

    #[test]
    fn main_media_should_fit_the_usable_height_in_cards_and_detail() {
        for (width, height) in [(80, 20), (100, 40), (200, 20)] {
            for (pixels, video) in [
                ((320, 1280), false),
                ((1080, 1920), true),
                ((1600, 900), false),
            ] {
                let mut app = card_feed_app(kitty());
                app.tabs[4].feed.posts.truncate(1);
                let post = &mut app.tabs[4].feed.posts[0];
                post.preview_size = Some(pixels);
                if video {
                    post.media = Media::Video {
                        gif: false,
                        url: Some(VIDEO.into()),
                        poster: None,
                    };
                }
                let mut pictures = ListPictures::default();
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal
                    .draw(|f| {
                        draw_feed(f, &mut app, f.area(), &mut pictures);
                    })
                    .unwrap();
                let picture = &pictures.cards[0];
                assert_eq!(
                    picture.area.as_size(),
                    picture.size,
                    "card {pixels:?} in {width}x{height}"
                );
                assert_eq!(picture.skipped, 0);
                assert!(
                    usize::from(picture.size.height) * 5
                        <= app.active_tab().feed.cards.viewport * 3,
                    "card media must stay within 60% of its viewport"
                );
                assert!(app.active_tab().feed.cards.height <= app.active_tab().feed.cards.viewport);

                let mut app = if video {
                    open_video_post()
                } else {
                    image_post("caption")
                };
                app.post_view
                    .as_mut()
                    .unwrap()
                    .page
                    .as_mut()
                    .unwrap()
                    .post
                    .preview_size = Some(pixels);
                let pictures = detail_pictures(&mut app, width, height);
                let picture = &pictures[0];
                assert_eq!(
                    picture.area.as_size(),
                    picture.size,
                    "detail {pixels:?} in {width}x{height}"
                );
                assert_eq!(picture.skipped, 0);
                assert!(
                    usize::from(picture.size.height) * 5
                        <= app.post_view.as_ref().unwrap().page_height * 3,
                    "detail media must stay within 60% of its viewport"
                );
                assert!(
                    picture.area.bottom() < height,
                    "leave space for status/spacing"
                );
                assert!(picture.area.right() <= width);
                if video {
                    assert_eq!(app.video_area.unwrap().as_size(), picture.size);
                }
            }
        }
    }

    #[test]
    fn fitted_media_status_should_wrap_in_a_narrow_portrait_box() {
        let mut terminal = Terminal::new(TestBackend::new(16, 8)).unwrap();
        terminal
            .draw(|f| draw_picture_status(f, Rect::new(1, 1, 12, 6), "image unavailable: offline"))
            .unwrap();
        let text = buffer_text(terminal.backend().buffer());
        assert!(text.contains("offline"), "{text}");
        assert!(text.contains("unavailable"), "{text}");
    }

    #[test]
    fn video_width_regression_portrait_detail_should_fit_height_and_scroll_with_content() {
        let mut app = open_video_post();
        app.post_view
            .as_mut()
            .unwrap()
            .page
            .as_mut()
            .unwrap()
            .post
            .preview_size = Some((1080, 1920));

        let first = detail_pictures(&mut app, 80, 30);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].size, Size::new(17, 15));
        assert_eq!(first[0].area.width, 17);

        app.scroll_post(10);
        let scrolled = detail_pictures(&mut app, 80, 30);
        assert_eq!(scrolled[0].skipped, 9);
        assert_eq!(scrolled[0].size, first[0].size);
        assert!(app.post_view.as_ref().unwrap().image_shown);
    }

    #[test]
    fn video_width_regression_unknown_portrait_should_use_the_first_frame_shape() {
        let mut app = open_video_post();
        let url = app.reddit.media_url(VIDEO).unwrap();
        app.video = Some(video::Player::showing(
            &url,
            video::Frame {
                width: 180,
                height: 320,
                pixels: video::Pixels::Raw(std::sync::Arc::new(image::DynamicImage::new_rgb8(
                    180, 320,
                ))),
                same_as_first: false,
            },
        ));

        let actual = detail_pictures(&mut app, 80, 30);

        assert_eq!(actual[0].size, Size::new(17, 15));
        assert_eq!(actual[0].pixels, (180, 320));
    }

    #[test]
    fn video_width_regression_tall_detail_should_keep_controls_in_the_footer() {
        let mut app = open_video_post();
        app.post_view
            .as_mut()
            .unwrap()
            .page
            .as_mut()
            .unwrap()
            .post
            .preview_size = Some((1080, 1920));
        let url = app.reddit.media_url(VIDEO).unwrap();
        let mut player = video::Player::showing(
            &url,
            video::Frame {
                width: 180,
                height: 320,
                pixels: video::Pixels::Raw(std::sync::Arc::new(image::DynamicImage::new_rgb8(
                    180, 320,
                ))),
                same_as_first: false,
            },
        );
        if !player.paused() {
            player.toggle_pause(Instant::now());
        }
        app.video = Some(player);

        let actual = screen(&mut app, 100, 20);

        let footer = actual
            .lines()
            .last()
            .unwrap()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(footer.contains("p play"), "{actual}");
        assert!(footer.contains("m sound"), "{actual}");
    }

    #[test]
    fn video_width_regression_tall_detail_failure_should_keep_retry_in_the_footer() {
        let mut app = open_video_post();
        app.post_view
            .as_mut()
            .unwrap()
            .page
            .as_mut()
            .unwrap()
            .post
            .preview_size = Some((1080, 1920));
        let url = app.reddit.media_url(VIDEO).unwrap();
        app.video = Some(video::Player::unavailable(&url, Size::new(70, 63)));

        let actual = screen(&mut app, 100, 20);

        assert!(
            actual.lines().last().unwrap().contains("p retry"),
            "{actual}"
        );
    }

    #[test]
    fn media_failure_without_a_poster_or_frame_should_not_reserve_a_blank_picture() {
        let mut app = open_video_post();
        let url = app.reddit.media_url(VIDEO).unwrap();
        app.video = Some(video::Player::unavailable(&url, Size::new(70, 63)));

        let pictures = detail_pictures(&mut app, 100, 50);
        assert!(pictures.iter().all(|p| p.size.height == 0));
        assert!(app.video_area.is_none());
        let actual = screen(&mut app, 100, 50);
        assert!(
            actual.lines().last().unwrap().contains("p retry"),
            "{actual}"
        );
    }

    #[test]
    fn media_failure_from_an_old_source_should_not_hide_the_current_video() {
        let mut app = open_video_post();
        let url = app.reddit.media_url(VIDEO).unwrap();
        app.video = Some(video::Player::unavailable(&url, Size::new(70, 63)));
        let media = &mut app
            .post_view
            .as_mut()
            .unwrap()
            .page
            .as_mut()
            .unwrap()
            .post
            .media;
        if let Media::Video { url, .. } = media {
            *url = Some("https://v.redd.it/current/video.mp4".into());
        }

        let pictures = detail_pictures(&mut app, 100, 50);

        assert!(pictures.iter().any(|p| p.size.height > 0));
        assert!(app.video_area.is_some());
    }

    #[test]
    fn video_width_regression_split_detail_should_leave_the_list_poster_static() {
        let mut picker = kitty();
        picker.set_protocol_type(ProtocolType::Iterm2);
        let mut app = card_feed_app(picker);
        let poster = "https://i.redd.it/p1.jpeg";
        app.tabs[4].feed.posts.truncate(1);
        app.tabs[4].feed.posts[0].media = Media::Video {
            gif: false,
            url: Some(VIDEO.into()),
            poster: Some(poster.into()),
        };
        app.tabs[4].feed.posts[0].preview_size = Some((160, 90));
        app.card_images.insert(
            poster.into(),
            CardImageSlot::Ready {
                image: std::sync::Arc::new(image::DynamicImage::ImageRgb8(
                    image::RgbImage::from_pixel(160, 90, image::Rgb([0, 0, 255])),
                )),
                protocol: None,
            },
        );
        screen(&mut app, 200, 40);
        app.open_current_post();
        let post = app.post_view.as_ref().unwrap().post.clone();
        app.post_view.as_mut().unwrap().page = Some(PostPage {
            post,
            body: String::new(),
            comments: vec![],
        })
        .into();
        let url = app.reddit.media_url(VIDEO).unwrap();
        app.video = Some(video::Player::showing(
            &url,
            video::Frame {
                width: 160,
                height: 90,
                pixels: video::Pixels::Raw(std::sync::Arc::new(image::DynamicImage::ImageRgb8(
                    image::RgbImage::from_pixel(160, 90, image::Rgb([255, 0, 0])),
                ))),
                same_as_first: false,
            },
        ));

        let actual = screen(&mut app, 200, 40);

        use base64::Engine;
        let images: Vec<_> = actual
            .split("doNotMoveCursor=1:")
            .skip(1)
            .map(|chunk| {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(chunk.split('\u{7}').next().unwrap())
                    .unwrap();
                image::load_from_memory(&bytes).unwrap().to_rgb8()
            })
            .collect();
        assert!(
            images
                .iter()
                .any(|i| i.pixels().any(|p| p.0 == [0, 0, 255])),
            "list poster must stay blue"
        );
        assert!(
            images
                .iter()
                .any(|i| i.pixels().any(|p| p.0 == [255, 0, 0])),
            "detail video must be red"
        );
    }

    #[test]
    fn a_tall_detail_image_should_fit_below_the_fixed_header() {
        let mut app = image_post("body below the full image");
        let slot = ready(&app, 320, 1280);
        app.images.insert("https://i.redd.it/big.jpeg".into(), slot);

        let shown = screen(&mut app, 100, 40);
        let view = app.post_view.as_ref().unwrap();

        assert!(shown.contains("body below the full image"));
        assert_eq!(view.rendered_pictures.first().map(|p| p.2), Some(19));
        assert_eq!(view.scroll, 0);
    }

    fn detail_pictures(app: &mut App, width: u16, height: u16) -> Vec<CardPicture> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut pictures = ListPictures::default();
        terminal
            .draw(|f| {
                draw_post(f, app, f.area(), &mut pictures);
            })
            .unwrap();
        pictures.cards
    }

    #[test]
    fn a_detail_image_should_clip_visible_rows_and_scroll_continuously_into_the_body() {
        let mut app = image_post(&paragraphs("body after image", 1..=20));
        let slot = ready(&app, 320, 1280);
        app.images.insert("https://i.redd.it/big.jpeg".into(), slot);
        let first = detail_pictures(&mut app, 80, 30);
        assert_eq!(first[0].size, Size::new(8, 15));
        assert_eq!(first[0].skipped, 0);
        assert_eq!(first[0].area.width, 8);

        app.scroll_post(6);
        let middle = detail_pictures(&mut app, 80, 30);
        assert_eq!(middle[0].skipped, 5);
        assert_eq!(
            middle[0].area.height,
            first[0].size.height - middle[0].skipped
        );
        assert_eq!(app.post_view.as_ref().unwrap().scroll, 6);

        app.scroll_post(6);
        let end = detail_pictures(&mut app, 80, 30);
        assert_eq!(end[0].skipped, 11);
        assert_eq!(end[0].area.height, 4);
        assert_eq!(app.post_view.as_ref().unwrap().scroll, 12);
        app.scroll_post(-6);
        let back = detail_pictures(&mut app, 80, 30);
        assert_eq!(back[0].skipped, 5);
    }

    #[test]
    fn a_detail_image_resize_should_keep_the_body_being_read_in_place() {
        let mut app = image_post(&paragraphs("body", 1..=20));
        let slot = ready(&app, 320, 1280);
        app.images.insert("https://i.redd.it/big.jpeg".into(), slot);
        detail_pictures(&mut app, 80, 30);
        app.scroll_post(26);
        detail_pictures(&mut app, 80, 30);
        let old = app.post_view.as_ref().unwrap().scroll;

        detail_pictures(&mut app, 100, 40);

        let view = app.post_view.as_ref().unwrap();
        assert_eq!(view.rendered_pictures[0].2, 21);
        assert_eq!(view.scroll, old + 6);
    }

    #[test]
    fn a_loading_detail_image_should_reserve_known_geometry_and_keep_it_after_failure() {
        let mut app = image_post("body after image");
        app.post_view
            .as_mut()
            .unwrap()
            .page
            .as_mut()
            .unwrap()
            .post
            .preview_size = Some((320, 1280));
        let first = detail_pictures(&mut app, 80, 30);
        assert_eq!(first[0].size, Size::new(8, 15));
        app.images.insert(
            "https://i.redd.it/big.jpeg".into(),
            ImageSlot::Failed("offline".into()),
        );
        let failed = detail_pictures(&mut app, 80, 30);
        assert_eq!(failed[0].size, first[0].size);

        app.post_view.as_mut().unwrap().page = None.into();
        assert!(detail_pictures(&mut app, 80, 30).is_empty());
    }

    #[test]
    fn main_and_embedded_copies_of_a_detail_image_should_have_independent_sizes() {
        let mut app = image_post(&format!("{PICTURE}https://i.redd.it/big.jpeg"));
        let slot = ready(&app, 320, 1280);
        app.images.insert("https://i.redd.it/big.jpeg".into(), slot);
        detail_pictures(&mut app, 80, 30);

        let pictures = &app.post_view.as_ref().unwrap().rendered_pictures;
        assert_eq!(pictures.len(), 2);
        assert_eq!(pictures[0].2, 15);
        assert_eq!(pictures[1].2, TEXT_PICTURE_ROWS as usize);
    }

    #[test]
    fn a_detail_gallery_should_use_each_selected_images_own_shape_and_hide_spoilers() {
        let mut app = image_post("gallery caption");
        let media = Media::Gallery {
            thumb: None,
            previews: Vec::new(),
            images: vec![
                "https://i.redd.it/big.jpeg".into(),
                "https://i.redd.it/wide.jpeg".into(),
            ],
        };
        app.post_view
            .as_mut()
            .unwrap()
            .page
            .as_mut()
            .unwrap()
            .post
            .media = media;
        for (url, w, h) in [("big", 320, 1280), ("wide", 1280, 320)] {
            let slot = ready(&app, w, h);
            app.images
                .insert(format!("https://i.redd.it/{url}.jpeg"), slot);
        }
        assert_eq!(detail_pictures(&mut app, 80, 30)[0].size.height, 15);
        app.scroll_post(20);
        app.gallery_step(true);
        let next = detail_pictures(&mut app, 80, 30);
        assert_eq!(next[0].url.as_deref(), Some("https://i.redd.it/wide.jpeg"));
        assert_eq!(next[0].size, Size::new(76, 10));
        assert_eq!(app.post_view.as_ref().unwrap().scroll, 0);
        app.post_view
            .as_mut()
            .unwrap()
            .page
            .as_mut()
            .unwrap()
            .post
            .spoiler = true;
        assert!(detail_pictures(&mut app, 80, 30).is_empty());
        app.toggle_spoilers();
        assert_eq!(detail_pictures(&mut app, 80, 30)[0].size.height, 10);
    }

    #[test]
    fn detail_comment_navigation_should_move_past_the_full_image_and_return_to_it() {
        let mut app = image_post("short");
        let slot = ready(&app, 320, 1280);
        app.images.insert("https://i.redd.it/big.jpeg".into(), slot);
        detail_pictures(&mut app, 80, 30);
        app.activate_comment();
        detail_pictures(&mut app, 80, 30);
        let view = app.post_view.as_ref().unwrap();
        assert!(!view.free_scroll);
        assert_eq!(
            view.scroll, 0,
            "the first comment already fits below the image"
        );
        assert!(view.row_starts[view.cursor] > view.rendered_pictures[0].2);
        assert!(view.row_starts[view.cursor] < view.scroll + view.page_height);
        press_keys(&mut app, "gg");
        let top = detail_pictures(&mut app, 80, 30);
        assert_eq!(top[0].skipped, 0);
    }

    #[test]
    fn the_whole_detail_image_should_render_in_its_first_viewport() {
        let mut app = image_post("body below image");
        screen(&mut app, 100, 40);
        app.on_msg(Msg::Image {
            url: "https://i.redd.it/big.jpeg".into(),
            result: Ok(image::DynamicImage::new_rgb8(320, 1280).into()),
        });
        screen(&mut app, 100, 40);

        let cells = placeholder_cells(&mut app, 100, 40);

        assert_eq!(cells.len(), 10 * 19);
        assert_eq!(app.post_view.as_ref().unwrap().scroll, 0);
    }

    #[test]
    fn toggling_a_detail_image_should_keep_body_and_comments_at_the_same_position() {
        for comments in [false, true] {
            let mut app = image_post_with_comments(37);
            if !comments {
                app.post_view.as_mut().unwrap().page.as_mut().unwrap().body =
                    "repeated body paragraph\n\n".repeat(80);
            }
            let slot = ready(&app, 320, 1280);
            app.images.insert("https://i.redd.it/big.jpeg".into(), slot);
            detail_pictures(&mut app, 80, 30);
            let view = app.post_view.as_mut().unwrap();
            view.scroll = if comments {
                view.row_starts[10] + 1
            } else {
                160
            };
            detail_pictures(&mut app, 80, 30);
            let view = app.post_view.as_ref().unwrap();
            let before = view.rendered_document[view.scroll].to_string();
            let scroll = view.scroll;

            app.toggle_images();
            assert!(detail_pictures(&mut app, 80, 30).is_empty());
            let view = app.post_view.as_ref().unwrap();
            assert_eq!(view.rendered_document[view.scroll].to_string(), before);
            assert_eq!(view.scroll, scroll - 15);

            app.toggle_images();
            detail_pictures(&mut app, 80, 30);
            let view = app.post_view.as_ref().unwrap();
            assert_eq!(view.rendered_document[view.scroll].to_string(), before);
            assert_eq!(view.scroll, scroll);
        }
    }

    #[test]
    fn a_short_detail_image_should_stay_when_page_scrolling_clamps_at_the_top() {
        let mut app = image_post("short");
        let at_top = screen(&mut app, 100, 40);
        key(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
        let paged = screen(&mut app, 100, 40);
        let redrawn = screen(&mut app, 100, 40);

        assert!(at_top.contains("loading image"));
        assert!(paged.contains("loading image"));
        assert_eq!(paged, redrawn);
        assert_eq!(app.post_view.as_ref().unwrap().scroll, 0);
    }

    #[test]
    fn when_a_post_image_is_small_the_text_should_start_right_below_it() {
        // A 140×73 image fills 68 columns of 8×16 cells: 18 picture rows.
        let mut app = image_post("first line of the body");
        let slot = ready(&app, 140, 73);
        app.images.insert("https://i.redd.it/big.jpeg".into(), slot);

        let s = screen(&mut app, 100, 50);

        let row = |text: &str| s.lines().position(|l| l.contains(text)).unwrap();
        let gap = row("first line of the body")
            - s.lines()
                .position(|line| line.contains('╰') || line.contains('╚'))
                .unwrap();
        // 18 picture rows plus three rows of spacing.
        assert_eq!(gap, 21, "rows between header and text");
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
        let placement = |w, h| {
            let mut app = open_video_post();
            let frame = video::Frame {
                width: w,
                height: h,
                pixels: video::Pixels::Raw(std::sync::Arc::new(image::DynamicImage::new_rgb8(
                    w, h,
                ))),
                same_as_first: false,
            };
            let url = app.reddit.media_url(VIDEO).unwrap();
            app.video = Some(video::Player::showing(&url, frame));
            detail_pictures(&mut app, 100, 40)[0].size
        };

        assert_eq!(placement(160, 90), Size::new(75, 21));
        assert_eq!(placement(1280, 720), Size::new(75, 21));
    }

    #[test]
    fn a_wide_video_should_fit_the_height_when_width_would_overflow() {
        let mut app = open_video_post();
        app.post_view
            .as_mut()
            .unwrap()
            .page
            .as_mut()
            .unwrap()
            .post
            .preview_size = Some((800, 600));
        let actual = detail_pictures(&mut app, 200, 40);
        assert_eq!(actual[0].size, Size::new(56, 21));
        assert_eq!(actual[0].area.width, 56);
        assert_eq!(actual[0].area.height, actual[0].size.height);
        assert_eq!(actual[0].area.x, 71);
    }

    #[test]
    fn the_image_waits_for_the_post_page() {
        let shown = std::cell::RefCell::new(String::new());
        open_long_post(|app| {
            app.image_picker = kitty();
            app.show_images = true;
            app.post_view.as_mut().unwrap().post.media = Media::Image {
                url: "https://i.redd.it/big.jpeg".into(),
                preview: None,
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
        press_keys(&mut app, "za");
        let loading = footer(&mut app);
        press(&mut app, 'k');
        press_keys(&mut app, "za");
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
        press_keys(&mut app, "za");
        let s = screen(&mut app, 100, 30);

        assert!(s.contains("+3 folded"), "{s}");
        assert!(!s.contains("more replies"), "{s}");
    }

    #[test]
    fn footer_hints_follow_the_post_and_the_tab() {
        let footer = |app: &mut App| {
            app.status = None;
            let s = screen(app, 200, 30);
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
        assert!(plain.contains(" bookmark"), "{plain}");
        assert!(saved.contains("unbookmark"), "{saved}");
        assert!(
            opened.contains("time") && opened.contains("close"),
            "{opened}"
        );
        assert!(
            searching.contains("time") && searching.contains("close"),
            "{searching}"
        );
        feed.search("neovim".into());
        let header = screen(&mut feed, 120, 6);
        assert!(
            header.contains("neovim") && header.contains("⇅ relevance") && header.contains("◷ all"),
            "{header}"
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
        for keys in ["zs", "j", "j", "j", "h", "l"] {
            press_keys(&mut app, keys);
            screen(&mut app, 100, 60);
        }
        let view = app.post_view.as_ref().unwrap();
        let held = (view.cursor, view.hold_view.is_some(), view.scroll);

        // Hiding the spoilers again adds lines above the held comment.
        press_keys(&mut app, "zs");
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
        press_keys(&mut app, "zi");
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
        press_keys(&mut app, "zi");
        screen(&mut app, 100, 60);

        press_keys(&mut app, "zi");
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
        press_keys(&mut app, "zs");
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
            key(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
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
        press_keys(&mut app, "zs");
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
            preview: None,
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
    fn a_header_age_rewrapping_above_keeps_the_text_being_read() {
        let opened = |age: i64, width: u16| {
            let (mut app, _) = open_long_post(|_| {});
            let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
            page.comments[0].body = paragraphs("line", 1..=40);
            page.comments[0].created = Some(Utc::now() - chrono::Duration::seconds(age));
            screen(&mut app, width, 30);
            app
        };
        let len = |app: &App| app.post_view.as_ref().unwrap().rendered_document.len();
        // A width where "just now" wraps the header and "1m ago" does not.
        let width = (40..140)
            .find(|&width| len(&opened(10, width)) > len(&opened(120, width)))
            .expect("some width rewraps the header");
        let shown = |s: &str, needle: &str| {
            s.lines()
                .enumerate()
                .find_map(|(row, l)| l.split(needle).nth(1).map(|r| (row, r.to_string())))
        };
        let created = Utc::now() - chrono::Duration::seconds(58);
        let mut apps: Vec<_> = (0..2)
            .map(|_| {
                let (mut app, _) = open_long_post(|_| {});
                let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
                page.comments[0].body = paragraphs("line", 1..=40);
                page.comments[0].created = Some(created);
                let mut second = page.comments[0].clone();
                second.id = "second".into();
                second.body = "short".into();
                second.created = None;
                page.comments.push(second);
                screen(&mut app, width, 30);
                app
            })
            .collect();
        // Reading by scrolling, then with a later comment selected.
        let view = apps[0].post_view.as_mut().unwrap();
        view.scroll = view.row_starts[0] + 10;
        let view = apps[1].post_view.as_mut().unwrap();
        view.cursor = 1;
        view.free_scroll = false;
        view.scroll = view.row_starts[1] - 5;
        screen(&mut apps[1], width, 30);
        let befores: Vec<_> = apps
            .iter_mut()
            .map(|app| {
                let screen = screen(app, width, 30);
                (shown(&screen, "line "), len(app))
            })
            .collect();
        let selected = shown(&screen(&mut apps[1], width, 30), "▌");

        let wait = created + chrono::Duration::milliseconds(60_300) - Utc::now();
        std::thread::sleep(wait.to_std().unwrap_or_default());

        for (app, (before, lines_before)) in apps.iter_mut().zip(befores) {
            let after = shown(&screen(app, width, 30), "line ");
            assert!(len(app) < lines_before, "the header lost a line");
            assert!(before.is_some());
            assert_eq!(after, before, "the text being read stays");
        }
        assert!(selected.is_some());
        assert_eq!(shown(&screen(&mut apps[1], width, 30), "▌"), selected);
    }

    #[test]
    fn a_header_shrinking_above_while_one_grows_below_keeps_the_text_being_read() {
        let opened = |age: i64, width: u16| {
            let (mut app, _) = open_long_post(|_| {});
            let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
            page.comments[0].body = paragraphs("line", 1..=40);
            page.comments[0].created = Some(Utc::now() - chrono::Duration::seconds(age));
            screen(&mut app, width, 30);
            app
        };
        let len = |app: &App| app.post_view.as_ref().unwrap().rendered_document.len();
        // A width where "just now" wraps and "1m ago" does not, while "9m ago" fits and
        // "10m ago" wraps: the document keeps its length as one header shrinks and one grows.
        let width = (40..140)
            .find(|&width| {
                len(&opened(10, width)) > len(&opened(120, width))
                    && len(&opened(590, width)) < len(&opened(610, width))
            })
            .expect("some width rewraps both headers");
        let created = Utc::now() - chrono::Duration::seconds(58);
        let (mut app, _) = open_long_post(|_| {});
        let page = app.post_view.as_mut().unwrap().page.as_mut().unwrap();
        page.comments[0].body = paragraphs("line", 1..=40);
        page.comments[0].created = Some(created);
        let mut below = page.comments[0].clone();
        below.id = "below".into();
        below.body = "short".into();
        below.created = Some(created - chrono::Duration::seconds(540));
        page.comments.push(below);
        screen(&mut app, width, 30);
        let view = app.post_view.as_mut().unwrap();
        view.scroll = view.row_starts[0] + 10;
        let shown = |s: &str| {
            s.lines()
                .enumerate()
                .find_map(|(row, l)| l.split("line ").nth(1).map(|r| (row, r.to_string())))
        };
        let lines_before = len(&app);
        let before = shown(&screen(&mut app, width, 30));

        let wait = created + chrono::Duration::milliseconds(60_300) - Utc::now();
        std::thread::sleep(wait.to_std().unwrap_or_default());
        // A spinner advancing in the same draw, as while anything loads.
        app.loading_frame += 1;
        let after = shown(&screen(&mut app, width, 30));

        assert_eq!(len(&app), lines_before, "the rewraps cancel out");
        assert!(before.is_some());
        assert_eq!(after, before, "the text being read stays");
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

        press_keys(&mut app, "zs");
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
        let scroll_after = |batches: &[&[&str]]| {
            let mut app = post_with_rewrapping_spoilers(0);
            let view = app.post_view.as_mut().unwrap();
            view.scroll = view.row_starts[0] + 10;
            screen(&mut app, 100, 30);
            for batch in batches {
                for &key in *batch {
                    press_keys(&mut app, key);
                }
                screen(&mut app, 100, 30);
            }
            app.post_view.as_ref().unwrap().scroll
        };

        // Paging after `v` moves on from the kept text, drawn or not in between.
        assert_eq!(
            scroll_after(&[&["zs", "\u{6}"]]),
            scroll_after(&[&["zs"], &["\u{6}"]])
        );
        // Hiding again adds lines above; `g` in the same batch still goes to the top.
        assert_eq!(scroll_after(&[&["zs"], &["zs", "gg"]]), 0);
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

        press_keys(&mut app, "zs");
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
            press_keys(&mut app, "zs");
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
        let after = |batches: &[&[&str]]| {
            let mut app = post_with_rewrapping_spoilers(0);
            let view = app.post_view.as_mut().unwrap();
            view.cursor = 0;
            view.free_scroll = false;
            view.scroll = view.row_starts[0] + 10;
            screen(&mut app, 100, 30);
            for batch in batches {
                for &key in *batch {
                    press_keys(&mut app, key);
                }
                screen(&mut app, 100, 30);
            }
            let s = screen(&mut app, 100, 30);
            let first = s
                .lines()
                .find_map(|l| l.split("line ").nth(1).map(str::to_string));
            (first, app.post_view.as_ref().unwrap().scroll)
        };

        assert_eq!(
            after(&[&["zs", "\u{6}"]]).0,
            after(&[&["zs"], &["\u{6}"]]).0
        );
        // Hiding again adds lines above; `g` in the same batch still goes to the top.
        assert_eq!(after(&[&["zs"], &["zs", "gg"]]).1, 0);
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
        press_keys(&mut app, "zs");
        let shown = screen(&mut app, 100, 30);

        assert!(masked.contains("the butler did it"), "{masked}");
        assert!(masked.contains("so ▒▒▒▒▒▒▒ was the sled"), "{masked}");
        assert!(
            hidden.contains("zs reveals the post and its comment"),
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
        press_keys(&mut app, "zs");
        screen(&mut app, 100, 30);
        press_keys(&mut app, "zs");
        press(&mut app, 'j');
        let after_hide_j = reading(&app);
        // Hidden (positions cleared), then reveal and `G` before the next draw:
        // `G` scrolls down to the last comment, it does not hold the view.
        screen(&mut app, 100, 30);
        press_keys(&mut app, "zs");
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
        for keys in ["j", "\u{6}", "\u{6}"] {
            press_keys(&mut app, keys);
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
        key(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
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
        let run = |keys: &[&str], draw_between: bool, loads: &[&str]| {
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
            for key in ["G", "\u{6}"] {
                press_keys(&mut app, key);
                screen(&mut app, 100, 30);
            }
            let before = first_line(&screen(&mut app, 100, 30));
            for &key in keys {
                press_keys(&mut app, key);
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
        let (_, selected) = run(&["l"], true, &["c"]);
        // The main loop handles keys, then messages, then draws: `l` and the
        // replies, or two loads, can land before one draw.
        let (_, same_pass) = run(&["l"], false, &["c"]);
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
        press_keys(&mut app, "za");
        press(&mut app, 'j');
        press_keys(&mut app, "za");
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
        key(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
        screen(&mut app, 100, 30);
        let paged = app.post_view.as_ref().unwrap().scroll;

        press(&mut app, 'k');
        screen(&mut app, 100, 30);
        let after_k = app.post_view.as_ref().unwrap().scroll;
        press_keys(&mut app, "[[");
        screen(&mut app, 100, 30);
        let view = app.post_view.as_ref().unwrap();

        assert!(paged > 0);
        assert_eq!(after_k, paged - 1, "`k` scrolls up a line, not down");
        assert_eq!(view.scroll, 0, "`[[` from the body goes to the post's top");
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

        press_keys(&mut app, "zi");
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
        for p in &mut spoilers.tabs[4].feed.posts {
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
        app.tabs[4].feed.posts[0].spoiler = true;

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
                original_deleted: false,
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
            original_deleted: false,
        }
    }

    /// r/s with eight posts; all but `p2` have an image.
    fn card_feed_app(picker: Picker) -> App {
        let mut app = app(&["s"]);
        app.image_picker = picker;
        app.show_images = true;
        app.switch_tab(4);
        let tab = app.tabs[4].id;
        let generation = app.tabs[4].feed.generation;
        let posts = (1..=8)
            .map(|i| {
                let media = if i == 2 {
                    Media::None
                } else {
                    Media::Image {
                        url: format!("https://i.redd.it/p{i}.jpeg"),
                        preview: None,
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

    #[test]
    fn when_a_crosspost_has_a_deleted_original_its_card_should_label_it() {
        let app = card_feed_app(kitty());
        let post = post_with("cross", Media::None);
        let mut stored = serde_json::to_value(post).unwrap();
        stored["original_deleted"] = serde_json::Value::Bool(true);
        let post = serde_json::from_value(stored).unwrap();

        let card = expanded_card(
            &app,
            &post,
            120,
            &CardOpts {
                selected: true,
                show_subreddit: true,
                reserve: 0,
            },
            50,
        );
        let text = card
            .lines
            .iter()
            .flat_map(|line| line.spans.iter().map(|s| s.content.as_ref()))
            .collect::<String>();

        assert!(text.contains("Original post deleted"), "{text}");
    }

    #[test]
    fn when_a_cached_crosspost_lacks_parent_status_its_card_should_keep_the_known_label() {
        let mut app = card_feed_app(kitty());
        app.state.mark_original_deleted("cross", crate::app::now());
        let post = post_with("cross", Media::None);

        let card = expanded_card(
            &app,
            &post,
            120,
            &CardOpts {
                selected: true,
                show_subreddit: true,
                reserve: 0,
            },
            50,
        );
        let text = card
            .lines
            .iter()
            .flat_map(|line| line.spans.iter().map(|s| s.content.as_ref()))
            .collect::<String>();

        assert!(text.contains("Original post deleted"), "{text}");
    }

    #[test]
    fn when_link_thumbnails_are_small_a_large_pane_should_keep_the_card_compact() {
        let app = card_feed_app(kitty());
        let mut post = post_with(
            "article",
            Media::Link {
                url: "https://volkskrant.nl/article".into(),
                thumb: Some("https://preview.redd.it/article.png".into()),
            },
        );
        post.title = "An article headline".into();
        post.preview_size = Some((140, 70));
        let card = expanded_card(
            &app,
            &post,
            180,
            &CardOpts {
                selected: true,
                show_subreddit: true,
                reserve: 0,
            },
            80,
        );
        assert!(
            card.image_size.width <= 27 && card.image_size.height <= 8,
            "140px thumbnails must not become pane-wide: {:?}",
            card.image_size
        );
        assert!(
            card.height() <= 12,
            "compact image and text share their rows"
        );
    }

    #[test]
    fn when_decoded_link_pixels_are_small_the_declared_large_preview_should_not_enlarge_them() {
        let mut app = card_feed_app(kitty());
        app.tabs[4].feed.posts.truncate(1);
        let url = "https://preview.redd.it/article.jpeg";
        let post = &mut app.tabs[4].feed.posts[0];
        post.media = Media::Link {
            url: "https://example.com/story".into(),
            thumb: Some(url.into()),
        };
        post.preview_size = Some((1600, 900));
        app.card_image_metadata.insert(
            url.into(),
            crate::worker::ImageMetadata {
                size: (140, 70),
                fingerprint: [0; 32],
            },
        );

        let card = expanded_card(
            &app,
            &app.tabs[4].feed.posts[0],
            180,
            &CardOpts {
                selected: true,
                show_subreddit: true,
                reserve: 0,
            },
            80,
        );
        assert!(
            card.image_size.width <= 27 && card.image_size.height <= 8,
            "{:?}",
            card.image_size
        );
    }

    #[test]
    fn when_link_images_are_large_their_preview_should_use_thirty_percent_of_the_viewport() {
        let app = card_feed_app(kitty());
        let mut post = post_with(
            "article",
            Media::Link {
                url: "https://example.com/article".into(),
                thumb: Some("https://preview.redd.it/article.png".into()),
            },
        );
        post.preview_size = Some((1600, 900));
        let card = expanded_card(
            &app,
            &post,
            180,
            &CardOpts {
                selected: true,
                show_subreddit: true,
                reserve: 0,
            },
            80,
        );
        assert_eq!(card.image_size.height, 24);
    }

    #[test]
    fn when_a_link_has_no_image_its_text_card_should_identify_the_domain() {
        let app = card_feed_app(kitty());
        let post = post_with(
            "article",
            Media::Link {
                url: "https://volkskrant.nl/article".into(),
                thumb: None,
            },
        );
        let card = expanded_card(
            &app,
            &post,
            100,
            &CardOpts {
                selected: true,
                show_subreddit: true,
                reserve: 0,
            },
            60,
        );
        let text = card
            .visible(0, card.height())
            .iter()
            .flat_map(|line| line.spans.iter().map(|span| span.content.as_ref()))
            .collect::<String>();
        assert!(
            text.contains("VO"),
            "use a small domain monogram when there is no image: {text}"
        );
        assert_eq!(card.image_size, Size::default());
    }

    #[test]
    fn an_unselected_video_without_autoplay_loads_and_displays_a_still_frame() {
        let mut app = card_feed_app(kitty());
        app.config.media.video_autoplay = false;
        let mut first = post_with(
            "video",
            Media::Video {
                gif: false,
                url: Some(VIDEO.into()),
                poster: None,
            },
        );
        first.title = "Video preview".into();
        first.preview_size = Some((160, 90));
        let mut selected = post_with("selected", Media::None);
        selected.title = "Selected text post".into();
        app.tabs[4].feed.posts = vec![first, selected];
        app.tabs[4].feed.selected = 1;
        app.tabs[4].feed.offset = 0;
        app.tabs[4].feed.cards = Default::default();
        first_screen(&mut app, 100, 40);
        assert!(
            app.card_images.contains_key(VIDEO),
            "visible unselected video must request a frame"
        );

        app.on_msg(Msg::Image {
            url: VIDEO.into(),
            result: Ok(image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                160,
                90,
                image::Rgb([180, 80, 50]),
            ))
            .into()),
        });
        let actual = screen(&mut app, 100, 40);

        assert!(actual.contains('\u{10EEEE}'), "{actual}");
        assert!(!actual.contains("select to preview"), "{actual}");
        assert!(
            app.video.is_none(),
            "a still preview must not start playback"
        );
    }

    #[test]
    fn when_a_youtube_card_waits_for_playback_it_should_show_the_provider_poster_and_play_overlay()
    {
        let mut app = card_feed_app(kitty());
        app.config.media.video_autoplay = false;
        app.tabs[4].feed.posts.truncate(1);
        app.tabs[4].feed.posts[0].media = Media::Link {
            url: "https://www.youtube.com/shorts/dQw4w9WgXcQ".into(),
            thumb: Some("https://preview.redd.it/youtube.jpg".into()),
        };
        let mut pictures = ListPictures::default();
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();

        terminal
            .draw(|f| {
                draw_feed(f, &mut app, f.area(), &mut pictures);
            })
            .unwrap();

        assert_eq!(
            pictures.cards[0].url.as_deref(),
            Some("https://preview.redd.it/youtube.jpg")
        );
        assert!(matches!(
            pictures.cards[0].indicator,
            Some(Indicator::Video {
                gif: false,
                play: true
            })
        ));
        let actual = buffer_text(terminal.backend().buffer());
        assert!(actual.contains("▶ YouTube"), "{actual}");
    }

    #[test]
    fn when_a_youtube_detail_loads_it_should_keep_its_poster_and_provider() {
        let mut app = open_video_post();
        app.config.media.video_autoplay = false;
        let media = Media::Link {
            url: "https://www.youtube.com/watch?v=dQw4w9WgXcQ".into(),
            thumb: Some("https://preview.redd.it/youtube.jpg".into()),
        };
        let view = app.post_view.as_mut().unwrap();
        view.post.media = media.clone();
        view.page.as_mut().unwrap().post.media = media;
        let mut pictures = ListPictures::default();
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();

        terminal
            .draw(|f| {
                draw_post(f, &mut app, f.area(), &mut pictures);
            })
            .unwrap();

        assert_eq!(
            pictures.cards[0].url.as_deref(),
            Some("https://preview.redd.it/youtube.jpg")
        );
        assert!(buffer_text(terminal.backend().buffer()).contains("▶ YouTube"));
    }

    fn feed_app(picker: Picker) -> App {
        let mut app = card_feed_app(picker);
        app.state.toggle_list_view();
        app
    }

    fn feed_with_next_page(view: ListView) -> App {
        feed_with_next_page_in(app(&["s"]), view)
    }

    fn feed_with_next_page_in(mut app: App, view: ListView) -> App {
        app.switch_tab(4);
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
    fn rapid_navigation_should_reuse_loaded_native_media_without_new_preparation_or_uploads() {
        for kind in ["image", "video", "gallery", "link"] {
            let mut app = card_with_image(640, 320);
            let url = "https://i.redd.it/p1.jpeg";
            app.tabs[4].feed.posts[0].media = match kind {
                "video" => Media::Video {
                    gif: false,
                    url: Some(VIDEO.into()),
                    poster: Some(url.into()),
                },
                "gallery" => Media::Gallery {
                    thumb: None,
                    previews: Vec::new(),
                    images: vec![url.into(), "other".into()],
                },
                "link" => Media::Link {
                    url: "https://example.com/story".into(),
                    thumb: Some(url.into()),
                },
                _ => Media::Image {
                    url: url.into(),
                    preview: None,
                },
            };
            app.tabs[4]
                .feed
                .posts
                .extend((0..10).map(|i| post_with(&format!("tail{i}"), Media::None)));
            screen(&mut app, 100, 40);
            for delta in [3, 1, -1, -3, 8, -8] {
                app.scroll_card(delta);
                let shown = first_screen(&mut app, 100, 40);
                assert!(
                    !shown.contains("preparing image"),
                    "warm {kind} must draw immediately"
                );
                assert!(shown.contains('\u{10EEEE}'), "{kind}");
                assert!(
                    !shown.contains("_G"),
                    "warm {kind} must not upload pixels again"
                );
                assert!(
                    !app.card_renderer.pending(),
                    "warm {kind} must not queue encoding"
                );
            }
        }
    }

    #[test]
    fn when_card_previews_resize_their_native_strips_should_fill_the_fitted_box() {
        for kind in ["image", "gallery", "video", "gif", "link"] {
            let mut app = card_with_image(320, 640);
            app.config.media.video_autoplay = false;
            let url = "https://i.redd.it/p1.jpeg";
            app.tabs[4].feed.posts[0].media = match kind {
                "gallery" => Media::Gallery {
                    images: vec![url.into()],
                    previews: Vec::new(),
                    thumb: None,
                },
                "video" | "gif" => Media::Video {
                    url: Some(VIDEO.into()),
                    poster: Some(url.into()),
                    gif: kind == "gif",
                },
                "link" => Media::Link {
                    url: "https://example.com/story".into(),
                    thumb: Some(url.into()),
                },
                _ => Media::Image {
                    url: url.into(),
                    preview: None,
                },
            };
            let mut fitted = Vec::new();
            for (width, height) in [(160, 60), (100, 30), (160, 60)] {
                let cells = placeholder_cells(&mut app, width, height);
                let CardImageSlot::Ready {
                    protocol: Some(protocol),
                    ..
                } = app.card_images.get(url).unwrap()
                else {
                    panic!("decoded {kind}");
                };
                assert!(!cells.is_empty(), "{kind} at {width}x{height}");
                assert!(protocol.size.height <= height * 3 / 5);
                let left = cells.iter().map(|p| p.0).min().unwrap();
                let right = cells.iter().map(|p| p.0).max().unwrap();
                let top = cells.iter().map(|p| p.1).min().unwrap();
                let bottom = cells.iter().map(|p| p.1).max().unwrap();
                assert_eq!(
                    (right - left + 1, bottom - top + 1),
                    (protocol.size.width, protocol.size.height)
                );
                for ((_, _, rows, cols), tile) in &protocol.tiles {
                    let crate::card_image::Tile::KittyImage { upload, size, .. } = tile.as_ref()
                    else {
                        panic!("native still");
                    };
                    assert_eq!((*cols, *rows), (size.width, size.height));
                    assert!(
                        upload.contains(&format!(",c={cols},r={rows},")),
                        "{kind} native placement"
                    );
                }
                fitted.push(protocol.size);
                assert!(
                    matches!(app.card_images.get(url), Some(CardImageSlot::Ready { .. })),
                    "resize must keep decoded {kind}"
                );
            }
            assert_ne!(fitted[0], fitted[1]);
            assert_eq!(fitted[0], fitted[2]);
            let warm = first_screen(&mut app, 160, 60);
            assert!(
                !warm.contains("_G"),
                "returning to a loaded size must reuse {kind} uploads"
            );
            assert!(!app.card_renderer.pending());
        }
    }

    #[test]
    fn warm_navigation_replay_should_display_loaded_cards_without_media_work() {
        let mut app = card_feed_app(kitty());
        app.tabs[4].feed.posts = (0..12)
            .map(|i| {
                let mut post = post_with(
                    &format!("warm{i}"),
                    Media::Image {
                        url: format!("https://i.redd.it/warm{i}.png"),
                        preview: None,
                    },
                );
                post.preview_size = Some((320, 640));
                post
            })
            .collect();
        for i in 0..12 {
            app.tabs[4].feed.selected = i;
            app.tabs[4].feed.cards = CardScroll::default();
            let url = format!("https://i.redd.it/warm{i}.png");
            app.card_pictures_drawn(&[&url]);
            app.on_msg(Msg::Image {
                url,
                result: Ok(image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(
                    320,
                    640,
                    |x, y| image::Rgb([x as u8, y as u8, i as u8]),
                ))
                .into()),
            });
            screen(&mut app, 140, 50);
        }
        let started = Instant::now();
        let (mut delayed, mut preparation, mut uploads) = (0, 0, 0);
        for delta in std::iter::repeat_n(-1, 11)
            .chain(std::iter::repeat_n(1, 11))
            .cycle()
            .take(66)
        {
            app.move_selection(delta);
            let shown = first_screen(&mut app, 140, 50);
            delayed +=
                usize::from(shown.contains("preparing image") || shown.contains("loading image"));
            preparation += usize::from(app.card_renderer.pending());
            uploads += usize::from(shown.contains("_G"));
        }
        eprintln!(
            "navigation replay: 66 moves in {:?}; delayed frames={delayed}; preparation batches={preparation}; upload frames={uploads}",
            started.elapsed()
        );
        assert_eq!((delayed, preparation, uploads), (0, 0, 0));
    }

    #[test]
    fn when_cached_pixels_are_decoded_again_the_warm_card_should_not_reencode() {
        let mut app = card_with_image(640, 320);
        screen(&mut app, 201, 57);
        let url = "https://i.redd.it/p1.jpeg";
        app.card_pictures_drawn(&["https://i.redd.it/large.png"]);
        app.on_msg(Msg::Image {
            url: "https://i.redd.it/large.png".into(),
            result: Ok(image::DynamicImage::new_rgba8(4096, 4096).into()),
        });
        app.card_pictures_drawn(&[]);
        assert!(
            !app.card_images.contains_key(url),
            "exercise the decoded-pixel cache limit"
        );
        screen(&mut app, 201, 57);

        app.on_msg(Msg::Image {
            url: url.into(),
            result: Ok(
                image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                    640,
                    320,
                    image::Rgba([0, 0, 0, 255]),
                ))
                .into(),
            ),
        });
        let shown = first_screen(&mut app, 201, 57);

        assert!(
            !app.card_renderer.pending(),
            "identical decoded content must reuse the warm encoding"
        );
        assert!(!shown.contains("_G") && !shown.contains("preparing image"));
    }

    #[test]
    fn when_a_partial_card_becomes_fully_visible_its_remaining_strips_should_be_ready() {
        let mut app = card_with_image(640, 640);
        app.tabs[4].feed.posts.push(post_with(
            "p2",
            Media::Image {
                url: "https://i.redd.it/p2.jpeg".into(),
                preview: None,
            },
        ));
        app.card_pictures_drawn(&["https://i.redd.it/p1.jpeg", "https://i.redd.it/p2.jpeg"]);
        app.on_msg(Msg::Image {
            url: "https://i.redd.it/p2.jpeg".into(),
            result: Ok(image::DynamicImage::new_rgb8(640, 640).into()),
        });
        screen(&mut app, 201, 57);

        app.move_selection(1);
        let shown = first_screen(&mut app, 201, 57);

        assert!(
            !shown.contains("preparing image"),
            "already-decoded clipped cards should prepare their fitted canvas"
        );
        assert!(!app.card_renderer.pending());
        assert!(
            !shown.contains("_G"),
            "the offscreen strips should already be uploaded"
        );
    }

    #[test]
    fn the_same_loaded_image_on_another_post_should_reuse_its_native_encoding() {
        let mut app = card_with_image(640, 320);
        screen(&mut app, 100, 40);
        app.tabs[4].feed.posts[0].id = "crosspost".into();
        let shown = first_screen(&mut app, 100, 40);
        assert!(!shown.contains("preparing image"));
        assert!(!shown.contains("_G"));
        assert!(!app.card_renderer.pending());
    }

    #[test]
    fn rapid_list_navigation_should_defer_cold_graphics_preparation() {
        let mut app = card_with_decoded_image();
        app.move_selection(0);
        app.move_selection(0);
        first_screen(&mut app, 100, 24);
        assert!(
            !app.card_renderer.pending(),
            "list input must not queue cold graphics during a quick run"
        );
    }

    #[test]
    fn when_sidebar_navigation_shows_decoded_card_pixels_it_should_defer_missing_preparation() {
        let mut app = card_with_decoded_image();
        app.focus_sidebar();
        app.switch_tab(1);
        app.switch_tab(4);

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
        app.switch_tab(4);

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
        app.switch_tab(4);
        first_screen(&mut app, 100, 24);

        std::thread::sleep(crate::app::NAV_SETTLE);
        assert!(app.tick());
        let actual = screen(&mut app, 100, 24);

        assert!(actual.contains('\u{10EEEE}'), "{actual}");
        assert!(!actual.contains("preparing image"), "{actual}");
    }

    #[test]
    fn when_a_collection_loads_more_it_should_use_the_shared_append_loader() {
        use crate::app::experience::Screen;
        use crate::reddit::experience::{Entry, Page, Query, Request};
        let mut app = app(&[]);
        app.overlay = Some(Overlay::Experience);
        app.experience.screens.push(Screen::Browse {
            pagination: Default::default(),
            query: Query::Multis,
            page: Page {
                title: "Custom feeds".into(),
                text: String::new(),
                entries: vec![Entry {
                    icon: Default::default(),
                    title: "Existing feed".into(),
                    body: String::new(),
                    target: None,
                    query: None,
                    post: None,
                }],
                after: Some("next".into()),
                url: "/api/multi/mine".into(),
            },
            selected: 0,
            scroll: 0,
        });
        app.experience.pending = Some((
            91,
            Request::Read {
                query: Query::Multis,
                after: Some("next".into()),
            },
        ));

        let actual = screen(&mut app, 100, 30);

        assert!(actual.contains("Existing feed"), "{actual}");
        assert!(
            actual.contains("━") && actual.contains("Loading results"),
            "{actual}"
        );
        assert!(matches!(
            app.experience.screens.last(),
            Some(Screen::Browse {
                selected: 0,
                scroll: 0,
                ..
            })
        ));
        app.on_experience(
            91,
            Err("offline".into()),
            crate::cache::Freshness {
                fetched_at: crate::app::now(),
                source: Default::default(),
            },
        );
        for (width, height) in [(24, 10), (40, 12), (100, 30)] {
            let actual = screen(&mut app, width, height);
            assert!(actual.contains("Existing feed"), "{actual}");
            assert!(actual.contains("r retry"), "{actual}");
        }
        press(&mut app, 'r');
        assert!(
            matches!(&app.experience.pending, Some((_, Request::Read { after: Some(cursor), .. })) if cursor == "next")
        );
    }

    #[test]
    fn when_a_pending_collection_is_extremely_narrow_its_cancel_hint_should_survive() {
        use crate::app::experience::Screen;
        use crate::reddit::experience::{Page, Query, Request};
        for width in [5, 6] {
            let mut app = app(&[]);
            app.overlay = Some(Overlay::Experience);
            app.experience.screens.push(Screen::Browse {
                pagination: Default::default(),
                query: Query::Multis,
                page: Page {
                    title: "Custom feeds".into(),
                    text: String::new(),
                    entries: vec![],
                    after: Some("next".into()),
                    url: String::new(),
                },
                selected: 0,
                scroll: 0,
            });
            app.experience.pending = Some((
                1,
                Request::Read {
                    query: Query::Multis,
                    after: Some("next".into()),
                },
            ));
            let actual = screen(&mut app, width, 10);
            assert!(actual.contains('⎋'), "{actual}");
        }
    }

    #[test]
    fn when_replies_load_more_the_compact_loader_should_keep_the_comment_rows_stable() {
        let (mut app, _) = open_long_post(|_| {});
        let view = app.post_view.as_mut().unwrap();
        let page = view.page.as_mut().unwrap();
        page.body.clear();
        page.comments[0].more = Some(crate::model::MoreReplies {
            count: Some(2),
            cursor: "next".into(),
        });
        screen(&mut app, 100, 30);
        let rows = app.post_view.as_ref().unwrap().rendered_document.len();
        app.post_view
            .as_mut()
            .unwrap()
            .loading_more
            .insert("c".into());

        let actual = screen(&mut app, 100, 30);

        assert!(
            actual.contains("Loading replies") && actual.contains("━"),
            "{actual}"
        );
        assert_eq!(
            app.post_view.as_ref().unwrap().rendered_document.len(),
            rows
        );
    }

    #[test]
    fn when_subscriptions_first_load_the_initial_loader_should_use_the_shared_spinner() {
        let mut app = app(&[]);
        app.all_subs.loading = true;

        let actual = screen(&mut app, 100, 30);

        assert!(
            actual.contains("Loading subscriptions") && actual.contains('⠋'),
            "{actual}"
        );
    }

    #[test]
    fn when_images_are_pending_cards_and_embedded_pictures_should_share_the_media_spinner() {
        let mut card = card_feed_app(kitty());
        let mut post = picture_comment("https://preview.redd.it/pending.png");

        let card_view = first_screen(&mut card, 100, 30);
        let post_view = screen(&mut post, 100, 30);

        assert!(
            card_view.contains('⠋') && card_view.contains("loading image"),
            "{card_view}"
        );
        assert!(
            post_view.contains('⠋') && post_view.contains("loading image"),
            "{post_view}"
        );
    }

    #[test]
    fn when_initial_or_append_loading_waits_for_cooldown_the_loader_should_explain_the_wait() {
        use crate::reddit::tests::{Fixture, Reply};
        use std::time::{Duration, Instant};
        for append in [false, true] {
            let fixture = Fixture::new(|_| Reply {
                status: 429,
                body: r#"{"kind":"Listing","data":{"children":[],"after":null}}"#.into(),
                headers: vec![("Retry-After".into(), "30".into())],
            });
            let reddit = fixture.reddit();
            assert!(reddit.search("s", "first", None).is_err());
            let mut app = app_with_client(&["s"], reddit.clone());
            app.switch_tab(4);
            if append {
                app.on_msg(Msg::Feed {
                    tab: app.active_tab().id,
                    generation: app.active_tab().feed.generation,
                    append: false,
                    result: Ok(crate::model::Listing {
                        posts: vec![post_with("existing", Media::None)],
                        after: Some("next".into()),
                    }),
                });
                app.load_feed(4, true);
            }
            app.status = None;
            let deadline = Instant::now() + Duration::from_secs(1);
            while reddit.pause().is_none() {
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
            app.tick();

            let actual = screen(&mut app, 100, 24);
            reddit.retire();

            assert!(actual.contains("Waiting for Reddit"), "{actual}");
            assert!(actual.contains("HTTP 429"), "{actual}");
        }
    }

    #[test]
    fn when_an_empty_feed_has_a_failed_next_page_it_should_offer_the_cursor_retry_key() {
        let mut app = app(&["s"]);
        app.switch_tab(4);
        let tab = app.active_tab().id;
        app.on_msg(Msg::Feed {
            tab,
            generation: app.active_tab().feed.generation,
            append: false,
            result: Ok(crate::model::Listing {
                posts: vec![],
                after: Some("next".into()),
            }),
        });
        app.load_feed(4, true);
        app.on_msg(Msg::Feed {
            tab,
            generation: app.active_tab().feed.generation,
            append: true,
            result: Err("failed continuation".into()),
        });

        let actual = screen(&mut app, 100, 24);

        assert!(
            actual.contains("Could not load more posts") && actual.contains("press r to retry"),
            "{actual}"
        );
    }

    #[test]
    fn when_an_initial_page_streams_card_and_compact_views_should_keep_the_append_loader() {
        for view in [ListView::Card, ListView::Compact] {
            let mut app = app(&["s"]);
            app.switch_tab(4);
            if app.state.list_view() != view {
                app.state.toggle_list_view();
            }
            let mut post = post_with("arrived", Media::None);
            post.title = "Arrived post".into();
            app.on_msg(Msg::FeedProgress {
                tab: app.active_tab().id,
                generation: app.active_tab().feed.generation,
                post: Box::new(post),
                source: crate::cache::Source::default(),
            });

            let actual = screen(&mut app, 100, 24);

            assert!(
                actual.contains("Arrived post")
                    && actual.contains("Loading posts")
                    && actual.contains('━'),
                "{actual}"
            );
            assert!(app.active_tab().feed.loading());
            assert!(actual.find("Arrived post").unwrap() < actual.find("Loading posts").unwrap());
        }
    }

    #[test]
    fn when_compact_pagination_starts_it_should_render_a_skeleton_after_the_entries() {
        let mut app = feed_with_next_page(ListView::Compact);
        app.load_feed(4, true);

        let actual = screen(&mut app, 80, 20);

        assert!(actual.contains("━"), "{actual}");
        assert!(actual.contains("Loading posts"), "{actual}");
        assert_eq!(
            actual.lines().filter(|line| line.contains("━")).count(),
            2,
            "{actual}"
        );
        assert!(
            !actual.lines().last().unwrap().contains("more posts"),
            "{actual}"
        );
        assert!(actual.find("Existing post").unwrap() < actual.find("Loading posts").unwrap());
        assert_eq!(app.active_tab().feed.selected, 0);
        assert_eq!(app.active_tab().feed.offset, 0);
    }

    #[test]
    fn when_card_pagination_starts_it_should_render_a_skeleton_after_the_entries() {
        let mut app = feed_with_next_page(ListView::Card);
        screen(&mut app, 80, 20);
        let geometry = app.active_tab().feed.cards.clone();
        app.load_feed(4, true);

        let actual = screen(&mut app, 80, 20);

        assert!(actual.contains("━"), "{actual}");
        assert!(actual.contains("Loading posts"), "{actual}");
        assert_eq!(
            actual.lines().filter(|line| line.contains("━")).count(),
            2,
            "{actual}"
        );
        assert!(
            !actual.lines().last().unwrap().contains("more posts"),
            "{actual}"
        );
        let cards = &app.active_tab().feed.cards;
        assert_eq!(
            (cards.height, cards.viewport, cards.line),
            (geometry.height, geometry.viewport, geometry.line)
        );
    }

    #[test]
    fn navigation_should_prefetch_near_the_visible_end_without_draining_filtered_pages() {
        for (view, height, delayed_draw) in [
            (ListView::Compact, 72, false),
            (ListView::Card, 96, false),
            (ListView::Compact, 72, true),
            (ListView::Card, 96, true),
        ] {
            let mut app = feed_with_next_page(view);
            app.config.filters.keywords = vec!["hidden".into()];
            app.tabs[4].feed.posts = (0..25)
                .map(|i| {
                    let mut post = post_with(&format!("p{i}"), Media::None);
                    post.title = format!("Entry {i:02} end");
                    post
                })
                .collect();
            screen(&mut app, 100, height);
            screen(&mut app, 100, height + 1);
            assert!(
                !app.active_tab().feed.loading_more(),
                "redraws must not fetch"
            );

            app.move_selection(1);
            if delayed_draw {
                std::thread::sleep(crate::app::NAV_SETTLE);
                app.tick();
            }
            let actual = screen(&mut app, 100, height);
            assert!(actual.contains("Entry 20 end"), "view={view:?}: {actual}");
            assert!(!actual.contains("Entry 24 end"), "view={view:?}: {actual}");
            assert_eq!(app.active_tab().feed.selected, 1);
            if !delayed_draw {
                assert!(
                    !app.active_tab().feed.loading_more(),
                    "navigation must settle first"
                );
                std::thread::sleep(crate::app::NAV_SETTLE);
                app.tick();
            }

            assert!(
                app.active_tab().feed.loading_more(),
                "view={view:?}: the visible edge should prefetch before selection reaches the end"
            );
            let generation = app.active_tab().feed.generation;
            let mut hidden = post_with("filtered", Media::None);
            hidden.title = "hidden entry".into();
            app.on_msg(Msg::Feed {
                tab: app.active_tab().id,
                generation,
                append: true,
                result: Ok(crate::model::Listing {
                    posts: vec![hidden],
                    after: Some("again".into()),
                }),
            });
            screen(&mut app, 100, height);
            screen(&mut app, 100, height + 1);
            app.tick();

            assert!(
                !app.active_tab().feed.loading_more(),
                "a filtered append must wait for new navigation"
            );
            assert_eq!(app.active_tab().feed.generation, generation);
            assert_eq!(app.active_tab().feed.selected, 1);

            app.select_edge(true);
            std::thread::sleep(crate::app::NAV_SETTLE);
            app.tick();
            assert!(app.active_tab().feed.loading_more());
            let generation = app.active_tab().feed.generation;
            let mut hidden = post_with("filtered-again", Media::None);
            hidden.title = "hidden again".into();
            app.on_msg(Msg::Feed {
                tab: app.active_tab().id,
                generation,
                append: true,
                result: Ok(crate::model::Listing {
                    posts: vec![hidden],
                    after: Some("last".into()),
                }),
            });
            screen(&mut app, 100, height);
            assert!(
                !app.active_tab().feed.loading_more(),
                "the request before drawing consumes navigation intent"
            );
            assert_eq!(app.active_tab().feed.generation, generation);
        }
    }

    #[test]
    fn scrolling_should_defer_pagination_until_settled_then_show_the_append_loader() {
        let mut app = feed_with_next_page(ListView::Card);
        screen(&mut app, 80, 20);
        assert!(!app.active_tab().feed.loading_more());
        app.scroll_card(1);

        screen(&mut app, 80, 20);
        assert!(!app.active_tab().feed.loading_more());
        std::thread::sleep(crate::app::NAV_SETTLE);
        app.tick();
        let actual = screen(&mut app, 80, 20);

        assert!(app.active_tab().feed.loading_more());
        assert!(
            actual.contains("Loading posts") && actual.contains("━"),
            "{actual}"
        );
        assert!(
            !actual.lines().last().unwrap().contains("more posts"),
            "{actual}"
        );
    }

    #[test]
    fn when_a_long_notification_is_visible_the_append_loader_should_remain_visible() {
        let mut app = feed_with_next_page(ListView::Compact);
        app.load_feed(4, true);
        app.info("Checking membership and keeping the current reading position while posts load");

        let actual = screen(&mut app, 80, 20);

        let footer = actual.lines().last().unwrap();
        assert!(footer.contains("Checking"), "{actual}");
        assert!(
            actual.contains("Loading posts") && actual.contains("━"),
            "{actual}"
        );
        assert!(!footer.contains("more posts"), "{actual}");
        assert!(footer.contains("F1"), "{actual}");
    }

    #[test]
    fn when_a_loaded_split_post_has_focus_pagination_should_stay_visible() {
        let mut app = feed_with_next_page(ListView::Compact);
        app.open_current_post();
        let view = app.post_view.as_ref().unwrap();
        app.on_msg(Msg::Post {
            generation: view.generation,
            result: Ok(Box::new(PostPage {
                post: view.post.clone(),
                body: "Loaded detail".into(),
                comments: vec![],
            })),
        });
        app.load_feed(4, true);

        let actual = screen(&mut app, 158, 20);

        assert!(actual.contains("Loaded detail"), "{actual}");
        assert!(app.post_focused());
        assert_eq!(actual.matches("Loading posts").count(), 1, "{actual}");
        assert!(
            !actual.lines().last().unwrap().contains("more posts"),
            "{actual}"
        );
    }

    #[test]
    fn when_split_views_load_independently_each_should_keep_its_own_progress() {
        for replies in [false, true] {
            let mut app = feed_with_next_page(ListView::Compact);
            app.open_current_post();
            let view = app.post_view.as_ref().unwrap();
            app.on_msg(Msg::Post {
                generation: view.generation,
                result: Ok(Box::new(PostPage {
                    post: view.post.clone(),
                    body: paragraphs("paragraph", 1..=60),
                    comments: vec![reply("c".into())],
                })),
            });
            app.load_feed(4, true);
            if replies {
                let view = app.post_view.as_mut().unwrap();
                view.page.as_mut().unwrap().comments[0].more = Some(crate::model::MoreReplies {
                    count: Some(2),
                    cursor: "next".into(),
                });
                view.loading_more.insert("c".into());
            } else {
                app.reload();
            }
            let actual = screen(&mut app, 158, 20);
            assert!(actual.contains("Loading posts"), "{actual}");
            let activity = if replies {
                "⠋ replies"
            } else {
                "⠋ refresh"
            };
            assert!(
                actual.lines().last().unwrap().contains(activity),
                "{actual}"
            );
        }

        let mut app = feed_with_next_page(ListView::Card);
        app.tabs[4]
            .feed
            .posts
            .extend((0..8).map(|i| post_with(&format!("post{i}"), Media::None)));
        app.open_current_post();
        app.load_feed(4, true);
        let actual = screen(&mut app, 158, 20);
        assert!(actual.contains("Loading post"), "{actual}");
        assert!(
            actual
                .lines()
                .last()
                .unwrap()
                .contains("⠋ Loading more posts"),
            "{actual}"
        );
        let view = app.post_view.as_ref().unwrap();
        app.on_msg(Msg::Post {
            generation: view.generation,
            result: Ok(Box::new(PostPage {
                post: view.post.clone(),
                body: "Retained detail".into(),
                comments: vec![],
            })),
        });
        app.reload();
        for (focus, activity) in [
            (Focus::List, "⠋ Loading more posts"),
            (Focus::Post, "⠋ refresh"),
        ] {
            app.focus = focus;
            let actual = screen(&mut app, 158, 20);
            assert!(
                actual.lines().last().unwrap().contains(activity),
                "{actual}"
            );
        }
    }

    #[test]
    fn when_a_post_with_an_overflow_title_is_pending_its_loader_should_not_overwrite_the_title() {
        for (width, focus) in [(50, Focus::Post), (158, Focus::List)] {
            let (mut app, _) = open_long_post(|_| {});
            app.show_sidebar = false;
            app.focus = focus;
            let view = app.post_view.as_mut().unwrap();
            view.post.title = (0..200).map(|i| format!("Segment{i:02} ")).collect();
            view.page = None.into();
            view.loading = true;
            let actual = screen(&mut app, width, 20);
            let view = app.post_view.as_ref().unwrap();
            for line in view
                .rendered_document
                .iter()
                .skip(view.scroll)
                .take(view.page_height)
            {
                let text = line.to_string();
                if !text.trim().is_empty() {
                    let text: String = text.chars().take(40).collect();
                    assert!(
                        actual.contains(text.trim_end()),
                        "Missing {text:?}:\n{actual}"
                    );
                }
            }
            assert!(
                actual.lines().last().unwrap().contains("⠋ post"),
                "{actual}"
            );
        }
    }

    #[test]
    fn when_a_notification_is_visible_the_card_append_loader_should_remain_visible() {
        let mut app = feed_with_next_page(ListView::Card);
        app.load_feed(4, true);
        app.info("Still browsing");

        let actual = screen(&mut app, 80, 20);

        let footer = actual.lines().last().unwrap();
        assert!(
            actual.contains("Loading posts")
                && !footer.contains("more posts")
                && footer.contains("Still browsing"),
            "{actual}"
        );
    }

    #[test]
    fn when_the_append_position_is_offscreen_pagination_should_use_the_footer() {
        let mut app = feed_with_next_page(ListView::Card);
        app.tabs[4]
            .feed
            .posts
            .extend((0..8).map(|i| post_with(&format!("post{i}"), Media::None)));
        app.load_feed(4, true);

        let actual = screen(&mut app, 80, 20);

        assert!(
            !actual.contains("Loading posts") && !actual.contains("━"),
            "{actual}"
        );
        assert!(
            actual.lines().last().unwrap().contains("⠋ more posts"),
            "{actual}"
        );
    }

    #[test]
    fn when_jump_covers_the_append_loader_pagination_should_use_the_footer() {
        let mut app = feed_with_next_page(ListView::Compact);
        app.load_feed(4, true);
        app.overlay = Some(Overlay::Picker(crate::app::PickerState::default()));

        let actual = screen(&mut app, 80, 20);

        assert!(!actual.contains("Loading posts"), "{actual}");
        assert!(
            actual.lines().last().unwrap().contains("⠋ more posts"),
            "{actual}"
        );
    }

    #[test]
    fn when_the_append_loader_animates_cached_graphics_should_not_be_prepared_or_uploaded_again() {
        let mut app = card_with_image(640, 320);
        screen(&mut app, 100, 40);
        app.tabs[4].feed.after = Some("next".into());
        app.load_feed(4, true);
        screen(&mut app, 100, 40);
        app.loading_frame = 1;

        let actual = first_screen(&mut app, 100, 40);

        assert!(
            actual.contains("Loading posts") && actual.contains("━"),
            "{actual}"
        );
        assert!(actual.contains('\u{10EEEE}'));
        assert!(
            !actual.contains("_G"),
            "warm graphics should not upload again"
        );
        assert!(
            !app.card_renderer.pending(),
            "warm graphics should not queue encoding"
        );
    }

    #[test]
    fn when_a_new_query_waits_in_a_narrow_pane_its_initial_loader_should_remain_visible() {
        let mut app = feed_with_next_page(ListView::Card);
        app.search("a query long enough to fill the header".into());

        let actual = screen(&mut app, 50, 12);

        assert!(
            actual.contains("⠋ Loading posts") && !actual.lines().last().unwrap().contains("posts"),
            "{actual}"
        );
    }

    #[test]
    fn when_a_page_finishes_it_should_remove_the_loading_boundary() {
        let mut app = feed_with_next_page(ListView::Card);
        app.load_feed(4, true);
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
        assert!(!actual.contains("more posts"), "{actual}");
        assert!(
            !actual.contains("Loading posts") && !actual.contains("━"),
            "{actual}"
        );
    }

    #[test]
    fn when_a_page_fails_it_should_replace_loading_with_the_retry_action() {
        let mut app = feed_with_next_page(ListView::Card);
        app.load_feed(4, true);
        app.on_msg(Msg::Feed {
            tab: app.active_tab().id,
            generation: app.active_tab().feed.generation,
            append: true,
            result: Err("offline".into()),
        });

        let actual = screen(&mut app, 80, 20);

        assert!(actual.contains("Next page failed · r retries"), "{actual}");
        assert!(!actual.contains("more posts"), "{actual}");
        assert!(
            !actual.contains("Loading posts") && !actual.contains("━"),
            "{actual}"
        );
    }

    #[test]
    fn when_cached_posts_are_refreshing_a_single_background_loader_should_keep_activity_visible() {
        let mut app = feed_with_next_page(ListView::Card);
        app.load_feed(4, false);
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
            actual.contains("⠋ Refreshing posts")
                && !actual.lines().last().unwrap().contains("refresh"),
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
        assert!(
            app.images_loading(),
            "still graphics preparation remains visible"
        );
        app.card_renderer.wait_for_completion();
        assert!(
            app.images_loading(),
            "unpublished still graphics remain visible"
        );
        assert!(app.card_renderer.poll());
        assert!(!app.images_loading());
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
            actual.lines().last().unwrap().contains("⠋ images"),
            "{actual}"
        );
    }

    #[test]
    fn when_compact_thumbnails_are_pending_their_reserved_slots_should_show_activity() {
        let mut app = feed_app(kitty());

        let actual = first_screen(&mut app, 100, 20);

        assert!(actual.contains('⠋'), "{actual}");
    }

    fn refreshing_thumbnail_app() -> App {
        let mut app = feed_app(kitty());
        app.tabs[4].feed.posts.truncate(1);
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
            actual.lines().last().unwrap().contains("⠋ images"),
            "{actual}"
        );
    }

    #[test]
    fn footer_activity_should_keep_the_same_keys_in_the_same_cells() {
        for width in [5, 6, 7, 8, 9, 10, 20, 32, 40, 64, 80, 100, 180] {
            for context in ["list", "sidebar", "detail", "unsaved"] {
                let mut app = if context == "detail" {
                    open_video_post()
                } else {
                    feed_with_next_page(ListView::Card)
                };
                app.config.media.video_autoplay = false;
                app.image_picker = kitty();
                app.show_images = true;
                app.health = Health::Up;
                app.status = None;
                if context == "sidebar" {
                    app.focus_sidebar();
                }
                if context == "unsaved" {
                    app.settings.store.persistence = crate::config::store::Persistence::Unsaved;
                }
                let footer = |app: &App| {
                    let mut terminal = Terminal::new(TestBackend::new(width, 1)).unwrap();
                    terminal
                        .draw(|f| draw_footer(f, app, f.area(), &VisibleLoaders::default(), &[]))
                        .unwrap();
                    terminal.backend().buffer().clone()
                };
                let keys = |buffer: &ratatui::buffer::Buffer| {
                    (0..width)
                        .filter_map(|x| {
                            let cell = &buffer[(x, 0)];
                            (cell.bg == SURFACE1).then(|| (x, cell.symbol().to_string()))
                        })
                        .collect::<Vec<_>>()
                };
                let idle = footer(&app);
                let expected = keys(&idle);
                assert!(buffer_text(&idle).contains("F1"));
                if width >= 20 {
                    assert!(!expected.is_empty());
                }
                app.card_images
                    .insert("pending".into(), CardImageSlot::Loading);
                for frame in [0, 4] {
                    app.loading_frame = frame;
                    let busy = footer(&app);
                    assert!(
                        buffer_text(&busy).contains(loading::glyph(frame)),
                        "{width}: {}",
                        buffer_text(&busy)
                    );
                    assert_eq!(keys(&busy), expected, "{context} at width {width}");
                    if width >= 40 {
                        assert!(buffer_text(&busy).contains("images"));
                    }
                }
                app.card_images.clear();
                assert_eq!(
                    footer(&app),
                    idle,
                    "completion must restore the idle footer"
                );
                if context == "detail" {
                    app.post_view.as_mut().unwrap().loading = true;
                } else {
                    app.load_feed(app.active, true);
                }
                let busy = footer(&app);
                assert!(buffer_text(&busy).contains(loading::glyph(app.loading_frame)));
                assert_eq!(keys(&busy), expected, "content loading at width {width}");
            }
        }
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
            actual.lines().last().unwrap().contains("⠋ images"),
            "{actual}"
        );
    }

    #[test]
    fn when_a_skipped_thumbnail_stays_offscreen_the_footer_should_not_show_refresh_activity() {
        let mut app = refreshing_thumbnail_app();
        app.on_msg(Msg::ThumbSkipped {
            url: "https://i.redd.it/p1.jpeg".into(),
        });
        app.tabs[4].feed.posts.clear();

        let actual = first_screen(&mut app, 100, 20);

        assert!(!actual.contains('\u{10EEEE}'), "{actual}");
        assert!(
            !actual.lines().last().unwrap().contains("⠋ images"),
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
            !actual.lines().last().unwrap().contains("⠋ images"),
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
            !actual.lines().last().unwrap().contains("⠋ images"),
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
            !actual.lines().last().unwrap().contains("⠋ images"),
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
            !actual.lines().last().unwrap().contains("⠋ images"),
            "{actual}"
        );
    }

    #[test]
    fn when_media_opening_outlasts_its_notification_the_footer_should_keep_showing_activity() {
        use crate::reddit::tests::{Fixture, Reply};
        let fixture = Fixture::new(|request| {
            let body = if request.url.contains("/comments/") {
                serde_json::from_str(include_str!("../tests/fixtures/post.json")).unwrap()
            } else {
                serde_json::json!({"data": {"children": [], "after": null}})
            };
            Reply::json(body)
        });
        let mut app =
            feed_with_next_page_in(app_with_client(&["s"], fixture.reddit()), ListView::Card);
        app.tabs[4].feed.posts[0].media = Media::Video {
            gif: false,
            url: None,
            poster: None,
        };
        app.open_media();
        app.status.as_mut().unwrap().at = Instant::now() - std::time::Duration::from_secs(7);
        app.tick();
        assert!(app.status.is_none());

        let actual = screen(&mut app, 80, 20);

        assert!(
            actual.lines().last().unwrap().contains("⠋ media"),
            "{actual}"
        );
    }

    #[test]
    fn when_media_loading_fails_the_footer_should_show_the_error_and_stop_activity() {
        let mut app = feed_with_next_page(ListView::Card);
        app.tabs[4].feed.posts[0].media = Media::Video {
            gif: false,
            url: None,
            poster: None,
        };
        app.open_media();
        app.on_msg(Msg::MediaResolved {
            permalink: app.tabs[4].feed.posts[0].permalink.clone(),
            result: Err("offline".into()),
        });

        let actual = screen(&mut app, 80, 20);

        let footer = actual.lines().last().unwrap();
        assert!(
            footer.contains("media: offline") && !footer.contains("⠋ media"),
            "{actual}"
        );
    }

    #[test]
    fn when_a_feed_opens_it_should_default_to_cards_with_wrapped_titles() {
        let mut app = card_feed_app(Picker::halfblocks());
        app.show_images = false;
        app.tabs[4].feed.posts[0].title =
            "First line of the title followed by a second line ending in 東京".into();

        let actual = screen(&mut app, 70, 20);

        assert!(actual.contains("zc Card"), "{actual}");
        assert!(actual.contains("First line of the title"), "{actual}");
        assert!(actual.contains("ending in 東"), "{actual}");
        assert!(actual.contains('京'), "{actual}");
        assert!(actual.contains("comments"), "{actual}");
    }

    #[test]
    fn when_view_is_toggled_it_should_keep_the_selected_post_and_restore_cards() {
        let mut app = card_feed_app(Picker::halfblocks());
        app.move_selection(4);
        let state_path = app.state_path().to_path_buf();
        let before = app.current_post().unwrap().id;

        press_keys(&mut app, "zc");
        let compact = screen(&mut app, 100, 20);
        let (saved, warning) = crate::state::State::load(&state_path);
        assert!(warning.is_none());
        assert_eq!(saved.list_view(), ListView::Compact);
        assert!(compact.contains("zc Compact"), "{compact}");
        assert_eq!(app.current_post().unwrap().id, before);

        press_keys(&mut app, "zc");
        let cards = screen(&mut app, 100, 20);
        assert!(cards.contains("zc Card"), "{cards}");
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
        app.tabs[4].feed.posts.truncate(1);
        app.tabs[4].feed.posts[0].title = "Full width image".into();
        screen(&mut app, 100, 20);
        app.on_msg(Msg::Image {
            url: "https://i.redd.it/p1.jpeg".into(),
            result: Ok(image::DynamicImage::new_rgb8(width, height).into()),
        });
        app
    }

    #[test]
    fn encoding_failures_should_show_an_error_in_the_first_failed_frame() {
        let mut app = card_with_image(320, 180);
        screen(&mut app, 100, 20);
        app.card_renderer.fail_cached_tiles("encoder failed");

        let actual = first_screen(&mut app, 100, 20);

        assert!(actual.contains("image unavailable:"), "{actual}");
        assert!(matches!(
            app.card_images.get("https://i.redd.it/p1.jpeg"),
            Some(CardImageSlot::Failed(error)) if error == "encoder failed"
        ));
    }

    fn video_card_with_poster(image: image::DynamicImage) -> App {
        let mut app = card_feed_app(kitty());
        app.tabs[4].feed.posts.truncate(1);
        app.tabs[4].feed.posts[0].media = Media::Video {
            gif: false,
            url: Some("https://v.redd.it/example/HLSPlaylist.m3u8".into()),
            poster: Some("https://i.redd.it/p1.jpeg".into()),
        };
        app.tabs[4].feed.posts[0].preview_size = Some((image.width(), image.height()));
        first_screen(&mut app, 100, 20);
        app.on_msg(Msg::Image {
            url: "https://i.redd.it/p1.jpeg".into(),
            result: Ok(image.into()),
        });
        app
    }

    #[test]
    fn card_media_detail_sidebar_should_offer_return_instead_of_play_keys() {
        let mut app = open_video_post();
        app.config.media.video_autoplay = false;
        app.focus_sidebar();

        let actual = screen(&mut app, 180, 40);
        let words = actual.split_whitespace().collect::<Vec<_>>().join(" ");

        assert!(words.contains("l return"), "{actual}");
        assert!(!words.contains("p play"), "{actual}");
    }

    #[test]
    fn card_media_review_manual_detail_should_show_play_instead_of_loading() {
        let mut app = open_video_post();
        app.config.media.video_autoplay = false;

        let actual = screen(&mut app, 100, 30);
        let words = actual.split_whitespace().collect::<Vec<_>>().join(" ");

        assert!(words.contains("p play"), "{actual}");
        assert!(!actual.contains("loading video"), "{actual}");
    }

    #[test]
    fn card_media_known_portrait_size_without_a_poster_should_keep_its_geometry() {
        let mut app = video_card_with_poster(image::DynamicImage::new_rgb8(320, 640));
        if let Media::Video { poster, .. } = &mut app.tabs[4].feed.posts[0].media {
            *poster = None;
        }
        let mut pictures = ListPictures::default();
        let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();

        terminal
            .draw(|f| {
                draw_feed(f, &mut app, f.area(), &mut pictures);
            })
            .unwrap();

        assert_eq!(pictures.cards[0].pixels, (320, 640));
        assert_eq!(pictures.cards[0].size.height, pictures.cards[0].size.width);
    }

    #[test]
    fn card_media_unavailable_stream_should_not_offer_a_play_button() {
        let mut app = video_card_with_poster(image::DynamicImage::new_rgb8(320, 180));
        if let Media::Video { url, .. } = &mut app.tabs[4].feed.posts[0].media {
            *url = None;
        }
        let mut pictures = ListPictures::default();
        let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();

        terminal
            .draw(|f| {
                draw_feed(f, &mut app, f.area(), &mut pictures);
            })
            .unwrap();

        assert!(matches!(
            pictures.cards[0].indicator,
            Some(Indicator::Video { play: false, .. })
        ));
        assert!(!app.video_controls_available());
    }

    #[test]
    fn card_media_converted_gif_should_reach_the_gif_overlay() {
        let mut app = video_card_with_poster(image::DynamicImage::new_rgb8(320, 180));
        if let Media::Video { gif, .. } = &mut app.tabs[4].feed.posts[0].media {
            *gif = true;
        }
        let mut pictures = ListPictures::default();
        let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();

        terminal
            .draw(|f| {
                draw_feed(f, &mut app, f.area(), &mut pictures);
            })
            .unwrap();

        assert!(matches!(
            pictures.cards[0].indicator,
            Some(Indicator::Video { gif: true, .. })
        ));
    }

    #[test]
    fn card_media_pause_key_should_control_the_selected_card() {
        let mut app = video_card_with_poster(image::DynamicImage::new_rgb8(320, 180));
        app.video = Some(video::Player::silent(VIDEO, Size::new(68, 17)));

        press_keys(&mut app, "p");

        assert!(app.video.as_ref().unwrap().paused());
    }

    #[test]
    fn card_media_waiting_video_should_show_play_controls() {
        let mut app = video_card_with_poster(image::DynamicImage::new_rgb8(320, 180));

        let actual = screen(&mut app, 100, 20);

        let footer = actual
            .lines()
            .last()
            .unwrap()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(footer.contains("p play"), "{actual}");
    }

    #[test]
    fn video_startup_should_emit_loading_text_without_losing_image_uploads() {
        for protocol in [
            ProtocolType::Kitty,
            ProtocolType::Sixel,
            ProtocolType::Iterm2,
        ] {
            let mut app = video_card_with_poster(image::DynamicImage::new_rgb8(320, 180));
            app.image_picker.set_protocol_type(protocol);
            app.config.media.video_autoplay = true;
            let size = if protocol == ProtocolType::Kitty {
                Size::new(10, 1)
            } else {
                Size::new(40, 8)
            };
            let picture = CardPicture {
                url: Some("https://i.redd.it/p1.jpeg".into()),
                area: Rect::new(0, 0, size.width, size.height),
                size,
                skipped: 0,
                pixels: (320, 180),
                indicator: Some(Indicator::Video {
                    gif: false,
                    play: true,
                }),
                selected: true,
                detail: false,
                post_id: Some("p1".into()),
                video_url: Some("https://v.redd.it/example/HLSPlaylist.m3u8".into()),
                video_frame: false,
            };
            let draw = |app: &mut App| {
                let mut terminal =
                    Terminal::new(TestBackend::new(size.width, size.height)).unwrap();
                loop {
                    terminal
                        .draw(|f| draw_card_pictures(f, app, std::slice::from_ref(&picture), None))
                        .unwrap();
                    if !app.card_renderer.pending() {
                        break;
                    }
                    app.card_renderer.wait_for_completion();
                    app.card_renderer.poll();
                }
                terminal.backend().buffer().clone()
            };

            let startup = draw(&mut app);
            let empty = ratatui::buffer::Buffer::empty(startup.area);
            let updates = empty.diff(&startup);
            assert!(
                updates
                    .iter()
                    .any(|(_, _, cell)| cell.symbol() == loading::glyph(0)),
                "{protocol:?}: loading must reach terminal updates"
            );

            app.config.media.video_autoplay = false;
            let idle = draw(&mut app);
            if protocol == ProtocolType::Kitty {
                assert!(
                    buffer_text(&idle).contains("_Ga=T"),
                    "the image upload must survive a one-row loading slot"
                );
            }
        }
    }

    #[test]
    fn video_startup_should_stop_loading_when_the_initial_thread_request_fails() {
        let mut app = video_card_with_poster(image::DynamicImage::new_rgb8(320, 180));
        app.config.media.video_autoplay = true;
        app.open_current_post();
        screen(&mut app, 100, 30);
        let generation = app.post_view.as_ref().unwrap().generation;
        assert_eq!(
            loading_activity(&app, &VisibleLoaders::default()),
            Some("video")
        );

        app.on_msg(Msg::Post {
            generation,
            result: Err("fixture request failed".into()),
        });
        let actual = screen(&mut app, 100, 30);

        assert!(actual.contains("Could not load the post"), "{actual}");
        assert_ne!(
            loading_activity(&app, &VisibleLoaders::default()),
            Some("video")
        );
        assert!(!app.loading_active());
    }

    #[test]
    fn video_startup_should_stay_visible_over_posters_and_retained_frames_until_playing() {
        let frame = video::Frame {
            width: 32,
            height: 18,
            pixels: video::Pixels::Raw(std::sync::Arc::new(image::DynamicImage::new_rgb8(32, 18))),
            same_as_first: false,
        };
        for detail in [false, true] {
            let mut app = if detail {
                open_video_post()
            } else {
                video_card_with_poster(image::DynamicImage::new_rgb8(320, 180))
            };
            app.config.media.video_autoplay = true;
            let url = app
                .playback_url(if detail {
                    VIDEO
                } else {
                    "https://v.redd.it/example/HLSPlaylist.m3u8"
                })
                .unwrap();
            let height = if detail { 30 } else { 20 };

            let actual = screen(&mut app, 100, height);
            assert!(
                actual
                    .lines()
                    .take(height as usize - 1)
                    .any(|line| line.contains("loading video")),
                "{actual}"
            );
            assert!(app.loading_active());

            for resolving in [true, false] {
                let mut player = if resolving {
                    video::Player::pending(&url, Size::new(32, 18))
                } else {
                    video::Player::silent(&url, Size::new(32, 18))
                };
                player.retain_frame(Some(frame.clone()));
                app.video = Some(player);

                let actual = screen(&mut app, 100, height);
                assert!(
                    actual
                        .lines()
                        .take(height as usize - 1)
                        .any(|line| line.contains("loading video")),
                    "{actual}"
                );
                assert!(app.loading_active());
                assert_eq!(
                    loading_activity(&app, &VisibleLoaders::default()),
                    Some("video")
                );
            }

            app.video.as_mut().unwrap().show(frame.clone());
            let actual = screen(&mut app, 100, height);
            assert!(!actual.contains("loading video"), "{actual}");
            assert!(!app.loading_active());

            app.video = Some(video::Player::unavailable(&url, Size::new(32, 18)));
            let actual = screen(&mut app, 100, height);
            assert!(!actual.contains("loading video"), "{actual}");
            assert!(!app.loading_active());

            app.video = None;
            app.config.media.video_autoplay = false;
            let actual = screen(&mut app, 100, height);
            assert!(!actual.contains("loading video"), "{actual}");
            assert!(app.video_loading().is_none());
            assert_ne!(
                loading_activity(&app, &VisibleLoaders::default()),
                Some("video")
            );
        }
    }

    #[test]
    fn when_a_video_poster_is_solid_dark_its_card_should_keep_its_play_indicator() {
        let mut app = video_card_with_poster(image::DynamicImage::ImageRgba8(
            image::RgbaImage::from_pixel(404, 719, image::Rgba([2, 0, 3, 255])),
        ));
        let reserved_height = app.active_tab().feed.cards.height;

        let actual = screen(&mut app, 100, 20);

        assert!(
            actual.contains("video") || actual.contains("gif"),
            "{actual}"
        );
        assert!(
            actual.contains('\u{10EEEE}')
                || app.image_picker.protocol_type() == ProtocolType::Halfblocks,
            "{actual}"
        );
        assert_eq!(app.active_tab().feed.cards.height, reserved_height);
        assert!(!app.card_renderer.pending());
    }

    #[test]
    fn when_a_video_poster_is_transparent_its_card_should_keep_its_play_indicator() {
        let mut app = video_card_with_poster(image::DynamicImage::ImageRgba8(
            image::RgbaImage::from_fn(2, 2, |x, y| image::Rgba([x as u8, y as u8, 255, 0])),
        ));
        app.image_picker = Picker::halfblocks();

        let actual = screen(&mut app, 100, 20);

        assert!(
            actual.contains("video") || actual.contains("gif"),
            "{actual}"
        );
        assert!(
            actual.contains('\u{10EEEE}')
                || app.image_picker.protocol_type() == ProtocolType::Halfblocks,
            "{actual}"
        );
        assert!(!app.card_renderer.pending());
    }

    #[test]
    fn when_a_video_poster_has_visible_content_its_card_should_render_the_pixels() {
        let mut app = video_card_with_poster(image::DynamicImage::ImageRgb8(
            image::RgbImage::from_fn(320, 180, |x, _| image::Rgb([x as u8, 128, 255])),
        ));

        let actual = screen(&mut app, 100, 20);

        assert!(actual.contains('\u{10EEEE}'));
        assert!(!actual.contains("▶ video · open post"));
    }

    #[test]
    fn when_a_flat_poster_url_is_used_by_an_image_post_it_should_still_render_the_pixels() {
        let mut app = video_card_with_poster(image::DynamicImage::new_rgb8(320, 180));
        app.tabs[4].feed.posts[0].media = Media::Image {
            url: "https://i.redd.it/p1.jpeg".into(),
            preview: None,
        };

        let actual = screen(&mut app, 100, 20);

        assert!(actual.contains('\u{10EEEE}'));
        assert!(!actual.contains("▶ video · open post"));
    }

    #[test]
    fn when_a_gif_card_has_a_flat_first_frame_it_should_keep_its_play_indicator() {
        let mut app = card_feed_app(kitty());
        app.config.media.video_autoplay = false;
        app.tabs[4].feed.posts.truncate(1);
        app.tabs[4].feed.posts[0].media = Media::Image {
            url: "https://i.redd.it/animation.gif".into(),
            preview: None,
        };
        first_screen(&mut app, 100, 20);
        app.on_msg(Msg::Image {
            url: "https://i.redd.it/animation.gif".into(),
            result: Ok(image::DynamicImage::new_rgba8(320, 180).into()),
        });

        let actual = screen(&mut app, 100, 20);

        assert!(
            actual.contains("video") || actual.contains("gif"),
            "{actual}"
        );
        assert!(
            actual.contains('\u{10EEEE}')
                || app.image_picker.protocol_type() == ProtocolType::Halfblocks,
            "{actual}"
        );
    }

    #[test]
    fn when_a_flat_video_preview_returns_after_eviction_its_indicator_should_stay_visible() {
        let mut app = video_card_with_poster(image::DynamicImage::new_rgb8(320, 180));
        screen(&mut app, 100, 20);
        app.card_images.clear();

        let actual = screen(&mut app, 100, 20);

        assert!(
            actual.contains("video") || actual.contains("gif"),
            "{actual}"
        );
        assert!(
            actual.contains('\u{10EEEE}')
                || app.image_picker.protocol_type() == ProtocolType::Halfblocks,
            "{actual}"
        );
        assert!(!actual.contains("loading image"));
    }

    #[test]
    fn when_a_retained_flat_video_preview_is_upgraded_it_should_render_the_new_pixels() {
        let mut app = video_card_with_poster(image::DynamicImage::new_rgb8(320, 180));
        screen(&mut app, 100, 20);
        app.card_images.clear();
        first_screen(&mut app, 100, 20);
        app.on_msg(Msg::Image {
            url: "https://i.redd.it/p1.jpeg".into(),
            result: Ok(image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(
                320,
                180,
                |x, _| image::Rgb([x as u8, 128, 255]),
            ))
            .into()),
        });

        let actual = screen(&mut app, 100, 20);

        assert!(actual.contains('\u{10EEEE}'));
        assert!(!actual.contains("▶ video · open post"));
    }

    #[test]
    fn when_a_retained_video_preview_is_upgraded_to_a_flat_frame_it_should_keep_the_play_indicator()
    {
        let mut app = video_card_with_poster(image::DynamicImage::ImageRgb8(
            image::RgbImage::from_fn(320, 180, |x, _| image::Rgb([x as u8, 128, 255])),
        ));
        screen(&mut app, 100, 20);
        app.card_images.clear();
        first_screen(&mut app, 100, 20);
        app.on_msg(Msg::Image {
            url: "https://i.redd.it/p1.jpeg".into(),
            result: Ok(image::DynamicImage::new_rgb8(320, 180).into()),
        });

        let actual = screen(&mut app, 100, 20);

        assert!(
            actual.contains("video") || actual.contains("gif"),
            "{actual}"
        );
        assert!(
            actual.contains('\u{10EEEE}')
                || app.image_picker.protocol_type() == ProtocolType::Halfblocks,
            "{actual}"
        );
    }

    #[test]
    fn when_card_style_marks_new_posts_the_dot_should_stay_orange_in_a_highlighted_title() {
        let mut app = feed_with_next_page(ListView::Card);
        let mut post = post_with("fresh", Media::None);
        post.title = "Target ● body".into();
        post.stickied = true;
        app.state.toggle_bookmark(&post, 1);
        app.tabs[4].feed.posts[0] = post;
        app.config.highlight.keywords = vec!["Target".into()];

        let actual = screen_buffer(&mut app, 100, 20);

        let dots: Vec<_> = (25..98)
            .map(|x| &actual[(x, 5)])
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
        app.tabs[4].feed.posts[0] = post;

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
        app.tabs[4].feed.posts[0] = post;

        let actual = screen_buffer(&mut app, 100, 20);

        let dots: Vec<_> = (25..98)
            .map(|x| &actual[(x, 5)])
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
            (4..24).all(|y| actual[(26, y)].symbol() == "▌" && actual[(26, y)].fg == ACCENT),
            "{}",
            buffer_text(&actual)
        );
        assert!(actual[(28, 6)].symbol().contains('\u{10EEEE}'));
        assert_eq!(actual[(26, 24)].symbol(), " ", "inter-card gap stays clear");
    }

    #[test]
    fn when_card_style_leaves_the_list_unfocused_the_whole_bar_should_use_its_muted_color() {
        let mut app = card_with_image(640, 320);
        app.focus = Focus::Sidebar;

        let actual = screen_buffer(&mut app, 100, 40);

        assert!(
            (4..24).all(|y| actual[(26, y)].symbol() == "│" && actual[(26, y)].fg == OVERLAY),
            "{}",
            buffer_text(&actual)
        );
    }

    #[test]
    fn when_card_style_scrolls_into_an_image_the_bar_should_remain_visible_with_native_pixels() {
        let mut app = card_with_image(320, 640);
        app.tabs[4].feed.posts.push(post_with("tail", Media::None));
        app.tabs[4].feed.posts.push(post_with("tail2", Media::None));
        app.scroll_card(3);
        std::thread::sleep(crate::app::NAV_SETTLE);
        app.tick();

        let actual = screen_buffer(&mut app, 100, 20);

        assert!(
            (4..12).all(|y| actual[(26, y)].symbol() == "▌" && actual[(26, y)].fg == ACCENT),
            "{}",
            buffer_text(&actual)
        );
        assert!(actual[(57, 4)].symbol().contains('\u{10EEEE}'));
        assert_eq!(app.active_tab().feed.cards.line, 3);
    }

    #[test]
    fn when_a_short_card_view_loads_pagination_it_should_keep_the_footer_status_visible() {
        let mut app = feed_with_next_page(ListView::Card);
        app.load_feed(4, true);

        let actual = screen(&mut app, 50, 5);

        assert!(actual.lines().last().unwrap().contains('⠋'), "{actual}");
        assert!(actual.contains("Existing"), "{actual}");
    }

    #[test]
    fn when_a_large_card_image_arrives_it_should_leave_the_ui_thread_responsive() {
        let mut app = card_feed_app(kitty());
        app.tabs[4].feed.posts.truncate(1);
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
        assert_eq!(app.active_tab().feed.cards.line, 0);
    }

    #[test]
    fn when_revisiting_a_card_after_pixel_eviction_its_geometry_should_stay_reserved() {
        let mut app = card_with_image(320, 640);
        app.tabs[4].feed.posts.push(post_with("p2", Media::None));
        screen(&mut app, 100, 20);
        app.card_images.clear();
        app.move_selection(1);
        screen(&mut app, 100, 20);
        app.move_selection(-1);

        let actual = screen(&mut app, 100, 20);

        assert_eq!(app.active_tab().feed.cards.height, 11, "{actual}");
        assert!(actual.contains("p2"), "{actual}");
    }

    #[test]
    fn when_returning_to_card_media_after_eviction_its_preview_should_stay_visible() {
        for video in [false, true] {
            let mut app = video_card_with_poster(image::DynamicImage::ImageRgb8(
                image::RgbImage::from_fn(320, 640, |x, _| image::Rgb([x as u8, 128, 255])),
            ));
            app.tabs[4].feed.posts[0].title = "Full width image".into();
            if video {
                app.tabs[4].feed.posts[0].media = Media::Video {
                    gif: false,
                    url: Some("https://v.redd.it/example/HLSPlaylist.m3u8".into()),
                    poster: Some("https://i.redd.it/p1.jpeg".into()),
                };
            } else {
                app.tabs[4].feed.posts[0].media = Media::Image {
                    url: "https://i.redd.it/p1.jpeg".into(),
                    preview: None,
                };
            }
            screen(&mut app, 100, 20);
            for number in 0..65 {
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
            assert_eq!(app.active_tab().feed.cards.height, 11);
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
            assert_eq!(app.active_tab().feed.cards.height, 11);
        }
    }

    #[test]
    fn when_a_card_has_listing_dimensions_loading_and_failure_should_keep_its_height() {
        let mut app = card_feed_app(kitty());
        app.tabs[4].feed.posts.truncate(1);
        app.tabs[4].feed.posts[0].title = "Reserved portrait".into();
        app.tabs[4].feed.posts[0].preview_size = Some((320, 640));
        screen(&mut app, 100, 20);
        assert_eq!(app.active_tab().feed.cards.height, 11);
        app.scroll_card(16);

        app.on_msg(Msg::Image {
            url: "https://i.redd.it/p1.jpeg".into(),
            result: Err("offline".into()),
        });
        let actual = screen(&mut app, 100, 20);

        assert_eq!(app.active_tab().feed.cards.height, 11, "{actual}");
        assert_eq!(app.active_tab().feed.cards.line, 0);
        assert!(actual.contains("offline"), "{actual}");
    }

    #[test]
    fn when_a_reserved_image_finishes_loading_the_selected_card_should_not_move() {
        let mut app = card_feed_app(kitty());
        app.tabs[4].feed.posts.truncate(1);
        app.tabs[4].feed.posts[0].title = "Reserved landscape".into();
        app.tabs[4].feed.posts[0].preview_size = Some((640, 320));
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
        app.tabs[4].feed.posts.iter_mut().for_each(|post| {
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
    fn a_top_card_scrolled_into_keeps_its_place_when_an_answer_reranks_or_drops_it() {
        // The scroll may not be drawn yet when the answer arrives.
        for (drawn, drop) in [(true, false), (false, false), (true, true), (false, true)] {
            let mut app = card_feed_app(kitty());
            screen(&mut app, 100, 20);
            let read = app.current_post().unwrap().id;
            app.scroll_card(3);
            if drawn {
                screen(&mut app, 100, 20);
                assert!(app.active_tab().feed.cards.line > 0);
            }
            let mut posts = app.tabs[4].feed.posts.clone();
            if drop {
                posts.remove(0);
            } else {
                posts.swap(0, 1);
            }
            let (tab, generation) = (app.tabs[4].id, app.tabs[4].feed.generation);

            app.on_msg(Msg::Feed {
                tab,
                generation,
                append: false,
                result: Ok(crate::model::Listing { posts, after: None }),
            });
            screen(&mut app, 100, 20);

            assert_eq!(
                app.current_post().unwrap().id,
                read,
                "drawn {drawn} drop {drop}"
            );
            assert_eq!(app.active_tab().feed.cards.line, 3);
        }

        // Scrolling back to the top, or up at it, before the answer is drawn leaves the
        // top untouched: it shows the answer's first post from its first row.
        for back in [true, false] {
            let mut app = card_feed_app(kitty());
            screen(&mut app, 100, 20);
            if back {
                app.scroll_card(3);
                screen(&mut app, 100, 20);
                app.scroll_card(-3);
            } else {
                app.scroll_card(-5);
            }
            let mut posts = app.tabs[4].feed.posts.clone();
            posts.swap(0, 1);
            let first = posts[0].id.clone();
            let (tab, generation) = (app.tabs[4].id, app.tabs[4].feed.generation);

            app.on_msg(Msg::Feed {
                tab,
                generation,
                append: false,
                result: Ok(crate::model::Listing { posts, after: None }),
            });
            screen(&mut app, 100, 20);

            assert_eq!(app.current_post().unwrap().id, first, "back {back}");
            assert_eq!(app.active_tab().feed.selected, 0);
            assert_eq!(app.active_tab().feed.cards.line, 0);
        }
    }

    #[test]
    fn scroll_keys_before_the_draw_scroll_from_the_new_top() {
        // Between an answer at the top, or `r` bringing new posts from further down, and
        // the next draw, scroll keys scroll from the new first post.
        for (page, manual) in [(false, false), (true, false), (false, true), (true, true)] {
            let mut app = card_feed_app(kitty());
            screen(&mut app, 100, 20);
            let mut posts = app.tabs[4].feed.posts.clone();
            if manual {
                app.move_selection(3);
                screen(&mut app, 100, 20);
                let mut new = posts[0].clone();
                new.id = "new".into();
                posts.insert(0, new);
                app.reload();
            } else {
                posts.swap(0, 1);
            }
            let first = posts[0].id.clone();
            let (tab, generation) = (app.tabs[4].id, app.tabs[4].feed.generation);
            app.on_msg(Msg::Feed {
                tab,
                generation,
                append: false,
                result: Ok(crate::model::Listing { posts, after: None }),
            });

            if page {
                app.page_list(1, 2);
            } else {
                app.scroll_card(3);
            }
            screen(&mut app, 100, 20);

            let feed = &app.active_tab().feed;
            let case = format!("page {page} manual {manual}");
            if page {
                assert!(feed.selected <= 1, "{case}: {}", feed.selected);
                assert!(feed.selected > 0 || feed.cards.line > 0, "{case}");
            } else {
                assert_eq!(app.current_post().unwrap().id, first, "{case}");
                assert_eq!((feed.offset, feed.cards.line), (0, 3), "{case}");
            }
        }
    }

    #[test]
    fn when_a_scrolled_loading_card_receives_pixels_it_should_keep_the_visible_row() {
        let mut app = card_feed_app(kitty());
        app.tabs[4].feed.posts.truncate(1);
        app.tabs[4].feed.posts.push(post_with("tail", Media::None));
        app.tabs[4].feed.posts.push(post_with("tail2", Media::None));
        app.tabs[4].feed.posts[0].title = "Loading image".into();
        screen(&mut app, 100, 20);
        app.scroll_card(3);
        screen(&mut app, 100, 20);
        assert_eq!(app.active_tab().feed.cards.line, 3);
        app.scroll_card(-1);
        screen(&mut app, 100, 20);

        app.on_msg(Msg::Image {
            url: "https://i.redd.it/p1.jpeg".into(),
            result: Ok(image::DynamicImage::new_rgb8(640, 1280).into()),
        });
        screen(&mut app, 100, 20);

        assert_eq!(app.active_tab().feed.cards.line, 2);
        assert_eq!(app.active_tab().feed.cards.pending, 0);
    }

    #[test]
    fn when_paging_a_bookmark_before_its_image_loads_it_should_keep_all_reserved_media_visible() {
        let mut app = card_feed_app(kitty());
        app.tabs[4].feed.posts[0].title = "Reserved bookmark".into();
        app.tabs[4].feed.posts[0].preview_size = Some((320, 640));
        app.state.toggle_bookmark(&app.current_post().unwrap(), 1);
        app.switch_tab(1);
        screen(&mut app, 100, 20);

        app.page_list(1, 1);
        screen(&mut app, 100, 20);

        assert_eq!(app.active_tab().feed.cards.line, 0);
        assert_eq!(app.active_tab().feed.cards.height, 11);
        assert_eq!(app.current_post().unwrap().id, "p1");
    }

    #[test]
    fn when_a_landscape_card_image_loads_it_should_fit_the_available_height() {
        let mut app = card_with_image(640, 320);

        let cells = placeholder_cells(&mut app, 100, 20);

        assert_eq!(cells.iter().map(|cell| cell.0).min(), Some(45));
        assert_eq!(cells.iter().map(|cell| cell.0).max(), Some(76));
        assert!(matches!(app.card_images.get("https://i.redd.it/p1.jpeg"),
            Some(CardImageSlot::Ready { protocol: Some(protocol), .. })
                if protocol.size == Size::new(32, 8)));
    }

    #[test]
    fn when_a_portrait_card_image_loads_it_should_fit_height_and_center_in_the_viewport() {
        let mut app = card_with_image(320, 640);

        let cells = placeholder_cells(&mut app, 100, 20);

        assert_eq!(cells.iter().map(|cell| cell.0).min(), Some(57));
        assert_eq!(cells.iter().map(|cell| cell.0).max(), Some(64));
        assert!(matches!(app.card_images.get("https://i.redd.it/p1.jpeg"),
            Some(CardImageSlot::Ready { protocol: Some(protocol), .. })
                if protocol.size == Size::new(8, 8)));
    }

    #[test]
    fn when_a_small_card_image_loads_it_should_fill_its_fitted_box_without_the_text_upscaling_limit()
     {
        let mut app = card_with_image(16, 8);

        let cells = placeholder_cells(&mut app, 100, 20);

        assert_eq!(cells.iter().map(|cell| cell.0).min(), Some(45));
        assert_eq!(cells.iter().map(|cell| cell.0).max(), Some(76));
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
                (Size::new(68, 34), (2, 2), None),
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
            (Size::new(68, 27200), (2, 1600), None),
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
        let crate::card_image::Tile::Image(encoded) = protocol.tiles[&(0, 0, 1, 66)].as_ref()
        else {
            panic!("still-image tile");
        };
        let pixels = card_png(encoded);
        assert_eq!(pixels.dimensions(), (528, 16));
        assert!((120..=136).contains(&pixels.get_pixel(264, 8)[0]));
    }

    #[test]
    fn when_paging_a_single_fitted_card_it_should_keep_the_selected_post() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = card_with_image(320, 640);
        screen(&mut app, 100, 20);

        crate::keys::handle(
            &mut app,
            KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
        );
        let cells = placeholder_cells(&mut app, 100, 20);

        assert_eq!(app.current_post().unwrap().id, "p1");
        assert_eq!(app.active_tab().feed.cards.line, 0);
        assert_eq!(cells.iter().map(|cell| cell.0).max(), Some(64));
    }

    fn two_tall_media_cards() -> App {
        let mut app = card_feed_app(kitty());
        let mut first = app.tabs[4].feed.posts[0].clone();
        first.title = "Full width image".into();
        first.preview_size = Some((320, 640));
        let mut next = post_with(
            "p2",
            Media::Video {
                gif: false,
                url: Some("https://v.redd.it/example/HLSPlaylist.m3u8".into()),
                poster: Some("https://i.redd.it/p2.jpeg".into()),
            },
        );
        next.title = "Following video".into();
        next.preview_size = Some((320, 640));
        app.reload();
        app.on_msg(Msg::Feed {
            tab: app.active_tab().id,
            generation: app.active_tab().feed.generation,
            append: false,
            result: Ok(crate::model::Listing {
                posts: vec![first, next],
                after: None,
            }),
        });
        first_screen(&mut app, 100, 20);
        app.on_msg(Msg::Image {
            url: "https://i.redd.it/p1.jpeg".into(),
            result: Ok(image::DynamicImage::new_rgb8(320, 640).into()),
        });
        screen(&mut app, 100, 20);
        app
    }

    #[test]
    fn when_half_paging_past_a_tall_card_the_next_card_should_enter_without_snapping() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = two_tall_media_cards();
        app.scroll_card(0);
        screen(&mut app, 100, 20);

        crate::keys::handle(
            &mut app,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
        );
        let actual = screen(&mut app, 100, 20);

        assert_eq!(
            actual
                .lines()
                .position(|line| line.contains("Following video")),
            Some(10)
        );
        assert_eq!(app.active_tab().feed.cards.line, 7);
        assert_eq!(app.current_post().unwrap().id, "p1");
    }

    #[test]
    fn when_half_paging_back_across_a_card_header_it_should_preserve_the_remaining_distance() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = two_tall_media_cards();
        app.move_selection(1);
        screen(&mut app, 100, 20);

        crate::keys::handle(
            &mut app,
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
        );
        let actual = screen(&mut app, 100, 20);

        assert_eq!(app.active_tab().feed.offset, 0);
        assert_eq!(app.active_tab().feed.cards.line, 2);
        assert_eq!(
            actual
                .lines()
                .position(|line| line.contains("Following video")),
            Some(15)
        );
    }

    #[test]
    fn when_half_page_keys_are_batched_their_row_distances_should_accumulate() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = two_tall_media_cards();
        app.tabs[4].feed.posts.push(post_with("tail", Media::None));
        app.scroll_card(0);
        screen(&mut app, 100, 20);

        crate::keys::handle(
            &mut app,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
        );
        crate::keys::handle(
            &mut app,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
        );
        let actual = screen(&mut app, 100, 20);

        assert_eq!(app.active_tab().feed.offset, 1);
        assert_eq!(app.active_tab().feed.cards.line, 2);
        assert_eq!(
            actual
                .lines()
                .position(|line| line.contains("Following video")),
            None
        );
        assert_eq!(app.active_tab().feed.cards.pending, 0);
    }

    #[test]
    fn when_line_scrolling_crosses_a_separator_it_should_move_exactly_one_row_each_way() {
        let mut app = two_tall_media_cards();
        app.tabs[4].feed.posts.push(post_with("tail", Media::None));
        app.scroll_card(11);
        screen(&mut app, 100, 20);

        app.scroll_card(1);
        let down = screen(&mut app, 100, 20);
        app.scroll_card(-1);
        let up = screen(&mut app, 100, 20);

        assert_eq!(
            down.lines()
                .position(|line| line.contains("Following video")),
            Some(5)
        );
        assert_eq!(
            up.lines().position(|line| line.contains("Following video")),
            Some(6)
        );
        assert_eq!(app.active_tab().feed.cards.line, 11);
    }

    #[test]
    fn when_a_half_page_spans_short_cards_it_should_consume_all_rows() {
        let mut app = card_feed_app(kitty());
        for post in &mut app.tabs[4].feed.posts {
            post.media = Media::None;
            post.title = post.id.clone();
        }
        screen(&mut app, 100, 20);

        app.page_list(1, 2);
        screen(&mut app, 100, 20);

        assert_eq!(app.active_tab().feed.offset, 1);
        assert_eq!(app.active_tab().feed.cards.line, 3);
        assert_eq!(app.current_post().unwrap().id, "p3");
    }

    #[test]
    fn when_scrolling_from_a_short_card_into_tall_media_it_should_keep_the_overshoot() {
        let mut app = two_tall_media_cards();
        app.tabs[4].feed.posts[0].media = Media::None;
        let mut following = app.tabs[4].feed.posts[1].clone();
        following.id = "tail".into();
        app.tabs[4].feed.posts.push(following);
        screen(&mut app, 100, 20);

        app.page_list(1, 2);
        screen(&mut app, 100, 20);

        assert_eq!(app.active_tab().feed.offset, 1);
        assert_eq!(app.active_tab().feed.cards.line, 3);
        assert_eq!(app.current_post().unwrap().id, "p2");
    }

    #[test]
    fn when_item_selection_follows_continuous_scrolling_it_should_reveal_the_selected_post() {
        let mut app = two_tall_media_cards();
        app.scroll_card(8);
        screen(&mut app, 100, 20);

        press(&mut app, 'j');
        let actual = screen(&mut app, 100, 20);

        assert_eq!(app.current_post().unwrap().id, "p2");
        assert_eq!(app.active_tab().feed.offset, 0);
        assert_eq!(app.active_tab().feed.cards.line, 9);
        assert_eq!(
            actual
                .lines()
                .position(|line| line.contains("Following video")),
            Some(8)
        );
    }

    #[test]
    fn when_bookmarks_half_page_across_tall_media_the_next_card_should_enter_naturally() {
        let mut app = two_tall_media_cards();
        app.state
            .toggle_bookmark(&app.tabs[4].feed.posts[1].clone(), 1);
        app.state
            .toggle_bookmark(&app.tabs[4].feed.posts[0].clone(), 2);
        app.switch_tab(1);
        screen(&mut app, 100, 20);
        app.scroll_card(0);
        screen(&mut app, 100, 20);

        app.page_list(1, 2);
        let actual = screen(&mut app, 100, 20);

        assert_eq!(app.active_tab().feed.cards.line, 7);
        assert_eq!(
            actual
                .lines()
                .position(|line| line.contains("Following video")),
            Some(10)
        );
    }

    #[test]
    fn when_bookmarks_reorder_before_the_viewport_its_anchor_should_follow_the_post() {
        let mut app = two_tall_media_cards();
        app.state
            .toggle_bookmark(&app.tabs[4].feed.posts[1].clone(), 1);
        app.state
            .toggle_bookmark(&app.tabs[4].feed.posts[0].clone(), 2);
        app.switch_tab(1);
        screen(&mut app, 100, 20);
        app.scroll_card(8);
        screen(&mut app, 100, 20);
        let selected = app.current_post().unwrap().id;

        app.state
            .toggle_bookmark(&post_with("inserted", Media::None), 3);
        // The state merge re-selects the bookmark by id.
        let index = app
            .visible_bookmarks()
            .position(|bookmark| bookmark.post.id == selected)
            .unwrap();
        app.tabs[1].feed.selected = index;
        screen(&mut app, 100, 20);

        assert_eq!(app.active_tab().feed.cards.post.as_deref(), Some("p1"));
        assert_eq!(app.active_tab().feed.offset, 1);
        assert_eq!(app.active_tab().feed.cards.line, 8);
    }

    #[test]
    fn when_the_scrolled_bookmark_is_removed_it_should_reveal_a_surviving_post() {
        let mut app = two_tall_media_cards();
        let second = app.tabs[4].feed.posts[1].clone();
        app.state.toggle_bookmark(&second, 1);
        app.state
            .toggle_bookmark(&app.tabs[4].feed.posts[0].clone(), 2);
        app.switch_tab(1);
        screen(&mut app, 100, 20);
        app.scroll_card(82);
        screen(&mut app, 100, 20);

        app.state.toggle_bookmark(&second, 3);
        screen(&mut app, 100, 20);

        assert_eq!(app.active_tab().feed.cards.post.as_deref(), Some("p1"));
        assert_eq!(app.active_tab().feed.cards.line, 0);
    }

    #[test]
    fn when_bookmarks_are_empty_row_keys_should_not_queue_a_scroll_for_future_entries() {
        let mut app = two_tall_media_cards();
        let first = app.tabs[4].feed.posts[0].clone();
        app.state.toggle_bookmark(&first, 1);
        app.switch_tab(1);
        screen(&mut app, 100, 20);
        app.state.toggle_bookmark(&first, 2);
        screen(&mut app, 100, 20);

        app.scroll_card(8);

        assert_eq!(app.active_tab().feed.cards.pending, 0);
    }

    #[test]
    fn buffered_feed_refresh_shows_the_list_top_without_reordering_pinned_posts() {
        for (view, index) in [
            (crate::state::ListView::Card, 0),
            (crate::state::ListView::Compact, 0),
            (crate::state::ListView::Card, 4),
            (crate::state::ListView::Compact, 4),
        ] {
            for width in [40, 100] {
                let mut app = app(&["rust"]);
                if app.state.list_view() != view {
                    app.state.toggle_list_view();
                }
                app.toggle_sidebar();
                app.switch_tab(index);
                let tab = app.active_tab().id;
                let mut first = post_with("old", Media::None);
                first.title = "Existing post".into();
                first.stickied = true;
                let second = post_with("second", Media::None);
                let answer = |generation, posts| Msg::Feed {
                    tab,
                    generation,
                    append: false,
                    result: Ok(crate::model::Listing { posts, after: None }),
                };
                app.on_msg(answer(
                    app.active_tab().feed.generation,
                    vec![first.clone(), second.clone()],
                ));
                let before = screen(&mut app, width, 20);
                let row = before
                    .lines()
                    .position(|line| line.contains(&first.title))
                    .unwrap();
                app.load_feed(index, false);
                let mut next = post_with("new", Media::None);
                next.title = "Incoming post".into();

                app.on_msg(answer(
                    app.active_tab().feed.generation,
                    vec![first, next.clone(), second],
                ));
                let pending = screen(&mut app, width, 20);

                assert!(pending.contains("New posts · r refresh"), "{pending}");
                assert!(!pending.contains(&next.title), "{pending}");
                assert_eq!(
                    pending
                        .lines()
                        .position(|line| line.contains("Existing post")),
                    Some(row)
                );
                crate::keys::handle(
                    &mut app,
                    crossterm::event::KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
                );
                let refreshed = screen(&mut app, width, 20);
                assert!(!refreshed.contains("r refresh"), "{refreshed}");
                assert_eq!(
                    app.active_tab()
                        .feed
                        .posts
                        .iter()
                        .map(|post| post.id.as_str())
                        .collect::<Vec<_>>(),
                    ["old", "new", "second"]
                );
                assert_eq!(app.current_post().unwrap().id, "old");
                assert!(refreshed.contains("Existing post"), "{refreshed}");
                assert!(refreshed.contains(&next.title), "{refreshed}");
            }
        }
    }

    #[test]
    fn live_reorders_keep_the_selected_card_selected_and_visible() {
        let mut app = app(&["rust"]);
        if app.state.list_view() != crate::state::ListView::Card {
            app.state.toggle_list_view();
        }
        app.toggle_sidebar();
        app.switch_tab(4);
        let tab = app.active_tab().id;
        let posts: Vec<_> = (0..8)
            .map(|n| {
                let mut post = post_with(&format!("p{n}"), Media::None);
                post.title = format!("Post number {n}");
                post
            })
            .collect();
        let answer = |generation, posts| Msg::Feed {
            tab,
            generation,
            append: false,
            result: Ok(crate::model::Listing { posts, after: None }),
        };
        app.on_msg(answer(app.active_tab().feed.generation, posts.clone()));
        screen(&mut app, 100, 20);
        app.move_selection(1);
        screen(&mut app, 100, 20);
        assert_eq!(app.current_post().unwrap().id, "p1");

        let mut reordered = posts.clone();
        let selected = reordered.remove(1);
        reordered.push(selected);
        app.load_feed(app.active, false);
        app.on_msg(answer(app.active_tab().feed.generation, reordered));
        let shown = screen(&mut app, 100, 20);

        assert_eq!(app.current_post().unwrap().id, "p1");
        assert!(shown.contains("Post number 1"), "{shown}");
    }

    #[test]
    fn a_card_entering_or_leaving_above_the_selection_keeps_the_selected_card_on_its_row() {
        let mut app = app(&["rust"]);
        if app.state.list_view() != crate::state::ListView::Card {
            app.state.toggle_list_view();
        }
        app.toggle_sidebar();
        app.switch_tab(4);
        let tab = app.active_tab().id;
        let post = |id: &str| {
            let mut post = post_with(id, Media::None);
            post.title = format!("Post {id}");
            post
        };
        let answer = |generation, ids: &[&str]| Msg::Feed {
            tab,
            generation,
            append: false,
            result: Ok(crate::model::Listing {
                posts: ids.iter().map(|id| post(id)).collect(),
                after: None,
            }),
        };
        let row = |shown: &str| shown.lines().position(|line| line.contains("Post p2"));
        app.on_msg(answer(
            app.active_tab().feed.generation,
            &["p0", "p1", "p2", "p3", "p4", "p5", "p6", "p7", "p8", "p9"],
        ));
        screen(&mut app, 100, 30);
        app.move_selection(2);
        let before = row(&screen(&mut app, 100, 30));
        assert!(before.is_some());

        for ids in [
            [
                "p0", "m", "p1", "p2", "p3", "p4", "p5", "p6", "p7", "p8", "p9",
            ]
            .as_slice(),
            &["p0", "p1", "p2", "p3", "p4", "p5", "p6", "p7", "p8", "p9"],
        ] {
            app.load_feed(app.active, false);
            app.on_msg(answer(app.active_tab().feed.generation, ids));
            let shown = screen(&mut app, 100, 30);

            assert_eq!(app.current_post().unwrap().id, "p2");
            assert_eq!(row(&shown), before, "{shown}");
        }
    }

    #[test]
    fn shrinking_the_terminal_keeps_a_low_selected_card_selected_and_visible() {
        let mut app = app(&["rust"]);
        if app.state.list_view() != crate::state::ListView::Card {
            app.state.toggle_list_view();
        }
        app.toggle_sidebar();
        app.switch_tab(4);
        let tab = app.active_tab().id;
        app.on_msg(Msg::Feed {
            tab,
            generation: app.active_tab().feed.generation,
            append: false,
            result: Ok(crate::model::Listing {
                posts: (0..10)
                    .map(|n| {
                        let mut post = post_with(&format!("p{n}"), Media::None);
                        post.title = format!("Post p{n}");
                        post
                    })
                    .collect(),
                after: None,
            }),
        });
        screen(&mut app, 100, 40);
        app.move_selection(6);

        for height in [29, 14] {
            assert!(screen(&mut app, 100, 40).contains("Post p6"));
            let shown = screen(&mut app, 100, height);

            assert_eq!(app.current_post().unwrap().id, "p6");
            let lines: Vec<_> = shown.lines().collect();
            let title = lines.iter().position(|line| line.contains("Post p6"));
            assert!(
                title.is_some_and(|title| lines[title + 1].contains("comments")),
                "{height}: {shown}"
            );
        }
    }

    #[test]
    fn a_redraw_without_input_leaves_the_card_list_in_place() {
        let mut app = app(&["rust"]);
        if app.state.list_view() != crate::state::ListView::Card {
            app.state.toggle_list_view();
        }
        app.toggle_sidebar();
        app.switch_tab(4);
        let tab = app.active_tab().id;
        app.on_msg(Msg::Feed {
            tab,
            generation: app.active_tab().feed.generation,
            append: false,
            result: Ok(crate::model::Listing {
                posts: (0..10)
                    .map(|n| {
                        let mut post = post_with(&format!("p{n}"), Media::None);
                        post.title = format!("Post p{n}");
                        post
                    })
                    .collect(),
                after: None,
            }),
        });
        screen(&mut app, 100, 20);
        app.move_selection(5);
        screen(&mut app, 100, 20);
        app.scroll_card(-2);
        let shown = screen(&mut app, 100, 20);

        assert_eq!(screen(&mut app, 100, 20), shown);
    }

    #[test]
    fn narrowing_the_list_keeps_a_rewrapped_selected_card_whole() {
        let mut app = app(&["rust"]);
        if app.state.list_view() != crate::state::ListView::Card {
            app.state.toggle_list_view();
        }
        app.toggle_sidebar();
        app.switch_tab(4);
        let tab = app.active_tab().id;
        app.on_msg(Msg::Feed {
            tab,
            generation: app.active_tab().feed.generation,
            append: false,
            result: Ok(crate::model::Listing {
                posts: (0..10)
                    .map(|n| {
                        let mut post = post_with(&format!("p{n}"), Media::None);
                        post.title = format!(
                            "Post p{n} alpha beta gamma delta epsilon zeta eta theta iota kappa end{n}"
                        );
                        post
                    })
                    .collect(),
                after: None,
            }),
        });
        screen(&mut app, 160, 30);
        app.move_selection(6);
        screen(&mut app, 160, 30);
        app.scroll_card(-3);
        assert!(screen(&mut app, 160, 30).contains("end6"));

        let shown = screen(&mut app, 40, 30);

        assert_eq!(app.current_post().unwrap().id, "p6");
        let lines: Vec<_> = shown.lines().collect();
        let title = lines.iter().position(|line| line.contains("end6"));
        assert!(
            title.is_some_and(|title| lines[title + 1].contains("comments")),
            "{shown}"
        );
    }

    #[test]
    fn existing_feed_updates_apply_while_only_new_posts_require_refresh() {
        for view in [
            crate::state::ListView::Card,
            crate::state::ListView::Compact,
        ] {
            let mut app = app(&["AcademicUAP"]);
            if app.state.list_view() != view {
                app.state.toggle_list_view();
            }
            app.toggle_sidebar();
            app.switch_tab(4);
            app.config.filters.authors.push("blocked".into());
            let tab = app.active_tab().id;
            let answer = |generation, posts| Msg::Feed {
                tab,
                generation,
                append: false,
                result: Ok(crate::model::Listing { posts, after: None }),
            };
            let mut first = post_with(
                "first",
                Media::Video {
                    url: Some("https://v.redd.it/video/HLSPlaylist.m3u8?a=old".into()),
                    poster: None,
                    gif: false,
                },
            );
            first.title = "Existing post".into();
            let second = post_with("second", Media::None);
            let mut hidden = post_with("known-hidden", Media::None);
            hidden.author = "blocked".into();
            app.on_msg(answer(
                app.active_tab().feed.generation,
                vec![first.clone(), second.clone(), hidden.clone()],
            ));
            screen(&mut app, 100, 20);

            first.title = "Updated existing post".into();
            first.score = Some(42);
            first.comments = 12;
            first.media = Media::Video {
                url: Some("https://v.redd.it/video/HLSPlaylist.m3u8?a=new".into()),
                poster: None,
                gif: false,
            };
            hidden.author = "visible".into();
            let mut filtered_new = post_with("filtered-new", Media::None);
            filtered_new.author = "blocked".into();
            app.load_feed(4, false);
            app.on_msg(answer(
                app.active_tab().feed.generation,
                vec![
                    second.clone(),
                    first.clone(),
                    hidden.clone(),
                    filtered_new.clone(),
                ],
            ));
            let updated = screen(&mut app, 100, 20);

            assert!(updated.contains("Updated existing post"), "{updated}");
            assert!(!updated.contains("r refresh"), "{updated}");
            // The untouched top stays the top as the list reorders.
            assert_eq!(app.current_post().unwrap().id, "second");
            assert_eq!(app.active_tab().feed.posts[0].id, "second");
            let updated_first = &app.active_tab().feed.posts[1];
            assert_eq!(updated_first.id, "first");
            assert_eq!(updated_first.score, Some(42));
            assert_eq!(updated_first.comments, 12);
            assert_eq!(updated_first.media, first.media);
            assert!(
                app.config
                    .visible(&app.active_tab().feed.posts)
                    .any(|post| post.id == "known-hidden")
            );

            first.title = "Live existing edit".into();
            first.score = Some(84);
            let mut incoming = post_with("new", Media::None);
            incoming.title = "Incoming post".into();
            app.load_feed(4, false);
            app.on_msg(answer(
                app.active_tab().feed.generation,
                vec![
                    incoming.clone(),
                    second.clone(),
                    first.clone(),
                    hidden.clone(),
                    filtered_new,
                ],
            ));
            let pending = screen(&mut app, 100, 20);

            assert!(pending.contains("New posts · r refresh"), "{pending}");
            assert!(pending.contains("Live existing edit"), "{pending}");
            assert!(!pending.contains("Incoming post"), "{pending}");
            assert_eq!(app.current_post().unwrap().id, "second");
            assert!(
                app.active_tab()
                    .feed
                    .posts
                    .iter()
                    .any(|post| post.id == "first" && post.score == Some(84))
            );

            let mut filtered_next = post_with("filtered-next", Media::None);
            filtered_next.author = "blocked".into();
            app.load_feed(4, false);
            app.on_msg(answer(
                app.active_tab().feed.generation,
                vec![incoming, second, first.clone(), hidden, filtered_next],
            ));
            assert!(
                !app.active_tab()
                    .feed
                    .posts
                    .iter()
                    .any(|post| post.id == "filtered-new")
            );

            press_keys(&mut app, "r");
            let refreshed = screen(&mut app, 100, 20);
            assert!(!refreshed.contains("r refresh"), "{refreshed}");
            assert!(refreshed.contains("Incoming post"), "{refreshed}");
            assert_eq!(app.current_post().unwrap().id, "new");

            app.load_feed(4, false);
            app.on_msg(answer(app.active_tab().feed.generation, vec![first]));
            let removed = screen(&mut app, 100, 20);
            assert!(!removed.contains("r refresh"), "{removed}");
            assert_eq!(app.current_post().unwrap().id, "first");
            assert_eq!(app.active_tab().feed.posts.len(), 1);
        }
    }

    #[test]
    fn manual_refresh_reveals_arrival_headers_after_fetch_or_buffered_updates() {
        let mut app = two_tall_media_cards();
        let initial = screen(&mut app, 100, 20);
        let title_row = initial
            .lines()
            .position(|line| line.contains("Full width image"))
            .unwrap();
        app.scroll_card(8);
        screen(&mut app, 100, 20);
        let mut posts = app.tabs[4].feed.posts.clone();
        let mut fetched = post_with(
            "inserted",
            Media::Image {
                url: "https://i.redd.it/fetched.jpeg".into(),
                preview: None,
            },
        );
        fetched.title = "Fetched arrival".into();
        posts.insert(0, fetched);
        app.reload();

        app.on_msg(Msg::Feed {
            tab: app.active_tab().id,
            generation: app.active_tab().feed.generation,
            append: false,
            result: Ok(crate::model::Listing { posts, after: None }),
        });
        let refreshed = screen(&mut app, 100, 20);

        assert_eq!(app.current_post().unwrap().id, "inserted");
        assert_eq!(
            app.active_tab().feed.cards.post.as_deref(),
            Some("inserted")
        );
        assert_eq!(app.active_tab().feed.cards.line, 0);
        assert_eq!(
            refreshed
                .lines()
                .position(|line| line.contains("Fetched arrival")),
            Some(title_row)
        );
        app.on_msg(Msg::Image {
            url: "https://i.redd.it/fetched.jpeg".into(),
            result: Ok(image::DynamicImage::new_rgb8(320, 640).into()),
        });
        let loaded = screen(&mut app, 100, 20);
        assert_eq!(app.active_tab().feed.cards.line, 0);
        assert_eq!(
            loaded
                .lines()
                .position(|line| line.contains("Fetched arrival")),
            Some(title_row)
        );

        app.move_selection(1);
        screen(&mut app, 100, 20);
        app.scroll_card(8);
        screen(&mut app, 100, 20);

        let mut posts = app.active_tab().feed.posts.clone();
        let mut incoming = post_with("newer", Media::None);
        incoming.title = "Newest post".into();
        posts.insert(0, incoming);
        app.load_feed(app.active, false);
        app.on_msg(Msg::Feed {
            tab: app.active_tab().id,
            generation: app.active_tab().feed.generation,
            append: false,
            result: Ok(crate::model::Listing { posts, after: None }),
        });
        screen(&mut app, 100, 20);
        assert_eq!(app.active_tab().feed.cards.post.as_deref(), Some("p1"));
        assert_eq!(app.active_tab().feed.cards.line, 8);

        press_keys(&mut app, "r");
        let refreshed = screen(&mut app, 100, 20);

        assert_eq!(app.current_post().unwrap().id, "newer");
        assert_eq!(app.active_tab().feed.cards.post.as_deref(), Some("newer"));
        assert_eq!(app.active_tab().feed.cards.line, 0);
        assert_eq!(
            refreshed
                .lines()
                .position(|line| line.contains("Newest post")),
            Some(title_row)
        );
    }

    #[test]
    fn when_a_refresh_removes_the_viewport_anchor_it_should_reveal_a_surviving_post() {
        let mut app = two_tall_media_cards();
        app.scroll_card(82);
        screen(&mut app, 100, 20);
        let posts = vec![app.tabs[4].feed.posts[0].clone()];
        app.reload();

        app.on_msg(Msg::Feed {
            tab: app.active_tab().id,
            generation: app.active_tab().feed.generation,
            append: false,
            result: Ok(crate::model::Listing { posts, after: None }),
        });
        screen(&mut app, 100, 20);

        assert_eq!(app.active_tab().feed.cards.post.as_deref(), Some("p1"));
        assert_eq!(app.active_tab().feed.cards.line, 0);
        assert_eq!(app.current_post().unwrap().id, "p1");
    }

    #[test]
    fn when_resize_shortens_the_anchor_card_it_should_clamp_to_that_cards_content() {
        let mut app = two_tall_media_cards();
        app.scroll_card(14);
        screen(&mut app, 100, 20);

        screen(&mut app, 80, 12);

        assert_eq!(app.active_tab().feed.cards.post.as_deref(), Some("p1"));
        assert_eq!(app.active_tab().feed.cards.line, 5);
        assert_eq!(app.current_post().unwrap().id, "p1");
    }

    #[test]
    fn when_scrolling_a_short_loaded_feed_it_should_prefetch_at_the_visible_end() {
        let mut app = card_feed_app(kitty());
        app.tabs[4].feed.posts = (0..20)
            .map(|i| {
                let mut post = post_with(&format!("short{i}"), Media::None);
                post.title = post.id.clone();
                post
            })
            .collect();
        screen(&mut app, 100, 100);
        app.tabs[4].feed.after = Some("next".into());

        app.scroll_card(1);
        screen(&mut app, 100, 100);

        assert!(!app.active_tab().feed.loading());
        std::thread::sleep(crate::app::NAV_SETTLE);
        app.tick();
        assert!(app.active_tab().feed.loading());
        assert_eq!(app.active_tab().feed.selected, 0);
        assert_eq!(app.active_tab().feed.cards.line, 0);
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
        assert_eq!(app.active_tab().feed.cards.line, 0);
        assert_eq!(app.current_post().unwrap().id, "p1");
    }

    #[test]
    fn when_scrolling_up_a_card_by_control_y_it_should_move_one_line_without_copying() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = card_with_image(320, 640);
        app.tabs[4].feed.posts.push(post_with("tail", Media::None));
        app.tabs[4].feed.posts.push(post_with("tail2", Media::None));
        screen(&mut app, 100, 20);
        app.scroll_card(5);

        crate::keys::handle(
            &mut app,
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL),
        );
        screen(&mut app, 100, 20);

        assert_eq!(app.active_tab().feed.cards.line, 4);
        assert!(app.status.is_none(), "Ctrl+y must not copy a link");
    }

    #[test]
    fn when_pressing_g_inside_the_first_card_it_should_return_to_its_top() {
        let mut app = card_with_image(320, 640);
        screen(&mut app, 100, 20);
        app.scroll_card(5);

        press_keys(&mut app, "gg");
        let actual = screen(&mut app, 100, 20);

        assert!(actual.contains("Full width image"), "{actual}");
        assert_eq!(app.active_tab().feed.cards.line, 0);
    }

    #[test]
    fn when_resizing_a_card_it_should_refit_the_image_to_the_new_width() {
        let mut app = card_with_image(640, 320);
        placeholder_cells(&mut app, 100, 20);

        let cells = placeholder_cells(&mut app, 80, 20);

        assert_eq!(cells.iter().map(|cell| cell.0).min(), Some(35));
        assert_eq!(cells.iter().map(|cell| cell.0).max(), Some(66));
        assert!(matches!(app.card_images.get("https://i.redd.it/p1.jpeg"),
            Some(CardImageSlot::Ready { protocol: Some(protocol), .. })
                if protocol.size == Size::new(32, 8)));
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
        app.tabs[4].feed.posts.push(post_with("tail", Media::None));
        app.tabs[4].feed.posts.push(post_with("tail2", Media::None));
        screen(&mut app, 100, 20);
        app.scroll_card(3);
        screen(&mut app, 100, 20);
        app.open_current_post();
        screen(&mut app, 100, 20);
        app.close_post();
        screen(&mut app, 100, 20);
        assert_eq!(app.active_tab().feed.cards.line, 3);

        app.on_msg(Msg::Image {
            url: "https://i.redd.it/p1.jpeg".into(),
            result: Ok(image::DynamicImage::new_rgb8(320, 640).into()),
        });
        placeholder_cells(&mut app, 100, 20);

        assert_eq!(app.active_tab().feed.cards.line, 3);
        assert_eq!(app.current_post().unwrap().id, "p1");
    }

    #[test]
    fn when_scrolling_below_kittys_row_limit_image_tiles_should_use_local_coordinates() {
        let mut app = card_with_image(100, 1000);
        app.tabs[4]
            .feed
            .posts
            .extend((0..200).map(|i| post_with(&format!("tail{i}"), Media::None)));
        screen(&mut app, 100, 600);
        app.scroll_card(300);
        screen(&mut app, 100, 600);

        let mut terminal = Terminal::new(TestBackend::new(100, 600)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();

        let CardImageSlot::Ready {
            protocol: Some(protocol),
            ..
        } = app.card_images.get("https://i.redd.it/p1.jpeg").unwrap()
        else {
            panic!("card protocol");
        };
        assert_eq!(protocol.size, Size::new(66, 330));
        assert!(protocol.tiles.contains_key(&(298, 0, 6, 66)));
        assert_eq!(protocol.tiles.len(), 3);
        assert_eq!(protocol.tiles[&(298, 0, 6, 66)].size(), Size::new(66, 16));
        assert_eq!(protocol.tiles[&(320, 0, 10, 66)].size(), Size::new(66, 10));
        let row = terminal.backend().buffer()[(28, 4)]
            .symbol()
            .split_once('\u{10EEEE}')
            .unwrap()
            .1
            .chars()
            .next();
        assert_eq!(row, Some('\u{34B}'));
    }

    #[test]
    fn when_a_card_is_wider_than_kittys_column_limit_it_should_render_its_right_edge() {
        let mut app = card_with_image(640, 320);

        let cells = placeholder_cells(&mut app, 340, 150);

        assert_eq!(cells.iter().map(|cell| cell.0).min(), Some(34));
        assert_eq!(cells.iter().map(|cell| cell.0).max(), Some(333));
    }

    #[test]
    fn when_scrolling_an_iterm2_card_it_should_encode_only_the_visible_image_rows() {
        let mut app = card_with_image(320, 640);
        app.tabs[4].feed.posts.push(post_with("tail", Media::None));
        app.tabs[4].feed.posts.push(post_with("tail2", Media::None));
        app.image_picker.set_protocol_type(ProtocolType::Iterm2);
        screen(&mut app, 100, 20);
        app.scroll_card(3);
        std::thread::sleep(crate::app::NAV_SETTLE);
        app.tick();

        screen(&mut app, 100, 20);

        let CardImageSlot::Ready {
            protocol: Some(protocol),
            ..
        } = app.card_images.get("https://i.redd.it/p1.jpeg").unwrap()
        else {
            panic!("card protocol");
        };
        assert_eq!(protocol.tiles.len(), 1);
        assert_eq!(protocol.tiles[&(1, 0, 7, 8)].size(), Size::new(8, 7));
        assert!(matches!(
            *protocol.tiles[&(1, 0, 7, 8)],
            crate::card_image::Tile::Image(ratatui_image::protocol::Protocol::ITerm2(_))
        ));
    }

    #[test]
    fn when_scrolling_a_sixel_card_it_should_encode_only_the_visible_image_rows() {
        let mut app = card_with_image(320, 640);
        app.tabs[4].feed.posts.push(post_with("tail", Media::None));
        app.tabs[4].feed.posts.push(post_with("tail2", Media::None));
        app.image_picker.set_protocol_type(ProtocolType::Sixel);
        screen(&mut app, 100, 20);
        app.scroll_card(3);
        std::thread::sleep(crate::app::NAV_SETTLE);
        app.tick();

        screen(&mut app, 100, 20);

        let CardImageSlot::Ready {
            protocol: Some(protocol),
            ..
        } = app.card_images.get("https://i.redd.it/p1.jpeg").unwrap()
        else {
            panic!("card protocol");
        };
        assert_eq!(protocol.tiles.len(), 1);
        assert_eq!(protocol.tiles[&(1, 0, 7, 8)].size(), Size::new(8, 7));
        assert!(matches!(
            *protocol.tiles[&(1, 0, 7, 8)],
            crate::card_image::Tile::Image(ratatui_image::protocol::Protocol::Sixel(_))
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
        assert_eq!(app.active_tab().feed.cards.line, 0);
    }

    #[test]
    fn when_paging_back_from_a_short_card_it_should_keep_the_full_row_distance() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = card_with_image(320, 640);
        app.tabs[4].feed.posts.push(post_with("p2", Media::None));
        screen(&mut app, 100, 12);
        app.move_selection(1);
        screen(&mut app, 100, 12);
        crate::keys::handle(&mut app, KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
        screen(&mut app, 100, 12);
        assert_eq!(app.active_tab().feed.cards.line, 0);
        assert_eq!(app.current_post().unwrap().id, "p1");
    }

    #[test]
    fn when_a_card_is_a_spoiler_it_should_show_the_tag_without_loading_its_image() {
        let mut app = card_feed_app(kitty());
        app.tabs[4].feed.posts[0].spoiler = true;

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

        press_keys(&mut app, "zi");
        screen(&mut app, 100, 20);

        assert!(app.card_images.is_empty());
    }

    #[test]
    fn when_a_card_is_a_gallery_it_should_preview_the_first_image_instead_of_its_thumbnail() {
        let mut app = card_feed_app(kitty());
        app.tabs[4].feed.posts[0].media = Media::Gallery {
            images: vec!["https://i.redd.it/full.jpeg".into()],
            previews: Vec::new(),
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

        assert!(actual.contains("zc Card"), "{actual}");
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
    fn empty_nsfw_disabled_feed_should_explain_how_to_show_nsfw() {
        for (width, height) in [(40, 12), (24, 8)] {
            let mut app = feed_app(Picker::halfblocks());
            app.config.show_nsfw = false;
            app.show_sidebar = false;
            app.reload();
            app.on_msg(Msg::Feed {
                tab: app.active_tab().id,
                generation: app.active_tab().feed.generation,
                append: false,
                result: Ok(crate::model::Listing {
                    posts: vec![],
                    after: None,
                }),
            });

            let actual = screen(&mut app, width, height);

            assert!(actual.contains("No visible posts"), "{actual}");
            let words = actual.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(words.contains("NSFW is off"), "{actual}");
            let compact: String = actual.chars().filter(|c| !c.is_whitespace()).collect();
            assert!(
                compact.contains("F2") && compact.contains("ShowNSFW"),
                "{actual}"
            );
            assert!(!words.contains("restart"), "{actual}");
            assert!(!actual.contains("No posts here"), "{actual}");
        }
    }

    #[test]
    fn empty_nsfw_disabled_search_should_explain_the_setting() {
        let mut app = feed_app(Picker::halfblocks());
        app.config.show_nsfw = false;
        app.search("something".into());
        app.on_msg(Msg::Feed {
            tab: app.active_tab().id,
            generation: app.active_tab().feed.generation,
            append: false,
            result: Ok(crate::model::Listing {
                posts: vec![],
                after: None,
            }),
        });

        let actual = screen(&mut app, 100, 20);

        assert!(actual.contains("No results"), "{actual}");
        assert!(
            actual.contains("F2") && actual.contains("Show NSFW"),
            "{actual}"
        );
        assert!(!actual.contains("restart"), "{actual}");
    }

    #[test]
    fn empty_nsfw_enabled_feed_should_keep_the_normal_empty_message() {
        let mut app = feed_app(Picker::halfblocks());
        app.config.show_nsfw = true;
        app.reload();
        app.on_msg(Msg::Feed {
            tab: app.active_tab().id,
            generation: app.active_tab().feed.generation,
            append: false,
            result: Ok(crate::model::Listing {
                posts: vec![],
                after: None,
            }),
        });

        let actual = screen(&mut app, 100, 20);

        assert!(actual.contains("No posts here"), "{actual}");
        assert!(!actual.contains("show_nsfw"), "{actual}");
    }

    #[test]
    fn empty_nsfw_disabled_island_should_suggest_the_existing_setting() {
        for width in [40, 100] {
            let mut app = app(&["empty"]);
            app.config.show_nsfw = false;
            app.show_sidebar = false;
            app.on_msg(Msg::Group {
                subreddit: "empty".into(),
                generation: app.all_subs.groups[0].generation,
                result: Ok(crate::model::Listing {
                    posts: vec![],
                    after: None,
                }),
            });

            let actual = screen(&mut app, width, 20);

            assert!(actual.contains("NSFW off"), "{actual}");
            assert!(
                actual.contains("F2") && actual.contains("Show NSFW"),
                "{actual}"
            );
            assert!(!actual.contains("restart"), "{actual}");
        }
    }

    #[test]
    fn empty_nsfw_disabled_content_tools_should_fit_the_setting_guidance() {
        use crate::app::experience::Screen;
        use crate::reddit::experience::{Page, Query, UserFeed};
        for query in [
            Query::User {
                name: "someone".into(),
                feed: UserFeed::Posts,
            },
            Query::Communities,
            Query::Multi {
                owner: "someone".into(),
                name: "example".into(),
            },
        ] {
            let mut app = app(&[]);
            app.config.show_nsfw = false;
            app.overlay = Some(Overlay::Experience);
            app.experience.screens = vec![Screen::Browse {
                pagination: Default::default(),
                query,
                page: Page {
                    title: "Empty collection".into(),
                    text: String::new(),
                    entries: vec![],
                    after: None,
                    url: String::new(),
                },
                selected: 0,
                scroll: 0,
            }];

            let actual = screen(&mut app, 60, 16);

            assert!(actual.contains("NSFW is off"), "{actual}");
            assert!(
                actual.contains("F2") && actual.contains("Show NSFW"),
                "{actual}"
            );
            assert!(!actual.contains("restart"), "{actual}");
        }
    }

    #[test]
    fn empty_nsfw_guidance_should_preserve_notifications_and_supplied_text() {
        use crate::app::experience::Screen;
        use crate::reddit::experience::{Inbox, Page, Query};
        for (show_nsfw, query, text, expected) in [
            (false, Query::Inbox(Inbox::Unread), "", "No items."),
            (true, Query::Communities, "", "No items."),
            (
                false,
                Query::Communities,
                "A supplied explanation",
                "A supplied explanation",
            ),
        ] {
            let mut app = app(&[]);
            app.config.show_nsfw = show_nsfw;
            app.overlay = Some(Overlay::Experience);
            app.experience.screens = vec![Screen::Browse {
                pagination: Default::default(),
                query,
                page: Page {
                    title: "Empty collection".into(),
                    text: text.into(),
                    entries: vec![],
                    after: None,
                    url: String::new(),
                },
                selected: 0,
                scroll: 0,
            }];

            let actual = screen(&mut app, 60, 16);

            assert!(actual.contains(expected), "{actual}");
            assert!(!actual.contains("show_nsfw"), "{actual}");
        }
    }

    #[test]
    fn an_empty_feed_says_whether_the_config_hid_the_posts_or_a_search_found_none() {
        let mut app = feed_app(Picker::halfblocks());
        let answer = |app: &mut App, posts: Vec<Post>| {
            let tab = app.tabs[4].id;
            let generation = app.tabs[4].feed.generation;
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
    fn partial_bottom_posts_should_render_without_hiding_selected_metadata() {
        for bookmarks in [false, true] {
            for (height, selected, offset, bottom) in [
                (8, 0, 0, "Second post"),
                (9, 1, 0, "author2"),
                (8, 1, 1, "Third post"),
                (8, 2, 2, "┃"),
            ] {
                let mut app = feed_app(Picker::halfblocks());
                app.show_sidebar = false;
                app.tabs[4].feed.posts.truncate(3);
                for (index, post) in app.tabs[4].feed.posts.iter_mut().enumerate() {
                    post.title = ["First post", "Second post", "Third post"][index].into();
                    post.author = format!("author{}", index + 1);
                }
                if bookmarks {
                    for post in app.tabs[4].feed.posts.clone().iter().rev() {
                        app.state.toggle_bookmark(post, 1);
                    }
                    app.switch_tab(1);
                }
                app.tabs[app.active].feed.selected = selected;

                let actual = screen(&mut app, 100, height);

                assert!(
                    actual
                        .lines()
                        .nth((height - 2) as usize)
                        .unwrap()
                        .contains(bottom),
                    "{actual}"
                );
                assert!(
                    actual.contains(&format!("author{}", selected + 1)),
                    "{actual}"
                );
                assert_eq!(app.active_tab().feed.offset, offset, "{actual}");
            }
        }
    }

    #[test]
    fn feed_rows_load_and_place_thumbnails_only_for_visible_image_posts() {
        let mut app = feed_app(kitty());

        let s = screen(&mut app, 100, 20);

        let mut requested: Vec<&str> = app.thumbs.keys().map(String::as_str).collect();
        requested.sort();
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

        let taller = screen(&mut app, 100, 23);
        assert!(taller.contains("p6 x"), "{taller}");
        assert!(app.thumbs.contains_key("https://i.redd.it/p6.jpeg"));
        assert!(!app.thumbs.contains_key("https://i.redd.it/p7.jpeg"));

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

        // Sidebar 24 + inset 2: the list starts at x 26 with the selection bar,
        // so the 4×2 thumbnail fills x 27..=30 on the row's two lines only.
        let expected: Vec<(u16, u16)> = (p1_row..p1_row + 2)
            .flat_map(|y| (27..=30).map(move |x| (x, y)))
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
        app.switch_tab(4);
        let generation = app.tabs[4].feed.generation;
        let id = app.tabs[4].id;
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
                    .take(app.sidebar_width() as usize)
                    .collect::<String>()
                    .split_whitespace()
                    .any(|word| word == "s")
            })
            .unwrap() as u16;
        // Row 0 holds the focus outline's top edge.
        let card_row = s
            .lines()
            .skip(1)
            .position(|l| l.contains('╭') || l.contains('╔'))
            .unwrap() as u16
            + 1;
        let card_x = s
            .lines()
            .nth(card_row as usize)
            .unwrap()
            .chars()
            .position(|c| c == '╭' || c == '╔')
            .unwrap() as u16;
        // Sidebar: inset 2 + bar + number = 4; card title: corner + space = 2.
        let mut expected = vec![
            (5, sidebar_row),
            (6, sidebar_row),
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
    fn popup_media_jump_should_preserve_card_pixels_around_its_bounds() {
        let mut app = card_with_image(640, 320);
        let before = placeholder_cells(&mut app, 100, 40);
        press_keys(&mut app, " bb");

        let actual = placeholder_cells(&mut app, 100, 40);

        assert!(!actual.is_empty());
        assert!(
            actual
                .iter()
                .all(|&(x, y)| !Rect::new(22, 15, 56, 10).contains((x, y).into()))
        );
        assert!(
            actual
                .iter()
                .any(|&(x, y)| x >= 78 && (15..25).contains(&y)),
            "right-side image strip remains visible"
        );
        crate::keys::handle(
            &mut app,
            crossterm::event::KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        );
        assert_eq!(placeholder_cells(&mut app, 100, 40), before);
    }

    #[test]
    fn popup_media_jump_should_preserve_iterm2_card_fragments() {
        // 528×256 fills the 66-column card in exactly 16 rows, so no tile holds padding.
        let mut app = card_with_image(528, 256);
        app.image_picker.set_protocol_type(ProtocolType::Iterm2);
        let CardImageSlot::Ready { image, .. } = app
            .card_images
            .get_mut("https://i.redd.it/p1.jpeg")
            .unwrap()
        else {
            panic!("loaded card")
        };
        *image = std::sync::Arc::new(image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(
            528,
            256,
            |x, _| image::Rgb([if x < 264 { 0 } else { 255 }, 0, 0]),
        )));
        screen(&mut app, 100, 40);
        press_keys(&mut app, " bb");

        let actual = screen(&mut app, 100, 40);

        assert!(actual.contains("doNotMoveCursor=1:"));
        assert!(actual.contains("Jump"));
        let CardImageSlot::Ready {
            protocol: Some(protocol),
            ..
        } = app.card_images.get("https://i.redd.it/p1.jpeg").unwrap()
        else {
            panic!("encoded fragments")
        };
        let right: Vec<_> = protocol
            .tiles
            .iter()
            .filter(|(key, _)| key.1 == 50)
            .collect();
        assert!(!right.is_empty());
        assert!(
            right.iter().all(|(_, tile)| match tile.as_ref() {
                crate::card_image::Tile::Image(protocol) =>
                    card_png(protocol).pixels().all(|p| p.0 == [255, 0, 0]),
                _ => false,
            }),
            "right strips must sample the original image's right half"
        );
    }

    #[test]
    fn popup_media_jump_should_preserve_sixel_card_fragments() {
        let mut app = card_with_image(320, 640);
        app.image_picker.set_protocol_type(ProtocolType::Sixel);
        press_keys(&mut app, " bb");

        let actual = screen(&mut app, 100, 40);

        assert!(actual.contains("\u{1b}P"));
        assert!(actual.contains("Jump"));
    }

    #[test]
    fn popup_media_jump_should_preserve_halfblock_pixels_outside_the_popup() {
        let mut app = card_with_image(320, 640);
        app.image_picker.set_protocol_type(ProtocolType::Halfblocks);
        press_keys(&mut app, " bb");
        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        while app.card_renderer.pending() {
            app.card_renderer.wait_for_completion();
            app.card_renderer.poll();
            terminal.draw(|f| draw(f, &mut app)).unwrap();
        }

        let actual = terminal.backend().buffer();

        assert!(
            actual
                .content
                .iter()
                .any(|cell| cell.fg == Color::Rgb(0, 0, 0) || cell.bg == Color::Rgb(0, 0, 0))
        );
        assert!((15..25).all(
            |y| (22..78).all(|x| actual[(x, y)].fg != Color::Rgb(0, 0, 0)
                && actual[(x, y)].bg != Color::Rgb(0, 0, 0))
        ));
    }

    #[test]
    fn popup_media_jump_should_keep_the_paused_detail_video_frame() {
        let mut app = open_video_post();
        let url = app.reddit.media_url(VIDEO).unwrap();
        let frame = video::Frame {
            width: 160,
            height: 90,
            pixels: video::Pixels::Raw(std::sync::Arc::new(image::DynamicImage::new_rgb8(160, 90))),
            same_as_first: false,
        };
        let mut player = video::Player::showing(&url, frame);
        player.area = app.video_area.unwrap().as_size();
        app.video = Some(player);
        press_keys(&mut app, "p");
        press_keys(&mut app, " bb");
        app.tick();

        let actual = placeholder_cells(&mut app, 100, 40);

        assert!(!actual.is_empty());
        assert!(app.video.as_ref().unwrap().hidden());
        assert!(app.video.as_ref().unwrap().paused());
        crate::keys::handle(
            &mut app,
            crossterm::event::KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        );
        app.tick();
        assert!(app.video.as_ref().unwrap().paused());
        assert!(!app.video.as_ref().unwrap().hidden());
    }

    #[test]
    fn popup_media_inline_errors_should_not_paint_over_the_dialog() {
        let mut app = image_post("");
        let url = "https://i.redd.it/failed.png";
        app.images
            .insert(url.into(), ImageSlot::Failed("unavailable".into()));
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        let mut expected = None;

        terminal
            .draw(|f| {
                f.render_widget(
                    Paragraph::new("Opaque popup\nOpaque popup\nOpaque popup"),
                    f.area(),
                );
                expected = Some(f.buffer_mut().clone());
                draw_inline_image(
                    f,
                    &mut app,
                    url,
                    Rect::new(0, 0, 40, 3),
                    Some(Rect::new(0, 0, 40, 3)),
                );
            })
            .unwrap();

        assert_eq!(terminal.backend().buffer(), &expected.unwrap());
    }

    #[test]
    fn popup_media_help_should_preserve_thumbnails_outside_its_bounds() {
        let mut app = feed_app(kitty());
        screen(&mut app, 200, 40);
        let slot = ready(&app, 32, 32);
        app.thumbs.insert("https://i.redd.it/p1.jpeg".into(), slot);
        app.help_scroll = Some(0);

        let actual = placeholder_cells(&mut app, 200, 40);

        assert_eq!(actual.len(), 8, "uncovered 4x2 thumbnail");
    }

    #[test]
    fn thumbnails_wait_for_an_overlay_to_close() {
        let mut app = feed_app(kitty());
        screen(&mut app, 100, 20);
        let slot = ready(&app, 32, 32);
        app.thumbs.insert("https://i.redd.it/p1.jpeg".into(), slot);

        app.help_scroll = Some(0);
        let under_help = placeholder_cells(&mut app, 100, 20);
        app.help_scroll = None;
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
                        preview: None,
                    },
                )
            })
            .collect();

        // 12 lines leave the card 4 inner lines: p1's two lines fit, p2's do not.
        screen(&mut app, 100, 12);
        let mut top: Vec<String> = app.thumbs.keys().cloned().collect();
        top.sort();
        app.move_selection(2);
        std::thread::sleep(crate::app::NAV_SETTLE);
        app.tick();
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

    fn press_keys(app: &mut App, keys: &str) {
        for c in keys.chars() {
            if c.is_ascii_control() {
                key(
                    app,
                    KeyCode::Char(char::from_u32(c as u32 + 96).unwrap()),
                    KeyModifiers::CONTROL,
                );
            } else {
                press(app, c);
            }
        }
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
        key(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
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
    fn search_tab_post_results_should_name_each_posts_community() {
        let mut app = app(&[]);
        app.open_search(None);
        crate::keys::paste(&mut app, "editor");
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        let mut post = post_with("found", Media::None);
        post.subreddit = "vim".into();
        app.on_msg(Msg::Feed {
            tab: app.active_tab().id,
            generation: app.active_tab().feed.generation,
            append: false,
            result: Ok(crate::model::Listing {
                posts: vec![post],
                after: None,
            }),
        });

        let shown = screen(&mut app, 100, 20);

        assert!(shown.contains("r/vim"), "{shown}");
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(
            help::entries(&app)
                .iter()
                .any(|(key, text)| key == "Tab / S-Tab"
                    && text == "Switch Posts / Communities / People"),
            "list help names the kind keys"
        );
        app.tabs[crate::app::SEARCH_TAB].feed.after = Some("next".into());
        app.load_feed(crate::app::SEARCH_TAB, true);
        app.overlay = Some(Overlay::Picker(crate::app::PickerState::default()));
        let covered = screen(&mut app, 100, 20);
        assert!(
            covered.lines().last().unwrap().contains("more posts"),
            "a covered results loader moves to the footer: {covered}"
        );
        app.overlay = None;
        assert!(shown.contains("Posts  matching “editor”"), "{shown}");
        assert_eq!(
            shown.matches("editor").count(),
            2,
            "query bar and title only"
        );
    }

    #[test]
    fn reported_search_reverse_navigation_should_keep_the_viewport() {
        let mut app = app(&[]);
        app.open_search(None);
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        app.search.discovered.loading = false;
        app.search.discovered.entries = (0..60)
            .map(|i| crate::app::search::Row {
                profile: None,
                label: format!("r/community{i:02}"),
                detail: String::new(),
                target: crate::worker::FeedTarget::Subreddit(format!("community{i:02}")),
            })
            .collect();
        for _ in 0..35 {
            press(&mut app, 'j');
            screen(&mut app, 100, 24);
        }
        let before = screen(&mut app, 100, 24);
        let first = before
            .lines()
            .find(|line| line.contains("r/community"))
            .unwrap()
            .trim()
            .to_owned();
        press(&mut app, 'k');
        let after = screen(&mut app, 100, 24);
        assert!(
            after.lines().any(|line| line.trim() == first),
            "reverse navigation moved the viewport: {after}"
        );
    }

    #[test]
    fn long_explore_descriptions_should_show_visible_summaries_and_later_entries() {
        let mut app = app(&[]);
        app.open_search(None);
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        app.search.discovered.loading = false;
        app.search.discovered.entries = vec![
            crate::app::search::Row {
                profile: None,
                label: "r/long_description".into(),
                detail: "A descriptive word ".repeat(60),
                target: crate::worker::FeedTarget::Subreddit("long_description".into()),
            },
            crate::app::search::Row {
                profile: None,
                label: "r/second".into(),
                detail: "A short description".into(),
                target: crate::worker::FeedTarget::Subreddit("second".into()),
            },
        ];
        app.search.selected = 2;
        let actual = screen(&mut app, 70, 20);
        assert!(
            actual.contains("r/second"),
            "long summaries must leave room for later entries: {actual}"
        );
        assert!(
            actual
                .lines()
                .any(|line| line.contains("descriptive") && line.contains('…')),
            "a shortened description needs a visible ellipsis: {actual}"
        );
    }

    #[test]
    fn search_revision_should_prefetch_from_the_visible_window_before_the_last_rows() {
        use crate::reddit::experience::{Entry, Page, Query, Response};
        let mut app = app(&[]);
        app.open_search(None);
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        app.on_msg(Msg::Search {
            generation: app.search.discovered_generation(),
            result: Ok(Response::Page(Page {
                title: "Discover".into(),
                text: String::new(),
                url: "/".into(),
                after: Some("next".into()),
                entries: (0..50)
                    .map(|i| Entry {
                        title: format!("r/community{i}"),
                        body: String::new(),
                        query: Some(Query::Community(format!("community{i}"))),
                        icon: Default::default(),
                        target: None,
                        post: None,
                    })
                    .collect(),
            })),
        });
        app.search.selected = 0;
        screen(&mut app, 160, 30);
        assert!(
            app.search.discovered.loading,
            "keep another API page ready while the first page is being read"
        );
        let generation = app.search.discovered_generation();
        screen(&mut app, 160, 30);
        assert_eq!(
            app.search.discovered_generation(),
            generation,
            "one continuation at a time"
        );
    }

    #[test]
    fn post_notice_preview_should_announce_only_new_leading_posts() {
        for (ids, new_posts) in [
            (vec!["a", "b"], false),
            (vec!["a", "tail", "b"], false),
            (vec!["b", "a"], false),
            (vec!["leader", "b", "a"], true),
        ] {
            let mut app = app(&[]);
            app.open_search(None);
            app.search.discovered.loading = false;
            app.search.preview.target = Some(app.search.selected_row().unwrap().target);
            app.search.preview.posts =
                vec![post_with("a", Media::None), post_with("b", Media::None)];
            app.search.preview.update = Some(
                ids.iter()
                    .map(|id| {
                        let mut post = post_with(id, Media::None);
                        post.score = Some(123);
                        post.comments = 42;
                        post
                    })
                    .collect(),
            );
            let shown = screen(&mut app, 200, 30);
            assert_eq!(shown.contains("New posts"), new_posts, "{ids:?}: {shown}");
        }
    }

    #[test]
    fn post_notice_preview_status_should_belong_to_the_selected_source() {
        let mut app = app(&[]);
        app.open_search(None);
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        app.search.discovered.loading = false;
        app.search.preview.target = Some(app.search.selected_row().unwrap().target);
        app.search.preview.posts = vec![post_with("a", Media::None)];
        app.search.preview.update = Some(vec![post_with("leader", Media::None)]);
        app.search.preview.error = Some("previous source failed".into());
        let current = screen(&mut app, 200, 30);
        assert!(
            current.contains("New posts") && current.contains("refresh failed"),
            "{current}"
        );

        press(&mut app, 'j');
        let next = screen(&mut app, 200, 30);
        assert!(
            next.contains("r/all") && next.contains("Loading preview"),
            "{next}"
        );
        assert!(
            !next.contains("New posts") && !next.contains("refresh failed"),
            "{next}"
        );
    }

    #[test]
    fn search_key_audit_rows_should_use_row_motions_after_post_cards() {
        for viewport in [0, 35] {
            let mut app = app(&[]);
            app.open_search(None);
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            app.search.discovered.entries = (0..30)
                .map(|i| crate::app::search::Row {
                    profile: None,
                    label: format!("r/source{i}"),
                    detail: String::new(),
                    target: crate::worker::FeedTarget::Subreddit(format!("source{i}")),
                })
                .collect();
            app.tabs[crate::app::SEARCH_TAB].feed.cards.viewport = viewport;
            for (code, modifiers, selected) in [
                (KeyCode::Char('e'), KeyModifiers::CONTROL, 1),
                (KeyCode::Char('y'), KeyModifiers::CONTROL, 0),
                (KeyCode::Char('f'), KeyModifiers::CONTROL, 10),
                (KeyCode::Char('d'), KeyModifiers::CONTROL, 15),
                (KeyCode::Char('u'), KeyModifiers::CONTROL, 10),
                (KeyCode::Char('b'), KeyModifiers::CONTROL, 0),
                (KeyCode::PageDown, KeyModifiers::NONE, 10),
                (KeyCode::PageUp, KeyModifiers::NONE, 0),
            ] {
                key(&mut app, code, modifiers);
                assert_eq!(
                    app.search.selected, selected,
                    "{code:?} with card viewport{viewport}"
                );
            }
            press(&mut app, 'G');
            assert_eq!(app.search.selected, 31);
            press_keys(&mut app, "gg");
            assert_eq!(app.search.selected, 0);
        }
    }

    #[test]
    fn search_key_audit_query_should_share_editor_modifier_filtering() {
        let mut app = app(&[]);
        app.open_search(None);
        crate::keys::paste(&mut app, "alpha rust");
        app.search.selected = 1;
        for (code, modifiers) in [
            (KeyCode::Enter, KeyModifiers::CONTROL),
            (KeyCode::Tab, KeyModifiers::CONTROL),
            (KeyCode::Esc, KeyModifiers::ALT),
            (KeyCode::Left, KeyModifiers::ALT),
            (KeyCode::Delete, KeyModifiers::CONTROL),
            (KeyCode::Down, KeyModifiers::ALT),
        ] {
            let expected = (
                app.active,
                app.active_tab().feed.generation,
                app.search.kind,
                app.search.selected,
                app.search.query.value.clone(),
                app.search.query.cursor,
                app.search.editing,
            );
            key(&mut app, code, modifiers);
            assert_eq!(
                (
                    app.active,
                    app.active_tab().feed.generation,
                    app.search.kind,
                    app.search.selected,
                    app.search.query.value.clone(),
                    app.search.query.cursor,
                    app.search.editing
                ),
                expected,
                "unsupported editor chord {modifiers:?}+{code:?}"
            );
        }
        key(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL);
        assert_eq!(app.search.query.cursor, 0);
        key(&mut app, KeyCode::Char('e'), KeyModifiers::CONTROL);
        key(&mut app, KeyCode::Backspace, KeyModifiers::ALT);
        assert_eq!(app.search.query.value, "alpha ");
        key(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
        assert!(app.search.query.value.is_empty());
    }

    #[test]
    fn key_context_audit_open_and_media_keys_should_require_a_usable_target() {
        use crate::keys::map::{self, Action};
        use crate::reddit::experience::Query;
        let mut empty = app(&[]);
        empty.active = 0;
        empty.focus_main();
        assert!(!map::available(&empty, Action::Right));
        assert!(!map::available(&empty, Action::Browser));
        empty.active = crate::app::ALL_SUBS_TAB;
        assert!(!map::available(&empty, Action::NextCommunity));
        let mut rules = description_popup(Query::Rules("rust".into()), "Rules", &[]);
        assert!(!map::available(&rules, Action::Right));
        let before = rules.experience.screens.len();
        press(&mut rules, 'l');
        assert_eq!(rules.experience.screens.len(), before);

        let (mut post, _) = open_long_post(|_| {});
        post.post_view
            .as_mut()
            .unwrap()
            .page
            .as_mut()
            .unwrap()
            .comments = vec![reply("leaf".into())];
        screen(&mut post, 100, 24);
        press(&mut post, 'G');
        assert!(!map::available(&post, Action::Right));
        let gallery = Media::Gallery {
            images: vec![],
            previews: vec![],
            thumb: None,
        };
        post.post_view
            .as_mut()
            .unwrap()
            .page
            .as_mut()
            .unwrap()
            .post
            .media = gallery;
        assert!(!map::available(&post, Action::NextImage));
        let video = Media::Video {
            gif: false,
            url: Some(VIDEO.into()),
            poster: None,
        };
        post.post_view
            .as_mut()
            .unwrap()
            .page
            .as_mut()
            .unwrap()
            .post
            .media = video;
        post.show_images = true;
        post.config.media.image_preview = true;
        assert!(map::available(&post, Action::Pause));
        for action in [Action::Mute, Action::SeekBack, Action::SeekForward] {
            assert!(!map::available(&post, action));
        }
        post.focus_sidebar();
        assert!(!map::available(&post, Action::Media));
        post.close_post();
        post.active = crate::app::SEARCH_TAB;
        post.search.query.insert("video");
        post.search.editing = true;
        post.focus_main();
        let mut video_post = post_with("typing", Media::None);
        video_post.media = Media::Video {
            gif: false,
            url: Some(VIDEO.into()),
            poster: None,
        };
        post.tabs[crate::app::SEARCH_TAB].feed.posts = vec![video_post];
        assert!(!map::available(&post, Action::Pause));
    }

    #[test]
    fn key_context_audit_end_should_reach_the_last_visible_content() {
        let (mut post, _) = open_long_post(|_| {});
        post.post_view
            .as_mut()
            .unwrap()
            .page
            .as_mut()
            .unwrap()
            .comments
            .clear();
        screen(&mut post, 100, 24);
        press(&mut post, 'G');
        let bottom = screen(&mut post, 100, 24);
        assert!(bottom.contains("paragraph 60"), "{bottom}");
        assert!(post.post_view.as_ref().unwrap().scroll > 0);
        press_keys(&mut post, "gg");
        assert_eq!(post.post_view.as_ref().unwrap().scroll, 0);

        let text = (1..=60)
            .map(|index| format!("Rule {index}\n"))
            .collect::<String>();
        let mut rules = description_popup(
            crate::reddit::experience::Query::Rules("rust".into()),
            &text,
            &[],
        );
        popup_snapshot(&mut rules, 80, 24);
        press(&mut rules, 'G');
        let (_, bottom) = popup_snapshot(&mut rules, 80, 24);
        assert!(bottom.contains("Rule 60"), "{bottom}");
        let scroll = match rules.experience.screens.last().unwrap() {
            crate::app::experience::Screen::Browse { scroll, .. } => *scroll,
            _ => unreachable!(),
        };
        key(&mut rules, KeyCode::Char('e'), KeyModifiers::CONTROL);
        popup_snapshot(&mut rules, 80, 24);
        key(&mut rules, KeyCode::Char('y'), KeyModifiers::CONTROL);
        popup_snapshot(&mut rules, 80, 24);
        assert!(matches!(rules.experience.screens.last(),
            Some(crate::app::experience::Screen::Browse { scroll: actual, .. }) if *actual == scroll - 1));

        let mut islands = app(&["first", "last"]);
        islands.all_subs.groups[1].posts = ["one", "two", "three"]
            .into_iter()
            .map(|id| post_with(id, Media::None))
            .collect();
        press(&mut islands, 'G');
        assert_eq!(islands.current_post().unwrap().id, "three");
        press_keys(&mut islands, "gg");
        assert_eq!((islands.all_subs.group, islands.all_subs.post), (0, 0));
    }

    #[test]
    fn key_context_audit_narrow_confirmation_should_keep_its_explicit_enter_control() {
        use crate::app::experience::Screen;
        use crate::reddit::experience::{Action, Target};
        let mut app = app(&[]);
        app.overlay = Some(Overlay::Experience);
        app.experience.screens = vec![Screen::Confirm {
            label: "Delete this post?".into(),
            action: Action::Delete {
                target: Target::from_post(&post_with("delete", Media::None)),
            },
            native: "/r/s/comments/delete/x/".into(),
        }];
        let (_, shown) = popup_snapshot(&mut app, 24, 12);
        assert!(shown.contains("Enter") || shown.contains('⏎'), "{shown}");
        press(&mut app, 'l');
        assert!(matches!(
            app.experience.screens.last(),
            Some(Screen::Confirm { .. })
        ));
        assert!(app.experience.pending.is_none());
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.overlay.is_none());
    }

    #[test]
    fn key_context_audit_empty_picker_should_keep_the_query_until_a_target_exists() {
        let mut app = app(&[]);
        press_keys(&mut app, " bb");
        crate::keys::paste(&mut app, "???");
        assert!(app.picker_entries("???").is_empty());
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(&app.overlay, Some(Overlay::Picker(picker)) if picker.input == "???"));
        key(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
        crate::keys::paste(&mut app, "Home");
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.overlay.is_none());
        assert_eq!(app.active, 0);
    }

    #[test]
    fn key_context_audit_go_commands_should_focus_the_destination_feed() {
        for width in [100, 200] {
            for command in ["gh", "ga", "gp", "gc"] {
                let (mut app, _) = open_long_post(|_| {});
                let view = app.post_view.take();
                match command {
                    "gh" => app.active = 0,
                    "ga" => app.active = crate::app::ALL_SUBS_TAB,
                    "gp" => app.open_subreddit("popular"),
                    _ => app.open_subreddit("s"),
                }
                let destination = app.active_tab().id;
                app.post_view = view;
                app.focus = Focus::Post;
                screen(&mut app, width, 24);

                press_keys(&mut app, command);

                assert_eq!(app.active_tab().id, destination, "{command}");
                assert!(app.post_view.is_none(), "{width}: {command}");
                assert_eq!(app.focus, Focus::List);
            }
        }
    }

    #[test]
    fn key_context_audit_invalid_terminal_keys_should_cancel_only_the_prefix() {
        for invalid in [
            KeyCode::Tab,
            KeyCode::Left,
            KeyCode::Home,
            KeyCode::Backspace,
            KeyCode::F(3),
        ] {
            let mut app = app(&["first", "second"]);
            app.active = 4;
            app.focus_main();
            press(&mut app, ']');
            assert_eq!(app.keymap.prefix, "]");
            key(&mut app, invalid, KeyModifiers::NONE);
            assert!(app.keymap.prefix.is_empty(), "{invalid:?}");
            assert_eq!(app.active, 4, "consume the invalid key without navigating");
            assert_eq!(app.focus, Focus::List);
        }
    }

    #[test]
    fn key_context_audit_auth_confirmation_should_only_accept_its_own_verbs() {
        for phase in [
            crate::auth::Phase::Confirm,
            crate::auth::Phase::DisconnectConfirm,
        ] {
            let mut app = app(&[]);
            app.auth.phase = phase;
            app.auth.active_id = Some("reader".into());
            app.auth.selected = 1;
            app.overlay = Some(Overlay::Auth);
            for code in [
                KeyCode::Char('j'),
                KeyCode::Char('d'),
                KeyCode::Char('r'),
                KeyCode::Home,
            ] {
                key(&mut app, code, KeyModifiers::NONE);
                assert_eq!(app.auth.phase, phase, "{code:?}");
                assert_eq!(app.auth.selected, 1, "{code:?}");
            }
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            assert!(app.overlay.is_none());
        }
    }

    #[test]
    fn key_context_audit_local_find_reload_should_keep_the_local_results() {
        use crate::app::experience::Screen;
        let (mut app, _) = open_long_post(|_| {});
        let generation = app.post_view.as_ref().unwrap().generation;
        press(&mut app, '/');
        crate::keys::paste(&mut app, "reply");
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        app.post_view
            .as_mut()
            .unwrap()
            .page
            .as_mut()
            .unwrap()
            .comments
            .insert(0, reply("new".into()));
        press(&mut app, 'r');
        assert!(
            app.experience.pending.is_none(),
            "Find must not read a remote thread"
        );
        assert!(
            matches!(app.experience.screens.last(), Some(Screen::Browse { page, .. }) if page.entries.len() == 2)
        );
        assert!(
            matches!(app.experience.screens.last(), Some(Screen::Browse { page, selected, .. })
            if page.entries[*selected].target.as_ref().unwrap().id == "t1_c")
        );
        assert_eq!(app.post_view.as_ref().unwrap().generation, generation);
        assert!(matches!(app.overlay, Some(Overlay::Experience)));
    }

    #[test]
    fn key_context_audit_opened_multi_post_should_regain_content_actions() {
        use crate::app::experience::Screen;
        use crate::keys::map::{self, Action};
        use crate::reddit::experience::{Entry, Page, Query, Request, Target};
        let mut app = app(&[]);
        let post = post_with("selected", Media::None);
        let target = Target::from_post(&post);
        app.overlay = Some(Overlay::Experience);
        app.experience.screens = vec![Screen::Browse {
            query: Query::Multi {
                owner: "reader".into(),
                name: "feed".into(),
            },
            page: Page {
                title: "Feed".into(),
                text: String::new(),
                entries: vec![Entry {
                    icon: Default::default(),
                    title: "Selected".into(),
                    body: String::new(),
                    query: None,
                    target: Some(target.clone()),
                    post: Some(post),
                }],
                after: None,
                url: "/user/reader/m/feed".into(),
            },
            selected: 0,
            scroll: 0,
            pagination: Default::default(),
        }];
        for action in [Action::Upvote, Action::Save, Action::Reply] {
            assert!(
                map::available(&app, action),
                "nonconflicting verb {action:?}"
            );
        }
        assert!(!map::available(&app, Action::Downvote));
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.post_focused());
        for action in [
            Action::Upvote,
            Action::Downvote,
            Action::Save,
            Action::Reply,
        ] {
            assert!(map::available(&app, action), "{action:?}");
        }
        press(&mut app, 'd');
        assert!(
            matches!(&app.experience.pending, Some((_, Request::Read { query: Query::Inspect(actual), .. })) if actual == &target)
        );
    }

    #[test]
    fn key_context_audit_messages_should_offer_message_actions_instead_of_post_verbs() {
        use crate::app::experience::Screen;
        use crate::keys::map::{self, Action};
        use crate::reddit::experience::{Inbox, Query, Target};
        let mut app = description_popup(Query::Inbox(Inbox::All), "", &["Message"]);
        if let Some(Screen::Browse { page, .. }) = app.experience.screens.last_mut() {
            page.entries[0].query = None;
            page.entries[0].target = Some(Target {
                id: "t4_message".into(),
                author: "sender".into(),
                subreddit: String::new(),
                permalink: String::new(),
            });
        }
        for action in [
            Action::Upvote,
            Action::Downvote,
            Action::Save,
            Action::Reply,
        ] {
            assert!(!map::available(&app, action), "{action:?}");
        }
        press_keys(&mut app, " a");
        assert!(
            matches!(app.experience.screens.last(), Some(Screen::Menu { title, .. }) if title == "Message actions")
        );
    }

    #[cfg(unix)]
    #[test]
    fn key_context_audit_browser_should_follow_the_item_and_keep_the_parent_shortcut() {
        use crate::app::experience::Screen;
        use crate::keys::map::{self, Action};
        use crate::reddit::experience::{Entry, Page, Query, Target};
        let mut app = app(&[]);
        let post = post_with("selected", Media::None);
        let item_url = post.reddit_url();
        app.overlay = Some(Overlay::Experience);
        app.experience.screens = vec![Screen::Browse {
            query: Query::Profile("reader".into()),
            page: Page {
                title: "Profile".into(),
                text: String::new(),
                entries: vec![Entry {
                    icon: Default::default(),
                    title: "Selected".into(),
                    body: String::new(),
                    query: None,
                    target: Some(Target::from_post(&post)),
                    post: Some(post),
                }],
                after: None,
                url: "/user/reader/".into(),
            },
            selected: 0,
            scroll: 0,
            pagination: Default::default(),
        }];
        assert_eq!(opened_browser_url(&mut app, "o"), item_url);
        assert_eq!(
            opened_browser_url(&mut app, "\u{f}"),
            "https://www.reddit.com/user/reader/"
        );
        app.overlay = None;
        for name in ["popular", "all"] {
            app.open_subreddit(name);
            assert!(map::available(&app, Action::SourceBrowser));
            assert_eq!(
                opened_browser_url(&mut app, "o"),
                format!("https://www.reddit.com/r/{name}/")
            );
            app.focus_sidebar();
            assert_eq!(
                opened_browser_url(&mut app, "o"),
                format!("https://www.reddit.com/r/{name}/")
            );
            app.focus_main();
        }
    }

    #[test]
    fn search_source_motions_should_lift_details_to_the_parent_rows() {
        use crate::reddit::experience::{Entry, Page, Query, Response};
        use crate::worker::FeedTarget;
        for width in [100, 200] {
            for (command, expected) in [("]s", 4), ("[s", 2)] {
                let mut app = app(&["sidebar_only"]);
                app.open_search(None);
                key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
                app.on_msg(Msg::Search {
                    generation: app.search.discovered_generation(),
                    result: Ok(Response::Page(Page {
                        title: "Discover".into(),
                        text: String::new(),
                        url: "/".into(),
                        after: None,
                        entries: ["alpha", "beta", "gamma"]
                            .into_iter()
                            .map(|name| Entry {
                                title: format!("r/{name}"),
                                body: String::new(),
                                query: Some(Query::Community(name.into())),
                                icon: Default::default(),
                                target: None,
                                post: None,
                            })
                            .collect(),
                    })),
                });
                app.search.selected = 3;
                screen(&mut app, width, 24);
                press(&mut app, 'l');
                let source = app.active;
                let mut post = post_with("nested", Media::None);
                post.subreddit = "beta".into();
                app.on_msg(Msg::Feed {
                    tab: app.active_tab().id,
                    generation: app.active_tab().feed.generation,
                    append: false,
                    result: Ok(crate::model::Listing {
                        posts: vec![post.clone()],
                        after: None,
                    }),
                });
                press(&mut app, 'l');
                let mut root = reply("root".into());
                root.replies.push(reply("deep".into()));
                app.on_msg(Msg::Post {
                    generation: app.post_view.as_ref().unwrap().generation,
                    result: Ok(Box::new(PostPage {
                        post,
                        body: "A long passage before the replies.\n".repeat(35),
                        comments: vec![root],
                    })),
                });
                screen(&mut app, width, 24);
                press(&mut app, 'G');
                screen(&mut app, width, 24);
                assert!(app.post_view.as_ref().unwrap().scroll > 0);
                if app.detail_fits() && command == "[s" {
                    press(&mut app, 'h');
                    assert_eq!(app.focus, Focus::List);
                    assert!(app.post_view.is_some());
                }

                press_keys(&mut app, command);

                assert_eq!(app.active, crate::app::SEARCH_TAB, "{width}: {command}");
                assert_eq!(app.search.selected, expected);
                assert_eq!(app.focus, Focus::List);
                assert!(!app.search_editing());
                assert!(app.post_view.is_none());
                assert!(!app.ancestor_following());
                assert_eq!(app.tabs[source].feed.posts[0].id, "nested");
                let destination = app.search.selected_row().unwrap().target;
                press(&mut app, 'l');
                assert!(app.search_feed_open());
                let FeedTarget::Subreddit(name) = destination else {
                    unreachable!()
                };
                assert_eq!(app.active_tab().subreddit(), Some(name.as_str()));
                if app.detail_fits() {
                    key(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL);
                    press(&mut app, 'h');
                }
                press_keys(&mut app, command);
                assert_eq!(app.active, crate::app::SEARCH_TAB);
                assert_eq!(app.search.selected, if command == "]s" { 4 } else { 1 });
            }
        }
    }

    #[test]
    fn search_source_motions_should_use_shown_sources_without_opened_subreddit_tabs() {
        use crate::keys::map::{self, Action};
        use crate::reddit::experience::SearchKind;
        use crate::worker::FeedTarget;
        let mut explore = app(&[]);
        explore.open_search(None);
        key(&mut explore, KeyCode::Esc, KeyModifiers::NONE);
        for action in [Action::NextCommunity, Action::PreviousCommunity] {
            assert!(map::available(&explore, action));
        }
        press_keys(&mut explore, "]s");
        assert_eq!(explore.search.selected, 1);
        press_keys(&mut explore, "[s");
        assert_eq!(explore.search.selected, 0);
        press_keys(&mut explore, "[s");
        assert_eq!(explore.search.selected, 0);

        for kind in [SearchKind::Communities, SearchKind::People] {
            let mut app = app(&["unrelated"]);
            app.open_search(Some(kind));
            app.search.query.insert("topic");
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            app.search.results.entries = ["first", "second"]
                .into_iter()
                .map(|name| crate::app::search::Row {
                    profile: None,
                    label: name.into(),
                    detail: String::new(),
                    target: if kind == SearchKind::People {
                        FeedTarget::User(name.into())
                    } else {
                        FeedTarget::Subreddit(name.into())
                    },
                })
                .collect();
            let help = help::entries(&app);
            assert!(
                help.iter()
                    .any(|(keys, text)| keys == "]s / [s" && text.contains("Search source"))
            );
            press(&mut app, 'l');
            press_keys(&mut app, "]s");
            assert_eq!(app.active, crate::app::SEARCH_TAB);
            assert_eq!(app.search.selected, 1);
            press(&mut app, 'l');
            press_keys(&mut app, "]s");
            assert_eq!(app.active, crate::app::SEARCH_TAB);
            assert_eq!(app.search.selected, 1, "lift even at the last source");
            press_keys(&mut app, "[s");
            assert_eq!(app.search.selected, 0);
            assert_eq!(app.search.query.value, "topic");
            assert_eq!(app.search.kind, kind);
            press(&mut app, '/');
            assert!(!map::available(&app, Action::FocusLeft));
            assert!(!map::available(&app, Action::FocusRight));
            press_keys(&mut app, "]s[s");
            assert_eq!(app.search.query.value, "topic]s[s");
            assert_eq!(app.search.selected, 0);
            app.show_auth();
            assert!(!map::available(&app, Action::NextCommunity));
            assert!(!map::available(&app, Action::PreviousCommunity));
        }
    }

    #[test]
    fn search_source_motions_should_hide_unreachable_post_and_pane_branches() {
        use crate::keys::map::{self, Action};
        let mut app = app(&[]);
        app.open_search(None);
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        screen(&mut app, 100, 24);
        assert!(!map::available(&app, Action::FocusRight));
        assert!(
            !help::entries(&app)
                .iter()
                .any(|(keys, _)| keys.contains("C-w l"))
        );
        screen(&mut app, 200, 24);
        assert!(map::available(&app, Action::FocusRight));
        press_keys(&mut app, "\u{17}l");
        assert!(app.search_feed_open());
        let post = post_with("parent", Media::None);
        app.on_msg(Msg::Feed {
            tab: app.active_tab().id,
            generation: app.active_tab().feed.generation,
            append: false,
            result: Ok(crate::model::Listing {
                posts: vec![post],
                after: None,
            }),
        });
        press(&mut app, 'l');
        assert!(map::available(&app, Action::NextPost));
        assert!(map::available(&app, Action::PreviousPost));
        assert!(!map::available(&app, Action::FocusRight));
        assert!(map::available(&app, Action::FocusLeft));
        app.tabs[app.active].feed.posts.clear();
        assert!(!map::available(&app, Action::NextPost));
        assert!(!map::available(&app, Action::PreviousPost));
        assert!(
            !help::entries(&app)
                .iter()
                .any(|(keys, _)| keys.contains("]p"))
        );
        press_keys(&mut app, "[s");
        assert_eq!(app.active, crate::app::SEARCH_TAB);
        assert_eq!(app.search.selected, 0);
        app.focus_sidebar();
        assert!(!map::available(&app, Action::FocusLeft));
        assert!(map::available(&app, Action::FocusRight));
        assert!(!map::available(&app, Action::NextCommunity));
        press_keys(&mut app, "\u{17}l");
        assert_eq!(app.focus, Focus::List);

        app.open_search(Some(crate::reddit::experience::SearchKind::People));
        crate::keys::paste(&mut app, "missing");
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        screen(&mut app, 200, 24);
        assert!(app.search.rows_loading());
        assert!(!map::available(&app, Action::FocusRight));
        press_keys(&mut app, "\u{17}l");
        assert_eq!(app.active, crate::app::SEARCH_TAB);
        app.on_msg(Msg::Search {
            generation: app.search.results_generation(),
            result: Err("offline".into()),
        });
        screen(&mut app, 200, 24);
        assert!(app.search.rows_error().is_some());
        assert!(!map::available(&app, Action::FocusRight));
        press(&mut app, 'r');
        app.on_msg(Msg::Search {
            generation: app.search.results_generation(),
            result: Ok(crate::reddit::experience::Response::Page(
                crate::reddit::experience::Page {
                    title: "People".into(),
                    text: String::new(),
                    entries: vec![],
                    after: None,
                    url: "/search".into(),
                },
            )),
        });
        screen(&mut app, 200, 24);
        assert!(!app.search.rows_loading());
        assert!(!map::available(&app, Action::FocusRight));
        assert!(
            !help::entries(&app)
                .iter()
                .any(|(keys, _)| keys.contains("C-w l"))
        );
        press(&mut app, 'r');
        app.on_msg(Msg::Search {
            generation: app.search.results_generation(),
            result: Ok(crate::reddit::experience::Response::Page(
                crate::reddit::experience::Page {
                    title: "People".into(),
                    text: String::new(),
                    after: None,
                    url: "/search".into(),
                    entries: vec![crate::reddit::experience::Entry {
                        title: "writer".into(),
                        body: String::new(),
                        icon: Default::default(),
                        target: None,
                        post: None,
                        query: Some(crate::reddit::experience::Query::Profile("writer".into())),
                    }],
                },
            )),
        });
        screen(&mut app, 200, 24);
        assert!(map::available(&app, Action::FocusRight));
        press_keys(&mut app, "\u{17}l");
        assert!(app.search_feed_open());
        assert!(matches!(&app.active_tab().kind, TabKind::User { name, .. } if name == "writer"));
    }

    #[test]
    fn search_key_audit_source_commands_should_keep_the_parent() {
        use crate::worker::FeedTarget;
        for target in [
            FeedTarget::Subreddit("rust".into()),
            FeedTarget::User("writer".into()),
        ] {
            for command in ["l", "gc", " cf"] {
                let mut app = app(&[]);
                app.open_search(None);
                key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
                app.search.discovered.entries.push(crate::app::search::Row {
                    profile: None,
                    label: "selected source".into(),
                    detail: String::new(),
                    target: target.clone(),
                });
                app.search.selected = 2;
                screen(&mut app, 200, 24);
                press_keys(&mut app, command);
                match &target {
                    FeedTarget::Subreddit(name) => {
                        assert_eq!(app.active_tab().subreddit(), Some(name.as_str()))
                    }
                    FeedTarget::User(name) => assert!(
                        matches!(&app.active_tab().kind, TabKind::User { name: actual, .. } if actual == name)
                    ),
                    FeedTarget::Home => unreachable!(),
                }
                assert!(
                    app.search_feed_open(),
                    "{command:?} should keep the same parent as Enter"
                );
                press(&mut app, 'h');
                assert_eq!(app.active, crate::app::SEARCH_TAB);
                assert_eq!(app.search.selected, 2);
            }
        }
    }

    #[test]
    fn search_key_audit_source_links_should_include_explore_feeds() {
        use crate::keys::map::{self, Action};
        use crate::worker::FeedTarget;
        for (target, path, membership) in [
            (
                FeedTarget::Subreddit("popular".into()),
                "/r/popular/",
                false,
            ),
            (FeedTarget::Subreddit("all".into()), "/r/all/", false),
            (FeedTarget::Subreddit("rust".into()), "/r/rust/", true),
            (FeedTarget::User("writer".into()), "/user/writer/", true),
        ] {
            let mut app = app(&[]);
            app.open_search(None);
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            app.search.discovered.entries.push(crate::app::search::Row {
                profile: None,
                label: "selected source".into(),
                detail: String::new(),
                target,
            });
            app.search.selected = 2;
            assert!(
                map::available(&app, Action::CopyLink),
                "selected row has a Reddit URL"
            );
            assert!(map::available(&app, Action::CommunityFeed));
            assert!(map::available(&app, Action::SourceBrowser));
            assert_eq!(map::available(&app, Action::Membership), membership);
            assert_eq!(
                app.copy_link_target().unwrap(),
                (format!("https://www.reddit.com{path}"), false)
            );
            press(&mut app, 'y');
            assert!(map::hints(&app).iter().any(|(key, _)| key == "y"));
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            let dir = tempfile::tempdir().unwrap();
            let script = dir.path().join("browser.sh");
            std::fs::write(&script, "printf '%s' \"$2\" > \"$1\"\n").unwrap();
            for (index, command) in ["o", " oc"].into_iter().enumerate() {
                let output = dir.path().join(format!("url{index}"));
                app.config.commands.browser = vec![
                    "/bin/sh".into(),
                    script.to_string_lossy().into(),
                    output.to_string_lossy().into(),
                ];
                press_keys(&mut app, command);
                let expected = format!("https://www.reddit.com{path}");
                let deadline = Instant::now() + std::time::Duration::from_secs(2);
                while std::fs::read_to_string(&output).ok().as_deref() != Some(&expected) {
                    assert!(
                        Instant::now() < deadline,
                        "{command:?} did not open {expected}"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }
        }
    }

    #[test]
    fn search_key_audit_focus_and_back_should_follow_visible_parent_panes() {
        for width in [100, 200] {
            let mut app = app(&[]);
            app.open_search(None);
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            screen(&mut app, width, 24);
            press_keys(&mut app, "\u{17}l");
            if app.detail_fits() {
                assert!(app.search_feed_open(), "focus the visible post preview");
                press_keys(&mut app, "\u{17}h");
                assert_eq!(app.active, crate::app::SEARCH_TAB);
            } else {
                assert_eq!(app.active, crate::app::SEARCH_TAB, "there is no right pane");
            }
            press(&mut app, 'l');
            let feed = app.active;
            app.focus_sidebar();
            key(&mut app, KeyCode::Right, KeyModifiers::NONE);
            if app.detail_fits() {
                assert_eq!(
                    app.active,
                    crate::app::SEARCH_TAB,
                    "the visible middle pane is Search"
                );
                press(&mut app, 'l');
            } else {
                assert_eq!(app.active, feed);
            }
            app.search("local filter".into());
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            assert_eq!(
                app.active, feed,
                "clear the local filter before leaving its feed"
            );
            assert!(app.active_tab().feed.search.is_none());
            key(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
            assert_eq!(app.active, crate::app::SEARCH_TAB);
            assert_eq!(app.focus, Focus::List);
        }
    }

    #[test]
    fn search_key_audit_parent_retry_guidance_should_not_target_the_child_feed() {
        let mut app = app(&[]);
        app.open_search(None);
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        app.search.discovered.loading = false;
        app.search.discovered.error = Some("parent failed".into());
        screen(&mut app, 200, 24);
        press(&mut app, 'l');
        let actual = screen(&mut app, 200, 24);
        assert!(actual.contains("parent failed"), "{actual}");
        assert!(
            !actual.contains("r retries"),
            "r reloads the focused child feed: {actual}"
        );
    }

    #[test]
    fn search_key_audit_kind_hints_should_follow_list_focus_and_ancestor_navigation() {
        let mut app = app(&[]);
        app.open_search(None);
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        app.search.query.value = "query".into();
        app.tabs[crate::app::SEARCH_TAB].feed.search = Some("query".into());
        app.tabs[crate::app::SEARCH_TAB].feed.posts = vec![post_with("result", Media::None)];
        screen(&mut app, 200, 24);
        press(&mut app, 'l');
        press(&mut app, 'h');
        assert!(app.post_view.is_some());
        assert!(
            crate::keys::footer_hints(&app)
                .iter()
                .any(|(key, label)| key == "Tab" && label == "kind")
        );
        let before = app.search.kind;
        key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        assert_ne!(app.search.kind, before);

        let mut app = self::app(&[]);
        app.switch_tab(0);
        app.tabs[0].feed.posts = vec![post_with("origin", Media::None)];
        app.open_current_post();
        press_keys(&mut app, "gtgtgt");
        assert_eq!(app.active, crate::app::SEARCH_TAB);
        assert!(app.ancestor_following());
        assert!(
            !help::entries(&app).iter().any(
                |(key, description)| key.contains("Tab") && description.contains("Communities")
            ),
            "Tab keeps navigating buffers while following post depth"
        );
        let before = app.search.kind;
        key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        assert_ne!(app.active, crate::app::SEARCH_TAB);
        assert_eq!(app.search.kind, before);
    }

    #[test]
    fn search_key_audit_other_row_views_should_keep_motion_parity() {
        for all_subs in [false, true] {
            let mut app = app(&["rust", "zig"]);
            let mut posts = (0..20)
                .map(|index| post_with(&format!("item{index}"), Media::None))
                .collect::<Vec<_>>();
            if all_subs {
                app.switch_tab(crate::app::ALL_SUBS_TAB);
                app.config.all_subs_posts_per_group = 10;
                app.all_subs.groups[1].posts = posts.split_off(10);
                app.all_subs.groups[0].posts = posts;
            } else {
                app.switch_tab(0);
                if app.state.list_view() != ListView::Compact {
                    app.state.toggle_list_view();
                }
                app.tabs[0].feed.posts = posts;
            }
            app.focus_main();
            assert_eq!(app.current_post().unwrap().id, "item0");
            key(&mut app, KeyCode::Char('e'), KeyModifiers::CONTROL);
            assert_eq!(app.current_post().unwrap().id, "item1");
            key(&mut app, KeyCode::Char('y'), KeyModifiers::CONTROL);
            assert_eq!(app.current_post().unwrap().id, "item0");
            key(&mut app, KeyCode::Char('d'), KeyModifiers::CONTROL);
            assert_eq!(app.current_post().unwrap().id, "item5");
            key(&mut app, KeyCode::Char('f'), KeyModifiers::CONTROL);
            assert_eq!(app.current_post().unwrap().id, "item15");
            key(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
            assert_eq!(app.current_post().unwrap().id, "item10");
            key(&mut app, KeyCode::Char('b'), KeyModifiers::CONTROL);
            assert_eq!(app.current_post().unwrap().id, "item0");
        }
    }

    #[test]
    fn search_key_audit_ownership_sidebar_should_keep_the_active_nested_feed() {
        let mut app = app(&["rust"]);
        app.config.show_nsfw = true;
        app.open_search(None);
        app.search.discovered.loading = false;
        app.search.discovered.entries.push(crate::app::search::Row {
            profile: None,
            label: "r/rust".into(),
            detail: String::new(),
            target: crate::worker::FeedTarget::Subreddit("rust".into()),
        });
        app.search.selected = 2;
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        press(&mut app, 'l');
        app.on_msg(Msg::Feed {
            tab: app.active_tab().id,
            generation: app.active_tab().feed.generation,
            append: false,
            result: Ok(crate::model::Listing {
                posts: vec![post_with("readable", Media::None)],
                after: None,
            }),
        });
        app.load_feed(app.active, false);
        app.on_msg(Msg::Feed {
            tab: app.active_tab().id,
            generation: app.active_tab().feed.generation,
            append: false,
            result: Err("offline".into()),
        });
        screen(&mut app, 80, 30);
        key(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL);
        press(&mut app, 'h');
        assert_eq!(app.focus, Focus::Sidebar);
        let child = app.active;
        let failed = screen(&mut app, 200, 30);
        assert!(app.search_feed_open() && app.post_view.is_none());
        assert!(
            failed.contains("r retry") && !failed.contains("C-w l then r"),
            "{failed}"
        );
        let generation = app.active_tab().feed.generation;
        press(&mut app, 'r');
        assert_eq!(app.active, child);
        assert_ne!(app.active_tab().feed.generation, generation);
        app.search("empty".into());
        app.on_msg(Msg::Feed {
            tab: app.active_tab().id,
            generation: app.active_tab().feed.generation,
            append: false,
            result: Ok(crate::model::Listing {
                posts: vec![],
                after: None,
            }),
        });
        let empty = screen(&mut app, 200, 30);
        assert!(empty.contains("Esc then Esc clears it"), "{empty}");
        press(&mut app, '/');
        assert!(matches!(app.overlay, Some(Overlay::Search { .. })));
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.focus, Focus::List);
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.active, child);
        assert!(app.active_tab().feed.search.is_none() && app.search_feed_open());
    }

    #[test]
    fn search_key_audit_ownership_empty_feeds_should_focus_list_for_local_controls() {
        let mut app = app(&["rust"]);
        app.config.show_nsfw = true;
        app.open_search(None);
        app.search.discovered.loading = false;
        app.search.discovered.entries.push(crate::app::search::Row {
            profile: None,
            label: "r/rust".into(),
            detail: String::new(),
            target: crate::worker::FeedTarget::Subreddit("rust".into()),
        });
        app.search.selected = 2;
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        press(&mut app, 'l');
        let post = post_with("retained", Media::None);
        app.on_msg(Msg::Feed {
            tab: app.active_tab().id,
            generation: app.active_tab().feed.generation,
            append: false,
            result: Ok(crate::model::Listing {
                posts: vec![post.clone()],
                after: None,
            }),
        });
        screen(&mut app, 200, 30);
        press(&mut app, 'l');
        let detail = app.post_view.as_ref().unwrap().generation;
        app.on_msg(Msg::Post {
            generation: detail,
            result: Ok(Box::new(PostPage {
                post,
                body: "Retained detail".into(),
                comments: vec![],
            })),
        });
        key(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL);
        press(&mut app, 'h');
        app.search("empty".into());
        app.on_msg(Msg::Feed {
            tab: app.active_tab().id,
            generation: app.active_tab().feed.generation,
            append: false,
            result: Ok(crate::model::Listing {
                posts: vec![],
                after: None,
            }),
        });
        key(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL);
        press(&mut app, 'l');
        let empty = screen(&mut app, 200, 30);
        assert!(
            empty.contains("C-w h then / edits") && empty.contains("C-w h then Esc clears"),
            "{empty}"
        );
        key(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL);
        press(&mut app, 'h');
        press(&mut app, '/');
        assert!(matches!(app.overlay, Some(Overlay::Search { .. })));
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.active_tab().feed.search.is_none());
        assert_eq!(app.post_view.as_ref().unwrap().generation, detail);
        app.on_msg(Msg::Feed {
            tab: app.active_tab().id,
            generation: app.active_tab().feed.generation,
            append: false,
            result: Ok(crate::model::Listing {
                posts: vec![],
                after: None,
            }),
        });
        key(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL);
        press(&mut app, 'l');
        let empty = screen(&mut app, 200, 30);
        assert!(empty.contains("C-w h then SPC f s"), "{empty}");
        key(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL);
        press(&mut app, 'h');
        press_keys(&mut app, " fs");
        assert!(matches!(app.overlay, Some(Overlay::Sort { .. })));
    }

    #[test]
    fn search_key_audit_ownership_retained_detail_should_update_its_cached_controls() {
        for kind in [
            "error",
            "spoiler",
            "more",
            "gallery",
            "media",
            "spoiler_gallery",
        ] {
            let (mut app, _) = open_long_post(|_| {});
            let mut page = app
                .post_view
                .as_ref()
                .unwrap()
                .page
                .as_ref()
                .unwrap()
                .clone();
            page.body = "Reading the retained post".into();
            page.comments[0].body = "Reading its comment".into();
            page.comments[0].more = Some(crate::model::MoreReplies {
                cursor: "/next".into(),
                count: Some(2),
            });
            page.post.spoiler = matches!(kind, "spoiler" | "spoiler_gallery");
            page.post.media = match kind {
                "gallery" | "spoiler_gallery" => Media::Gallery {
                    images: vec![
                        "https://i.redd.it/one.jpg".into(),
                        "https://i.redd.it/two.jpg".into(),
                    ],
                    previews: vec![],
                    thumb: None,
                },
                "media" => Media::Image {
                    url: "https://i.redd.it/one.jpg".into(),
                    preview: None,
                },
                _ => Media::None,
            };
            app.open_search(None);
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            press(&mut app, 'l');
            app.on_msg(Msg::Feed {
                tab: app.active_tab().id,
                generation: app.active_tab().feed.generation,
                append: false,
                result: Ok(crate::model::Listing {
                    posts: vec![page.post.clone()],
                    after: None,
                }),
            });
            screen(&mut app, 200, 40);
            press(&mut app, 'l');
            let generation = app.post_view.as_ref().unwrap().generation;
            app.on_msg(Msg::Post {
                generation,
                result: if kind == "error" {
                    Err("offline".into())
                } else {
                    Ok(Box::new(page))
                },
            });
            let cue = match kind {
                "error" => "press r to retry",
                "spoiler" | "spoiler_gallery" => "zs reveals the post",
                "more" => "more replies · Enter",
                "gallery" => "[i ]i browse",
                _ => "O opens",
            };
            let focused = screen(&mut app, 200, 40);
            assert!(focused.contains(cue), "{kind}: {focused}");
            if kind == "spoiler_gallery" {
                assert!(!focused.contains("[i ]i browse"), "{focused}");
                press_keys(&mut app, "]i");
                assert_eq!(app.post_view.as_ref().unwrap().gallery_index, 0);
                press_keys(&mut app, "zs");
                let revealed = screen(&mut app, 200, 40);
                assert!(revealed.contains("[i ]i browse"), "{revealed}");
                press_keys(&mut app, "]i");
                assert_eq!(app.post_view.as_ref().unwrap().gallery_index, 1);
                press_keys(&mut app, "zs");
                assert!(!screen(&mut app, 200, 40).contains("[i ]i browse"));
            }
            for sidebar in [false, true] {
                key(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL);
                press(&mut app, 'h');
                if sidebar {
                    key(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL);
                    press(&mut app, 'h');
                }
                let background = screen(&mut app, 200, 40);
                assert!(
                    !background.contains(cue),
                    "{kind}: another pane owns these keys: {background}"
                );
                if sidebar {
                    key(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL);
                    press(&mut app, 'l');
                }
                key(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL);
                press(&mut app, 'l');
                let restored = screen(&mut app, 200, 40);
                assert!(
                    restored.contains(cue),
                    "{kind}: cached controls must return with focus: {restored}"
                );
                assert_eq!(app.post_view.as_ref().unwrap().generation, generation);
            }
        }
    }

    #[test]
    fn search_key_audit_ownership_shared_island_keys_should_name_the_selected_group() {
        let mut app = app(&["rust", "vim"]);
        app.switch_tab(crate::app::ALL_SUBS_TAB);
        for group in &mut app.all_subs.groups {
            group.error = Some("offline".into());
            group.posts = vec![post_with("readable", Media::None)];
        }
        app.focus_main();
        let rows = screen(&mut app, 200, 30);
        assert_eq!(rows.matches("✗ r retry").count(), 1, "{rows}");
        press(&mut app, 'l');
        let detail = screen(&mut app, 200, 30);
        assert!(
            !detail.contains("✗ r retry") && !detail.contains("zc Card"),
            "{detail}"
        );
    }

    #[test]
    fn search_key_audit_feed_retry_should_leave_focused_detail_before_reloading() {
        for nested in [false, true] {
            for append in [false, true] {
                let mut app = app(&[]);
                app.open_search(Some(crate::reddit::experience::SearchKind::Posts));
                if !nested {
                    crate::keys::paste(&mut app, "query");
                    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
                }
                key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
                if nested {
                    press(&mut app, 'l');
                }
                app.on_msg(Msg::Feed {
                    tab: app.active_tab().id,
                    generation: app.active_tab().feed.generation,
                    append: false,
                    result: Ok(crate::model::Listing {
                        posts: vec![post_with("readable", Media::None)],
                        after: Some("next".into()),
                    }),
                });
                screen(&mut app, 200, 30);
                app.load_feed(app.active, append);
                app.on_msg(Msg::Feed {
                    tab: app.active_tab().id,
                    generation: app.active_tab().feed.generation,
                    append,
                    result: Err("offline".into()),
                });
                press(&mut app, 'l');
                assert_eq!(app.focus, Focus::Post);
                let failed = screen(&mut app, 200, 30);
                assert!(
                    failed.contains("C-w h then r retry"),
                    "feed retry must first focus the visible list: {failed}"
                );
                if append {
                    assert!(
                        failed.contains("Next page failed · C-w h then r retries"),
                        "{failed}"
                    );
                }
                let feed = app.active_tab().feed.generation;
                let detail = app.post_view.as_ref().unwrap().generation;
                key(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL);
                press(&mut app, 'h');
                assert_eq!(app.focus, Focus::List);
                assert!(app.post_view.is_some());
                press(&mut app, 'r');
                assert_ne!(app.active_tab().feed.generation, feed);
                assert_eq!(app.post_view.as_ref().unwrap().generation, detail);
            }
        }
    }

    #[test]
    fn search_key_audit_posts_guidance_should_follow_editor_sidebar_and_auth_ownership() {
        for cached in [false, true] {
            let mut app = app(&[]);
            app.open_search(Some(crate::reddit::experience::SearchKind::Posts));
            crate::keys::paste(&mut app, "query");
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            press(&mut app, '/');
            if cached {
                app.tabs[crate::app::SEARCH_TAB].feed.posts =
                    vec![post_with("cached", Media::None)];
            }
            app.on_msg(Msg::Feed {
                tab: app.active_tab().id,
                generation: app.active_tab().feed.generation,
                append: false,
                result: Err("offline".into()),
            });
            let failed = screen(&mut app, 200, 24);
            assert!(
                failed.contains("Esc then r"),
                "retry must leave the editor first: {failed}"
            );
            let generation = app.active_tab().feed.generation;
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            press(&mut app, 'r');
            assert_ne!(app.active_tab().feed.generation, generation);
            assert_eq!(app.search.query.value, "query");
            if !cached {
                app.on_msg(Msg::Feed {
                    tab: app.active_tab().id,
                    generation: app.active_tab().feed.generation,
                    append: false,
                    result: Ok(crate::model::Listing {
                        posts: vec![],
                        after: None,
                    }),
                });
                app.focus_sidebar();
                let empty = screen(&mut app, 200, 24);
                assert!(empty.contains("No results"), "{empty}");
                assert!(
                    !empty.contains("Tab for another kind"),
                    "Sidebar Tab switches buffers: {empty}"
                );
                let kind = app.search.kind;
                key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
                assert_ne!(app.active, crate::app::SEARCH_TAB);
                assert_eq!(app.search.kind, kind);
            }
        }
        for view in [ListView::Card, ListView::Compact] {
            let mut app = app(&[]);
            app.open_search(Some(crate::reddit::experience::SearchKind::Posts));
            crate::keys::paste(&mut app, "query");
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            press(&mut app, '/');
            if app.state.list_view() != view {
                app.state.toggle_list_view();
            }
            app.on_msg(Msg::Feed {
                tab: app.active_tab().id,
                generation: app.active_tab().feed.generation,
                append: false,
                result: Ok(crate::model::Listing {
                    posts: vec![post_with("existing", Media::None)],
                    after: Some("next".into()),
                }),
            });
            app.load_feed(crate::app::SEARCH_TAB, true);
            app.on_msg(Msg::Feed {
                tab: app.active_tab().id,
                generation: app.active_tab().feed.generation,
                append: true,
                result: Err("offline".into()),
            });
            let failed = screen(&mut app, 200, 30);
            assert!(
                failed.contains("Next page failed · Esc then r retries"),
                "the continuation boundary must agree with the query editor: {failed}"
            );
            let generation = app.active_tab().feed.generation;
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            press(&mut app, 'r');
            assert_ne!(app.active_tab().feed.generation, generation);
            assert_eq!(app.search.query.value, "query");
        }
        for preview in [false, true] {
            let mut app = app(&[]);
            app.open_search(Some(crate::reddit::experience::SearchKind::Posts));
            let old = post_with("existing", Media::None);
            let incoming = post_with("incoming", Media::None);
            if preview {
                app.search.discovered.loading = false;
                app.search.preview.target = Some(app.search.selected_row().unwrap().target);
                app.search.preview.posts = vec![old.clone()];
                app.search.preview.update = Some(vec![incoming, old]);
                app.search.preview.loading = false;
            } else {
                crate::keys::paste(&mut app, "query");
                key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
                press(&mut app, '/');
                app.on_msg(Msg::Feed {
                    tab: app.active_tab().id,
                    generation: app.active_tab().feed.generation,
                    append: false,
                    result: Ok(crate::model::Listing {
                        posts: vec![old.clone()],
                        after: None,
                    }),
                });
                screen(&mut app, 200, 30);
                app.load_feed(crate::app::SEARCH_TAB, false);
                app.on_msg(Msg::Feed {
                    tab: app.active_tab().id,
                    generation: app.active_tab().feed.generation,
                    append: false,
                    result: Ok(crate::model::Listing {
                        posts: vec![incoming, old],
                        after: None,
                    }),
                });
            }
            let pending = screen(&mut app, 200, 30);
            assert!(
                pending.contains("New posts · Esc then r refresh"),
                "{pending}"
            );
            let query = app.search.query.value.clone();
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            press(&mut app, 'r');
            let refreshed = screen(&mut app, 200, 30);
            assert!(refreshed.contains("incoming"), "{refreshed}");
            assert!(!refreshed.contains("New posts"), "{refreshed}");
            assert_eq!(app.search.query.value, query);
        }
        let mut app = app(&[]);
        app.reddit = Reddit::disconnected(false).unwrap();
        app.health = Health::SignedOut;
        app.open_search(Some(crate::reddit::experience::SearchKind::Posts));
        crate::keys::paste(&mut app, "query");
        let connect = screen(&mut app, 200, 24);
        let brand = connect.lines().nth(2).unwrap();
        assert!(
            brand.contains("list keys") && brand.contains("F1"),
            "{connect}"
        );
        assert!(connect.contains("Press Esc then SPC p a"), "{connect}");
        assert!(
            connect.lines().last().unwrap().contains("Esc then SPC p a"),
            "{connect}"
        );
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(crate::keys::map::available(
            &app,
            crate::keys::map::Action::Accounts
        ));
    }

    #[test]
    fn search_revision_should_keep_incremental_panes_and_restore_parent_lists() {
        for width in [100, 160, 200] {
            let mut app = app(&[]);
            app.open_search(None);
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            app.search.discovered.loading = false;
            app.search.discovered.entries.push(crate::app::search::Row {
                profile: None,
                label: "r/zig".into(),
                detail: "Community summary marker".into(),
                target: crate::worker::FeedTarget::Subreddit("zig".into()),
            });
            app.search.selected = 2;
            screen(&mut app, width, 30);
            let split = app.detail_fits();
            press(&mut app, 'l');
            let mut first = post_with("nested_first", Media::None);
            first.subreddit = "zig".into();
            let mut second = post_with("nested_second", Media::None);
            second.subreddit = "zig".into();
            app.on_msg(Msg::Feed {
                tab: app.active_tab().id,
                generation: app.active_tab().feed.generation,
                append: false,
                result: Ok(crate::model::Listing {
                    posts: vec![first, second],
                    after: None,
                }),
            });
            press(&mut app, 'j');
            let feed = screen(&mut app, width, 30);
            assert_eq!(
                feed.contains("Community summary marker"),
                split,
                "community rows should remain beside the active feed: {feed}"
            );
            assert_eq!(app.current_post().unwrap().id, "nested_second");
            press(&mut app, 'l');
            let post = screen(&mut app, width, 30);
            assert!(app.post_view.is_some());
            assert!(
                !post.contains("Community summary marker"),
                "the feed replaces communities beside post detail: {post}"
            );
            press(&mut app, 'h');
            let back = screen(&mut app, width, 30);
            assert_eq!(app.focus, crate::app::Focus::List);
            if split {
                let generation = app
                    .post_view
                    .as_ref()
                    .expect("first h keeps detail")
                    .generation;
                assert!(
                    !back.contains("Community summary marker"),
                    "first h focuses posts without shifting the panes: {back}"
                );
                press(&mut app, 'l');
                assert_eq!(app.focus, crate::app::Focus::Post);
                assert_eq!(app.post_view.as_ref().unwrap().generation, generation);
                press(&mut app, 'h');
                press(&mut app, 'h');
            }
            let back = screen(&mut app, width, 30);
            assert!(
                app.post_view.is_none(),
                "leaving the post list restores the community/feed pair"
            );
            assert_eq!(app.focus, crate::app::Focus::List);
            assert_eq!(back.contains("Community summary marker"), split, "{back}");
            assert_eq!(app.current_post().unwrap().id, "nested_second");
            press(&mut app, 'h');
            assert_eq!(app.active, crate::app::SEARCH_TAB);
            assert_eq!(app.search.selected, 2);
        }
    }

    #[test]
    fn reported_explore_should_fill_its_pane_and_wrap_descriptions() {
        use crate::reddit::experience::{Entry, Page, Query, Response};
        let mut app = app(&[]);
        app.open_search(None);
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        app.on_msg(Msg::Search {
            generation: app.search.discovered_generation(),
            result: Ok(Response::Page(Page {
                title: "Discover".into(), text: String::new(), url: "/".into(),
                entries: vec![Entry {
                    title: "r/community_with_a_long_name".into(),
                    body: "A description with enough words to wrap across several lines ending in papaya".into(),
                    query: Some(Query::Community("community_with_a_long_name".into())),
                    target: None, post: None, icon: Default::default(),
                }],
                after: Some("next".into()),
            })),
        });
        let actual = screen(&mut app, 62, 36);
        assert!(
            app.search.discovered.loading,
            "an underfilled Explore pane should load the next page"
        );
        assert!(
            actual.contains("papaya"),
            "the description must fit by wrapping: {actual}"
        );
        let generation = app.search.discovered_generation();
        screen(&mut app, 62, 36);
        assert_eq!(
            app.search.discovered_generation(),
            generation,
            "one page at a time"
        );
        app.on_msg(Msg::Search {
            generation,
            result: Ok(Response::Page(Page {
                title: "Discover".into(),
                text: String::new(),
                url: "/".into(),
                entries: Vec::new(),
                after: Some("next".into()),
            })),
        });
        screen(&mut app, 62, 36);
        assert!(
            !app.search.discovered.loading,
            "a repeated cursor must stop automatic paging"
        );
    }

    #[test]
    fn reported_failed_video_card_should_compact_and_retry_without_resizing() {
        let mut app = card_feed_app(kitty());
        app.tabs[4].feed.posts[0].media = Media::Video {
            gif: false,
            url: Some(VIDEO.into()),
            poster: None,
        };
        screen(&mut app, 100, 30);
        let url = app.reddit.media_url(VIDEO).unwrap();
        app.video = Some(video::Player::unavailable(
            &url,
            app.video_area.unwrap().as_size(),
        ));
        let actual = screen(&mut app, 100, 30);
        assert!(
            app.active_tab().feed.cards.height <= 7,
            "failed media reserved empty rows: {actual}"
        );
        assert!(
            app.video_area.is_none(),
            "compaction must not schedule a decoder resize"
        );
        assert!(actual.contains("p retry"), "{actual}");
        app.toggle_video_pause();
        screen(&mut app, 100, 30);
        assert!(app.video_area.is_some(), "retry restores media geometry");
    }

    #[test]
    fn search_revision_icons_should_leave_a_gap_before_the_title() {
        use crate::reddit::experience::{Entry, Page, Query, Response};
        let mut app = app(&[]);
        app.image_picker = kitty();
        app.show_images = true;
        app.open_search(None);
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        app.on_msg(Msg::Search {
            generation: app.search.discovered_generation(),
            result: Ok(Response::Page(Page {
                title: "Discover".into(),
                text: String::new(),
                entries: vec![Entry {
                    icon: Default::default(),
                    title: "r/zig".into(),
                    body: String::new(),
                    query: Some(Query::Community("zig".into())),
                    target: None,
                    post: None,
                }],
                after: None,
                url: "/".into(),
            })),
        });
        app.on_msg(Msg::Icon {
            subreddit: "zig".into(),
            result: Ok(Some("/icon/zig.png".into())),
        });
        let actual = screen(&mut app, 120, 24);
        let row = actual.lines().find(|line| line.contains("r/zig")).unwrap();
        let label = row[..row.find("r/zig").unwrap()].width();
        let icon_right = usize::from(app.sidebar_width() + PANE_MARGIN.horizontal + 2 + ICON_COLS);
        assert_eq!(
            label.saturating_sub(icon_right),
            1,
            "icon and title need a clear gap: {actual}"
        );
        assert!(
            app.thumbs.contains_key("/icon/zig.png"),
            "a community row draws its icon"
        );
    }

    #[test]
    fn search_tab_should_say_what_it_shows_and_which_keys_act() {
        let mut app = app(&[]);
        app.open_search(None);
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        let lit = |app: &mut App| {
            let mut terminal = Terminal::new(TestBackend::new(120, 20)).unwrap();
            terminal.draw(|f| draw(f, app)).unwrap();
            let buffer = terminal.backend().buffer().clone();
            (0..120).any(|x| {
                buffer[(x, 0)].symbol() == "P"
                    && buffer[(x, 0)].modifier.contains(Modifier::UNDERLINED)
            })
        };

        let explore = screen(&mut app, 120, 20);
        assert!(
            !lit(&mut app) && explore.contains("Search posts (/ to type)"),
            "Explore lights no kind as the shown view: {explore}"
        );
        let footer: Vec<_> = crate::keys::footer_hints(&app)
            .into_iter()
            .map(|(key, label)| format!("{key} {label}"))
            .collect();
        assert_eq!(footer[..2], ["/ type", "Tab kind"]);
        let narrow = screen(&mut app, 70, 30);
        let title = narrow.lines().find(|l| l.contains("Explore")).unwrap();
        assert!(
            !title.contains('…'),
            "the title drops, not cuts, its subtitle: {narrow}"
        );

        press(&mut app, 'h');
        let hints = crate::keys::footer_hints(&app);
        assert_eq!(hints[0], ("j/k".into(), "tabs".into()));
        let kind = app.search.kind;
        key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        assert_ne!(app.active, crate::app::SEARCH_TAB);
        assert_eq!(app.search.kind, kind);

        app.open_search(None);
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        app.resize(200);
        press_keys(&mut app, "\u{17}h");
        assert_eq!(app.focus, Focus::Sidebar);
        assert_eq!(
            crate::keys::footer_hints(&app)[0],
            ("j/k".into(), "tabs".into())
        );
        assert!(
            !screen(&mut app, 200, 24).contains("Enter opens"),
            "the sidebar's Enter only returns focus to the list"
        );
        press(&mut app, 'h');
        assert_eq!(app.focus, Focus::Sidebar);
        press(&mut app, 'l');
        assert!(screen(&mut app, 200, 24).contains("Enter opens"));
        key(&mut app, KeyCode::F(2), KeyModifiers::NONE);
        assert!(matches!(app.overlay, Some(Overlay::Settings)));
        let hints = crate::keys::footer_hints(&app);
        assert_eq!(hints[0], ("j/k".into(), "move".into()));
        assert!(
            !hints.iter().any(|(_, label)| matches!(
                label.as_str(),
                "type" | "kind" | "open feed" | "rows" | "next row"
            )),
            "Settings owns the keys: {hints:?}"
        );
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);

        press(&mut app, '/');
        assert_eq!(
            crate::keys::footer_hints(&app)[0],
            ("↑↓".into(), "choose".into())
        );
        crate::keys::paste(&mut app, "rust");
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(lit(&mut app), "a searched kind is lit");
    }

    #[test]
    fn search_tab_reload_should_show_loading_failures_and_empty_results() {
        use crate::reddit::experience::{Entry, Page, Query, Response, SearchKind};
        let page = |entries| {
            Ok(Response::Page(Page {
                title: "Communities".into(),
                text: String::new(),
                entries,
                after: None,
                url: "/".into(),
            }))
        };
        let mut app = app(&[]);
        app.resize(130);
        app.open_search(Some(SearchKind::Communities));
        for editing in [false, true] {
            if editing {
                app.edit_search();
            } else {
                key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            }
            app.on_msg(Msg::Search {
                generation: app.search.discovered_generation(),
                result: Err("Reddit request timed out; retry when ready".into()),
            });
            for width in [70, 130] {
                let failed = screen(&mut app, width, 24);
                assert!(failed.contains("timed out"), "{failed}");
                assert!(failed.contains("r retries"), "{failed}");
                if editing {
                    assert!(failed.contains("Esc then r"), "{failed}");
                }
            }
            if editing {
                key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            }
            let failed = app.search.discovered_generation();
            press(&mut app, 'r');
            assert!(app.search.discovered.loading);
            assert_ne!(app.search.discovered_generation(), failed);
            assert!(
                app.search.text().is_empty(),
                "retry must not type into the query"
            );
        }
        app.edit_search();
        crate::keys::paste(&mut app, "rust");
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        let entries = vec![Entry {
            icon: Default::default(),
            title: "r/rust".into(),
            body: String::new(),
            query: Some(Query::Community("rust".into())),
            target: None,
            post: None,
        }];
        app.on_msg(Msg::Search {
            generation: app.search.results_generation(),
            result: page(entries.clone()),
        });
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(screen(&mut app, 130, 24).contains("r/rust"));

        press(&mut app, 'r');
        let loading = screen(&mut app, 130, 24);
        assert!(loading.contains("Searching Reddit"), "{loading}");
        app.on_msg(Msg::Search {
            generation: app.search.results_generation(),
            result: Err("offline".into()),
        });
        let failed = screen(&mut app, 130, 24);
        assert!(
            failed.contains("Search failed") && failed.contains("r to retry"),
            "{failed}"
        );

        press(&mut app, 'r');
        app.on_msg(Msg::Search {
            generation: app.search.results_generation(),
            result: page(Vec::new()),
        });
        let empty = screen(&mut app, 130, 24);
        assert!(empty.contains("No matches"), "{empty}");

        press(&mut app, 'r');
        app.on_msg(Msg::Search {
            generation: app.search.results_generation(),
            result: page(entries),
        });
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        let rows = screen(&mut app, 130, 24);
        assert!(
            rows.contains("r/rust") && !rows.contains("Enter opens"),
            "{rows}"
        );
    }

    #[test]
    fn search_tab_preview_should_draw_feed_cards_with_pictures() {
        let mut app = app(&[]);
        app.image_picker = kitty();
        app.show_images = true;
        app.open_search(None);
        app.resize(160);
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        app.search.preview.target = Some(app.search.selected_row().unwrap().target);
        app.search.preview.loading = false;
        app.search.preview.posts = vec![post_with(
            "pic",
            Media::Image {
                url: "https://i.redd.it/pic.jpeg".into(),
                preview: None,
            },
        )];

        let shown = screen(&mut app, 160, 24);

        assert!(
            app.card_images.contains_key("https://i.redd.it/pic.jpeg"),
            "the preview loads the card's picture"
        );
        assert!(shown.contains("comments"), "{shown}");
        assert_eq!(
            shown.matches('▌').count(),
            1,
            "only the rows mark a selection, not the preview's cards: {shown}"
        );
    }

    #[test]
    fn search_tab_rows_should_keep_the_preview_beside_them_at_common_widths() {
        let mut app = app(&[]);
        app.open_search(None);
        app.resize(160);
        assert!(
            help::entries(&app)
                .iter()
                .any(|(_, text)| text == "Move the cursor"),
            "typing help lists cursor keys"
        );
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);

        let shown = screen(&mut app, 160, 20);

        assert!(shown.contains("Enter opens"), "{shown}");
        let bar = shown
            .lines()
            .find(|line| line.contains("Search posts"))
            .unwrap();
        assert!(
            bar.contains("Posts │ Communities │ People"),
            "query and kinds share one bar above the list and preview: {shown}"
        );

        let target = app.search.selected_row().unwrap().target;
        app.search.preview.target = Some(target);
        app.search.preview.loading = false;
        let mut hidden = post_with("hidden", Media::None);
        hidden.title = "spoilers ahead".into();
        app.config.filters.keywords = vec!["spoilers".into()];
        app.search.preview.posts = vec![hidden, post_with("kept", Media::None)];
        app.search.preview.error = Some("offline".into());
        let shown = screen(&mut app, 160, 20);
        assert!(
            shown.contains("refresh failed") && shown.contains("kept"),
            "a failed refresh keeps the cached posts: {shown}"
        );

        press(&mut app, '/');
        key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        crate::keys::paste(&mut app, "zzzz");
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        let searching = screen(&mut app, 160, 20);
        assert!(
            searching.contains("Searching Reddit") && !searching.contains("j/k choose"),
            "no rows, no row hint beside them: {searching}"
        );
        assert!(
            !shown.contains("spoilers ahead"),
            "hide filters apply to the preview: {shown}"
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
        for _ in 0..7 {
            press(&mut app, 'j');
            seen.push(name(&app));
        }
        press(&mut app, 'k');
        let after_k = name(&app);
        press_keys(&mut app, "gg");
        let first = name(&app);
        let shown = screen(&mut app, 100, 20);
        assert!(shown.contains("⌕ Search"), "{shown}");

        assert_eq!(
            seen,
            [
                "Home",
                "Bookmarks",
                "All subs",
                "Search",
                "r/a",
                "r/b",
                "r/c",
                "r/c"
            ]
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
    fn prefix_hint_should_not_interrupt_a_fast_key_sequence() {
        let mut app = app(&["rust", "vim"]);
        let footer = crate::keys::footer_hints(&app);
        press(&mut app, ']');

        let shown = screen(&mut app, 100, 24);
        assert!(!shown.contains("Next · ] · Esc cancel"), "{shown}");
        assert!(!app.popup_open(), "a pending key is not yet a popup");
        assert_eq!(crate::keys::footer_hints(&app), footer);

        press(&mut app, 's');
        assert_eq!(app.all_subs.group, 1);

        app.reddit_menu();
        let before = popup_snapshot(&mut app, 80, 24);
        press(&mut app, 's');
        let after = popup_snapshot(&mut app, 80, 24);
        assert_eq!(
            after, before,
            "the menu stays visible while the hint is delayed"
        );
    }

    #[test]
    fn prefix_hint_should_wait_for_the_latest_key_and_stay_open_while_narrowing() {
        use std::time::{Duration, Instant};
        let mut app = app(&[]);
        press(&mut app, ' ');
        let before = Instant::now() - Duration::from_millis(400);
        app.keymap.schedule_hint(before);
        press(&mut app, 'p');
        assert!(!app.keymap.tick(before + crate::keys::map::HINT_DELAY));
        assert!(!app.popup_open());
        assert!(
            app.keymap
                .tick(Instant::now() + crate::keys::map::HINT_DELAY)
        );
        assert!(screen(&mut app, 80, 24).contains("Preferences"));
        press(&mut app, 's');
        assert!(matches!(app.overlay, Some(Overlay::Settings)));

        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        press(&mut app, ' ');
        assert!(
            app.keymap
                .tick(Instant::now() + crate::keys::map::HINT_DELAY)
        );
        press(&mut app, 'p');
        assert!(app.popup_open());
        assert!(screen(&mut app, 80, 24).contains("Preferences"));
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(!app.popup_open());
        assert!(
            !app.keymap
                .tick(Instant::now() + crate::keys::map::HINT_DELAY)
        );
    }

    #[test]
    fn vim_prefixes_should_wait_cancel_and_open_the_named_action() {
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut app = app(&["rust", "vim"]);
        app.focus_sidebar();
        app.sidebar_edge(true);
        let active = app.active;

        press(&mut app, 'g');
        assert_eq!(app.active, active, "a pending g must not move yet");
        press(&mut app, 'g');
        assert_eq!(app.active, 0);
        press(&mut app, ' ');
        app.keymap
            .tick(std::time::Instant::now() + crate::keys::map::HINT_DELAY);
        assert!(screen(&mut app, 80, 24).contains("Preferences"));
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.overlay.is_none());
        for c in " ps".chars() {
            press(&mut app, c);
        }
        assert!(matches!(app.overlay, Some(Overlay::Settings)));
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        app.overlay = Some(Overlay::Picker(crate::app::PickerState::default()));
        for c in " ps".chars() {
            press(&mut app, c);
        }
        assert!(matches!(&app.overlay, Some(Overlay::Picker(picker)) if picker.input == " ps"));
        key(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL);
        assert!(matches!(&app.overlay, Some(Overlay::Picker(picker)) if picker.input == " "));
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        press(&mut app, ' ');
        press(&mut app, 'p');
        app.switch_tab(4);
        press(&mut app, 's');
        assert!(
            app.overlay.is_none(),
            "a prefix must not survive switching contexts"
        );
    }

    #[test]
    fn leader_hints_should_page_without_skipping_choices_in_a_short_pane() {
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut app = app(&[]);
        press(&mut app, ' ');
        app.keymap
            .tick(std::time::Instant::now() + crate::keys::map::HINT_DELAY);
        let mut saw_preferences = false;
        for _ in 0..20 {
            let actual = screen(&mut app, 30, 8);
            saw_preferences |= actual.contains("Preferences");
            key(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
        }
        assert!(saw_preferences);
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.keymap.prefix.is_empty());
        assert!(!app.quit);
    }

    #[test]
    fn modified_keys_should_not_close_dialogs_or_confirm_account_actions() {
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut app = app(&["rust"]);
        app.show_settings();
        key(&mut app, KeyCode::Char('q'), KeyModifiers::CONTROL);
        assert!(matches!(app.overlay, Some(Overlay::Settings)));
        app.overlay = Some(Overlay::Sort { selected: 0 });
        key(&mut app, KeyCode::Char('q'), KeyModifiers::ALT);
        assert!(matches!(app.overlay, Some(Overlay::Sort { .. })));
        assert!(!app.quit);
        app.overlay = Some(Overlay::Membership {
            target: crate::model::MembershipTarget::Community("rust".into()),
            join: true,
        });
        key(&mut app, KeyCode::Enter, KeyModifiers::CONTROL);
        assert!(matches!(app.overlay, Some(Overlay::Membership { .. })));
    }

    #[test]
    fn alt_keys_should_preserve_the_current_view_and_dialog() {
        let (mut app, _) = open_long_post(|_| {});
        key(&mut app, KeyCode::Char('?'), KeyModifiers::ALT);
        assert!(app.post_view.is_some());
        assert!(app.help_scroll.is_none());

        let mut feed = feed_app(Picker::halfblocks());
        let active = feed.active;
        press_keys(&mut feed, " fs");
        key(&mut feed, KeyCode::Tab, KeyModifiers::ALT);
        assert!(matches!(feed.overlay, Some(Overlay::Sort { .. })));
        assert_eq!(feed.active, active);
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
        press_keys(&mut app, " bb");
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
        app.switch_tab(4);
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
    fn ctrl_f_pages_a_post_by_its_visible_height_and_stops_at_the_end() {
        let (mut app, first) = open_long_post(|_| {});
        let paragraphs = |s: &str| -> Vec<usize> {
            s.lines()
                .filter_map(|l| l.split("paragraph ").nth(1))
                .filter_map(|r| r.split_whitespace().next()?.parse().ok())
                .collect()
        };
        let before = paragraphs(&first);

        key(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
        let after = paragraphs(&screen(&mut app, 100, 30));
        for _ in 0..50 {
            key(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
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
    fn post_help_should_put_comment_navigation_before_reddit_tools() {
        let (mut app, _) = open_long_post(|_| {});
        press(&mut app, '?');

        let shown = screen(&mut app, 100, 24);

        assert!(shown.contains("Next / previous comment"));
        let entries = help::entries(&app);
        for motion in ["]] / [[", "]p / [p", "]s / [s", "gt / gT"] {
            assert!(
                entries.iter().any(|(key, _)| key == motion),
                "missing {motion}"
            );
        }
        let position = |wanted: &str| entries.iter().position(|(key, _)| key == wanted).unwrap();
        assert!(position("Here") < position("Move"));
        assert!(position("Move") < position("C-d / C-u"));
        assert!(position("Space leader") < position("SPC b b"));
    }

    #[test]
    fn help_should_follow_the_focused_pane_when_a_post_remains_beside_the_list() {
        let (mut app, _) = open_long_post(|_| {});
        app.resize(160);
        app.focus_main();
        press(&mut app, '?');

        let shown = screen(&mut app, 160, 20);

        assert!(shown.contains("Feed"));
        assert!(!shown.contains("Post and comments"));
    }

    #[test]
    fn help_should_prioritize_each_list_view() {
        for (tab, compact, title, expected_key) in [
            (0, false, "Feed", "Toggle Card / Compact"),
            (1, true, "Feed", "Remove the local bookmark"),
            (2, false, "Feed", "Next / previous community group"),
            (4, false, "Feed", "Search this community"),
        ] {
            let mut app = card_feed_app(Picker::halfblocks());
            if tab == 1 {
                let post = app.tabs[4].feed.posts[0].clone();
                app.state.toggle_bookmark(&post, 0);
            }
            app.active = tab;
            if compact {
                app.state.toggle_list_view();
            }
            key(&mut app, KeyCode::F(1), KeyModifiers::NONE);

            let shown = screen(&mut app, 120, 20);

            assert!(shown.contains(title), "missing {title}");
            assert!(shown.contains(expected_key), "missing {expected_key}");
        }
    }

    #[test]
    fn sidebar_help_should_prioritize_switching_tabs() {
        let mut app = app(&[]);
        app.focus = Focus::Sidebar;
        press(&mut app, '?');

        let shown = screen(&mut app, 100, 12);

        assert!(shown.contains("Sidebar and buffers"));
        assert!(shown.contains("Next / previous sidebar tab"));
        assert!(!shown.contains("Reddit: saved"));
    }

    #[test]
    fn compact_help_should_put_applicable_controls_before_card_line_scrolling() {
        let mut app = feed_app(Picker::halfblocks());
        app.active = 0;
        key(&mut app, KeyCode::F(1), KeyModifiers::NONE);

        let shown = screen(&mut app, 100, 12);

        assert!(shown.contains("Feed"));
        assert!(shown.contains("Card / Compact view"));
        assert!(!shown.contains("cards: scroll down / up one line"));
    }

    #[test]
    fn selected_comment_help_should_prioritize_comment_actions() {
        let (mut app, _) = open_long_post(|_| {});
        app.activate_comment();
        press(&mut app, '?');

        let shown = screen(&mut app, 100, 12);

        assert!(shown.contains("Post and comments"));
        assert!(!help::entries(&app).iter().any(|(key, _)| key == "za"));
        assert!(shown.contains("Reddit content actions"));
    }

    #[test]
    fn video_help_should_prioritize_playback_controls() {
        let mut app = open_video_post();
        press(&mut app, '?');

        let shown = screen(&mut app, 100, 12);

        assert!(shown.contains("Post and comments"));
        assert!(shown.contains("Play / pause / retry video"));
        assert!(!shown.contains("Reddit: saved"));
    }

    #[test]
    fn help_should_prioritize_reading_when_the_video_is_scrolled_away() {
        let mut app = open_video_post();
        app.scroll_post(1000);
        screen(&mut app, 100, 30);
        assert!(!app.post_view.as_ref().unwrap().image_shown);
        press(&mut app, '?');

        let shown = screen(&mut app, 100, 12);

        assert!(shown.contains("Post and comments"));
        assert!(shown.contains("Next / previous comment"));
        assert!(!shown.contains("video: pause"));
    }

    #[test]
    fn gallery_help_should_prioritize_browsing_images() {
        let mut app = image_post("caption");
        app.post_view
            .as_mut()
            .unwrap()
            .page
            .as_mut()
            .unwrap()
            .post
            .media = Media::Gallery {
            images: vec!["https://i.redd.it/first.png".into()],
            previews: Vec::new(),
            thumb: None,
        };
        press(&mut app, '?');

        let shown = screen(&mut app, 100, 12);

        assert!(shown.contains("Post and comments"));
        assert!(shown.contains("Next / previous gallery image"));
        assert!(!shown.contains("Reddit: saved"));
    }

    #[test]
    fn hidden_media_help_should_prioritize_revealing_spoilers() {
        let mut app = open_video_post();
        app.post_view
            .as_mut()
            .unwrap()
            .page
            .as_mut()
            .unwrap()
            .post
            .spoiler = true;
        press(&mut app, '?');

        let shown = screen(&mut app, 100, 10);

        assert!(shown.contains("Post and comments"));
        assert!(shown.contains("Reveal / hide spoilers"));
        assert!(!shown.contains("video: pause"));
        assert!(!help::entries(&app).iter().any(|(key, _)| key == "/"));
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        press_keys(&mut app, "zs");
        press(&mut app, '/');
        assert!(
            matches!(app.experience.screens.last(), Some(crate::app::experience::Screen::Input(form)) if form.title == "Find in loaded comments")
        );
    }

    #[test]
    fn help_should_preserve_text_dialogs_and_literal_question_marks() {
        for overlay in [
            Overlay::Picker(crate::app::PickerState {
                input: "draft".into(),
                selected: 2,
            }),
            Overlay::Search {
                input: "draft".into(),
            },
        ] {
            let mut app = app(&[]);
            app.overlay = Some(overlay);
            press(&mut app, '?');
            key(&mut app, KeyCode::F(1), KeyModifiers::NONE);
            crate::keys::paste(&mut app, "ignored paste");
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);

            assert!(app.help_scroll.is_none());
            match app.overlay.as_ref().unwrap() {
                Overlay::Picker(p) => assert_eq!(p.input, "draft?"),
                Overlay::Search { input } => assert_eq!(input, "draft?"),
                _ => panic!("help replaced the text dialog"),
            }
        }
    }

    #[test]
    fn sort_help_should_only_offer_the_current_feeds_supported_sorts() {
        for (context, rising, best) in [(0, false, false), (1, true, true), (2, true, false)] {
            let mut app = app(&["rust"]);
            match context {
                0 => app.open_user("blackvault"),
                1 => app.active = 0,
                _ => app.open_subreddit("rust"),
            }
            app.overlay = Some(Overlay::Sort { selected: 0 });
            key(
                &mut app,
                crossterm::event::KeyCode::F(1),
                crossterm::event::KeyModifiers::NONE,
            );
            let shown = screen(&mut app, 120, 60);
            assert_eq!(shown.contains("rising"), rising, "{shown}");
            assert_eq!(shown.contains("best"), best, "{shown}");
            assert!(shown.contains("controversial"), "{shown}");
        }
    }

    #[test]
    fn dialog_help_should_show_the_active_action_before_view_shortcuts() {
        for (overlay, title, description) in [
            (
                Overlay::Picker(crate::app::PickerState::default()),
                "Text entry",
                "Accept a single-line",
            ),
            (
                Overlay::Search {
                    input: "query".into(),
                },
                "Text entry",
                "Accept a single-line",
            ),
            (Overlay::Sort { selected: 0 }, "Choose sort", "Back / open"),
            (
                Overlay::Membership {
                    target: crate::model::MembershipTarget::Community("rust".into()),
                    join: true,
                },
                "Confirmation",
                "Confirm the displayed action",
            ),
            (Overlay::Auth, "Accounts", "Back / open"),
        ] {
            let mut app = app(&[]);
            app.auth.phase = crate::auth::Phase::Accounts;
            app.overlay = Some(overlay);
            key(&mut app, KeyCode::F(1), KeyModifiers::NONE);

            let shown = screen(&mut app, 100, 12);

            assert!(shown.contains(title), "missing {title}");
            assert!(shown.contains(description), "missing {description}");
            assert!(!shown.contains("Reddit: saved"));
        }
    }

    #[test]
    fn ui_audit_a_narrow_search_tab_should_keep_the_query_above_its_kinds() {
        let mut app = form_app("Search posts across Reddit");
        assert_eq!(app.active, crate::app::SEARCH_TAB);
        let actual = screen(&mut app, 32, 12);
        let field = actual.find("Search posts").expect("query field");
        assert!(field < actual.find("Posts").expect("kinds"), "{actual}");
    }

    #[test]
    fn ui_audit_optional_description_should_be_identified_before_submission() {
        let mut app = form_app("Create a custom feed");
        let actual = screen(&mut app, 80, 24);
        assert!(actual.contains("Description (optional)"), "{actual}");
    }

    #[test]
    fn ui_audit_caret_should_remain_visible_at_a_short_form_wrap_boundary() {
        let mut app = form_app("Create text post");
        let Some(crate::app::experience::Screen::Input(form)) = app.experience.screens.last_mut()
        else {
            panic!("form missing")
        };
        form.fields.truncate(1);
        form.fields[0].value = "12345678901234567890123456789012".into();
        form.fields[0].cursor = 32;
        let actual = screen(&mut app, 40, 6);
        assert!(actual.contains('▏'), "{actual}");
    }

    #[test]
    fn ui_audit_scrolled_multiline_input_should_keep_its_field_label_visible() {
        let mut app = form_app("Create text post");
        let Some(crate::app::experience::Screen::Input(form)) = app.experience.screens.last_mut()
        else {
            panic!("form missing")
        };
        form.active = 2;
        form.fields[2].value = "A paragraph of draft text.\n".repeat(30);
        form.fields[2].cursor = form.fields[2].value.len();

        let actual = screen(&mut app, 64, 12);

        assert!(actual.contains("Body (Markdown)"), "{actual}");
        assert!(actual.contains('▏'), "{actual}");
    }

    #[test]
    fn ui_audit_auth_confirmation_help_should_not_advertise_account_navigation() {
        let mut app = app(&[]);
        app.overlay = Some(Overlay::Auth);
        app.auth.phase = crate::auth::Phase::Confirm;
        let actual = help::entries(&app);
        let context: Vec<_> = actual.iter().take_while(|(key, _)| key != "Feed").collect();
        assert!(
            !context
                .iter()
                .any(|(_, text)| text.contains("select an account"))
        );
        assert!(
            !context
                .iter()
                .any(|(_, text)| text.contains("reconnect the selected"))
        );
    }

    #[test]
    fn ui_audit_single_field_help_should_not_advertise_field_navigation() {
        let app = form_app("Open a user profile");
        let actual = help::entries(&app);
        assert!(
            !actual
                .iter()
                .any(|(_, text)| text == "next / previous field")
        );
    }

    #[test]
    fn ui_audit_empty_jump_results_should_explain_the_empty_list() {
        let mut app = app(&[]);
        app.overlay = Some(Overlay::Picker(crate::app::PickerState {
            input: "not a reddit name!".into(),
            selected: 0,
        }));
        let actual = screen(&mut app, 80, 20);
        assert!(actual.contains("No matching"), "{actual}");
    }

    #[test]
    fn ui_audit_help_should_remain_discoverable_in_a_narrow_main_footer() {
        let mut app = feed_app(Picker::halfblocks());
        app.show_sidebar = false;
        app.health = Health::Up;
        let actual = screen(&mut app, 40, 16);
        assert!(actual.lines().last().unwrap().contains("F1"), "{actual}");
    }

    #[test]
    fn ui_audit_long_post_title_should_leave_room_to_read_the_document() {
        let (mut app, _) = open_long_post(|_| {});
        app.show_sidebar = false;
        let view = app.post_view.as_mut().unwrap();
        view.page.as_mut().unwrap().post.title =
            format!("{}TITLEEND", "A long title with many words ".repeat(24));
        screen(&mut app, 50, 14);
        let view = app.post_view.as_ref().unwrap();
        assert!(view.page_height >= 6);
        assert!(
            view.rendered_document
                .iter()
                .any(|line| line.to_string().contains("TITLEEND"))
        );
    }

    #[test]
    fn ui_audit_deep_comment_body_should_fit_the_available_columns() {
        let (mut app, _) = open_long_post(|_| {});
        app.show_sidebar = false;
        let view = app.post_view.as_mut().unwrap();
        let mut leaf = reply("leaf".into());
        leaf.body = "Every word in this deeply nested reply must remain readable".into();
        for depth in 0..24 {
            let mut parent = reply(format!("p{depth}"));
            parent.replies = vec![leaf];
            leaf = parent;
        }
        view.page.as_mut().unwrap().comments = vec![leaf];
        screen(&mut app, 40, 24);
        let lines = &app.post_view.as_ref().unwrap().rendered_document;
        let actual = lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(" ");
        assert!(lines.iter().all(|line| line.width() <= 34), "{actual}");
        assert!(actual.contains("readable"));
    }

    fn form_app(label: &str) -> App {
        use crate::app::experience::Screen;
        let mut app = app(&[]);
        app.reddit_menu();
        let Screen::Menu {
            choices, selected, ..
        } = app.experience.screens.last_mut().unwrap()
        else {
            panic!("expected menu")
        };
        *selected = choices.iter().position(|c| c.label == label).unwrap();
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        app
    }

    #[test]
    fn composer_help_should_preserve_the_draft_and_consume_submit_keys() {
        use crate::app::experience::Screen;
        let mut app = form_app("Create text post");
        crate::keys::paste(&mut app, "Հայերեն");
        press(&mut app, '?');
        key(&mut app, KeyCode::F(1), KeyModifiers::NONE);
        let shown = screen(&mut app, 100, 20);
        key(&mut app, KeyCode::Char('s'), KeyModifiers::CONTROL);

        assert!(shown.contains("Text entry"));
        assert!(shown.contains("post"));
        assert!(app.help_scroll.is_some());
        assert!(app.experience.pending.is_none());
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        let Some(Screen::Input(form)) = app.experience.screens.last() else {
            panic!("help lost the composer")
        };
        assert_eq!(form.fields[0].value, "Հայերեն?");
        assert_eq!(form.fields[0].cursor, "Հայերեն?".len());
    }

    #[test]
    fn menu_help_should_return_to_the_same_selected_action() {
        use crate::app::experience::Screen;
        let mut app = app(&[]);
        app.reddit_menu();
        press(&mut app, 'j');
        press(&mut app, '?');
        let shown = screen(&mut app, 100, 14);
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);

        assert!(shown.contains("Action menu"));
        assert!(matches!(
            app.experience.screens.last(),
            Some(Screen::Menu { selected: 1, .. })
        ));
        assert!(app.experience.pending.is_none());
        assert!(matches!(app.overlay, Some(Overlay::Experience)));
    }

    #[test]
    fn help_should_follow_a_read_result_without_dismissing_the_new_screen() {
        use crate::app::experience::Screen;
        use crate::reddit::experience::{Page, Query, Request, Response};
        let mut app = app(&[]);
        app.overlay = Some(Overlay::Experience);
        app.experience.screens.push(Screen::Loading(Query::Multis));
        app.experience.pending = Some((
            91,
            Request::Read {
                query: Query::Multis,
                after: None,
            },
        ));
        key(&mut app, KeyCode::F(1), KeyModifiers::NONE);
        app.on_experience(
            91,
            Ok(Response::Page(Page {
                title: "Loaded feeds".into(),
                text: "No feeds".into(),
                entries: vec![],
                after: None,
                url: "/api/multi/mine".into(),
            })),
            crate::cache::Freshness {
                fetched_at: crate::app::now(),
                source: Default::default(),
            },
        );
        let shown = screen(&mut app, 100, 14);
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);

        assert!(shown.contains("Reading dialog"));
        assert!(app.experience.pending.is_none());
        assert!(
            matches!(app.experience.screens.last(), Some(Screen::Browse { page, .. }) if page.title == "Loaded feeds")
        );
        assert!(matches!(app.overlay, Some(Overlay::Experience)));
    }

    #[test]
    fn help_should_not_cancel_or_resubmit_a_pending_write() {
        use crate::app::experience::Screen;
        use crate::reddit::experience::{Action, Request, Response};
        let mut app = app(&[]);
        app.overlay = Some(Overlay::Experience);
        app.experience.screens.push(Screen::Confirm {
            label: "Save post".into(),
            action: Action::Save {
                id: "t3_p".into(),
                save: true,
            },
            native: "/".into(),
        });
        app.experience.pending = Some((
            91,
            Request::Write(Action::Save {
                id: "t3_p".into(),
                save: true,
            }),
        ));
        key(&mut app, KeyCode::F(1), KeyModifiers::NONE);
        let shown = screen(&mut app, 100, 14);
        app.on_experience(
            91,
            Ok(Response::Written { url: None }),
            crate::cache::Freshness {
                fetched_at: crate::app::now(),
                source: Default::default(),
            },
        );
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);

        assert!(shown.contains("Sending to Reddit"));
        assert!(app.help_scroll.is_none());
        assert!(app.experience.pending.is_none());
        assert!(app.overlay.is_none());
        assert!(!app.quit);
    }

    #[test]
    fn narrow_help_should_wrap_descriptions_and_clamp_at_the_last_key() {
        let mut app = app(&[]);
        press(&mut app, '?');
        app.help_scroll = Some(usize::MAX);
        press(&mut app, 'j');

        let shown = screen(&mut app, 32, 12);

        assert!(shown.contains("Quit"));
        assert!(shown.contains("Quit redtab"));
        assert!(app.help_scroll.is_some_and(|s| s < usize::MAX));
    }

    #[test]
    fn help_scrolls_to_its_last_keys_in_a_short_terminal() {
        let mut app = app(&[]);
        press(&mut app, '?');
        for _ in 0..200 {
            press(&mut app, 'j');
            screen(&mut app, 80, 24);
        }

        let s = screen(&mut app, 80, 24);

        assert!(s.contains("Quit"), "{s}");
        assert!(
            app.help_scroll.is_some(),
            "j scrolls, it does not close help"
        );
    }

    #[test]
    fn every_help_key_is_set_apart_from_its_description() {
        let mut app = app(&[]);
        press(&mut app, '?');

        let mut s = String::new();
        let mut offset = 0;
        loop {
            app.help_scroll = Some(offset);
            s.push_str(&screen(&mut app, 160, 160));
            if app.help_scroll.unwrap() < offset {
                break;
            }
            offset += 20;
        }

        for (key, desc) in help::entries(&app).iter().filter(|(_, d)| !d.is_empty()) {
            assert!(
                s.split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .contains(&key.split_whitespace().collect::<Vec<_>>().join(" "))
                    && s.split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .contains(desc),
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
            gif: false,
            url: Some(VIDEO.into()),
            poster: None,
        };
        screen(&mut app, 100, 30);
        app
    }

    #[test]
    fn when_detail_uses_sixel_it_should_keep_viewport_fitted_document_geometry() {
        let mut app = open_video_post();
        app.image_picker.set_protocol_type(ProtocolType::Sixel);
        app.post_view
            .as_mut()
            .unwrap()
            .page
            .as_mut()
            .unwrap()
            .post
            .preview_size = Some((1080, 1920));
        let actual = detail_pictures(&mut app, 80, 30);
        assert_eq!(actual[0].size, Size::new(17, 15));
    }

    #[test]
    fn the_video_plays_only_while_its_post_shows_it() {
        let mut app = open_video_post();
        let step = |app: &mut App| {
            screen(app, 100, 30);
            if app.video.is_none()
                && app.inline_images_on()
                && let Some(area) = app.video_area
            {
                let url = app.reddit.media_url(VIDEO).unwrap();
                app.video = Some(video::Player::silent(&url, area.as_size()));
            }
            app.tick();
            app.video.as_ref().map(|p| p.hidden())
        };

        assert_eq!(step(&mut app), Some(false), "starts with the post");
        press(&mut app, '?');
        assert_eq!(step(&mut app), Some(true), "waits under the help");
        app.help_scroll = None;
        assert_eq!(step(&mut app), Some(false));
        key(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
        assert_eq!(step(&mut app), Some(true), "waits while scrolled away");
        press_keys(&mut app, "gg");
        assert_eq!(step(&mut app), Some(false));
        press_keys(&mut app, "zi");
        assert_eq!(step(&mut app), None, "images off stops it");
        press_keys(&mut app, "zi");
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
            preview: None,
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
        press_keys(&mut app, "m");
        assert!(app.video_sound_wanted());
        assert!(!app.video.as_ref().unwrap().sound());
        press_keys(&mut app, "m");
        assert!(!app.video_sound_wanted(), "and back off");

        // ffplay failing, even at a later frame, leaves later videos silent too.
        press_keys(&mut app, "m");
        app.video.as_mut().unwrap().fail_sound();
        app.tick();
        assert!(!app.video_sound_wanted());
    }

    #[test]
    fn a_resized_video_starts_again_as_it_was_once_the_size_holds() {
        let mut app = open_video_post();
        let step = |app: &mut App, height: u16| {
            screen(app, 100, height);
            app.tick();
        };
        step(&mut app, 30);
        let first = app.video.as_ref().unwrap().area;
        let url = app.reddit.media_url(VIDEO).unwrap();
        app.video = Some(video::Player::silent(&url, first));
        press_keys(&mut app, "p");

        // Two draws at once at the new size: it has not held still yet.
        step(&mut app, 40);
        step(&mut app, 40);
        assert_eq!(
            app.video.as_ref().unwrap().area,
            first,
            "a size passing by changes nothing"
        );
        std::thread::sleep(std::time::Duration::from_millis(300));
        step(&mut app, 40);
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
        app.post_view
            .as_mut()
            .unwrap()
            .page
            .as_mut()
            .unwrap()
            .post
            .preview_size = Some((1600, 900));
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
            pixels: video::Pixels::Raw(std::sync::Arc::new(image::DynamicImage::new_rgb8(160, 9))),
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
            pixels: video::Pixels::Raw(std::sync::Arc::new(image::DynamicImage::new_rgb8(16, 9))),
            same_as_first: false,
        };
        let url = app.reddit.media_url(VIDEO).unwrap();
        app.video = Some(video::Player::showing(&url, frame()));
        let sends = |app: &mut App| screen(app, 100, 40).matches("_Ga=T").count();

        assert_eq!(sends(&mut app), 2);
        assert_eq!(sends(&mut app), 0, "a redraw repeats the cells only");
        app.video.as_mut().unwrap().show(frame());
        assert_eq!(
            sends(&mut app),
            2,
            "the next frame sends both visible tiles"
        );
    }

    #[test]
    fn video_frame_preparation_should_not_show_image_loading_activity() {
        let mut app = open_video_post();
        let url = app.reddit.media_url(VIDEO).unwrap();
        let frame = || video::Frame {
            width: 16,
            height: 9,
            pixels: video::Pixels::Raw(std::sync::Arc::new(image::DynamicImage::new_rgb8(16, 9))),
            same_as_first: false,
        };
        app.video = Some(video::Player::showing(&url, frame()));
        for _ in 0..2 {
            let actual = first_screen(&mut app, 100, 40);
            assert!(app.card_renderer.pending(), "keep graphics backpressure");
            assert!(
                !app.images_loading(),
                "normal video preparation is playback"
            );
            assert!(
                !actual.lines().last().unwrap().contains("images"),
                "{actual}"
            );
            app.card_renderer.wait_for_completion();
            assert!(
                app.card_renderer.pending(),
                "unpublished tiles still need polling"
            );
            assert!(
                !app.images_loading(),
                "completed video tiles are playback too"
            );
            assert!(app.card_renderer.poll());
            assert!(!app.card_renderer.pending());
            app.video.as_mut().unwrap().show(frame());
        }
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
        let run = |edit_body: bool, draw_between: bool, before: &[&str], after: &[&str]| {
            let mut app = post_with_rewrapping_spoilers(40);
            let view = app.post_view.as_mut().unwrap();
            view.scroll = view.row_starts[0] - 30;
            screen(&mut app, 100, 30);
            for &key in before {
                press_keys(&mut app, key);
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
                press_keys(&mut app, key);
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
            (&[][..], &["zs", "\u{6}"][..]),
            (&["zs"][..], &[][..]),
            (&[][..], &["gg"][..]),
        ] {
            let batched = run(true, false, before, after);
            assert_eq!(batched, run(true, true, before, after));
            if after.contains(&"gg") {
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
            press_keys(&mut app, "zs");
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
        press_keys(&mut app, "gg");
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
            for keys in ["h", "\u{6}", "l"] {
                press_keys(&mut app, keys);
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
        press_keys(&mut app, "zs");
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
            press_keys(&mut app, "zs");
            key(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
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
                press_keys(&mut app, "zs");
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
    #[test]
    fn ui_audit_safety_labels_should_precede_a_long_author_in_compact_rows() {
        let mut app = feed_app(Picker::halfblocks());
        app.show_sidebar = false;
        app.config.show_nsfw = true;
        for post in &mut app.tabs[4].feed.posts {
            post.author = "an_author_name_that_is_long_enough_to_fill_the_metadata_row".into();
            post.nsfw = true;
            post.spoiler = true;
        }

        let actual = screen(&mut app, 40, 20);

        assert!(actual.contains("spoiler"), "{actual}");
    }

    #[test]
    fn ui_audit_narrow_loading_and_text_only_views_should_show_applicable_controls() {
        use crate::app::experience::Screen;
        use crate::reddit::experience::{Page, Query, Request};
        for width in [20, 24, 40, 72] {
            let mut app = app(&[]);
            app.overlay = Some(Overlay::Experience);
            let query = Query::Rules("rust".into());
            app.experience.screens = vec![Screen::Loading(query.clone())];
            app.experience.pending = Some((
                1,
                Request::Read {
                    query: query.clone(),
                    after: None,
                },
            ));
            let actual = screen(&mut app, width, 8);
            assert!(!actual.contains("r retry"), "{actual}");
            assert!(actual.contains("Esc"), "{actual}");
            app.experience.pending = None;
            app.experience.screens = vec![Screen::Browse {
                pagination: Default::default(),
                query,
                page: Page {
                    title: "Rules".into(),
                    text: "Community rules".into(),
                    entries: vec![],
                    after: None,
                    url: "/r/rust/about/rules".into(),
                },
                selected: 0,
                scroll: 0,
            }];
            let actual = screen(&mut app, width, 8);
            assert!(!actual.contains("Enter"), "{actual}");
            assert!(actual.contains("Esc"), "{actual}");
        }
    }

    pub(crate) fn auth_content(app: &mut App, width: u16, height: u16) -> String {
        let buf = screen_buffer(app, width, height);
        (0..height)
            .filter(|&y| buf[(2, y)].symbol() == "│" && buf[(width - 3, y)].symbol() == "│")
            .map(|y| {
                (4..width - 4)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .trim()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("")
    }

    #[test]
    fn ui_audit_short_accounts_should_preserve_identity_and_back_controls() {
        use crate::auth::Phase;
        for name in ["reader", "abcdefghijklmnopqrst"] {
            for (width, height) in [(14, 6), (18, 6), (20, 6), (20, 8), (24, 8), (40, 12)] {
                for phase in [Phase::Accounts, Phase::DisconnectConfirm] {
                    let mut app = app(&[]);
                    app.overlay = Some(Overlay::Auth);
                    app.auth.phase = phase;
                    app.auth.accounts = vec![saved_account(name, true)];
                    app.auth.active_id = Some(name.into());
                    app.auth.removal_name = Some(format!("u/{name}"));
                    let actual = auth_content(&mut app, width, height);
                    if phase == Phase::Accounts && height >= 8 {
                        assert!(actual.contains("expired"), "{actual}");
                    }
                    if phase == Phase::Accounts && width == 40 {
                        assert!(actual.contains(&format!("u/{name}")), "{actual}");
                    }
                    assert!(
                        actual.contains(&format!("u/{name}"))
                            || (actual.contains(&format!("u/{}", &name[..1]))
                                && actual.contains('…')),
                        "{phase:?}: {actual}"
                    );
                    assert!(
                        actual.contains("Esc") || actual.contains('⎋'),
                        "{phase:?}: {actual}"
                    );
                    assert!(
                        actual.contains("Enter") || actual.contains('⏎'),
                        "{phase:?}: {actual}"
                    );
                }
            }
        }
    }

    #[test]
    fn ui_audit_direct_inputs_should_backspace_complete_graphemes() {
        for (input, expected) in [("cafe\u{301}", "caf"), ("x👩‍💻", "x"), ("x🇮🇪", "x"), ("", "")]
        {
            for picker in [false, true] {
                let mut app = app(&[]);
                app.overlay = Some(if picker {
                    Overlay::Picker(crate::app::PickerState::default())
                } else {
                    Overlay::Search {
                        input: String::new(),
                    }
                });
                crate::keys::paste(&mut app, input);
                key(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
                let actual = match app.overlay.as_ref().unwrap() {
                    Overlay::Picker(picker) => &picker.input,
                    Overlay::Search { input } => input,
                    _ => unreachable!(),
                };
                assert_eq!(actual, expected);
            }
        }
    }

    #[test]
    fn ui_audit_failed_forms_should_keep_correction_visible() {
        for label in [
            "Open a user profile",
            "Create text post",
            "Create link post",
            "Create a custom feed",
        ] {
            for (width, height) in [(20, 6), (24, 8), (40, 12), (64, 18)] {
                let mut app = form_app(label);
                let form = app.experience.input().expect("form missing");
                let missing = form
                    .fields
                    .iter()
                    .rposition(|field| field.required)
                    .unwrap();
                for (i, field) in form.fields.iter_mut().enumerate() {
                    field.value = if i == missing {
                        String::new()
                    } else {
                        "fixture".into()
                    };
                    field.cursor = field.value.len();
                }
                key(&mut app, KeyCode::Char('s'), KeyModifiers::CONTROL);
                assert!(app.experience.error.is_some());
                assert!(app.experience.pending.is_none());
                let buf = screen_buffer(&mut app, width, height);
                assert!(
                    buf.content()
                        .iter()
                        .any(|cell| cell.fg == RED
                            && cell.symbol().chars().any(char::is_alphanumeric)),
                    "error feedback missing at {width}x{height}"
                );
                let before = buffer_text(&buf);
                assert!(before.contains('▏'), "{label}: {before}");
                crate::keys::paste(&mut app, "fixedbody");
                let actual = screen(&mut app, width, height);
                assert!(
                    actual.contains("fixedbody") && actual.contains('▏'),
                    "{label}: {actual}"
                );
                assert!(actual.contains("Esc"), "{actual}");
            }
        }
    }

    #[test]
    fn ui_audit_narrow_headers_should_prioritize_failure_and_retry() {
        let mut subs = app(&["s"]);
        subs.show_sidebar = false;
        subs.all_subs.groups[0].posts = vec![post_with("p", Media::None)];
        subs.all_subs.groups[0].loading = false;
        subs.on_msg(Msg::Subscriptions {
            generation: 0,
            result: Err("fixture failure".into()),
        });
        subs.status = None;
        let actual = screen(&mut subs, 24, 8);
        let header = actual.lines().take(3).collect::<Vec<_>>().join("\n");
        assert!(
            header.contains("subs failed") && header.contains("r retry"),
            "{actual}"
        );
        for retained in [false, true] {
            let mut group = app(&["a_very_long_community"]);
            group.show_sidebar = false;
            group.all_subs.groups[0].loading = false;
            group.all_subs.groups[0].error = Some("Fixture refresh failure".into());
            if retained {
                let mut post = post_with("cached", Media::None);
                post.subreddit = "a_very_long_community".into();
                group.all_subs.groups[0].posts = vec![post];
            }
            group.status = None;
            let actual = screen(&mut group, 24, 10);
            assert!(
                actual
                    .lines()
                    .any(|line| line.contains('✗') && line.contains("r retry")),
                "{actual}"
            );
        }
        for append in [false, true] {
            let mut feed = feed_with_next_page(ListView::Compact);
            feed.show_sidebar = false;
            feed.load_feed(4, append);
            feed.on_msg(Msg::Feed {
                tab: feed.active_tab().id,
                generation: feed.active_tab().feed.generation,
                append,
                result: Err("fixture failure".into()),
            });
            feed.status = None;
            let actual = screen(&mut feed, 24, 8);
            let header = actual.lines().take(3).collect::<Vec<_>>().join("\n");
            assert!(
                header.contains("failed") && header.contains("r retry"),
                "{actual}"
            );
        }
    }

    #[test]
    fn ui_audit_compact_dialogs_should_keep_input_and_cancel_visible() {
        for (width, height) in [(14, 6), (20, 6), (20, 8), (24, 8), (40, 12)] {
            for kind in ["jump", "search", "sort", "help", "form", "composer"] {
                let mut app = app(&["rust"]);
                match kind {
                    "jump" => {
                        press_keys(&mut app, " bb");
                        crate::keys::paste(&mut app, "rust");
                    }
                    "search" => {
                        app.overlay = Some(Overlay::Search {
                            input: "rust".into(),
                        })
                    }
                    "sort" => app.overlay = Some(Overlay::Sort { selected: 0 }),
                    "help" => app.help_scroll = Some(0),
                    "form" => app = form_app("Open a user profile"),
                    "composer" => app = form_app("Create text post"),
                    _ => unreachable!(),
                }
                let actual = screen(&mut app, width, height);
                if matches!(kind, "jump" | "search") {
                    assert!(
                        actual.contains('▏') && (width < 20 || actual.contains("rust")),
                        "{kind}: {actual}"
                    );
                }
                if matches!(kind, "form" | "composer") {
                    assert!(actual.contains('▏'), "{actual}");
                }
                assert!(
                    actual.contains("Esc") || actual.contains('⎋'),
                    "{kind}: {actual}"
                );
            }
        }
    }

    #[test]
    fn ui_audit_short_membership_dialogs_should_keep_target_and_cancel_visible() {
        for name in ["rust", "a_very_long_community"] {
            for join in [true, false] {
                for (width, height) in [(20, 6), (20, 8), (24, 8), (40, 12)] {
                    let mut app = app(&[]);
                    app.overlay = Some(Overlay::Membership {
                        target: crate::model::MembershipTarget::Community(name.into()),
                        join,
                    });
                    let body = auth_content(&mut app, width, height);
                    let actual = screen(&mut app, width, height);
                    assert!(body.contains(&format!("r/{name}")), "{actual}");
                    assert!(actual.contains("Esc") || actual.contains('⎋'), "{actual}");
                    assert!(actual.contains("Enter") || actual.contains('⏎'), "{actual}");
                }
            }
        }
    }

    #[test]
    fn ui_audit_feedback_should_precede_background_activity() {
        let mut app = feed_app(Picker::halfblocks());
        app.show_sidebar = false;
        app.images.insert(
            "https://preview.redd.it/pending.png".into(),
            ImageSlot::Loading,
        );
        app.error("Clipboard failure");
        let actual = screen(&mut app, 20, 8);
        let footer = actual.lines().last().unwrap();
        assert!(footer.contains('✗') && footer.contains("Clip"), "{actual}");
    }
    #[test]
    fn settings_f2_should_cover_every_field_and_restore_the_underlying_dialog() {
        let mut app = app(&[]);
        app.overlay = Some(Overlay::Search {
            input: "draft?".into(),
        });
        crate::keys::handle(
            &mut app,
            crossterm::event::KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE),
        );
        assert!(matches!(app.overlay, Some(Overlay::Settings)));
        let actual = screen(&mut app, 100, 30);
        for field in crate::config::schema::SETTINGS {
            assert!(actual.contains(field.label), "{}: {actual}", field.key);
        }
        assert!(actual.contains("Temporary"));
        crate::keys::handle(
            &mut app,
            crossterm::event::KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        );
        assert!(matches!(app.overlay, Some(Overlay::Search { ref input }) if input == "draft?"));
    }

    #[test]
    fn settings_editor_should_keep_unicode_drafts_across_help_and_reject_bad_arrays() {
        let mut app = app(&[]);
        app.show_settings();
        app.settings.selected = crate::config::schema::SETTINGS
            .iter()
            .position(|f| f.key == "filters.keywords")
            .unwrap();
        let key = |app: &mut App, code, modifiers| {
            crate::keys::handle(app, crossterm::event::KeyEvent::new(code, modifiers))
        };
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        key(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
        crate::keys::paste(&mut app, "[\"café 👩‍💻\"]");
        key(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);
        assert!(app.help_scroll.is_none(), "? is literal in settings inputs");
        key(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
        key(&mut app, KeyCode::Left, KeyModifiers::NONE);
        key(&mut app, KeyCode::Left, KeyModifiers::NONE);
        key(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(app.settings.editing.as_ref().unwrap().value, "[\"café \"]");
        key(&mut app, KeyCode::F(1), KeyModifiers::NONE);
        assert!(screen(&mut app, 70, 20).contains("Text entry"));
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.settings.editing.is_some());
        let actual = screen(&mut app, 70, 16);
        assert!(actual.contains("café"), "{actual}");
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.config.filters.keywords, ["café "]);
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        key(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
        crate::keys::paste(&mut app, "[42]");
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.settings.message.is_some());
        assert!(app.settings.editing.is_some());
        assert_eq!(app.config.filters.keywords, ["café "]);
    }

    #[test]
    fn scoped_settings_narrow_rows_should_keep_boolean_and_command_values_visible() {
        let mut app = app(&[]);
        app.change_setting("media.image_preview", false.into());
        app.change_setting("commands.image", toml::Value::Array(vec!["viewer".into()]));
        app.show_settings();
        for width in [24, 31, 32, 33, 40, 100] {
            for (key, label, value) in [
                ("media.image_preview", "Image previews", "Off"),
                ("commands.image", "Image command", "viewer"),
            ] {
                app.settings.selected = crate::config::schema::SETTINGS
                    .iter()
                    .position(|f| f.key == key)
                    .unwrap();
                let actual = screen(&mut app, width, 12);
                assert!(
                    actual.contains(label) && actual.contains(value),
                    "{width}: {actual}"
                );
            }
        }
    }

    #[test]
    fn settings_small_dialogs_should_keep_selection_value_and_escape_visible() {
        let mut app = app(&[]);
        app.show_settings();
        app.settings.selected = 10;
        for (width, height) in [(40, 12), (24, 8), (10, 6), (1, 1)] {
            let actual = screen(&mut app, width, height);
            if width >= 24 {
                assert!(actual.contains("Image command"), "{actual}");
                assert!(actual.contains("Esc"), "{actual}");
            }
        }
    }

    #[test]
    fn settings_popup_should_show_temporary_and_retry_revert_actual_file_changes() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[media]\nimage_preview = false\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
        let mut app = app(&[]);
        app.settings = crate::app::settings::Settings::new(
            crate::config::store::Store::open(Some(&path)).unwrap(),
        );
        let fixture = crate::config::managed::tests::Fixture::new();
        app.settings.chezmoi = fixture.tool.clone();
        app.show_settings();
        app.settings.selected = crate::config::schema::SETTINGS
            .iter()
            .position(|f| f.key == "media.image_preview")
            .unwrap();
        crate::keys::handle(
            &mut app,
            crossterm::event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert!(app.config.media.image_preview && app.show_images);
        app.settings.wait_managed();
        app.tick();
        let actual = screen(&mut app, 100, 30);
        assert!(
            actual.contains("Temporary") && actual.contains("read-only"),
            "{actual}"
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        press(&mut app, 'r');
        assert!(Config::load(Some(&path)).unwrap().media.image_preview);
        std::fs::write(&path, "[media]\nimage_preview = false\n").unwrap();
        press(&mut app, 'u');
        assert!(!app.show_images);
        assert!(screen(&mut app, 100, 30).contains("Saved"));
    }

    fn managed_settings_app() -> (App, crate::config::managed::tests::Fixture) {
        let fixture = crate::config::managed::tests::Fixture::new();
        let mut app = app(&[]);
        app.settings = crate::app::settings::Settings::new(fixture.store());
        app.settings.chezmoi = fixture.tool.clone();
        app.overlay = Some(Overlay::Search {
            input: "retained draft".into(),
        });
        app.show_settings();
        app.settings.selected = crate::config::schema::SETTINGS
            .iter()
            .position(|field| field.key == "media.image_preview")
            .unwrap();
        crate::keys::handle(
            &mut app,
            crossterm::event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        (app, fixture)
    }

    #[test]
    fn managed_settings_should_confirm_before_source_writes_and_keep_session_changes_on_cancel() {
        let (mut app, fixture) = managed_settings_app();
        assert!(!app.config.media.image_preview);
        app.settings.wait_managed();
        app.tick();
        let actual = screen(&mut app, 100, 30);
        assert!(actual.contains("Save through chezmoi?"), "{actual}");
        assert!(
            actual.contains("media.image_preview: true → false"),
            "{actual}"
        );
        assert!(
            Config::load(Some(&fixture.source))
                .unwrap()
                .media
                .image_preview
        );
        assert!(
            Config::load(Some(&fixture.target))
                .unwrap()
                .media
                .image_preview
        );

        crate::keys::handle(
            &mut app,
            crossterm::event::KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        );

        assert!(app.settings.managed.is_none());
        assert!(!app.config.media.image_preview);
        assert_eq!(
            app.settings.store.persistence,
            crate::config::store::Persistence::Temporary
        );
        assert!(!fixture.directory.path().join("applied-args").exists());
        crate::keys::handle(
            &mut app,
            crossterm::event::KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        );
        assert!(
            matches!(app.overlay, Some(Overlay::Search { ref input }) if input == "retained draft")
        );
    }

    #[test]
    fn managed_settings_should_apply_after_confirmation_and_preserve_help_and_the_covered_dialog() {
        let (mut app, fixture) = managed_settings_app();
        app.settings.wait_managed();
        app.tick();
        press(&mut app, 'l');
        key(&mut app, KeyCode::Right, KeyModifiers::NONE);
        key(&mut app, KeyCode::Enter, KeyModifiers::CONTROL);
        assert!(matches!(
            app.settings.managed,
            Some(crate::app::settings::ManagedSave::Confirm(_))
        ));
        assert!(!fixture.directory.path().join("applied-args").exists());
        crate::keys::handle(
            &mut app,
            crossterm::event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert!(app.settings.busy() && app.loading_active());
        press(&mut app, 'u');
        crate::keys::handle(
            &mut app,
            crossterm::event::KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        );
        assert!(app.settings.busy());
        crate::keys::handle(
            &mut app,
            crossterm::event::KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE),
        );
        assert!(!app.loading_active());
        app.settings.wait_managed();

        app.tick();

        assert_eq!(
            app.settings.store.persistence,
            crate::config::store::Persistence::Saved
        );
        assert!(
            !Config::load(Some(&fixture.target))
                .unwrap()
                .media
                .image_preview
        );
        assert!(app.help_scroll.is_some());
        crate::keys::handle(
            &mut app,
            crossterm::event::KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        );
        assert!(matches!(app.overlay, Some(Overlay::Settings)));
        crate::keys::handle(
            &mut app,
            crossterm::event::KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        );
        assert!(
            matches!(app.overlay, Some(Overlay::Search { ref input }) if input == "retained draft")
        );
    }

    #[test]
    fn managed_settings_confirmation_should_scroll_and_keep_controls_in_narrow_windows() {
        let (mut app, _fixture) = managed_settings_app();
        app.settings.wait_managed();
        app.tick();
        for (width, height) in [(80, 24), (40, 12), (24, 8), (10, 6), (1, 1)] {
            let before = screen(&mut app, width, height);
            crate::keys::handle(
                &mut app,
                crossterm::event::KeyEvent::new(KeyCode::End, KeyModifiers::NONE),
            );
            let after = screen(&mut app, width, height);
            if width >= 24 {
                assert!(
                    after.contains("Esc") && (after.contains('⏎') || after.contains("Enter")),
                    "{after}"
                );
            }
            if width == 40 {
                assert_ne!(before, after);
            }
            crate::keys::handle(
                &mut app,
                crossterm::event::KeyEvent::new(KeyCode::Home, KeyModifiers::NONE),
            );
        }
    }

    #[test]
    fn managed_settings_should_finish_an_approved_save_when_the_app_quits() {
        let (mut app, fixture) = managed_settings_app();
        app.settings.wait_managed();
        app.tick();
        crate::keys::handle(
            &mut app,
            crossterm::event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        crate::keys::handle(
            &mut app,
            crossterm::event::KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );
        assert!(app.quit);

        drop(app);

        assert!(
            !Config::load(Some(&fixture.target))
                .unwrap()
                .media
                .image_preview
        );
    }

    #[test]
    fn managed_settings_retry_should_confirm_even_when_values_are_already_applied() {
        let (mut app, fixture) = managed_settings_app();
        app.settings.wait_managed();
        app.tick();
        std::fs::write(fixture.directory.path().join("writable"), "").unwrap();
        crate::keys::handle(
            &mut app,
            crossterm::event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        app.settings.wait_managed();
        app.tick();
        assert_eq!(
            app.settings.store.persistence,
            crate::config::store::Persistence::Unsaved
        );
        app.settings.store.reload();
        assert!(!app.settings.store.dirty());

        press(&mut app, 'r');
        app.settings.wait_managed();
        app.tick();

        let actual = screen(&mut app, 100, 30);
        assert!(actual.contains("Save through chezmoi?"), "{actual}");
        assert!(
            actual.contains("previously approved file permissions"),
            "{actual}"
        );
        assert_ne!(
            app.settings.store.persistence,
            crate::config::store::Persistence::Saved
        );
    }
    #[test]
    fn sidebar_distinguishes_unknown_from_zero_and_keeps_badges_aligned() {
        let mut app = app(&["longcommunityname"]);
        let i = 4;
        for width in [24, 30] {
            let unknown = sidebar_item(&app, i, width);
            assert_eq!(line_width(&unknown.spans), width);
            assert!(unknown.to_string().trim_end().ends_with('?'));
        }
        app.state.observe(
            Some("longcommunityname"),
            &[],
            crate::cache::Freshness {
                fetched_at: 1,
                source: crate::cache::Source::default(),
            },
            1,
        );
        let zero = sidebar_item(&app, i, 24).to_string();
        assert!(!zero.contains('?'));
        let mut post = post_with("fetched", Media::None);
        post.subreddit = "longcommunityname".into();
        app.state.observe(
            Some("longcommunityname"),
            &[post],
            crate::cache::Freshness {
                fetched_at: 2,
                source: crate::cache::Source::default(),
            },
            2,
        );
        app.focus = Focus::Sidebar;
        app.active = i;
        for width in [24, 30] {
            let known = sidebar_item(&app, i, width);
            assert_eq!(line_width(&known.spans), width);
            assert!(known.to_string().trim_end().ends_with('1'));
        }
    }

    #[test]
    fn loading_obscured_and_spoiler_hidden_documents_do_not_mark_posts_read() {
        let (mut app, _) = open_long_post(|app| {
            assert!(!app.state.is_read("p"));
            app.overlay = Some(Overlay::Auth);
        });
        assert!(!app.state.is_read("p"));
        app.overlay = None;
        app.post_view
            .as_mut()
            .unwrap()
            .page
            .as_mut()
            .unwrap()
            .post
            .spoiler = true;
        screen(&mut app, 100, 30);
        assert!(!app.state.is_read("p"));

        app.post_view.as_mut().unwrap().reveal_spoilers = true;
        screen(&mut app, 100, 30);

        assert!(app.state.is_read("p"));
    }

    #[test]
    fn an_empty_detail_area_does_not_mark_its_loaded_page_read() {
        let (mut app, _) = open_long_post(|app| app.overlay = Some(Overlay::Auth));
        app.overlay = None;
        screen(&mut app, 1, 1);
        assert!(!app.state.is_read("p"));

        screen(&mut app, 100, 30);

        assert!(app.state.is_read("p"));
    }

    #[test]
    fn the_sidebar_reflects_a_displayed_post_read_in_the_same_frame() {
        let mut app = app(&["s"]);
        let first = post_with("p", Media::None);
        let second = post_with("q", Media::None);
        app.on_msg(Msg::Group {
            generation: app.all_subs.groups[0].generation,
            subreddit: "s".into(),
            result: Ok(crate::model::Listing {
                posts: vec![first.clone(), second],
                after: None,
            }),
        });
        app.open_current_post();
        let generation = app.post_view.as_ref().unwrap().generation;
        app.on_msg(Msg::Post {
            generation,
            result: Ok(Box::new(PostPage {
                post: first,
                body: "Readable post".into(),
                comments: vec![],
            })),
        });
        assert_eq!(app.badge(&app.tabs[4]), UnreadBadge::Count(2));

        let buffer = screen_buffer(&mut app, 100, 30);

        assert_eq!(app.badge(&app.tabs[4]), UnreadBadge::Count(1));
        let rows: Vec<_> = (0..30)
            .map(|y| {
                (0..app.sidebar_width())
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect();
        let row = rows.iter().find(|row| row.contains("# s")).unwrap();
        assert!(row.trim_end().ends_with('1'), "{row}");
    }
    #[cfg(unix)]
    #[test]
    fn empty_community_browser_key_should_open_the_community_without_a_post() {
        let mut app = app(&["s"]);
        app.active = 4;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("opened-url");
        app.config.commands.browser = vec![
            "/bin/sh".into(),
            "-c".into(),
            "printf '%s' \"$1\" > \"$0\"".into(),
            file.to_string_lossy().into_owned(),
        ];
        press_keys(&mut app, "o");
        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        let actual = loop {
            if let Ok(text) = std::fs::read_to_string(&file)
                && !text.is_empty()
            {
                break text;
            }
            assert!(
                Instant::now() < deadline,
                "native browser probe did not receive a URL"
            );
            std::thread::yield_now();
        };
        assert_eq!(actual, "https://www.reddit.com/r/s/");
    }
    #[cfg(unix)]
    fn opened_browser_url(app: &mut App, keys: &str) -> String {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("opened-url");
        app.config.commands.browser = vec![
            "/bin/sh".into(),
            "-c".into(),
            "printf '%s' \"$1\" > \"$0\"".into(),
            file.to_string_lossy().into_owned(),
        ];
        press_keys(app, keys);
        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if let Ok(text) = std::fs::read_to_string(&file)
                && !text.is_empty()
            {
                return text;
            }
            assert!(
                Instant::now() < deadline,
                "native browser probe did not receive a URL"
            );
            std::thread::yield_now();
        }
    }

    #[cfg(unix)]
    #[test]
    fn sidebar_browser_key_should_open_its_community_instead_of_its_selected_post() {
        let mut app = feed_with_next_page(ListView::Card);
        app.focus = Focus::Sidebar;

        let actual = opened_browser_url(&mut app, " oc");

        assert_eq!(actual, "https://www.reddit.com/r/s/");
    }

    #[cfg(unix)]
    #[test]
    fn community_browser_key_on_home_should_open_the_selected_posts_community() {
        let mut app = app(&[]);
        app.active = 0;
        let mut post = post_with("p", Media::None);
        post.subreddit = "fromhome".into();
        app.tabs[0].feed.posts = vec![post];

        let actual = opened_browser_url(&mut app, " oc");

        assert_eq!(actual, "https://www.reddit.com/r/fromhome/");
    }

    #[cfg(unix)]
    #[test]
    fn community_browser_key_in_detail_should_open_the_shown_posts_community() {
        let mut app = app(&["underlying"]);
        app.active = 4;
        let mut post = post_with("p", Media::None);
        post.subreddit = "detail".into();
        app.tabs[4].feed.posts = vec![post];
        app.open_current_post();
        app.focus = Focus::Post;

        let actual = opened_browser_url(&mut app, " oc");

        assert_eq!(actual, "https://www.reddit.com/r/detail/");
    }

    #[cfg(unix)]
    #[test]
    fn lowercase_browser_key_in_a_list_should_keep_opening_the_selected_post() {
        let mut app = feed_with_next_page(ListView::Card);
        app.tabs[4].feed.posts[0].permalink = "/r/s/comments/existing/title/".into();
        app.focus = Focus::List;

        let actual = opened_browser_url(&mut app, "o");

        assert_eq!(
            actual,
            "https://www.reddit.com/r/s/comments/existing/title/"
        );
    }
    #[test]
    fn following_should_show_submitted_posts_in_card_and_compact_views() {
        for mode in [ListView::Card, ListView::Compact] {
            let mut app = app(&[]);
            app.on_msg(Msg::Subscriptions {
                generation: 0,
                result: Ok(vec![crate::model::Subscription::User {
                    name: "blackvault".into(),
                    icon: None,
                }]),
            });
            app.open_user("blackvault");
            if app.state.list_view() != mode {
                app.state.toggle_list_view();
            }
            let mut post = post_with("p", Media::None);
            post.subreddit = "UFOs".into();
            post.title = "A submitted post from another community".into();
            app.on_msg(Msg::Feed {
                tab: app.active_tab().id,
                generation: app.active_tab().feed.generation,
                append: false,
                result: Ok(crate::model::Listing {
                    posts: vec![post],
                    after: Some("next".into()),
                }),
            });

            let actual = screen(&mut app, 120, 24);

            assert!(actual.contains("FOLLOWING"), "{actual}");
            assert!(actual.contains("u/blackvault"), "{actual}");
            assert!(
                actual.contains("A submitted post from another community"),
                "{actual}"
            );
            assert!(actual.contains("r/UFOs"), "{actual}");
            assert!(
                !actual.contains("r/u_blackvault") && !actual.contains("Joined"),
                "{actual}"
            );
            assert!(app.current_post().is_some());
        }
    }

    #[cfg(unix)]
    #[test]
    fn user_follow_confirmation_should_name_the_profile_and_action_in_small_terminals() {
        for (join, title, control) in [
            (true, "Follow on Reddit", "follow"),
            (false, "Unfollow on Reddit", "unfollow"),
        ] {
            let mut app = app(&[]);
            app.overlay = Some(Overlay::Membership {
                target: crate::model::MembershipTarget::User("blackvault".into()),
                join,
            });

            let shown = screen(&mut app, 48, 8);

            assert!(
                shown.contains(title) && shown.contains("u/blackvault"),
                "{shown}"
            );
            assert!(
                shown.contains(control) && shown.contains("Esc cancel"),
                "{shown}"
            );
            assert!(shown.contains("o open profile"), "{shown}");
        }
    }

    #[test]
    fn closing_a_followed_user_should_explain_how_to_unfollow() {
        let mut app = app(&[]);
        app.on_msg(Msg::Subscriptions {
            generation: 0,
            result: Ok(vec![crate::model::Subscription::User {
                name: "blackvault".into(),
                icon: None,
            }]),
        });
        app.open_user("blackvault");
        let tab = app.active_tab().id;

        press_keys(&mut app, " bd");

        assert_eq!(app.active_tab().id, tab);
        assert!(
            app.status
                .as_ref()
                .is_some_and(|status| status.text.contains("SPC m to unfollow"))
        );
    }

    #[test]
    fn unknown_follow_status_should_not_claim_the_person_is_unfollowed() {
        let mut app = app(&[]);
        app.open_user("blackvault");
        app.all_subs.freshness = None;

        let shown = screen(&mut app, 120, 24);

        assert!(
            shown.contains("Following unknown") && shown.contains("SPC m check"),
            "{shown}"
        );
        assert!(!shown.contains("Not following"), "{shown}");
    }

    #[test]
    fn followed_user_header_should_show_the_unfollow_shortcut() {
        let mut app = app(&[]);
        app.on_msg(Msg::Subscriptions {
            generation: 0,
            result: Ok(vec![crate::model::Subscription::User {
                name: "blackvault".into(),
                icon: None,
            }]),
        });
        app.open_user("blackvault");

        let shown = screen(&mut app, 120, 24);

        assert!(
            shown.contains("Following") && shown.contains("SPC m unfollow"),
            "{shown}"
        );
        assert!(
            shown.lines().last().unwrap().contains("unfollow"),
            "{shown}"
        );
    }

    #[test]
    fn followed_user_browser_keys_should_target_the_user_profile_even_when_empty() {
        let mut app = app(&[]);
        app.open_user("blackvault");
        app.focus = Focus::Sidebar;
        assert_eq!(
            opened_browser_url(&mut app, "o"),
            "https://www.reddit.com/user/blackvault/"
        );
        app.focus = Focus::List;
        assert_eq!(
            opened_browser_url(&mut app, " oc"),
            "https://www.reddit.com/user/blackvault/"
        );
        assert_eq!(
            opened_browser_url(&mut app, "o"),
            "https://www.reddit.com/user/blackvault/"
        );
    }

    #[test]
    fn followed_user_sidebar_tools_should_offer_profile_actions_without_subreddit_actions() {
        let mut app = app(&[]);
        app.open_user("blackvault");
        app.focus = Focus::Sidebar;
        press_keys(&mut app, " c");

        let actual = screen(&mut app, 120, 24);

        assert!(
            actual.contains("u/blackvault") && actual.contains("Open submitted posts"),
            "{actual}"
        );
        assert!(actual.contains("Open profile in browser"), "{actual}");
        assert!(
            !actual.contains("Mark community read")
                && !actual.contains("Wiki")
                && !actual.contains("Rules"),
            "{actual}"
        );
    }
}
