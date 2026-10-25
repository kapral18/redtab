use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    More,
    Initial,
    Background,
    Media,
    Action,
}

pub(super) fn glyph(phase: usize) -> &'static str {
    const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    FRAMES[phase % FRAMES.len()]
}

pub(super) fn style(kind: Kind) -> Style {
    match kind {
        Kind::More => Style::new().fg(ACCENT).bold(),
        Kind::Initial => Style::new().fg(TEXT),
        Kind::Background | Kind::Media => Style::new().fg(OVERLAY),
        Kind::Action => Style::new().fg(YELLOW),
    }
}

pub(super) fn caption(kind: Kind, label: &str, phase: usize) -> Line<'static> {
    Line::styled(format!("{} {label}", glyph(phase)), style(kind))
}

fn bar(width: usize, phase: usize) -> Vec<Span<'static>> {
    let highlight = phase.wrapping_mul(3) % width.saturating_add(6);
    (0..width)
        .map(|column| {
            Span::styled(
                "━",
                Style::new().fg(if column.abs_diff(highlight) < 3 {
                    OVERLAY
                } else {
                    SURFACE1
                }),
            )
        })
        .collect()
}

pub(super) fn compact(kind: Kind, label: &str, width: usize, phase: usize) -> Line<'static> {
    let mut line = caption(kind, label, phase);
    let remaining = width.saturating_sub(line.width() + 2);
    if kind == Kind::More && remaining >= 4 {
        line.spans.push(Span::raw("  "));
        line.spans.extend(bar(remaining.min(16), phase));
    }
    Line::from(fit_spans(line.spans, width))
}

pub(super) fn wrapped(kind: Kind, label: &str, width: usize, phase: usize) -> Vec<Line<'static>> {
    wrap_spans(caption(kind, label, phase).spans, width.max(1), &[])
}

pub(super) fn draw(
    f: &mut Frame,
    mut area: Rect,
    kind: Kind,
    label: &str,
    phase: usize,
) -> Option<Rect> {
    if area.is_empty() {
        return None;
    }
    let width = area.width.min(64);
    area.x += (area.width - width) / 2;
    area.width = width;
    if kind == Kind::More {
        if usize::from(area.width) < (label.width() + 4).max(18) {
            return None;
        }
        if area.height > 2 {
            area.y += 1;
            area.height -= 1;
        }
        area.height = area.height.min(3);
        let inner = area.inner(Margin::new(1, 0));
        let mut lines = vec![if inner.height == 1 {
            compact(kind, label, inner.width as usize, phase)
        } else {
            caption(kind, label, phase)
        }];
        for row in 1..inner.height {
            let columns = if row == 1 {
                inner.width / 4 * 3
            } else {
                inner.width / 2
            };
            let mut spans = vec![Span::raw("  ")];
            spans.extend(bar(columns as usize, phase));
            lines.push(Line::from(spans));
        }
        f.render_widget(Paragraph::new(lines), inner);
        return Some(Rect::new(inner.x, inner.y, (label.width() + 2) as u16, 1));
    }
    let lines = wrapped(kind, label, area.width as usize, phase);
    let height = lines.len().min(area.height as usize) as u16;
    let centered = matches!(kind, Kind::Initial | Kind::Media);
    let at = Rect::new(
        area.x,
        area.y
            + if centered {
                (area.height - height) / 2
            } else {
                0
            },
        area.width,
        height,
    );
    f.render_widget(
        Paragraph::new(lines).alignment(if centered {
            Alignment::Center
        } else {
            Alignment::Left
        }),
        at,
    );
    Some(at)
}
