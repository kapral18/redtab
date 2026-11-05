use super::*;

pub(super) fn markdown_html(source: &str) -> String {
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

pub(super) fn spoiler_delimiters(source: &str) -> String {
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

pub(super) fn literal_spoilers(text: pulldown_cmark::CowStr<'_>) -> pulldown_cmark::CowStr<'_> {
    if text.contains([SPOILER_START, SPOILER_END]) {
        text.replace(SPOILER_START, ">!")
            .replace(SPOILER_END, "!<")
            .into()
    } else {
        text
    }
}

pub(super) fn spoiler_events(
    events: Vec<pulldown_cmark::Event<'_>>,
) -> Vec<pulldown_cmark::Event<'_>> {
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

pub(super) fn markdown_superscripts(source: &str, options: pulldown_cmark::Options) -> String {
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

/// Preserve body structure and inline formatting for the terminal renderer.
pub(super) fn md_to_text(el: ElementRef<'_>, trusted_spoilers: bool) -> String {
    let mut out = String::new();
    render_children(el, &mut out, false, trusted_spoilers);
    tidy_blank_lines(&line_formats(&out))
}
