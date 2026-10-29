//! Render Reddit body_html, preserving spoilers, code, quotes and inline pictures.
use crate::model::{
    LINK_TARGET, LINK_TARGET_END, PICTURE, SPOILER_END, SPOILER_START, TEXT_BOLD, TEXT_CODE,
    TEXT_END, TEXT_HEADING, TEXT_ITALIC, TEXT_LINK, TEXT_STRIKE, text_format, unformatted,
};
use scraper::{ElementRef, Html, Node};

pub(super) fn body(html: &str, fallback: &str) -> String {
    let trusted_spoilers = html.is_empty();
    let rendered;
    let html = if html.is_empty() {
        rendered = markdown_html(&without_markers(fallback));
        &rendered
    } else {
        html
    };
    md_to_text(Html::parse_fragment(html).root_element(), trusted_spoilers)
}

fn markdown_html(source: &str) -> String {
    let options =
        pulldown_cmark::Options::ENABLE_TABLES | pulldown_cmark::Options::ENABLE_STRIKETHROUGH;
    let source = without_marker_entities(source);
    let source = spoiler_delimiters(&source);
    let markdown = markdown_superscripts(&source, options);
    let events =
        pulldown_cmark::TextMergeStream::new(pulldown_cmark::Parser::new_ext(&markdown, options))
            .collect();
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, spoiler_events(events).into_iter());
    html
}

fn without_marker_entities(source: &str) -> String {
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

fn spoiler_delimiters(source: &str) -> String {
    let mut out = String::new();
    let mut chars = source.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            out.push(c);
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else if c == '>' && chars.peek() == Some(&'!') {
            chars.next();
            out.push(SPOILER_START);
        } else if c == '!' && chars.peek() == Some(&'<') {
            chars.next();
            out.push(SPOILER_END);
        } else {
            out.push(c);
        }
    }
    out
}

fn without_html_marker_entities(source: &str) -> String {
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

fn literal_spoilers(text: pulldown_cmark::CowStr<'_>) -> pulldown_cmark::CowStr<'_> {
    if text.contains([SPOILER_START, SPOILER_END]) {
        text.replace(SPOILER_START, ">!")
            .replace(SPOILER_END, "!<")
            .into()
    } else {
        text
    }
}

fn spoiler_events(events: Vec<pulldown_cmark::Event<'_>>) -> Vec<pulldown_cmark::Event<'_>> {
    use pulldown_cmark::{Event, Tag, TagEnd};
    let mut pairs = std::collections::HashSet::new();
    let mut open = None;
    let mut code = false;
    for (index, event) in events.iter().enumerate() {
        match event {
            Event::Start(Tag::CodeBlock(_)) => code = true,
            Event::End(TagEnd::CodeBlock) => code = false,
            Event::Text(text) if !code => {
                let mut words = text.split_inclusive(char::is_whitespace);
                let mut word_end = 0;
                let mut url = false;
                for (at, c) in text.char_indices() {
                    if at >= word_end
                        && let Some(word) = words.next()
                    {
                        word_end = at + word.len();
                        url = absolute_link(literal_spoilers(word.into()).trim());
                    }
                    if url && open.is_none() {
                        continue;
                    }
                    if c == SPOILER_START && open.is_none() {
                        open = Some((index, at));
                    } else if c == SPOILER_END
                        && let Some(start) = open.take()
                    {
                        pairs.insert(start);
                        pairs.insert((index, at));
                    }
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    code = false;
    for (index, event) in events.into_iter().enumerate() {
        match event {
            Event::Start(Tag::CodeBlock(_)) => {
                code = true;
                out.push(event);
            }
            Event::End(TagEnd::CodeBlock) => {
                code = false;
                out.push(event);
            }
            Event::Text(text) if !code => {
                let mut start = 0;
                for (at, c) in text
                    .char_indices()
                    .filter(|(_, c)| matches!(*c, SPOILER_START | SPOILER_END))
                {
                    if at > start {
                        out.push(Event::Text(text[start..at].to_string().into()));
                    }
                    if pairs.contains(&(index, at)) {
                        out.push(Event::Text(c.to_string().into()));
                    } else {
                        out.push(Event::Text(
                            if c == SPOILER_START { ">!" } else { "!<" }.into(),
                        ));
                    }
                    start = at + c.len_utf8();
                }
                if start < text.len() {
                    out.push(Event::Text(text[start..].to_string().into()));
                }
            }
            Event::Text(text) => out.push(Event::Text(literal_spoilers(text))),
            Event::Code(text) => out.push(Event::Code(literal_spoilers(text))),
            Event::Html(text) => out.push(Event::Html(
                without_html_marker_entities(&literal_spoilers(text)).into(),
            )),
            Event::InlineHtml(text) => out.push(Event::InlineHtml(
                without_html_marker_entities(&literal_spoilers(text)).into(),
            )),
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }) => out.push(Event::Start(Tag::Link {
                link_type,
                dest_url: literal_spoilers(dest_url),
                title: literal_spoilers(title),
                id: literal_spoilers(id),
            })),
            Event::Start(Tag::Image {
                link_type,
                dest_url,
                title,
                id,
            }) => out.push(Event::Start(Tag::Image {
                link_type,
                dest_url: literal_spoilers(dest_url),
                title: literal_spoilers(title),
                id: literal_spoilers(id),
            })),
            other => out.push(other),
        }
    }
    out
}

fn markdown_superscripts(source: &str, options: pulldown_cmark::Options) -> String {
    use pulldown_cmark::{Event, Parser, Tag, TagEnd};
    let mut text_ranges = Vec::new();
    let mut code_block = false;
    for (event, range) in Parser::new_ext(source, options).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(_)) => code_block = true,
            Event::End(TagEnd::CodeBlock) => code_block = false,
            Event::Text(_) if !code_block => text_ranges.push(range),
            _ => {}
        }
    }
    let mut replacements = Vec::new();
    for range in text_ranges {
        let mut rest = range.start;
        while let Some(offset) = source[rest..range.end].find('^') {
            let start = rest + offset;
            rest = start + 1;
            if source[..start]
                .chars()
                .rev()
                .take_while(|c| *c == '\\')
                .count()
                % 2
                == 1
            {
                continue;
            }
            if source[rest..range.end].starts_with('(') {
                if let Some(close) = source[rest + 1..range.end].find(')') {
                    let end = rest + 1 + close;
                    replacements.push((start..rest + 1, "<sup>"));
                    replacements.push((end..end + 1, "</sup>"));
                    rest = end + 1;
                }
            } else {
                let end = source[rest..range.end]
                    .find(char::is_whitespace)
                    .map_or(range.end, |n| rest + n);
                if end > rest {
                    replacements.push((start..rest, "<sup>"));
                    replacements.push((end..end, "</sup>"));
                    rest = end;
                }
            }
        }
    }
    replacements.sort_by_key(|(range, _)| range.start);
    let mut markdown = String::new();
    let mut last = 0;
    for (range, replacement) in replacements {
        markdown.push_str(&source[last..range.start]);
        markdown.push_str(replacement);
        last = range.end;
    }
    markdown.push_str(&source[last..]);
    markdown
}

/// Only the parser may add internal body markers.
pub(super) fn without_markers(text: &str) -> std::borrow::Cow<'_, str> {
    strip_body_markers(text, false)
}

fn body_marker(c: char) -> bool {
    matches!(
        c,
        SPOILER_START | SPOILER_END | PICTURE | LINK_TARGET | LINK_TARGET_END
    ) || text_format(c)
}

fn strip_body_markers(text: &str, trusted_spoilers: bool) -> std::borrow::Cow<'_, str> {
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

fn has_class(el: &ElementRef<'_>, class: &str) -> bool {
    el.value().classes().any(|c| c == class)
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Tabs to spaces at 4-column stops: the terminal renderer drops control characters,
/// so tab-indented code would lose its indentation.
fn expand_tabs(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut col = 0;
    for c in line.chars() {
        if c == '\t' {
            let n = 4 - col % 4;
            out.extend(std::iter::repeat_n(' ', n));
            col += n;
        } else {
            out.push(c);
            col += 1;
        }
    }
    out
}

/// Preserve body structure and inline formatting for the terminal renderer.
fn md_to_text(el: ElementRef<'_>, trusted_spoilers: bool) -> String {
    let mut out = String::new();
    render_children(el, &mut out, false, trusted_spoilers);
    tidy_blank_lines(&line_formats(&out))
}

fn line_formats(text: &str) -> String {
    let mut stack: Vec<String> = Vec::new();
    text.lines()
        .map(|line| {
            let mut result = stack.concat();
            let mut chars = line.chars();
            while let Some(c) = chars.next() {
                if c == LINK_TARGET {
                    let target: String = chars
                        .by_ref()
                        .take_while(|c| *c != LINK_TARGET_END)
                        .collect();
                    let metadata = format!("{LINK_TARGET}{target}{LINK_TARGET_END}");
                    if let Some(format) = stack.last_mut() {
                        format.push_str(&metadata);
                    }
                    result.push_str(&metadata);
                    continue;
                }
                if c == TEXT_END {
                    stack.pop();
                } else if text_format(c) {
                    stack.push(c.to_string());
                }
                result.push(c);
            }
            result.extend(std::iter::repeat_n(TEXT_END, stack.len()));
            if unformatted(&result).trim().is_empty() {
                unformatted(&result).into_owned()
            } else {
                result
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_children(el: ElementRef<'_>, out: &mut String, pre: bool, trusted_spoilers: bool) {
    for child in el.children() {
        match child.value() {
            Node::Text(t) => {
                let t = strip_body_markers(t, trusted_spoilers);
                if pre {
                    out.push_str(&t);
                } else {
                    let linked = std::iter::once(el)
                        .chain(el.ancestors().filter_map(ElementRef::wrap))
                        .any(|parent| parent.value().name() == "a");
                    push_inline(out, &t, !linked);
                }
            }
            Node::Element(_) => {
                if let Some(child_el) = ElementRef::wrap(child) {
                    render_element(child_el, out, pre, trusted_spoilers);
                }
            }
            _ => {}
        }
    }
}

fn render_element(el: ElementRef<'_>, out: &mut String, pre: bool, trusted_spoilers: bool) {
    let name = el.value().name();
    match name {
        "strong" | "b" | "em" | "i" | "del" | "s" | "strike" => {
            out.push(match name {
                "strong" | "b" => TEXT_BOLD,
                "em" | "i" => TEXT_ITALIC,
                _ => TEXT_STRIKE,
            });
            render_children(el, out, pre, trusted_spoilers);
            out.push(TEXT_END);
        }
        "sup" => {
            out.push('^');
            render_children(el, out, pre, trusted_spoilers);
        }
        // Marked, so the post view can mask it until revealed.
        "span" if has_class(&el, "md-spoiler-text") => {
            out.push(SPOILER_START);
            render_children(el, out, pre, trusted_spoilers);
            out.push(SPOILER_END);
        }
        "br" => out.push('\n'),
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            block_break(out);
            out.push(TEXT_HEADING);
            render_children(el, out, pre, trusted_spoilers);
            out.push(TEXT_END);
            block_break(out);
        }
        "ul" | "ol" => {
            block_break(out);
            let depth = el
                .ancestors()
                .filter_map(ElementRef::wrap)
                .filter(|parent| matches!(parent.value().name(), "ul" | "ol"))
                .count();
            let start = el
                .attr("start")
                .and_then(|n| n.parse::<u64>().ok())
                .unwrap_or(1);
            for (index, item) in el
                .children()
                .filter_map(ElementRef::wrap)
                .filter(|child| child.value().name() == "li")
                .enumerate()
            {
                line_break(out);
                out.push_str(&"  ".repeat(depth));
                if name == "ol" {
                    out.push_str(&format!("{}. ", start.saturating_add(index as u64)));
                } else {
                    out.push_str("• ");
                }
                render_children(item, out, pre, trusted_spoilers);
                line_break(out);
            }
            block_break(out);
        }
        "p" | "div" | "table" | "hr" => {
            let first_list_paragraph = name == "p"
                && el
                    .parent()
                    .and_then(ElementRef::wrap)
                    .is_some_and(|parent| parent.value().name() == "li")
                && el.prev_siblings().all(|node| {
                    matches!(node.value(), Node::Text(t) if t.trim().is_empty())
                        || matches!(node.value(), Node::Comment(_))
                });
            if !first_list_paragraph {
                block_break(out);
            }
            if name == "hr" {
                out.push_str("────────");
            } else {
                render_children(el, out, pre, trusted_spoilers);
            }
            block_break(out);
        }
        "li" => {
            line_break(out);
            out.push_str("• ");
            render_children(el, out, pre, trusted_spoilers);
            line_break(out);
        }
        "tr" => {
            line_break(out);
            let cells: Vec<String> = el
                .children()
                .filter_map(ElementRef::wrap)
                .map(|cell| {
                    let mut s = String::new();
                    if cell.value().name() == "th" {
                        s.push(TEXT_BOLD);
                    }
                    render_children(cell, &mut s, false, trusted_spoilers);
                    if cell.value().name() == "th" {
                        s.push(TEXT_END);
                    }
                    // A row is one line: a picture in it can't have its own, so it's named.
                    let s = line_formats(&s);
                    let s = s
                        .split('\n')
                        .map(|line| {
                            if let Some((lead, picture)) = line.split_once(PICTURE) {
                                let end = picture.find(body_marker).unwrap_or(picture.len());
                                format!("{lead}[image]{}", &picture[end..])
                            } else {
                                line.to_string()
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    collapse_ws(&s)
                })
                .collect();
            out.push_str(&cells.join(" │ "));
            line_break(out);
        }
        "pre" => {
            block_break(out);
            let mut inner = String::new();
            render_children(el, &mut inner, true, trusted_spoilers);
            for line in inner.trim_end_matches('\n').lines() {
                out.push(TEXT_CODE);
                out.push_str("    ");
                out.push_str(&expand_tabs(line));
                out.push(TEXT_END);
                out.push('\n');
            }
            block_break(out);
        }
        "code" if !pre => {
            out.push(TEXT_CODE);
            render_children(el, out, true, trusted_spoilers);
            out.push(TEXT_END);
        }
        "blockquote" => {
            block_break(out);
            let mut inner = String::new();
            render_children(el, &mut inner, pre, trusted_spoilers);
            // A spoiler that spans lines is closed before each quote bar and reopened
            // after it, so the bars stay visible while the text is masked.
            let mut in_spoiler = false;
            for line in tidy_blank_lines(&line_formats(&inner)).lines() {
                if in_spoiler {
                    out.push(SPOILER_END);
                    out.push_str("│ ");
                    out.push(SPOILER_START);
                } else {
                    out.push_str("│ ");
                }
                out.push_str(line);
                out.push('\n');
                for c in line.chars() {
                    match c {
                        SPOILER_START => in_spoiler = true,
                        SPOILER_END => in_spoiler = false,
                        _ => {}
                    }
                }
            }
            block_break(out);
        }
        "a" => {
            let href = el
                .attr("href")
                .map(without_markers)
                .and_then(|href| {
                    url::Url::parse("https://www.reddit.com")
                        .ok()?
                        .join(&href)
                        .ok()
                })
                .filter(|url| matches!(url.scheme(), "http" | "https"));
            if let Some(href) = &href {
                out.push(TEXT_LINK);
                out.push(LINK_TARGET);
                out.push_str(href.as_str());
                out.push(LINK_TARGET_END);
            }
            if !pre
                && let Some(href) = &href
                && picture_source(href.as_str())
                && el
                    .parent()
                    .and_then(ElementRef::wrap)
                    .is_some_and(|parent| {
                        parent.value().name() == "p"
                            && parent.children().all(|child| {
                                child.id() == el.id()
                                    || matches!(child.value(), Node::Text(t) if t.trim().is_empty())
                            })
                    })
                && !el
                    .descendants()
                    .filter_map(ElementRef::wrap)
                    .any(|e| e.value().name() == "img")
            {
                picture(out, href.as_str());
                let mut caption = String::new();
                render_children(el, &mut caption, false, trusted_spoilers);
                if caption.trim() != href.as_str() {
                    out.push_str(caption.trim());
                }
            } else {
                render_children(el, out, pre, trusted_spoilers);
            }
            if href.is_some() {
                out.push(TEXT_END);
            }
        }
        "img" => {
            // Reddit-hosted pictures (pasted images, GIFs) get a line of their own;
            // other images, such as emotes, stay as their alt text.
            match el.attr("src").map(without_markers) {
                Some(src) if picture_source(&src) => picture(out, &src),
                _ => {
                    if let Some(alt) = el.attr("alt").filter(|a| !a.is_empty()) {
                        out.push_str(&format!("[{}]", without_markers(alt)));
                    }
                }
            }
        }
        "script" | "style" => {}
        _ => render_children(el, out, pre, trusted_spoilers),
    }
}

fn picture_source(src: &str) -> bool {
    url::Url::parse(src).is_ok_and(|u| {
        super::transport::media_allowed(&u)
            && matches!(
                u.host_str(),
                Some("i.redd.it" | "preview.redd.it" | "external-preview.redd.it")
            )
    })
}

fn picture(out: &mut String, src: &str) {
    // A picture opening a list item stays on its bullet's line.
    let line_start = out.rfind('\n').map_or(0, |n| n + 1);
    let lead = unformatted(&out[line_start..]).replace([SPOILER_START, SPOILER_END], "");
    let lead = lead.trim_start();
    if !lead.is_empty()
        && lead != "• "
        && !lead
            .strip_suffix(". ")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
    {
        line_break(out);
    }
    out.push(PICTURE);
    out.push_str(src);
    out.push('\n');
}

fn absolute_link(text: &str) -> bool {
    url::Url::parse(text)
        .is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some())
}

fn push_inline(out: &mut String, text: &str, autolink: bool) {
    let mut rest = text;
    while let Some(at) = rest.find([SPOILER_START, SPOILER_END]) {
        push_inline_segment(out, &rest[..at], autolink);
        let marker = rest[at..].chars().next().unwrap();
        out.push(marker);
        rest = &rest[at + marker.len_utf8()..];
    }
    push_inline_segment(out, rest, autolink);
}

fn push_inline_segment(out: &mut String, text: &str, autolink: bool) {
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

fn line_break(out: &mut String) {
    let end = layout_tail(out).len();
    let start = out[..end].trim_end_matches(' ').len();
    out.replace_range(start..end, "");
    let tail = layout_tail(out);
    if !tail.is_empty() && !tail.ends_with('\n') {
        out.push('\n');
    }
}

fn block_break(out: &mut String) {
    line_break(out);
    let tail = layout_tail(out);
    if !tail.is_empty() && !tail.ends_with("\n\n") {
        out.push('\n');
    }
}

fn layout_tail(mut text: &str) -> &str {
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

fn tidy_blank_lines(s: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn when_a_comment_has_labeled_links_it_should_show_labels_without_destination_dumps() {
        let actual = body(
            "<p>Hey <a href=\"/u/Separate_Finance_183\">u/Separate_Finance_183</a>, welcome to <a href=\"/r/SipsTea\">r/SipsTea</a>!</p><p><strong>Join <a href=\"https://discord.gg/Cey9fTXFwk\">Discord Server</a>.</strong></p><p><a href=\"https://example.org/rules\">the subreddit rules</a> · https://example.org/raw</p>",
            "",
        );

        assert_eq!(
            unformatted(&actual),
            "Hey u/Separate_Finance_183, welcome to r/SipsTea!\n\nJoin Discord Server.\n\nthe subreddit rules · https://example.org/raw"
        );
        assert_eq!(
            crate::model::body_links(&actual, false)
                .into_iter()
                .map(|link| (link.label, link.url))
                .collect::<Vec<_>>(),
            [
                (
                    "u/Separate_Finance_183".into(),
                    "https://www.reddit.com/u/Separate_Finance_183".into()
                ),
                (
                    "r/SipsTea".into(),
                    "https://www.reddit.com/r/SipsTea".into()
                ),
                (
                    "Discord Server".into(),
                    "https://discord.gg/Cey9fTXFwk".into()
                ),
                (
                    "the subreddit rules".into(),
                    "https://example.org/rules".into()
                ),
                (
                    "https://example.org/raw".into(),
                    "https://example.org/raw".into()
                ),
            ]
        );
    }

    #[test]
    fn when_raw_links_have_balanced_punctuation_it_should_keep_the_complete_destination() {
        for (source, expected) in [
            (
                "https://en.wikipedia.org/wiki/Rust_(programming_language).",
                "https://en.wikipedia.org/wiki/Rust_(programming_language)",
            ),
            ("https://example.org/a[b]).", "https://example.org/a[b]"),
        ] {
            for html in [String::new(), format!("<p>{source}</p>")] {
                let actual = body(&html, source);
                assert_eq!(unformatted(&actual), source);
                let links = crate::model::body_links(&actual, false);
                assert_eq!(links.len(), 1, "{actual:?}");
                assert_eq!(links[0].url, expected);
            }
        }
    }

    #[test]
    fn when_reddit_html_has_inline_markup_it_should_retain_styles_and_list_identity() {
        use crate::model::*;
        let actual = body(
            "<h2>Heading</h2><p><strong>bold <em>both</em></strong> <del>old</del> <code>x * y</code> x<sup>2</sup></p><ol start=3><li><p>third</p><ul><li>nested</li></ul></li><li><p>fourth</p></li></ol><p><a href='/r/rust/wiki/index'>guide</a></p>",
            "",
        );

        assert!(
            actual.contains(&format!("{TEXT_HEADING}Heading{TEXT_END}")),
            "{actual:?}"
        );
        assert!(
            actual.contains(&format!(
                "{TEXT_BOLD}bold {TEXT_ITALIC}both{TEXT_END}{TEXT_END}"
            )),
            "{actual:?}"
        );
        assert!(
            actual.contains(&format!("{TEXT_STRIKE}old{TEXT_END}")),
            "{actual:?}"
        );
        assert!(
            actual.contains(&format!("{TEXT_CODE}x * y{TEXT_END}")),
            "{actual:?}"
        );
        let plain = unformatted(&actual);
        assert!(
            plain.contains("3. third")
                && plain.contains("  • nested")
                && plain.contains("4. fourth"),
            "{plain}"
        );
        assert!(plain.contains("x^2") && plain.contains("guide"), "{plain}");
        let multiline = body(
            "<p><a href='https://example.org/multiline'><br>first<br>second</a></p>",
            "",
        );
        let actual = crate::model::body_links(&multiline, false);
        assert_eq!(actual.len(), 1);
        assert_eq!(actual[0].url, "https://example.org/multiline");
    }

    #[test]
    fn when_only_markdown_is_available_it_should_format_text_and_protect_literal_code() {
        use crate::model::*;
        let actual = body(
            "",
            "**bold** and *italic* and ~~old~~\n\n>!hidden!<\n\n`**literal** >!code!<`\n\n[guide](/r/rust)\n\n^word ^(two words)\n\nr/rust https://example.org/link.\n\n---",
        );

        assert!(
            actual.contains(&format!("{TEXT_BOLD}bold{TEXT_END}")),
            "{actual:?}"
        );
        assert!(
            actual.contains(&format!("{TEXT_ITALIC}italic{TEXT_END}")),
            "{actual:?}"
        );
        assert!(
            actual.contains(&format!("{TEXT_STRIKE}old{TEXT_END}")),
            "{actual:?}"
        );
        assert!(
            actual.contains(&format!("{SPOILER_START}hidden{SPOILER_END}")),
            "{actual:?}"
        );
        assert!(actual.contains("**literal** >!code!<"), "{actual:?}");
        assert!(!unformatted(&actual).contains("[guide]"), "{actual:?}");
        assert!(
            unformatted(&actual).contains("^word ^two words"),
            "{actual:?}"
        );
        assert!(
            crate::model::without_link_targets(&actual)
                .contains(&format!("{TEXT_LINK}https://example.org/link{TEXT_END}.")),
            "{actual:?}"
        );
        assert!(unformatted(&actual).contains("r/rust"), "{actual:?}");
    }

    #[test]
    fn when_spoiler_syntax_is_malformed_or_inside_a_link_it_should_preserve_literal_text_and_destinations()
     {
        for (source, expected) in [
            ("prefix >!< suffix", "prefix >!< suffix"),
            (r"prefix \>!hidden!< suffix", "prefix >!hidden!< suffix"),
            ("https://example.org/>!foo!<", "https://example.org/>!foo!<"),
            ("&#xFDD0;literal&#xFDD1;", "literal"),
            (r"\&#xFDD0;literal", "&#xFDD0;literal"),
            (r"\\&#xFDD0;literal", r"\literal"),
            (
                "https://example.org/&amp;>!foo!<",
                "https://example.org/&>!foo!<",
            ),
            ("`&#xFDD0;`", "&#xFDD0;"),
            ("<div>&#64976hidden&#64977</div>", "hidden"),
            ("<div>&#x000FDD0;hidden&#x000FDD1;</div>", "hidden"),
            ("```\n>!literal!< ^code\n```", "    >!literal!< ^code"),
            ("[link](https://example.org/>!foo!<)", "link"),
        ] {
            let actual = body("", source);
            assert_eq!(unformatted(&actual), expected);
            assert!(!actual.contains([SPOILER_START, SPOILER_END]), "{actual:?}");
        }
        let linked_spoiler = body("", ">![secret](https://example.org)!<");
        assert!(
            linked_spoiler.contains(SPOILER_START) && linked_spoiler.contains(SPOILER_END),
            "{linked_spoiler:?}"
        );
        assert!(
            unformatted(&linked_spoiler).contains("secret"),
            "{linked_spoiler:?}"
        );
        let url_spoiler = body("", ">!hidden https://example.org!<");
        assert!(
            url_spoiler.contains(SPOILER_START) && url_spoiler.contains(SPOILER_END),
            "{url_spoiler:?}"
        );
        let crossing = body("", "*prefix >!secret* still secret!<");
        assert!(
            unformatted(&crossing)
                .contains(&format!("{SPOILER_START}secret still secret{SPOILER_END}")),
            "{crossing:?}"
        );
        let mention = body("", ">!r/rust!<");
        assert!(
            mention.contains(TEXT_LINK) && unformatted(&mention).contains("r/rust"),
            "{mention:?}"
        );
        for source in [
            ">![secret](https://example.org)!<",
            "[>!secret!<](https://example.org)",
            ">!r/rust!<",
        ] {
            let actual = body("", source);
            assert!(
                crate::model::body_links(&actual, false).is_empty(),
                "{actual:?}"
            );
            assert_eq!(
                crate::model::body_links(&actual, true).len(),
                1,
                "{actual:?}"
            );
        }
        let actual = body("", "[link](https://example.org/>!foo!<)");
        assert_eq!(
            crate::model::body_links(&actual, false)[0].url,
            "https://example.org/%3E!foo!%3C"
        );
    }

    #[test]
    fn raw_reddit_image_links_render_pictures_with_captions() {
        let image = "https://preview.redd.it/a.png?width=320&format=png";
        let html = "<p><a href=\"https://preview.redd.it/a.png?width=320&amp;format=png\">caption λ</a></p>";
        let actual = body(html, "");
        assert_eq!(unformatted(&actual), format!("{PICTURE}{image}\ncaption λ"));
        assert_eq!(crate::model::body_links(&actual, false)[0].url, image);
        assert_eq!(
            unformatted(&body(
                "<p>See <a href=\"https://i.redd.it/a.png\">this picture</a>.</p>",
                ""
            )),
            "See this picture."
        );
        assert_eq!(
            unformatted(&body(
                "<p><a href=\"https://example.org/a.png\">external</a></p>",
                ""
            )),
            "external"
        );
    }
    #[test]
    fn a_spoiler_across_quoted_lines_keeps_each_quote_bar_outside_it() {
        let el = Html::parse_fragment(
            r#"<blockquote><p><span class="md-spoiler-text">line one<br/>line two</span></p></blockquote>"#,
        );

        let text = md_to_text(el.root_element(), false);

        assert_eq!(
            text,
            format!(
                "│ {SPOILER_START}line one\n{SPOILER_END}│ {SPOILER_START}line two{SPOILER_END}"
            )
        );
    }

    #[test]
    fn marker_characters_in_reddit_text_are_dropped() {
        let el = Html::parse_fragment(&format!(
            "<p>odd {SPOILER_START}glyph{SPOILER_END} here \
             <a href=\"http://x.test/&#xFDD0;\">link</a> after</p>"
        ));

        assert_eq!(
            unformatted(&md_to_text(el.root_element(), false)),
            "odd glyph here link after"
        );
    }

    #[test]
    fn a_picture_in_reddit_text_should_get_its_own_line_and_other_images_their_alt_text() {
        let el = Html::parse_fragment(
            "<p>look:</p>\
             <figure><a href=\"https://preview.redd.it/a.png?width=500&amp;s=x\">\
             <img loading=\"lazy\" src=\"https://preview.redd.it/a.png?width=500&amp;s=x\"></a></figure>\
             <p>a <a href=\"https://giphy.com/gifs/x\"><img src=\"https://external-preview.redd.it/g.gif?width=300\"></a></p>\
             <p>and <img src=\"https://emoji.redditmedia.com/e.png\" alt=\"smile\"> &#xFDD2;done</p>\
             <p><a href=\"https://example.org/guide\"><img src=\"https://emoji.redditmedia.com/e.png\" alt=\"smile\"> guide</a></p>",
        );
        let actual = md_to_text(el.root_element(), false);
        assert_eq!(
            unformatted(&actual),
            format!(
                "look:\n\n{PICTURE}https://preview.redd.it/a.png?width=500&s=x\n\na\n\
                 {PICTURE}https://external-preview.redd.it/g.gif?width=300\n\n\
                 and [smile] done\n\n[smile] guide"
            )
        );
        assert_eq!(
            crate::model::body_links(&actual, false)
                .iter()
                .map(|link| link.url.as_str())
                .collect::<Vec<_>>(),
            [
                "https://preview.redd.it/a.png?width=500&s=x",
                "https://giphy.com/gifs/x",
                "https://example.org/guide"
            ]
        );
    }

    #[test]
    fn a_picture_opening_a_list_item_should_stay_on_its_bullets_line() {
        for (html, visible_links) in [
            (
                "<ul><li><a href='https://giphy.com/gifs/x'><img src='https://preview.redd.it/a.png'></a></li></ul>",
                1,
            ),
            (
                "<ul><li><span class='md-spoiler-text'><a href='https://giphy.com/gifs/x'><img src='https://preview.redd.it/a.png'></a></span></li></ul>",
                0,
            ),
        ] {
            let actual = body(html, "");
            let visible = unformatted(&actual).replace([SPOILER_START, SPOILER_END], "");
            assert_eq!(
                visible.trim_end(),
                format!("• {PICTURE}https://preview.redd.it/a.png")
            );
            assert_eq!(
                crate::model::body_links(&actual, false).len(),
                visible_links
            );
            assert_eq!(
                crate::model::body_links(&actual, true)[0].url,
                "https://giphy.com/gifs/x"
            );
        }
    }

    #[test]
    fn a_picture_in_a_table_cell_should_be_named_and_keep_the_row_whole() {
        let el = Html::parse_fragment(
            "<table><tr><td>a</td><td><a href=\"https://giphy.com/gifs/x\"><img src=\"https://preview.redd.it/x.png\"></a></td><td>c</td></tr></table>",
        );
        let actual = md_to_text(el.root_element(), false);
        assert_eq!(unformatted(&actual), "a │ [image] │ c");
        assert_eq!(
            crate::model::body_links(&actual, false)[0].url,
            "https://giphy.com/gifs/x"
        );
    }

    #[test]
    fn markdown_keeps_structure() {
        let html = Html::parse_fragment(
            r#"<div class="md"><p>One <a href="https://x.y/z">link</a> and <code>c</code>.</p><blockquote><p>quoted</p></blockquote><ul><li>a</li><li>b</li></ul><pre><code>fn x() {
    1
}</code></pre></div>"#,
        );
        let selector = scraper::Selector::parse("div.md").unwrap();
        let md = html.select(&selector).next().unwrap();

        let text = md_to_text(md, false);

        assert_eq!(
            unformatted(&text),
            "One link and c.\n\n│ quoted\n\n• a\n• b\n\n    fn x() {\n        1\n    }"
        );
    }
}
