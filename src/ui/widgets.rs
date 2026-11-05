use super::*;

pub(super) fn format_count(n: i64) -> String {
    match n.unsigned_abs() {
        0..1_000 => n.to_string(),
        1_000..10_000 => format!("{:.1}k", n as f64 / 1_000.0),
        10_000..1_000_000 => format!("{}k", n / 1_000),
        _ => format!("{:.1}m", n as f64 / 1_000_000.0),
    }
}

pub(super) fn score_color(score: Option<i64>) -> Color {
    match score {
        Some(s) if s >= 1_000 => ACCENT,
        Some(s) if s >= 100 => PEACH,
        Some(_) => SUBTEXT,
        None => OVERLAY,
    }
}

/// Cut `s` to `width` columns with an ellipsis, on grapheme boundaries.
pub(super) fn truncate(s: &str, width: usize) -> String {
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

pub(super) fn choice_style(selected: bool) -> Style {
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
pub(super) fn panes(
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

pub(super) enum Beside {
    Preview,
    Feed,
    Post,
}

/// With two or more panes on screen, outlines the focused one in accent. Each pane then
/// keeps a one-cell inset on every side for it, so moving focus never moves content.
pub(super) fn outline_focused(f: &mut Frame, panes: &[(Rect, &'static str, bool)]) {
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

pub(super) fn selection_marker(selected: bool, focused: bool) -> &'static str {
    match (selected, focused) {
        (true, true) => "▌",
        (true, false) => "│",
        _ => " ",
    }
}

pub(super) fn fit_spans(spans: Vec<Span<'static>>, width: usize) -> Vec<Span<'static>> {
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

pub(super) fn wrap_spans(
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

pub(super) fn chip(text: impl Into<String>, fg: Color, bg: Color) -> Span<'static> {
    Span::styled(format!(" {} ", text.into()), Style::new().fg(fg).bg(bg))
}

pub(super) fn key_chip(key: &str) -> Span<'static> {
    Span::styled(
        format!(" {key} "),
        Style::new()
            .fg(TEXT)
            .bg(SURFACE1)
            .add_modifier(Modifier::BOLD),
    )
}

pub(super) fn pad_to(spans: &mut Vec<Span<'static>>, width: usize, style: Style) {
    let used = line_width(spans);
    if used < width {
        spans.push(Span::styled(" ".repeat(width - used), style));
    }
}

pub(super) fn line_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|s| s.content.width()).sum()
}

pub(super) fn centered(f: &mut Frame, area: Rect, lines: Vec<Line<'static>>) {
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

pub(super) fn empty_state(f: &mut Frame, area: Rect, icon: &str, title: &str, hint: &str) {
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

pub(super) fn empty_content_state(
    f: &mut Frame,
    area: Rect,
    title: &str,
    hint: &str,
    show_nsfw: bool,
) {
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

pub(super) fn unread_chip(badge: UnreadBadge) -> Option<Span<'static>> {
    match badge {
        UnreadBadge::Unknown => Some(chip("? unread", OVERLAY, SURFACE0)),
        UnreadBadge::Count(n) if n > 0 => Some(chip(format!("● {n} unread"), CRUST, ACCENT)),
        _ => None,
    }
}

pub(super) fn wrap_into(
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

/// `inner` moved to the middle of `outer`'s width, at its top.
pub(super) fn centred(inner: Rect, outer: Rect) -> Rect {
    Rect {
        x: outer.x + outer.width.saturating_sub(inner.width) / 2,
        y: outer.y,
        ..inner
    }
}

pub(super) fn wrapped_rows(text: &str, width: u16) -> usize {
    text.lines()
        .map(|line| textwrap::wrap(line, usize::from(width).max(1)).len().max(1))
        .sum()
}

pub(super) fn fitting_label<'a>(labels: &[&'a str], width: usize) -> &'a str {
    labels
        .iter()
        .copied()
        .find(|label| label.width() <= width)
        .unwrap_or("")
}
