//! Render Reddit body_html, preserving spoilers, code, quotes and inline pictures.
use crate::model::{PICTURE, SPOILER_END, SPOILER_START};
use scraper::{ElementRef, Html, Node};

pub(super) fn body(html: &str, fallback: &str) -> String {
    if html.is_empty() {
        return without_markers(fallback).into_owned();
    }
    md_to_text(Html::parse_fragment(html).root_element())
}

/// Strips spoiler and picture markers from Reddit text; only the parser may add them.
pub(super) fn without_markers(text: &str) -> std::borrow::Cow<'_, str> {
    const MARKERS: [char; 3] = [SPOILER_START, SPOILER_END, PICTURE];
    if text.contains(MARKERS) {
        text.replace(MARKERS, "").into()
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

/// Convert Reddit's rendered markdown HTML into readable plain text.
fn md_to_text(el: ElementRef<'_>) -> String {
    let mut out = String::new();
    render_children(el, &mut out, false);
    tidy_blank_lines(&out)
}

fn render_children(el: ElementRef<'_>, out: &mut String, pre: bool) {
    for child in el.children() {
        match child.value() {
            Node::Text(t) => {
                let t = without_markers(t);
                if pre {
                    out.push_str(&t);
                } else {
                    push_inline(out, &t);
                }
            }
            Node::Element(_) => {
                if let Some(child_el) = ElementRef::wrap(child) {
                    render_element(child_el, out, pre);
                }
            }
            _ => {}
        }
    }
}

fn render_element(el: ElementRef<'_>, out: &mut String, pre: bool) {
    let name = el.value().name();
    match name {
        // Marked, so the post view can mask it until revealed.
        "span" if has_class(&el, "md-spoiler-text") => {
            out.push(SPOILER_START);
            render_children(el, out, pre);
            out.push(SPOILER_END);
        }
        "br" => out.push('\n'),
        "p" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "ul" | "ol" | "table" | "hr" => {
            block_break(out);
            if name == "hr" {
                out.push_str("────────");
            } else {
                render_children(el, out, pre);
            }
            block_break(out);
        }
        "li" => {
            line_break(out);
            out.push_str("• ");
            render_children(el, out, pre);
            line_break(out);
        }
        "tr" => {
            line_break(out);
            let cells: Vec<String> = el
                .children()
                .filter_map(ElementRef::wrap)
                .map(|cell| {
                    let mut s = String::new();
                    render_children(cell, &mut s, false);
                    // A row is one line: a picture in it can't have its own, so it's named.
                    let s = s
                        .split('\n')
                        .map(|line| {
                            if line.contains(PICTURE) {
                                "[image]"
                            } else {
                                line
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
            render_children(el, &mut inner, true);
            for line in inner.trim_end_matches('\n').lines() {
                out.push_str("    ");
                out.push_str(&expand_tabs(line));
                out.push('\n');
            }
            block_break(out);
        }
        "code" if !pre => {
            out.push('`');
            render_children(el, out, true);
            out.push('`');
        }
        "blockquote" => {
            block_break(out);
            let mut inner = String::new();
            render_children(el, &mut inner, pre);
            // A spoiler that spans lines is closed before each quote bar and reopened
            // after it, so the bars stay visible while the text is masked.
            let mut in_spoiler = false;
            for line in tidy_blank_lines(&inner).lines() {
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
            if !pre
                && let Some(href) = el.attr("href").map(without_markers)
                && picture_source(&href)
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
                picture(out, &href);
                let mut caption = String::new();
                render_children(el, &mut caption, false);
                if caption.trim() != href {
                    out.push_str(caption.trim());
                }
                return;
            }
            let before = out.len();
            render_children(el, out, pre);
            let text = out[before..].trim();
            // The URL is page text too: it must not carry a spoiler marker.
            if let Some(href) = el.attr("href").map(without_markers)
                && href.starts_with("http")
                && text != href.as_ref()
                && !text.is_empty()
                && text.strip_prefix(PICTURE) != Some(href.as_ref())
            {
                out.push_str(&format!(" <{href}>"));
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
        _ => render_children(el, out, pre),
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
    if &out[line_start..] != "• " {
        line_break(out);
    }
    out.push(PICTURE);
    out.push_str(src);
    out.push('\n');
}

fn push_inline(out: &mut String, text: &str) {
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
    out.push_str(&collapsed);
    if text.ends_with(char::is_whitespace) {
        out.push(' ');
    }
}

fn line_break(out: &mut String) {
    while out.ends_with(' ') {
        out.pop();
    }
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
}

fn block_break(out: &mut String) {
    line_break(out);
    if !out.is_empty() && !out.ends_with("\n\n") {
        out.push('\n');
    }
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
    fn raw_reddit_image_links_render_pictures_with_captions() {
        let image = "https://preview.redd.it/a.png?width=320&format=png";
        let html = "<p><a href=\"https://preview.redd.it/a.png?width=320&amp;format=png\">caption λ</a></p>";
        assert_eq!(body(html, ""), format!("{PICTURE}{image}\ncaption λ"));
        assert_eq!(
            body(
                "<p>See <a href=\"https://i.redd.it/a.png\">this picture</a>.</p>",
                ""
            ),
            "See this picture <https://i.redd.it/a.png>."
        );
        assert_eq!(
            body(
                "<p><a href=\"https://example.org/a.png\">external</a></p>",
                ""
            ),
            "external <https://example.org/a.png>"
        );
    }
    #[test]
    fn a_spoiler_across_quoted_lines_keeps_each_quote_bar_outside_it() {
        let el = Html::parse_fragment(
            r#"<blockquote><p><span class="md-spoiler-text">line one<br/>line two</span></p></blockquote>"#,
        );

        let text = md_to_text(el.root_element());

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
            md_to_text(el.root_element()),
            "odd glyph here link <http://x.test/> after"
        );
    }

    #[test]
    fn a_picture_in_reddit_text_should_get_its_own_line_and_other_images_their_alt_text() {
        let el = Html::parse_fragment(
            "<p>look:</p>\
             <figure><a href=\"https://preview.redd.it/a.png?width=500&amp;s=x\">\
             <img loading=\"lazy\" src=\"https://preview.redd.it/a.png?width=500&amp;s=x\"></a></figure>\
             <p>a <a href=\"https://giphy.com/gifs/x\"><img src=\"https://external-preview.redd.it/g.gif?width=300\"></a></p>\
             <p>and <img src=\"https://emoji.redditmedia.com/e.png\" alt=\"smile\"> &#xFDD2;done</p>",
        );

        assert_eq!(
            md_to_text(el.root_element()),
            format!(
                "look:\n\n{PICTURE}https://preview.redd.it/a.png?width=500&s=x\n\na\n\
                 {PICTURE}https://external-preview.redd.it/g.gif?width=300\n <https://giphy.com/gifs/x>\n\n\
                 and [smile] done"
            )
        );
    }

    #[test]
    fn a_picture_opening_a_list_item_should_stay_on_its_bullets_line() {
        let el =
            Html::parse_fragment("<ul><li><img src=\"https://preview.redd.it/a.png\"></li></ul>");

        assert_eq!(
            md_to_text(el.root_element()),
            format!("• {PICTURE}https://preview.redd.it/a.png")
        );
    }

    #[test]
    fn a_picture_in_a_table_cell_should_be_named_and_keep_the_row_whole() {
        let el = Html::parse_fragment(
            "<table><tr><td>a</td><td><img src=\"https://preview.redd.it/x.png\"></td><td>c</td></tr></table>",
        );

        assert_eq!(md_to_text(el.root_element()), "a │ [image] │ c");
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

        let text = md_to_text(md);

        assert_eq!(
            text,
            "One link <https://x.y/z> and `c`.\n\n│ quoted\n\n• a\n• b\n\n    fn x() {\n        1\n    }"
        );
    }
}
