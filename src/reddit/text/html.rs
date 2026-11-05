use super::*;

pub(super) fn has_class(el: &ElementRef<'_>, class: &str) -> bool {
    el.value().classes().any(|c| c == class)
}

pub(super) fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Tabs to spaces at 4-column stops: the terminal renderer drops control characters,
/// so tab-indented code would lose its indentation.
pub(super) fn expand_tabs(line: &str) -> String {
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

pub(super) fn line_formats(text: &str) -> String {
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

pub(super) fn render_children(
    el: ElementRef<'_>,
    out: &mut String,
    pre: bool,
    trusted_spoilers: bool,
) {
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

pub(super) fn render_element(
    el: ElementRef<'_>,
    out: &mut String,
    pre: bool,
    trusted_spoilers: bool,
) {
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

pub(super) fn picture_source(src: &str) -> bool {
    url::Url::parse(src).is_ok_and(|u| {
        super::super::transport::media_allowed(&u)
            && matches!(
                u.host_str(),
                Some("i.redd.it" | "preview.redd.it" | "external-preview.redd.it")
            )
    })
}

pub(super) fn picture(out: &mut String, src: &str) {
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

pub(super) fn absolute_link(text: &str) -> bool {
    url::Url::parse(text)
        .is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some())
}
