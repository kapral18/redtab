//! Render Reddit body_html, preserving spoilers, code, quotes and inline pictures.
use crate::model::{
    LINK_TARGET, LINK_TARGET_END, PICTURE, SPOILER_END, SPOILER_START, TEXT_BOLD, TEXT_CODE,
    TEXT_END, TEXT_HEADING, TEXT_ITALIC, TEXT_LINK, TEXT_STRIKE, text_format, unformatted,
};
use scraper::{ElementRef, Html, Node};

mod html;
mod layout;
mod markdown;
mod markers;

use html::*;
use layout::*;
use markdown::*;
pub(in crate::reddit) use markers::*;

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

#[cfg(test)]
mod tests;
