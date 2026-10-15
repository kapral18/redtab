//! Parsers for Redlib HTML pages. Redlib renders Reddit as server-side HTML
//! with stable class names (`div.post`, `.post_title`, `div.comment`, …).

use std::sync::LazyLock;

use anyhow::{Result, bail};
use chrono::{DateTime, NaiveDateTime, Utc};
use scraper::{ElementRef, Html, Node, Selector};

use crate::model::{
    Comment, Listing, Media, MoreReplies, Post, PostPage, SPOILER_END, SPOILER_START,
};

fn sel(css: &str) -> Selector {
    Selector::parse(css).expect("static selector")
}

static POST: LazyLock<Selector> = LazyLock::new(|| sel("div.post"));
static SUBREDDIT: LazyLock<Selector> = LazyLock::new(|| sel(".post_subreddit"));
static AUTHOR: LazyLock<Selector> = LazyLock::new(|| sel(".post_author"));
static CREATED: LazyLock<Selector> = LazyLock::new(|| sel(".post_header .created"));
static TITLE: LazyLock<Selector> = LazyLock::new(|| sel(".post_title"));
static FLAIR: LazyLock<Selector> = LazyLock::new(|| sel(".post_flair"));
static SCORE: LazyLock<Selector> = LazyLock::new(|| sel(".post_score"));
static COMMENTS_LINK: LazyLock<Selector> = LazyLock::new(|| sel(".post_comments"));
static PERMALINK: LazyLock<Selector> = LazyLock::new(|| sel("#post_links a"));
static MEDIA_IMAGE: LazyLock<Selector> =
    LazyLock::new(|| sel(".post_media_content a.post_media_image"));
static MEDIA_VIDEO: LazyLock<Selector> =
    LazyLock::new(|| sel(".post_media_content video.post_media_video"));
static VIDEO_SOURCE: LazyLock<Selector> = LazyLock::new(|| sel("source"));
static GALLERY_LINK: LazyLock<Selector> = LazyLock::new(|| sel(".gallery figure > a"));
static THUMBNAIL: LazyLock<Selector> = LazyLock::new(|| sel("a.post_thumbnail"));
static IMG: LazyLock<Selector> = LazyLock::new(|| sel("img"));
static SPAN: LazyLock<Selector> = LazyLock::new(|| sel("span"));
static BODY_MD: LazyLock<Selector> = LazyLock::new(|| sel(".post_body .md"));
static POST_LINK: LazyLock<Selector> = LazyLock::new(|| sel("a#post_url"));
static NEXT: LazyLock<Selector> = LazyLock::new(|| sel(r#"a[accesskey="N"]"#));
static SUB_ICON: LazyLock<Selector> = LazyLock::new(|| sel("img#sub_icon"));
static PAGE_TITLE: LazyLock<Selector> = LazyLock::new(|| sel("title"));
static ERROR_H1: LazyLock<Selector> = LazyLock::new(|| sel("div#error > h1"));
static POLL: LazyLock<Selector> = LazyLock::new(|| sel("div.post_poll"));
static POLL_OPTION: LazyLock<Selector> = LazyLock::new(|| sel(".poll_option"));
static SPOILER_TAG: LazyLock<Selector> = LazyLock::new(|| sel(".post_title small.spoiler"));
static COMMENT_COUNT: LazyLock<Selector> = LazyLock::new(|| sel("#comment_count"));
static THREAD_COMMENT: LazyLock<Selector> = LazyLock::new(|| sel(".thread > .comment"));
static NSFW: LazyLock<Selector> = LazyLock::new(|| sel(".nsfw"));
static COMMENT_AUTHOR: LazyLock<Selector> =
    LazyLock::new(|| sel(":scope > details > summary .comment_author"));
static COMMENT_FLAIR: LazyLock<Selector> =
    LazyLock::new(|| sel(":scope > details > summary .author_flair"));
static COMMENT_CREATED: LazyLock<Selector> =
    LazyLock::new(|| sel(":scope > details > summary .created"));
static COMMENT_SCORE: LazyLock<Selector> =
    LazyLock::new(|| sel(":scope > .comment_left .comment_score"));
static COMMENT_BODY: LazyLock<Selector> = LazyLock::new(|| sel(":scope > details > .comment_body"));
static COMMENT_REPLIES: LazyLock<Selector> =
    LazyLock::new(|| sel(":scope > details > blockquote.replies > .comment"));
static COMMENT_MORE: LazyLock<Selector> =
    LazyLock::new(|| sel(":scope > details > blockquote.replies > a.deeper_replies"));

/// Parse a subreddit listing or search results page.
pub fn parse_listing(html: &str) -> Result<Listing> {
    let doc = Html::parse_document(html);
    check_error(&doc)?;
    let posts = doc
        .select(&POST)
        .filter(|el| !has_class(el, "highlighted"))
        .filter_map(parse_post)
        .collect();
    let after = doc
        .select(&NEXT)
        .find_map(|a| a.attr("href").and_then(query_param_after));
    let icon = doc
        .select(&SUB_ICON)
        .find_map(|img| img.attr("src"))
        .filter(|src| !src.is_empty())
        .map(str::to_string);
    Ok(Listing { posts, after, icon })
}

/// Parse a post page: the post, its full selftext, and the comment tree.
pub fn parse_post_page(html: &str) -> Result<PostPage> {
    let doc = Html::parse_document(html);
    check_error(&doc)?;
    let Some(el) = doc.select(&POST).find(|el| has_class(el, "highlighted")) else {
        bail!("Redlib returned no post on this page");
    };
    let Some(mut post) = parse_post(el) else {
        bail!("could not read the post");
    };
    if post.permalink.is_empty() {
        post.permalink = el
            .select(&PERMALINK)
            .find_map(|a| a.attr("href").filter(|h| h.contains("/comments/")))
            .unwrap_or_default()
            .to_string();
    }
    if post.id.is_empty() {
        post.id = id_from_permalink(&post.permalink).unwrap_or_default();
    }
    if let Some(count) = doc
        .select(&COMMENT_COUNT)
        .next()
        .and_then(|c| {
            c.text()
                .collect::<String>()
                .split_whitespace()
                .next()
                .map(str::to_string)
        })
        .and_then(|n| parse_count(&n))
    {
        post.comments = count;
    }
    let mut body = el
        .select(&BODY_MD)
        .next()
        .map(md_to_text)
        .unwrap_or_default();
    if let Some(poll) = el.select(&POLL).next() {
        let poll = poll_text(poll);
        body = if body.is_empty() {
            poll
        } else {
            format!("{body}\n\n{poll}")
        };
    }
    let comments = doc.select(&THREAD_COMMENT).map(parse_comment).collect();
    Ok(PostPage {
        post,
        body,
        comments,
    })
}

/// Parse a "More replies" page. Redlib roots it at the parent comment, so the
/// replies to graft are that comment's replies (or all roots if several).
pub fn parse_more_replies(html: &str) -> Result<Vec<Comment>> {
    let mut roots = parse_post_page(html)?.comments;
    Ok(match roots.len() {
        0 => Vec::new(),
        1 => roots.remove(0).replies,
        _ => roots,
    })
}

/// Redlib error pages carry `div#error`; post and listing pages title themselves
/// with the post or subreddit title, which may start with "Error" too.
fn check_error(doc: &Html) -> Result<()> {
    if let Some(h1) = doc.select(&ERROR_H1).next() {
        bail!(
            "Redlib error: {}",
            collapse_ws(&h1.text().collect::<String>())
        );
    }
    let title = doc
        .select(&PAGE_TITLE)
        .next()
        .map(|t| collapse_ws(&t.text().collect::<String>()))
        .unwrap_or_default();
    if title == "NSFW content gated" {
        bail!("this is NSFW; set show_nsfw = true in config.toml to see it");
    }
    Ok(())
}

/// Text from Reddit with the spoiler marker characters removed: they mean something
/// only where the parser adds them.
fn without_markers(text: &str) -> std::borrow::Cow<'_, str> {
    if text.contains([SPOILER_START, SPOILER_END]) {
        text.replace([SPOILER_START, SPOILER_END], "").into()
    } else {
        text.into()
    }
}

/// A poll as text: "N votes · time left", then one bullet per option.
fn poll_text(poll: ElementRef<'_>) -> String {
    let head: Vec<String> = poll
        .children()
        .filter_map(ElementRef::wrap)
        .filter(|e| e.value().name() == "span")
        .map(|s| collapse_ws(&s.text().collect::<String>()))
        .map(|s| s.trim_end_matches(',').to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let mut lines = vec![format!("Poll: {}", head.join(" · "))];
    for option in poll.select(&POLL_OPTION) {
        let text = option
            .select(&SPAN)
            .map(|s| collapse_ws(&s.text().collect::<String>()))
            .filter(|s| !s.is_empty())
            .last()
            .unwrap_or_default();
        lines.push(format!("• {text}"));
    }
    without_markers(&lines.join("\n")).into_owned()
}

fn parse_post(el: ElementRef<'_>) -> Option<Post> {
    let title_el = el.select(&TITLE).next()?;
    let flair = el
        .select(&FLAIR)
        .next()
        .map(|f| collapse_ws(&f.text().collect::<String>()))
        .filter(|f| !f.is_empty());
    // Listing titles are a link; post-page titles are bare text beside the flair.
    let mut permalink = String::new();
    let mut title = String::new();
    for child in title_el.children() {
        match child.value() {
            Node::Text(t) => title.push_str(t),
            Node::Element(e) if e.name() == "a" && !e.classes().any(|c| c == "post_flair") => {
                if let Some(href) = e.attr("href") {
                    permalink = href.to_string();
                }
                if let Some(a) = ElementRef::wrap(child) {
                    title.push_str(&a.text().collect::<String>());
                }
            }
            _ => {}
        }
    }
    let title = collapse_ws(&title);
    if title.is_empty() {
        return None;
    }
    let comments_link = el.select(&COMMENTS_LINK).next();
    if permalink.is_empty()
        && let Some(href) = comments_link.and_then(|a| a.attr("href"))
    {
        permalink = href.to_string();
    }
    let id = el
        .attr("id")
        .map(str::to_string)
        .or_else(|| id_from_permalink(&permalink))
        .unwrap_or_default();
    let created_el = el.select(&CREATED).next();
    Some(Post {
        id,
        subreddit: text_of(el, &SUBREDDIT)
            .map(|s| s.trim_start_matches("r/").to_string())
            .unwrap_or_default(),
        title,
        author: text_of(el, &AUTHOR)
            .map(|s| s.trim_start_matches("u/").to_string())
            .unwrap_or_default(),
        flair,
        score: el
            .select(&SCORE)
            .next()
            .and_then(|s| s.attr("title"))
            .and_then(|t| t.trim().parse().ok()),
        comments: comments_link
            .and_then(|a| a.attr("title"))
            .and_then(|t| t.split_whitespace().next())
            .and_then(parse_count)
            .unwrap_or(0),
        created: created_el
            .and_then(|c| c.attr("title"))
            .and_then(parse_redlib_time),
        age: created_el
            .map(|c| collapse_ws(&c.text().collect::<String>()))
            .unwrap_or_default(),
        permalink,
        media: parse_media(el),
        stickied: has_class(&el, "stickied"),
        nsfw: el.select(&NSFW).next().is_some(),
        spoiler: el.select(&SPOILER_TAG).next().is_some(),
    })
}

fn parse_media(el: ElementRef<'_>) -> Media {
    let gallery: Vec<String> = el
        .select(&GALLERY_LINK)
        .filter_map(|a| a.attr("href"))
        .map(str::to_string)
        .collect();
    if !gallery.is_empty() {
        return Media::Gallery {
            images: gallery,
            thumb: None,
        };
    }
    if let Some(a) = el.select(&MEDIA_IMAGE).next()
        && let Some(href) = a.attr("href")
    {
        return Media::Image {
            url: href.to_string(),
        };
    }
    if let Some(video) = el.select(&MEDIA_VIDEO).next() {
        let source = video
            .select(&VIDEO_SOURCE)
            .filter_map(|s| s.attr("src"))
            .find(|s| !s.is_empty());
        let url = source
            .or_else(|| video.attr("src").filter(|s| !s.is_empty()))
            .map(str::to_string);
        let poster = video
            .attr("poster")
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        return Media::Video { url, poster };
    }
    if let Some(thumb) = el.select(&THUMBNAIL).next() {
        let href = thumb.attr("href").unwrap_or_default().to_string();
        let thumb_img = thumb
            .select(&IMG)
            .find_map(|i| i.attr("src"))
            .map(str::to_string);
        let label = thumb
            .select(&SPAN)
            .last()
            .map(|s| collapse_ws(&s.text().collect::<String>()))
            .unwrap_or_default();
        if label == "gallery" {
            // Listings only show a thumbnail; the post page lists the images.
            return Media::Gallery {
                images: Vec::new(),
                thumb: thumb_img,
            };
        }
        if href.starts_with("http") {
            return Media::Link {
                url: href,
                thumb: thumb_img,
            };
        }
    }
    if let Some(link) = el.select(&POST_LINK).next().and_then(|a| a.attr("href"))
        && link.starts_with("http")
    {
        return Media::Link {
            url: link.to_string(),
            thumb: None,
        };
    }
    Media::None
}

fn parse_comment(el: ElementRef<'_>) -> Comment {
    let author_el = el.select(&COMMENT_AUTHOR).next();
    let author = author_el
        .map(|a| collapse_ws(&a.text().collect::<String>()))
        .map(|a| a.trim_start_matches("u/").to_string())
        .unwrap_or_else(|| "[deleted]".into());
    let more = el.select(&COMMENT_MORE).next().and_then(|a| {
        let path = a.attr("href")?.to_string();
        let text = a.text().collect::<String>();
        let count = text
            .split(['(', ')'])
            .nth(1)
            .and_then(|n| n.trim().parse().ok());
        Some(MoreReplies { path, count })
    });
    Comment {
        id: el.attr("id").unwrap_or_default().to_string(),
        author,
        flair: el
            .select(&COMMENT_FLAIR)
            .next()
            .map(|f| collapse_ws(&f.text().collect::<String>()))
            .filter(|f| !f.is_empty()),
        is_op: author_el.is_some_and(|a| has_class(&a, "op")),
        is_mod: author_el.is_some_and(|a| has_class(&a, "moderator")),
        score: el
            .select(&COMMENT_SCORE)
            .next()
            .and_then(|s| s.attr("title"))
            .and_then(|t| t.trim().parse().ok()),
        age: el
            .select(&COMMENT_CREATED)
            .next()
            .map(|c| collapse_ws(&c.text().collect::<String>()))
            .unwrap_or_default(),
        body: el
            .select(&COMMENT_BODY)
            .next()
            .map(md_to_text)
            .unwrap_or_default(),
        replies: el.select(&COMMENT_REPLIES).map(parse_comment).collect(),
        more,
    }
}

/// Redlib's absolute timestamps look like `Oct 13 2026, 12:20:20 UTC`.
pub fn parse_redlib_time(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim().trim_end_matches("UTC").trim();
    NaiveDateTime::parse_from_str(s, "%b %d %Y, %H:%M:%S")
        .ok()
        .map(|n| n.and_utc())
}

fn query_param_after(href: &str) -> Option<String> {
    let query = href.split_once('?').map_or(href, |(_, q)| q);
    query
        .split('&')
        .map(str::trim)
        .find_map(|kv| kv.strip_prefix("after="))
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// Counts Redlib shows as `42`, `1,234`, `1.1k` or `2.3m`.
pub fn parse_count(text: &str) -> Option<u64> {
    let t = text.trim().replace(',', "").to_lowercase();
    let (number, scale) = match t.strip_suffix('k') {
        Some(n) => (n.to_string(), 1_000.0),
        None => match t.strip_suffix('m') {
            Some(n) => (n.to_string(), 1_000_000.0),
            None => (t.clone(), 1.0),
        },
    };
    let value: f64 = number.parse().ok()?;
    (value >= 0.0).then(|| (value * scale).round() as u64)
}

fn id_from_permalink(permalink: &str) -> Option<String> {
    let mut parts = permalink.split('/');
    parts.find(|p| *p == "comments")?;
    parts.next().filter(|p| !p.is_empty()).map(str::to_string)
}

fn has_class(el: &ElementRef<'_>, class: &str) -> bool {
    el.value().classes().any(|c| c == class)
}

fn text_of(el: ElementRef<'_>, selector: &Selector) -> Option<String> {
    el.select(selector)
        .next()
        .map(|e| collapse_ws(&e.text().collect::<String>()))
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
pub fn md_to_text(el: ElementRef<'_>) -> String {
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
            let before = out.len();
            render_children(el, out, pre);
            let text = out[before..].trim().to_string();
            // The URL is page text too: it must not carry a spoiler marker.
            if let Some(href) = el.attr("href").map(without_markers)
                && href.starts_with("http")
                && text != *href
                && !text.is_empty()
            {
                out.push_str(&format!(" <{href}>"));
            }
        }
        "img" => {
            if let Some(alt) = el.attr("alt").filter(|a| !a.is_empty()) {
                out.push_str(&format!("[{}]", without_markers(alt)));
            }
        }
        "script" | "style" => {}
        _ => render_children(el, out, pre),
    }
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
    fn code_block_tabs_become_spaces() {
        assert_eq!(expand_tabs("\tfmt.Println(1)"), "    fmt.Println(1)");
        assert_eq!(expand_tabs("ab\tc"), "ab  c");
    }

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!(
            "{}/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    #[test]
    fn listing_reads_every_post_and_the_next_page_cursor() {
        let listing = parse_listing(&fixture("sub.html")).unwrap();

        assert_eq!(listing.posts.len(), 25);
        assert_eq!(listing.after.as_deref(), Some("t3_1x0ms3h"));
        let first = &listing.posts[0];
        assert_eq!(first.id, "1wglruo");
        assert_eq!(first.subreddit, "neovim");
        assert_eq!(first.author, "AutoModerator");
        assert_eq!(first.title, "Monthly Dotfile Review Thread");
        assert_eq!(first.flair.as_deref(), Some("Dotfile Review"));
        assert_eq!(first.score, Some(20));
        assert_eq!(first.comments, 12);
        assert_eq!(
            first.permalink,
            "/r/neovim/comments/1wglruo/monthly_dotfile_review_thread/"
        );
        assert!(first.stickied);
        assert_eq!(
            first.created,
            parse_redlib_time("Sep 15 2026, 01:00:35 UTC"),
        );
        assert!(first.age.ends_with("ago"));
        assert_eq!(
            listing.icon.as_deref(),
            Some(
                "/style/t5_30kix/styles/communityIcon_n2hvyn96zwk81.png?width=256&s=b2cd2f9dcc0cfbf94eee5a640f115dfca33dee1d"
            )
        );
    }

    #[test]
    fn listing_classifies_media() {
        let images = parse_listing(&fixture("img.html")).unwrap();
        assert!(matches!(&images.posts[0].media, Media::Image { url } if url.starts_with("/img/")));

        let videos = parse_listing(&fixture("vid.html")).unwrap();
        let video = videos
            .posts
            .iter()
            .find(|p| matches!(p.media, Media::Video { .. }))
            .unwrap();
        let Media::Video { url, poster } = &video.media else {
            unreachable!()
        };
        assert!(url.as_deref().unwrap().contains("format=mp4"));
        assert!(poster.is_some());

        let maps = parse_listing(&fixture("maps.html")).unwrap();
        let gallery = maps
            .posts
            .iter()
            .find(|p| matches!(p.media, Media::Gallery { .. }))
            .unwrap();
        let Media::Gallery { images, thumb } = &gallery.media else {
            unreachable!()
        };
        assert!(images.is_empty());
        assert!(
            thumb.as_deref().unwrap().starts_with("/preview/"),
            "{thumb:?}"
        );
        assert_eq!(gallery.media.list_thumb(), thumb.as_deref());

        let neovim = parse_listing(&fixture("sub.html")).unwrap();
        let link = neovim.posts.iter().find(|p| p.id == "1x4v4sn").unwrap();
        assert!(
            matches!(&link.media, Media::Link { url, thumb: Some(_) } if url.starts_with("https://nvim-mini.org/"))
        );
    }

    #[test]
    fn search_results_parse_like_a_listing() {
        let listing = parse_listing(&fixture("search.html")).unwrap();

        assert!(!listing.posts.is_empty());
        assert_eq!(listing.after.as_deref(), Some("t3_1vlgboo"));
        // Search pages carry no subreddit icon.
        assert_eq!(listing.icon, None);
    }

    #[test]
    fn missing_subreddit_is_an_empty_listing() {
        let listing = parse_listing(&fixture("missing.html")).unwrap();

        assert!(listing.posts.is_empty());
        assert_eq!(listing.after, None);
    }

    #[test]
    fn redlib_error_page_is_an_error() {
        let html = "<html><head><title>Error: Reddit error 404 - Redlib</title></head>\
            <body><div id=\"error\"><h1>Reddit error 404 \"null\": \"Not Found\" | /x</h1></div></body></html>";

        let err = parse_listing(html).unwrap_err().to_string();

        assert!(err.starts_with("Redlib error"), "{err}");
        assert!(err.contains("404"), "{err}");
    }

    #[test]
    fn a_post_titled_error_is_not_an_error_page() {
        let html = r#"<html><head><title>Error with Neovim and Tree Sitter - r/neovim</title></head><body>
            <div class="post highlighted" id="abc"><p class="post_header"><a class="post_subreddit">r/neovim</a></p>
            <h1 class="post_title">Error with Neovim and Tree Sitter</h1>
            <div class="post_poll"><span>61 votes,</span> <span>6d left</span>
              <div class="poll_option"><div class="poll_chart"></div><span></span><span>Yes</span></div>
              <div class="poll_option"><div class="poll_chart"></div><span></span><span>No</span></div>
            </div></div></body></html>"#;

        let page = parse_post_page(html).unwrap();

        assert_eq!(page.post.title, "Error with Neovim and Tree Sitter");
        assert_eq!(page.body, "Poll: 61 votes · 6d left\n• Yes\n• No");
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
    fn spoiler_tag_and_inline_spoiler_text_are_marked() {
        let html = r#"<html><head><title>x</title></head><body>
            <div class="post highlighted" id="abc"><p class="post_header"><a class="post_subreddit">r/tv</a></p>
            <h1 class="post_title">Finale thoughts <small class="spoiler">Spoiler</small></h1>
            <div class="post_body"><div class="md"><p>He was <span class="md-spoiler-text">the killer</span> all along.</p></div></div>
            </div></body></html>"#;

        let page = parse_post_page(html).unwrap();

        assert!(page.post.spoiler);
        assert_eq!(
            page.post.title, "Finale thoughts",
            "the tag is not part of the title"
        );
        assert_eq!(
            page.body,
            format!("He was {SPOILER_START}the killer{SPOILER_END} all along.")
        );
    }

    #[test]
    fn nsfw_gate_says_how_to_see_it() {
        let html = "<html><head><title>NSFW content gated</title></head><body></body></html>";

        let err = parse_listing(html).unwrap_err().to_string();

        assert!(err.contains("show_nsfw"), "{err}");
    }

    #[test]
    fn post_page_reads_the_post_and_a_nested_comment_tree() {
        let page = parse_post_page(&fixture("post.html")).unwrap();

        assert_eq!(page.post.title, "Five year anniversary of ‘mini.nvim’");
        assert_eq!(page.post.flair.as_deref(), Some("Plugin"));
        assert_eq!(page.post.id, "1x4v4sn");
        assert_eq!(page.post.comments, 13);
        let top = &page.comments[0];
        assert_eq!(top.id, "pggup53");
        assert_eq!(top.author, "echasnovski");
        assert!(top.is_op);
        assert_eq!(top.flair.as_deref(), Some("Plugin author"));
        assert_eq!(top.score, Some(47));
        assert!(top.body.starts_with("TL;DR:"));
        assert!(top.body.contains("• Today is five years"));
        assert_eq!(top.replies[0].id, "pggxkm7");
        assert_eq!(top.replies[0].replies[0].id, "pgh2tkd");
        let total: usize = page.comments.iter().map(count_tree).sum();
        assert_eq!(total, 13);
    }

    #[test]
    fn deep_threads_expose_more_replies_links() {
        let page = parse_post_page(&fixture("bigpost.html")).unwrap();

        let mut more = Vec::new();
        page.comments
            .iter()
            .for_each(|c| collect_more(c, &mut more));
        assert_eq!(more.len(), 69);
        assert!(more.iter().all(|m| m.path.contains("/comments/")));
        assert!(more.iter().any(|m| m.count == Some(1)));
    }

    #[test]
    fn post_page_media_variants() {
        let gallery = parse_post_page(&fixture("gallery.html")).unwrap();
        assert!(matches!(&gallery.post.media, Media::Gallery { images, .. } if images.len() > 1));

        let video = parse_post_page(&fixture("videopost.html")).unwrap();
        assert!(
            matches!(&video.post.media, Media::Video { url: Some(u), .. } if u.starts_with("/hls/"))
        );
    }

    #[test]
    fn more_replies_page_yields_the_parent_comments_replies() {
        // thread.html is the page behind `→ More replies` on pgh5me8 in bigpost.html.
        let replies = parse_more_replies(&fixture("thread.html")).unwrap();

        let ids: Vec<&str> = replies.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["pghb1om"]);
    }

    #[test]
    fn counts_accept_redlib_compact_forms() {
        assert_eq!(parse_count("42"), Some(42));
        assert_eq!(parse_count("1,234"), Some(1234));
        assert_eq!(parse_count("1.1k"), Some(1100));
        assert_eq!(parse_count("2.3M"), Some(2_300_000));
        assert_eq!(parse_count("comments"), None);
    }

    #[test]
    fn markdown_keeps_structure() {
        let html = Html::parse_fragment(
            r#"<div class="md"><p>One <a href="https://x.y/z">link</a> and <code>c</code>.</p><blockquote><p>quoted</p></blockquote><ul><li>a</li><li>b</li></ul><pre><code>fn x() {
    1
}</code></pre></div>"#,
        );
        let md = html.select(&sel("div.md")).next().unwrap();

        let text = md_to_text(md);

        assert_eq!(
            text,
            "One link <https://x.y/z> and `c`.\n\n│ quoted\n\n• a\n• b\n\n    fn x() {\n        1\n    }"
        );
    }

    fn count_tree(c: &Comment) -> usize {
        1 + c.replies.iter().map(count_tree).sum::<usize>()
    }

    fn collect_more(c: &Comment, out: &mut Vec<MoreReplies>) {
        if let Some(m) = &c.more {
            out.push(m.clone());
        }
        c.replies.iter().for_each(|r| collect_more(r, out));
    }
}
