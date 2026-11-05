use super::*;

pub(super) fn without_marker_entities(source: &str) -> String {
    use pulldown_cmark::{Event, Parser, Tag};
    let code: Vec<_> = Parser::new(source)
        .into_offset_iter()
        .filter_map(|(event, range)| {
            matches!(event, Event::Code(_) | Event::Start(Tag::CodeBlock(_))).then_some(range)
        })
        .collect();
    let mut code_index = 0;
    let mut out = String::new();
    let mut at = 0;
    while at < source.len() {
        while code.get(code_index).is_some_and(|range| range.end <= at) {
            code_index += 1;
        }
        if !code
            .get(code_index)
            .is_some_and(|range| range.contains(&at))
            && source[at..].starts_with("&#")
            && source[..at]
                .bytes()
                .rev()
                .take_while(|b| *b == b'\\')
                .count()
                .is_multiple_of(2)
            && let Some(end) = source.as_bytes()[at..]
                .iter()
                .take(12)
                .position(|b| *b == b';')
        {
            let value = &source[at + 2..at + end];
            let (digits, radix, limit) = value
                .strip_prefix(['x', 'X'])
                .map_or((value, 10, 7), |v| (v, 16, 6));
            if !digits.is_empty()
                && digits.len() <= limit
                && digits.bytes().all(|b| {
                    if radix == 16 {
                        b.is_ascii_hexdigit()
                    } else {
                        b.is_ascii_digit()
                    }
                })
                && u32::from_str_radix(digits, radix)
                    .ok()
                    .and_then(char::from_u32)
                    .is_some_and(body_marker)
            {
                at += end + 1;
                continue;
            }
        }
        let c = source[at..].chars().next().unwrap();
        out.push(c);
        at += c.len_utf8();
    }
    out
}

pub(super) fn without_html_marker_entities(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut at = 0;
    while at < source.len() {
        if source[at..].starts_with("&#") {
            let mut end = at + 2;
            let radix = if matches!(source.as_bytes().get(end), Some(b'x' | b'X')) {
                end += 1;
                16
            } else {
                10
            };
            let start = end;
            let mut value = Some(0u32);
            while let Some(digit) = source
                .as_bytes()
                .get(end)
                .and_then(|b| char::from(*b).to_digit(radix))
            {
                value = value
                    .and_then(|v| v.checked_mul(radix))
                    .and_then(|v| v.checked_add(digit));
                end += 1;
            }
            if end > start && value.and_then(char::from_u32).is_some_and(body_marker) {
                if source.as_bytes().get(end) == Some(&b';') {
                    end += 1;
                }
                at = end;
                continue;
            }
        }
        let c = source[at..].chars().next().unwrap();
        out.push(c);
        at += c.len_utf8();
    }
    out
}

/// Only the parser may add internal body markers.
pub(in crate::reddit) fn without_markers(text: &str) -> std::borrow::Cow<'_, str> {
    strip_body_markers(text, false)
}

pub(super) fn body_marker(c: char) -> bool {
    matches!(
        c,
        SPOILER_START | SPOILER_END | PICTURE | LINK_TARGET | LINK_TARGET_END
    ) || text_format(c)
}

pub(super) fn strip_body_markers(text: &str, trusted_spoilers: bool) -> std::borrow::Cow<'_, str> {
    let marker = |c| {
        matches!(c, PICTURE | LINK_TARGET | LINK_TARGET_END)
            || text_format(c)
            || (!trusted_spoilers && matches!(c, SPOILER_START | SPOILER_END))
    };
    if text.contains(marker) {
        text.chars()
            .filter(|c| !marker(*c))
            .collect::<String>()
            .into()
    } else {
        text.into()
    }
}
