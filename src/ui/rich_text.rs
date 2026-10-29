use super::*;
use crate::model::{
    TEXT_BOLD, TEXT_CODE, TEXT_END, TEXT_HEADING, TEXT_ITALIC, TEXT_LINK, TEXT_STRIKE, text_format,
};

pub(super) fn wrap(text: &str, width: usize, base: Style) -> Vec<Line<'static>> {
    let text: String = crate::model::without_link_targets(text)
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect();
    let mut lines = Vec::new();
    let mut formats = Vec::new();
    for paragraph in text.split('\n') {
        let plain = crate::model::unformatted(paragraph);
        let base = if plain.starts_with("│ ") {
            base.fg(SUBTEXT).add_modifier(Modifier::ITALIC)
        } else if plain.starts_with("    ") {
            base.fg(GREEN)
        } else {
            base
        };
        let spans = spans(paragraph, base, &mut formats);
        let mut cursor = 0;
        let mut span_index = 0;
        let mut span_start = 0;
        for piece in textwrap::wrap(&plain, width.max(1)) {
            let start = cursor
                + plain[cursor..]
                    .find(piece.as_ref())
                    .expect("unindented wrapping retains source text");
            let end = start + piece.len();
            let mut wrapped = Vec::new();
            while let Some(span) = spans.get(span_index) {
                let span_end = span_start + span.content.len();
                if span_start >= end {
                    break;
                }
                if span_end > start {
                    let a = start.saturating_sub(span_start);
                    let b = end.min(span_end) - span_start;
                    wrapped.push(Span::styled(span.content[a..b].to_string(), span.style));
                }
                if span_end > end {
                    break;
                }
                span_index += 1;
                span_start = span_end;
            }
            lines.push(Line::from(wrapped));
            cursor = end;
        }
    }
    lines
}

fn spans(text: &str, base: Style, formats: &mut Vec<char>) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut run = String::new();
    for c in text.chars().chain(std::iter::once(TEXT_END)) {
        if !text_format(c) {
            if !c.is_control() || c == '\t' {
                run.push(c);
            }
            continue;
        }
        if !run.is_empty() {
            let style = formats.iter().fold(base, |style, format| match *format {
                TEXT_BOLD => style.add_modifier(Modifier::BOLD),
                TEXT_ITALIC => style.add_modifier(Modifier::ITALIC),
                TEXT_STRIKE => style.add_modifier(Modifier::CROSSED_OUT),
                TEXT_CODE => style.fg(GREEN),
                TEXT_HEADING => style.fg(TEAL).add_modifier(Modifier::BOLD),
                TEXT_LINK => style.fg(BLUE).add_modifier(Modifier::UNDERLINED),
                _ => style,
            });
            spans.push(Span::styled(std::mem::take(&mut run), style));
        }
        if c == TEXT_END {
            formats.pop();
        } else {
            formats.push(c);
        }
    }
    spans
}
