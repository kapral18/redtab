/// Wrap inline `>!spoiler!<` text so the view can mask it. Unicode noncharacters;
/// the parser strips any that arrive in Reddit text.
pub const SPOILER_START: char = '\u{FDD0}';

pub const SPOILER_END: char = '\u{FDD1}';

/// Starts a line of post or comment text that stands for a picture, its URL after it. A
/// noncharacter too, stripped from Reddit text like the spoiler marks.
pub const PICTURE: char = '\u{FDD2}';

pub const TEXT_BOLD: char = '\u{FDD3}';

pub const TEXT_ITALIC: char = '\u{FDD4}';

pub const TEXT_STRIKE: char = '\u{FDD5}';

pub const TEXT_CODE: char = '\u{FDD6}';

pub const TEXT_HEADING: char = '\u{FDD7}';

pub const TEXT_LINK: char = '\u{FDD8}';

pub const TEXT_END: char = '\u{FDD9}';

pub const LINK_TARGET: char = '\u{FDDA}';

pub const LINK_TARGET_END: char = '\u{FDDB}';

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BodyLink {
    pub label: String,
    pub url: String,
}

pub fn without_link_targets(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains(LINK_TARGET) {
        return text.into();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(LINK_TARGET) {
        out.push_str(&rest[..at]);
        let Some(end) = rest[at..].find(LINK_TARGET_END) else {
            return out.into();
        };
        rest = &rest[at + end + LINK_TARGET_END.len_utf8()..];
    }
    out.push_str(rest);
    out.into()
}

pub fn body_links(text: &str, reveal_spoilers: bool) -> Vec<BodyLink> {
    let mut links = Vec::new();
    let mut formats = Vec::new();
    let mut current: Option<(BodyLink, bool)> = None;
    let mut spoiler = false;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            SPOILER_START => {
                spoiler = true;
                if let Some((_, hidden)) = &mut current {
                    *hidden = true;
                }
            }
            SPOILER_END => spoiler = false,
            LINK_TARGET => {
                let url: String = chars
                    .by_ref()
                    .take_while(|c| *c != LINK_TARGET_END)
                    .collect();
                if let Some((link, _)) = &mut current {
                    link.url = url;
                }
            }
            TEXT_LINK => {
                formats.push(c);
                current = Some((
                    BodyLink {
                        label: String::new(),
                        url: String::new(),
                    },
                    spoiler,
                ));
            }
            TEXT_END => {
                if formats.pop() == Some(TEXT_LINK)
                    && let Some((mut link, hidden)) = current.take()
                    && (!hidden || reveal_spoilers)
                    && url::Url::parse(&link.url).is_ok_and(|url| {
                        matches!(url.scheme(), "http" | "https") && url.host_str().is_some()
                    })
                {
                    link.label = if link.label.contains(PICTURE) {
                        "[image]".into()
                    } else {
                        link.label.split_whitespace().collect::<Vec<_>>().join(" ")
                    };
                    if !link.label.is_empty()
                        && !links
                            .iter()
                            .any(|existing: &BodyLink| existing.url == link.url)
                    {
                        links.push(link);
                    }
                }
            }
            _ if text_format(c) => formats.push(c),
            _ => {
                if let Some((link, _)) = &mut current
                    && (!c.is_control() || c.is_whitespace())
                {
                    link.label.push(c);
                }
            }
        }
    }
    links
}

pub fn text_format(c: char) -> bool {
    (TEXT_BOLD..=TEXT_END).contains(&c)
}

pub fn unformatted(text: &str) -> std::borrow::Cow<'_, str> {
    let text = without_link_targets(text);
    if text.contains(text_format) {
        text.chars()
            .filter(|c| !text_format(*c))
            .collect::<String>()
            .into()
    } else {
        text
    }
}
