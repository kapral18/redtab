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
        plain.contains("3. third") && plain.contains("  • nested") && plain.contains("4. fourth"),
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
    let html =
        "<p><a href=\"https://preview.redd.it/a.png?width=320&amp;format=png\">caption λ</a></p>";
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
        format!("│ {SPOILER_START}line one\n{SPOILER_END}│ {SPOILER_START}line two{SPOILER_END}")
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
