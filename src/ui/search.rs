//! The Search tab: query bar with kind tabs, result or Explore rows, and the feed preview.

use super::*;
use crate::app::search::RowsStatus;
use crate::reddit::experience::{PAGE_LIMIT, SearchKind};
use crate::worker::FeedTarget;

mod preview;
mod rows;

pub(in crate::ui) use preview::*;
pub(in crate::ui) use rows::*;

/// Below this width the kind tabs move under the query.
const ONE_BAR: u16 = 84;
const TEXT_START: usize = ICON_COLS as usize + 3;

/// The query and kind tabs across the whole tab, above the list and the preview;
/// returns the area below.
pub(super) fn draw_bar(f: &mut Frame, app: &App, area: Rect, margin: Margin) -> Rect {
    let inner = area.inner(margin);
    let one_line = inner.width >= ONE_BAR;
    let [bar, rest] = Layout::vertical([
        Constraint::Length(if one_line { 1 } else { 2 }),
        Constraint::Min(0),
    ])
    .areas(area);
    let bar = bar.inner(margin);
    if one_line {
        let [input, kinds] =
            Layout::horizontal([Constraint::Min(1), Constraint::Length(kinds_width())]).areas(bar);
        draw_input(f, app, input);
        draw_kinds(f, app, kinds);
    } else {
        let [input, kinds] =
            Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(bar);
        draw_input(f, app, input);
        draw_kinds(f, app, kinds);
    }
    rest
}

/// The list pane below the bar; returns the area showing a loader, as `draw_feed` does.
pub(super) fn draw(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    pictures: &mut ListPictures,
) -> Option<Rect> {
    if app.search.shows_rows() {
        draw_rows(f, app, area, pictures)
    } else {
        draw_feed(f, app, area, pictures)
    }
}

fn kinds_width() -> u16 {
    let labels: usize = SearchKind::ALL
        .iter()
        .map(|kind| kind.label().width() + 2)
        .sum();
    (labels + SearchKind::ALL.len() - 1 + 1) as u16
}

fn draw_input(f: &mut Frame, app: &App, area: Rect) {
    let editing = app.search_editing();
    let field = &app.search.query;
    let style = Style::new()
        .fg(TEXT)
        .bg(if editing { SURFACE1 } else { SURFACE0 });
    let mut spans = vec![Span::styled(
        " 🔍 ",
        style.fg(if editing { ACCENT } else { OVERLAY }),
    )];
    if field.value.is_empty() && !editing {
        spans.push(Span::styled(
            format!(
                "Search {}{}",
                app.search.kind.label().to_lowercase(),
                if app.active == crate::app::SEARCH_TAB {
                    " (/ to type)"
                } else {
                    ""
                }
            ),
            style.fg(OVERLAY),
        ));
    } else if field.value.is_empty() {
        spans.push(Span::styled("▏", style.fg(ACCENT)));
        spans.push(Span::styled(
            format!("Search {}", app.search.kind.label().to_lowercase()),
            style.fg(OVERLAY),
        ));
    } else {
        let room = usize::from(area.width).saturating_sub(5);
        let (before, after) = field.value.split_at(field.cursor);
        let shown_before = input_tail(before, room.saturating_sub(after.width().min(room / 2)));
        spans.push(Span::styled(shown_before, style));
        if editing {
            spans.push(Span::styled("▏", style.fg(ACCENT)));
        }
        let left = room.saturating_sub(line_width(&spans).saturating_sub(4));
        spans.push(Span::styled(truncate(after, left), style));
    }
    pad_to(&mut spans, usize::from(area.width), style);
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The kinds read as tabs: the one a query searches is lit, Tab / S-Tab move it.
/// Explore shows no kind's results, so there the chosen kind is only bold. Kinds lose
/// their padding where the padded row does not fit.
fn draw_kinds(f: &mut Frame, app: &App, area: Rect) {
    let exploring = app.search.exploring();
    let padded = area.width >= kinds_width();
    let mut spans = vec![Span::raw(" ")];
    for (i, kind) in SearchKind::ALL.into_iter().enumerate() {
        let label = kind.label();
        if i > 0 {
            spans.push(Span::styled("│", Style::new().fg(SURFACE1)));
        }
        let style = if kind == app.search.kind && exploring {
            Style::new().fg(TEXT).add_modifier(Modifier::BOLD)
        } else if kind == app.search.kind {
            Style::new()
                .fg(ACCENT)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::new().fg(SUBTEXT)
        };
        spans.push(Span::styled(
            if padded {
                format!(" {label} ")
            } else {
                label.to_string()
            },
            style,
        ));
    }
    f.render_widget(
        Paragraph::new(Line::from(fit_spans(spans, area.width.into()))),
        area,
    );
}

/// What the rows are; the footer carries the keys. An Explore subtitle that does
/// not fit whole is left out rather than cut.
fn draw_title(f: &mut Frame, app: &App, area: Rect) {
    let search = &app.search;
    let width = usize::from(area.width);
    let mut spans = if search.exploring() {
        let mut spans = vec![Span::styled(
            " Explore",
            Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
        )];
        let subtitle = "  popular feeds and suggested communities";
        if line_width(&spans) + subtitle.width() <= width {
            spans.push(Span::styled(subtitle, Style::new().fg(SUBTEXT)));
        }
        spans
    } else {
        vec![
            Span::styled(
                format!(" {}", search.kind.label()),
                Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  matching “{}”", search.text()),
                Style::new().fg(SUBTEXT),
            ),
        ]
    };
    spans = fit_spans(spans, width);
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}
