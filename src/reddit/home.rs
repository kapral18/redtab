//! Stream ranked browser Home cards without separate post lookups.

use std::{cell::Cell, collections::HashSet};

use anyhow::{Result, bail};
use html5ever::{
    tendril::StrTendril,
    tokenizer::{BufferQueue, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer},
    tree_builder::{TreeBuilder, TreeSink},
};
use scraper::{ElementRef, Html, HtmlTreeSink, Selector};

use crate::model::Post;

mod post;
use url::Url;

pub(super) const PATH: &str = "/svc/shreddit/feeds/home-feed";
pub(super) const ACCEPT: &str = "text/vnd.reddit.partial+html";

pub(super) struct Page {
    pub posts: Vec<Post>,
    pub after: Option<String>,
}

pub(super) fn url(api: &Url, cursor: &str) -> Result<Url> {
    let url = api
        .join(cursor)
        .map_err(|_| anyhow::anyhow!("invalid Home cursor"))?;
    if url.origin() != api.origin()
        || url.path() != PATH
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        bail!("invalid Home cursor origin or path");
    }
    Ok(url)
}

/// Publish only closed cards; the HTML tokenizer keeps quoted tags, comments and
/// script contents from masquerading as card boundaries.
pub(super) fn stream(
    mut reader: impl std::io::Read,
    api: &Url,
    progress: &mut dyn FnMut(&Post),
) -> Result<Page> {
    let sink = Cards {
        tree: TreeBuilder::new(HtmlTreeSink::new(Html::new_document()), Default::default()),
        depth: Cell::new(0),
        closed: Cell::new(0),
    };
    let tokenizer = Tokenizer::new(sink, Default::default());
    let input = BufferQueue::default();
    let mut pending = Vec::new();
    let mut chunk = [0; 8192];
    let mut page = Page {
        posts: Vec::new(),
        after: None,
    };
    let mut processed = 0;
    let mut seen = HashSet::new();
    loop {
        let count = reader.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        pending.extend_from_slice(&chunk[..count]);
        let valid = match std::str::from_utf8(&pending) {
            Ok(text) => text.len(),
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            Err(_) => bail!("Reddit returned invalid Home HTML"),
        };
        input.push_back(StrTendril::from_slice(std::str::from_utf8(
            &pending[..valid],
        )?));
        while !matches!(tokenizer.feed(&input), html5ever::TokenizerResult::Done) {}
        pending.drain(..valid);
        publish(
            &tokenizer.sink,
            &mut processed,
            &mut seen,
            &mut page,
            progress,
        )?;
    }
    if !pending.is_empty() || tokenizer.sink.depth.get() != 0 {
        bail!("Reddit returned incomplete Home HTML");
    }
    tokenizer.end();
    publish(
        &tokenizer.sink,
        &mut processed,
        &mut seen,
        &mut page,
        progress,
    )?;
    let document = tokenizer.sink.tree.sink.0.borrow();
    let next_selector = Selector::parse("#feed-next-page-partial").expect("static selector");
    let next = document.select(&next_selector).next();
    if processed == 0 && next.is_none() {
        bail!("Reddit returned an unrecognized Home feed; try refreshing or signing in again");
    }
    page.after = next
        .and_then(|el| el.value().attr("src"))
        .map(|cursor| url(api, cursor).map(|u| u.to_string()))
        .transpose()?;
    Ok(page)
}

struct Cards {
    tree: TreeBuilder<<HtmlTreeSink as TreeSink>::Handle, HtmlTreeSink>,
    depth: Cell<usize>,
    closed: Cell<usize>,
}

impl TokenSink for Cards {
    type Handle = <HtmlTreeSink as TreeSink>::Handle;

    fn process_token(&self, token: Token, line: u64) -> TokenSinkResult<Self::Handle> {
        if let Token::TagToken(tag) = &token
            && tag.name.as_ref() == "shreddit-post"
        {
            match tag.kind {
                TagKind::StartTag => self.depth.set(self.depth.get() + 1),
                TagKind::EndTag => {
                    if self.depth.get() == 1 {
                        self.closed.set(self.closed.get() + 1);
                    }
                    self.depth.set(self.depth.get().saturating_sub(1));
                }
            }
        }
        self.tree.process_token(token, line)
    }

    fn end(&self) {
        self.tree.end();
    }

    fn adjusted_current_node_present_but_not_in_html_namespace(&self) -> bool {
        self.tree
            .adjusted_current_node_present_but_not_in_html_namespace()
    }
}

fn publish(
    sink: &Cards,
    processed: &mut usize,
    seen: &mut HashSet<String>,
    page: &mut Page,
    progress: &mut dyn FnMut(&Post),
) -> Result<()> {
    if *processed == sink.closed.get() {
        return Ok(());
    }
    let document = sink.tree.sink.0.borrow();
    let selector = Selector::parse("shreddit-post").expect("static selector");
    for element in document
        .select(&selector)
        .filter(|el| {
            !el.ancestors()
                .filter_map(ElementRef::wrap)
                .any(|parent| parent.value().name() == "shreddit-post")
        })
        .take(sink.closed.get())
        .skip(*processed)
    {
        *processed += 1;
        if let Some(post) = post::parse(element)?
            && seen.insert(post.id.clone())
        {
            progress(&post);
            page.posts.push(post);
        }
    }
    Ok(())
}

#[cfg(test)]
fn parse(html: &str, api: &Url) -> Result<Page> {
    stream(html.as_bytes(), api, &mut |_| {})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Media;
    use crate::reddit::tests::native_post;

    fn card(attributes: &str, content: &str) -> String {
        native_post("a", "s")
            .replace(" post-type='text'", attributes)
            .replace("</shreddit-post>", &format!("{content}</shreddit-post>"))
    }

    fn parsed(attributes: &str, content: &str) -> Post {
        parse(
            &card(attributes, content),
            &Url::parse("https://www.reddit.com/").unwrap(),
        )
        .unwrap()
        .posts
        .remove(0)
    }

    #[test]
    fn when_native_video_or_gif_loads_it_should_preserve_playback_and_poster() {
        for (flag, gif) in [("", false), ("gif", true)] {
            let post = parsed(
                " post-type='video'",
                &format!(
                    r#"<shreddit-player {flag} src="https://v.redd.it/a/playlist.m3u8" poster="https://external-preview.redd.it/a.jpg" packaged-media-json='{{"playbackMp4s":{{"permutations":[{{"source":{{"dimensions":{{"width":1920,"height":1080}}}}}}]}}}}'></shreddit-player>"#
                ),
            );
            assert_eq!(
                post.media,
                Media::Video {
                    url: Some("https://v.redd.it/a/playlist.m3u8".into()),
                    poster: Some("https://external-preview.redd.it/a.jpg".into()),
                    gif
                }
            );
            assert_eq!(post.preview_size, Some((1920, 1080)));
        }
    }

    #[test]
    fn when_a_native_gallery_loads_it_should_keep_originals_and_bounded_lazy_previews() {
        let post = parsed(
            " post-type='gallery'",
            r#"<gallery-carousel>
            <img role="presentation" src="https://preview.redd.it/background.jpg" width="9000" height="6000">
            <img src="https://preview.redd.it/a.jpg?width=640" width="9000" height="6000" srcset="https://preview.redd.it/a.jpg?width=1920 1920w">
            <zoomable-img><img src="https://i.redd.it/a.jpg"></zoomable-img>
            <img data-lazy-src="https://preview.redd.it/b.jpg?width=640" width="800" height="600">
            <zoomable-img><img src="https://i.redd.it/b.jpg"></zoomable-img>
        </gallery-carousel>"#,
        );
        assert_eq!(
            post.media,
            Media::Gallery {
                images: vec![
                    "https://i.redd.it/a.jpg".into(),
                    "https://i.redd.it/b.jpg".into()
                ],
                previews: vec![
                    Some("https://preview.redd.it/a.jpg?width=1920".into()),
                    None
                ],
                thumb: Some("https://preview.redd.it/a.jpg?width=1920".into()),
            }
        );
        assert_eq!(post.preview_size, Some((1920, 1280)));
    }

    #[test]
    fn when_native_links_have_avatars_it_should_use_only_the_link_thumbnail() {
        let post = parsed(
            " post-type='link' content-href='https://youtu.be/abcdefghijk'",
            r#"<img src="https://styles.redditmedia.com/avatar.png"><div slot="thumbnail"><img src="https://external-preview.redd.it/link.jpg"></div>"#,
        );
        assert_eq!(
            post.media,
            Media::Link {
                url: "https://youtu.be/abcdefghijk".into(),
                thumb: Some("https://external-preview.redd.it/link.jpg".into())
            }
        );
        assert_eq!(
            post.media.youtube_url(),
            Some("https://youtu.be/abcdefghijk")
        );
        assert_eq!(
            parsed(
                " post-type='text'",
                "<img src='https://styles.redditmedia.com/avatar.png'>"
            )
            .media,
            Media::None
        );
    }

    #[test]
    fn when_native_flags_and_flair_are_present_it_should_keep_them_in_the_model() {
        let post = parsed(
            " nsfw spoiler post-type='text'",
            "<shreddit-post-flair>News &amp; views</shreddit-post-flair><span class='stickied-status'></span>",
        );
        assert!(post.nsfw && post.spoiler && post.stickied);
        assert_eq!(post.flair.as_deref(), Some("News & views"));
        let hidden = parsed(
            " is-not-brand-safe post-type='text'",
            "<span class='stickied-status hidden'></span>",
        );
        assert!(!hidden.nsfw && !hidden.spoiler && !hidden.stickied);
    }

    #[test]
    fn when_native_media_points_outside_the_allowlist_it_should_never_become_playable() {
        let post = parsed(
            " post-type='video'",
            "<shreddit-player src='https://other.example/video.m3u8' poster='https://other.example/poster.png'></shreddit-player>",
        );
        assert_eq!(
            post.media,
            Media::Video {
                url: None,
                poster: None,
                gif: false
            }
        );
    }

    #[test]
    fn when_html_chunks_split_unicode_and_tags_it_should_ignore_false_card_boundaries() {
        struct Bytewise<'a>(&'a [u8]);
        impl std::io::Read for Bytewise<'_> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                std::io::Read::read(&mut self.0, &mut buf[..1])
            }
        }
        let html = card(" post-type='text' data-test='</shreddit-post>'", "<!-- </shreddit-post> --><script>const fake='</shreddit-post>';</script><span>café</span>")
            .replace("post-title='a'","post-title='café &amp; чай'");
        let mut emitted = Vec::new();
        let page = stream(
            Bytewise(html.as_bytes()),
            &Url::parse("https://www.reddit.com/").unwrap(),
            &mut |post| emitted.push(post.clone()),
        )
        .unwrap();
        assert_eq!(page.posts.len(), 1);
        assert_eq!(page.posts[0].title, "café & чай");
        assert_eq!(emitted, page.posts);
    }

    #[test]
    fn when_a_card_is_truncated_or_utf8_is_invalid_it_should_fail_without_publishing_it() {
        let api = Url::parse("https://www.reddit.com/").unwrap();
        let html = card(" post-type='text'", "");
        let mut emitted = Vec::new();
        assert!(
            stream(
                html.trim_end_matches("</shreddit-post>").as_bytes(),
                &api,
                &mut |post| emitted.push(post.clone())
            )
            .is_err()
        );
        assert!(stream(&b"\xff"[..], &api, &mut |_| {}).is_err());
        assert!(emitted.is_empty());
    }

    #[test]
    fn when_home_markup_is_not_a_feed_it_should_fail_instead_of_returning_empty() {
        let api = Url::parse("https://www.reddit.com/").unwrap();
        assert!(parse("<html><body>Sign in</body></html>", &api).is_err());
        assert!(parse("", &api).is_err());
        assert!(parse("<shreddit-post id='bad/id'></shreddit-post>", &api).is_err());
    }

    #[test]
    fn when_a_cursor_leaves_home_it_should_fail_before_authentication() {
        let api = Url::parse("https://www.reddit.com/").unwrap();
        for cursor in [
            "https://other.example/svc/shreddit/feeds/home-feed",
            "/api/delete",
            "https://user:pass@www.reddit.com/svc/shreddit/feeds/home-feed",
            "/svc/shreddit/feeds/home-feed#fragment",
        ] {
            assert!(url(&api, cursor).is_err(), "{cursor}");
        }
    }

    #[test]
    fn when_a_feed_contains_ads_and_duplicates_it_should_keep_only_unique_organic_ids() {
        let card = crate::reddit::tests::native_post("a", "s");
        let page = parse(&format!("{card}{card}<shreddit-post id='t3_ad' is-ad></shreddit-post><shreddit-ad-post id='t3_ad2'></shreddit-ad-post>"), &Url::parse("https://www.reddit.com/").unwrap()).unwrap();
        assert_eq!(
            page.posts.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            ["a"]
        );
        assert!(page.after.is_none());
    }

    #[test]
    fn when_the_feed_end_marker_is_empty_it_should_complete_without_more_posts() {
        let page = parse(
            "<faceplate-partial id='feed-next-page-partial'></faceplate-partial>",
            &Url::parse("https://www.reddit.com/").unwrap(),
        )
        .unwrap();
        assert!(page.posts.is_empty() && page.after.is_none());
    }
}
