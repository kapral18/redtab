use super::*;

pub(super) fn push_inline(out: &mut String, text: &str, autolink: bool) {
    let mut rest = text;
    while let Some(at) = rest.find([SPOILER_START, SPOILER_END]) {
        push_inline_segment(out, &rest[..at], autolink);
        let marker = rest[at..].chars().next().unwrap();
        out.push(marker);
        rest = &rest[at + marker.len_utf8()..];
    }
    push_inline_segment(out, rest, autolink);
}

pub(super) fn push_inline_segment(out: &mut String, text: &str, autolink: bool) {
    let collapsed = collapse_ws(text);
    if collapsed.is_empty() {
        if text.chars().any(char::is_whitespace) && !out.ends_with([' ', '\n']) && !out.is_empty() {
            out.push(' ');
        }
        return;
    }
    if text.starts_with(char::is_whitespace) && !out.ends_with([' ', '\n']) && !out.is_empty() {
        out.push(' ');
    }
    for word in collapsed.split_inclusive(' ') {
        let mut core = word.trim_end_matches([' ', '.', ',', ';', '!', '?']);
        let mut parentheses = core
            .matches(')')
            .count()
            .saturating_sub(core.matches('(').count());
        let mut brackets = core
            .matches(']')
            .count()
            .saturating_sub(core.matches('[').count());
        loop {
            let excess = match core.as_bytes().last() {
                Some(b')') => &mut parentheses,
                Some(b']') => &mut brackets,
                _ => break,
            };
            if *excess == 0 {
                break;
            }
            *excess -= 1;
            core = core[..core.len() - 1].trim_end_matches(['.', ',', ';', '!', '?']);
        }
        let mention =
            core.trim_start_matches('/')
                .split_once('/')
                .filter(|(kind, name)| match *kind {
                    "r" => crate::config::is_subreddit_name(name),
                    "u" => crate::model::is_username(name),
                    _ => false,
                });
        if autolink && (mention.is_some() || absolute_link(core)) {
            out.push(TEXT_LINK);
            out.push(LINK_TARGET);
            if mention.is_some() {
                out.push_str(&format!(
                    "https://www.reddit.com/{}",
                    core.trim_start_matches('/')
                ));
            } else {
                out.push_str(core);
            }
            out.push(LINK_TARGET_END);
            out.push_str(core);
            out.push(TEXT_END);
            out.push_str(&word[core.len()..]);
        } else {
            out.push_str(word);
        }
    }
    if text.ends_with(char::is_whitespace) {
        out.push(' ');
    }
}

pub(super) fn line_break(out: &mut String) {
    let end = layout_tail(out).len();
    let start = out[..end].trim_end_matches(' ').len();
    out.replace_range(start..end, "");
    let tail = layout_tail(out);
    if !tail.is_empty() && !tail.ends_with('\n') {
        out.push('\n');
    }
}

pub(super) fn block_break(out: &mut String) {
    line_break(out);
    let tail = layout_tail(out);
    if !tail.is_empty() && !tail.ends_with("\n\n") {
        out.push('\n');
    }
}

pub(super) fn layout_tail(mut text: &str) -> &str {
    while let Some(c) = text.chars().next_back() {
        if text_format(c) || matches!(c, SPOILER_START | SPOILER_END) {
            text = &text[..text.len() - c.len_utf8()];
        } else if c == LINK_TARGET_END {
            text = &text[..text.rfind(LINK_TARGET).unwrap_or(0)];
        } else {
            break;
        }
    }
    text
}

pub(super) fn tidy_blank_lines(s: &str) -> String {
    let mut result = String::new();
    let mut blank_run = 0;
    for line in s.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            blank_run += 1;
            if blank_run > 1 || result.is_empty() {
                continue;
            }
        } else {
            blank_run = 0;
        }
        result.push_str(line);
        result.push('\n');
    }
    result.trim_end().to_string()
}
