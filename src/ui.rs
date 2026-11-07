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

mod all_subs;
mod auth_dialog;
mod card;
mod card_expanded;
mod card_list;
mod card_pictures;
mod document;
mod document_text;
mod feed_view;
mod footer;
mod inline_media;
mod popups;
mod post;
mod sidebar;
mod widgets;

use all_subs::*;
use auth_dialog::*;
use card::*;
use card_expanded::*;
use card_list::*;
use card_pictures::*;
pub(crate) use document::*;
use document_text::*;
use feed_view::*;
use footer::*;
use inline_media::*;
use popups::*;
use post::*;
use sidebar::*;
use widgets::*;

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

/// Shown while the OS credential store answers, which can take seconds.
pub fn draw_startup(f: &mut Frame, phase: usize) {
    let area = f.area();
    f.render_widget(Block::default().style(Style::new().fg(TEXT).bg(BASE)), area);
    let caption = loading::caption(
        loading::Kind::Initial,
        "Reading the saved Reddit sign-in",
        phase,
    );
    let [_, row, _] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(area);
    f.render_widget(Paragraph::new(caption).alignment(Alignment::Center), row);
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
    let wants_preview = app.post_view.is_none()
        && app.active == crate::app::SEARCH_TAB
        && app.search.shows_rows()
        && !app.search_preview_closed();
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

#[cfg(test)]
pub(crate) mod tests;
