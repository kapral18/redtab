//! Reddit JSON to the reader's persisted models. Bodies come from Reddit's rendered HTML
//! when present, else the raw text; see `text`.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::text;
use crate::model::{Comment, Listing, Media, MoreReplies, Post, PostPage, ReplyPage, THREAD_ROOT};

#[derive(Deserialize)]
struct Envelope {
    kind: String,
    data: ListData,
}

#[derive(Deserialize)]
struct ListData {
    children: Vec<Thing>,
    #[serde(default)]
    after: Option<String>,
}

#[derive(Deserialize)]
struct Thing {
    kind: String,
    data: Value,
}

fn listing(value: Value) -> Result<ListData> {
    let envelope: Envelope = serde_json::from_value(value).context("invalid Reddit listing")?;
    if envelope.kind != "Listing" {
        bail!("expected Reddit Listing");
    }
    Ok(envelope.data)
}

#[cfg(test)]
pub fn parse_listing(json: &str) -> Result<Listing> {
    parse_listing_value(serde_json::from_str(json)?)
}

pub(super) fn parse_listing_value(value: Value) -> Result<Listing> {
    let data = listing(value)?;
    let mut posts = Vec::new();
    for thing in data.children {
        if thing.kind != "t3" {
            continue;
        }
        posts.push(post(thing.data)?.0);
    }
    Ok(Listing {
        posts,
        after: data.after.filter(|s| !s.is_empty()),
    })
}

pub(super) fn parse_subscriptions(value: Value) -> Result<(Vec<String>, Option<String>)> {
    let data = listing(value)?;
    let mut names = Vec::new();
    for thing in data.children {
        let name = thing.data["display_name"]
            .as_str()
            .filter(|name| {
                !name.is_empty()
                    && name.len() <= 50
                    && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
            })
            .context("invalid Reddit subscription name")?;
        if thing.kind != "t5" {
            bail!("expected Reddit subreddit in subscriptions");
        }
        names.push(name.to_string());
    }
    Ok((names, data.after.filter(|s| !s.is_empty())))
}

#[derive(Deserialize)]
struct PostData {
    id: String,
    title: String,
    subreddit: String,
    permalink: String,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    link_flair_text: Option<String>,
    #[serde(default)]
    score: Option<i64>,
    #[serde(default)]
    hide_score: bool,
    #[serde(default)]
    num_comments: u64,
    #[serde(default)]
    created_utc: Option<f64>,
    #[serde(default)]
    stickied: bool,
    #[serde(default)]
    over_18: bool,
    #[serde(default)]
    spoiler: bool,
    #[serde(default)]
    selftext: String,
    #[serde(default)]
    selftext_html: Option<String>,
}

fn clean(s: &str) -> String {
    text::without_markers(s).into_owned()
}

fn created(seconds: Option<f64>) -> Option<DateTime<Utc>> {
    seconds
        .filter(|n| n.is_finite())
        .and_then(|n| DateTime::from_timestamp(n as i64, 0))
}

fn optional(s: Option<String>) -> Option<String> {
    s.map(|s| clean(&s)).filter(|s| !s.is_empty())
}

fn post(value: Value) -> Result<(Post, String)> {
    let data = PostData::deserialize(&value).context("invalid Reddit post")?;
    if !id_valid(&data.id) || post_id(&data.permalink) != Some(data.id.as_str()) {
        bail!("invalid Reddit post identity");
    }
    let mut body = text::body(
        data.selftext_html.as_deref().unwrap_or_default(),
        &data.selftext,
    );
    if let Some(poll) = value.get("poll_data") {
        if !body.is_empty() {
            body.push_str("\n\n");
        }
        body.push_str(&format!(
            "Poll: {} votes",
            poll["total_vote_count"].as_u64().unwrap_or(0)
        ));
        if let Some(options) = poll["options"].as_array() {
            for option in options {
                if let Some(label) = option["text"].as_str() {
                    body.push_str(&format!("\n• {}", clean(label)));
                }
            }
        }
    }
    let media = media(&value);
    Ok((
        Post {
            id: data.id,
            subreddit: clean(&data.subreddit),
            title: clean(&data.title),
            author: data
                .author
                .map_or_else(|| "[deleted]".into(), |s| clean(&s)),
            flair: optional(data.link_flair_text),
            score: if data.hide_score { None } else { data.score },
            comments: data.num_comments,
            created: created(data.created_utc),
            permalink: data.permalink,
            preview_size: preview_size(&value, &media),
            media,
            stickied: data.stickied,
            nsfw: data.over_18,
            spoiler: data.spoiler,
        },
        body,
    ))
}

fn media_url(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|s| url::Url::parse(s).is_ok_and(|u| super::transport::media_allowed(&u)))
        .map(str::to_string)
}

fn media_source(value: &Value) -> &Value {
    value["crosspost_parent_list"]
        .as_array()
        .and_then(|p| p.first())
        .unwrap_or(value)
}

fn dimensions(value: &Value, width: &str, height: &str) -> Option<(u32, u32)> {
    let width = u32::try_from(value[width].as_u64()?).ok()?;
    let height = u32::try_from(value[height].as_u64()?).ok()?;
    (width > 0 && height > 0).then_some((width, height))
}

fn preview_size(value: &Value, media: &Media) -> Option<(u32, u32)> {
    let url = media.preview_image().or_else(|| media.list_thumb())?;
    let source = media_source(value);
    if let Some(items) = source["gallery_data"]["items"].as_array() {
        for item in items {
            let Some(id) = item["media_id"].as_str() else {
                continue;
            };
            let image = &source["media_metadata"][id]["s"];
            if media_url(&image["u"])
                .or_else(|| media_url(&image["gif"]))
                .as_deref()
                == Some(url)
            {
                return dimensions(image, "x", "y");
            }
        }
    }
    let preview = &source["preview"]["images"][0]["source"];
    if (matches!(media, Media::Image { .. }) || preview["url"].as_str() == Some(url))
        && let Some(size) = dimensions(preview, "width", "height")
    {
        return Some(size);
    }
    (source["thumbnail"].as_str() == Some(url))
        .then(|| dimensions(source, "thumbnail_width", "thumbnail_height"))
        .flatten()
}

fn media(value: &Value) -> Media {
    let source = media_source(value);
    let thumb = media_url(&source["preview"]["images"][0]["source"]["url"])
        .or_else(|| media_url(&source["thumbnail"]));
    if let Some(items) = source["gallery_data"]["items"].as_array() {
        let images = items
            .iter()
            .filter_map(|item| {
                let id = item["media_id"].as_str()?;
                let data = &source["media_metadata"][id]["s"];
                media_url(&data["u"]).or_else(|| media_url(&data["gif"]))
            })
            .collect();
        return Media::Gallery { images, thumb };
    }
    for video in [
        &source["secure_media"]["reddit_video"],
        &source["media"]["reddit_video"],
        &source["preview"]["reddit_video_preview"],
    ] {
        if video.is_object() {
            return Media::Video {
                url: media_url(&video["hls_url"]).or_else(|| media_url(&video["fallback_url"])),
                poster: thumb,
            };
        }
    }
    if let Some(url) =
        media_url(&source["preview"]["images"][0]["variants"]["mp4"]["source"]["url"])
    {
        return Media::Video {
            url: Some(url),
            poster: thumb,
        };
    }
    if source["is_self"].as_bool() == Some(true) {
        return Media::None;
    }
    let url = source["url_overridden_by_dest"]
        .as_str()
        .or_else(|| source["url"].as_str())
        .unwrap_or_default();
    if url::Url::parse(url).is_ok_and(|u| {
        super::transport::media_allowed(&u)
            && (source["post_hint"].as_str() == Some("image")
                || [".jpg", ".jpeg", ".png", ".webp", ".gif"]
                    .iter()
                    .any(|ext| u.path().to_ascii_lowercase().ends_with(ext)))
    }) {
        return Media::Image {
            url: url.to_string(),
        };
    }
    // External image posts have a Reddit-hosted preview; link-post previews stay
    // thumbnails, while this one is the image itself.
    if source["post_hint"].as_str() == Some("image")
        && let Some(url) = &thumb
    {
        return Media::Image { url: url.clone() };
    }
    if url::Url::parse(url).is_ok_and(|u| matches!(u.scheme(), "http" | "https")) {
        return Media::Link {
            url: url.to_owned(),
            thumb,
        };
    }
    Media::None
}

#[cfg(test)]
pub fn parse_post_page(json: &str) -> Result<PostPage> {
    parse_post_page_value(serde_json::from_str(json)?)
}

pub(super) fn parse_post_page_value(value: Value) -> Result<PostPage> {
    let Value::Array(parts) = value else {
        bail!("expected Reddit thread array");
    };
    let mut parts = parts.into_iter();
    let posts = listing(parts.next().context("missing thread post")?)?;
    let thing = posts
        .children
        .into_iter()
        .find(|t| t.kind == "t3")
        .context("thread has no post")?;
    let (post, body) = post(thing.data)?;
    let comments = listing(parts.next().context("missing comments listing")?)?;
    let parent = format!("t3_{}", post.id);
    let (comments, more) = comments_from(comments.children, &post.id, &parent, 0)?;
    // All top-level comments share a stable logical parent, even when a refresh
    // moves an expanded comment into the initial page or removes the More row.
    let comments = vec![container(format!("{THREAD_ROOT}{parent}"), comments, more)];
    Ok(PostPage {
        post,
        body,
        comments,
    })
}

#[derive(Deserialize)]
struct CommentData {
    id: String,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    author_flair_text: Option<String>,
    #[serde(default)]
    is_submitter: bool,
    #[serde(default)]
    distinguished: Option<String>,
    #[serde(default)]
    score: Option<i64>,
    #[serde(default)]
    score_hidden: bool,
    #[serde(default)]
    created_utc: Option<f64>,
    #[serde(default)]
    body: String,
    #[serde(default)]
    body_html: Option<String>,
    #[serde(default)]
    replies: Value,
}

fn comment(value: Value, post: &str, depth: usize) -> Result<Comment> {
    let data: CommentData = serde_json::from_value(value).context("invalid Reddit comment")?;
    if !id_valid(&data.id) {
        bail!("invalid comment id");
    }
    let (replies, more) = if data.replies.is_object() {
        comments_from(
            listing(data.replies)?.children,
            post,
            &format!("t1_{}", data.id),
            depth + 1,
        )?
    } else {
        (Vec::new(), None)
    };
    Ok(Comment {
        id: data.id,
        author: data
            .author
            .map_or_else(|| "[deleted]".into(), |s| clean(&s)),
        flair: optional(data.author_flair_text),
        is_op: data.is_submitter,
        is_mod: data.distinguished.as_deref() == Some("moderator"),
        score: if data.score_hidden { None } else { data.score },
        created: created(data.created_utc),
        body: text::body(data.body_html.as_deref().unwrap_or_default(), &data.body),
        replies,
        more,
        sources: None,
    })
}

#[derive(Serialize, Deserialize, Clone)]
pub(super) struct MoreRequest {
    pub post: String,
    pub parent: String,
    pub children: Vec<String>,
}

/// Ids one reply cursor keeps; replies past them are not offered.
const MORE_LIMIT: usize = 10_000;

impl MoreRequest {
    pub fn validate(&self) -> Result<()> {
        if !id_valid(&self.post)
            || !self
                .parent
                .strip_prefix("t1_")
                .or_else(|| self.parent.strip_prefix("t3_"))
                .is_some_and(id_valid)
            || self.children.len() > MORE_LIMIT
            || self.children.iter().any(|id| !id_valid(id))
        {
            bail!("invalid reply cursor");
        }
        Ok(())
    }

    fn cursor(&self, count: Option<u64>) -> MoreReplies {
        MoreReplies {
            cursor: serde_json::to_string(self).expect("reply cursor"),
            count,
        }
    }
}

fn comments_from(
    things: Vec<Thing>,
    post: &str,
    parent: &str,
    depth: usize,
) -> Result<(Vec<Comment>, Option<MoreReplies>)> {
    if depth > 64 {
        bail!("comment nesting exceeds limit");
    }
    let mut comments = Vec::new();
    let mut children = Vec::new();
    let mut count = 0u64;
    let mut has_more = false;
    let mut ids = HashSet::new();
    for thing in things {
        match thing.kind.as_str() {
            "t1" => {
                let c = comment(thing.data, post, depth)?;
                if ids.insert(c.id.clone()) {
                    comments.push(c);
                }
            }
            "more" => {
                has_more = true;
                count = count.saturating_add(thing.data["count"].as_u64().unwrap_or(0));
                if let Some(list) = thing.data["children"].as_array() {
                    for id in list.iter().filter_map(Value::as_str) {
                        if !id_valid(id) {
                            bail!("invalid more comment id");
                        }
                        if children.len() < MORE_LIMIT && !children.iter().any(|c| c == id) {
                            children.push(id.to_string());
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let more = has_more.then(|| {
        // A truncated list offers only what it can load.
        let count = if children.len() == MORE_LIMIT {
            count.min(MORE_LIMIT as u64)
        } else {
            count
        };
        MoreRequest {
            post: post.to_owned(),
            parent: parent.to_owned(),
            children,
        }
        .cursor((count > 0).then_some(count))
    });
    Ok((comments, more))
}

fn container(id: String, replies: Vec<Comment>, more: Option<MoreReplies>) -> Comment {
    Comment {
        id,
        author: String::new(),
        flair: None,
        is_op: false,
        is_mod: false,
        score: None,
        created: None,
        body: String::new(),
        replies,
        more,
        sources: None,
    }
}

pub(super) fn focused_replies(value: Value, more: &MoreRequest) -> Result<ReplyPage> {
    fn find(comments: Vec<Comment>, id: &str) -> Option<Comment> {
        for c in comments {
            if c.id == id {
                return Some(c);
            }
            if let Some(c) = find(c.replies, id) {
                return Some(c);
            }
        }
        None
    }
    let page = parse_post_page_value(value)?;
    let c = find(page.comments, more.parent.trim_start_matches("t1_"))
        .context("reply parent missing")?;
    if c.replies.is_empty() && c.more.is_some() {
        bail!("Reddit returned no additional replies");
    }
    Ok(if c.more.is_none() {
        ReplyPage::complete(c.replies)
    } else {
        ReplyPage {
            order: c
                .replies
                .iter()
                .map(|c| c.id.clone())
                .chain(remaining_ids(c.more.as_ref()))
                .collect(),
            refreshed: Some(c.replies.iter().map(|c| c.id.clone()).collect()),
            comments: c.replies,
            more: c.more,
        }
    })
}

fn remaining_ids(more: Option<&MoreReplies>) -> Vec<String> {
    more.map(|c| {
        serde_json::from_str::<MoreRequest>(&c.cursor)
            .expect("parser cursor")
            .children
    })
    .unwrap_or_default()
}

/// Combine omitted IDs without introducing another parent into the comment tree.
fn combine_more(a: Option<MoreReplies>, b: Option<MoreReplies>) -> Option<MoreReplies> {
    match (a, b) {
        (Some(a), Some(b)) => {
            let mut left: MoreRequest = serde_json::from_str(&a.cursor).expect("parser cursor");
            let right: MoreRequest = serde_json::from_str(&b.cursor).expect("parser cursor");
            for id in right.children {
                if left.children.len() < MORE_LIMIT && !left.children.contains(&id) {
                    left.children.push(id);
                }
            }
            Some(left.cursor(None))
        }
        (a, b) => a.or(b),
    }
}

pub(super) fn expanded_replies(value: Value, more: &MoreRequest) -> Result<ReplyPage> {
    let response = &value["json"];
    if response["errors"].as_array().is_some_and(|e| !e.is_empty()) {
        bail!("Reddit could not expand replies");
    }
    let things = Vec::<Thing>::deserialize(&response["data"]["things"])
        .context("invalid expanded replies")?;
    let mut by_parent: HashMap<String, Vec<Thing>> = HashMap::new();
    for thing in things {
        let parent = thing.data["parent_id"]
            .as_str()
            .context("reply has no parent")?
            .to_string();
        by_parent.entry(parent).or_default().push(thing);
    }
    fn build(
        parent: &str,
        post: &str,
        map: &mut HashMap<String, Vec<Thing>>,
        depth: usize,
    ) -> Result<(Vec<Comment>, Option<MoreReplies>)> {
        let (mut comments, more) =
            comments_from(map.remove(parent).unwrap_or_default(), post, parent, depth)?;
        for c in &mut comments {
            let (nested, continuation) = build(&format!("t1_{}", c.id), post, map, depth + 1)?;
            let mut seen: HashSet<_> = c.replies.iter().map(|r| r.id.clone()).collect();
            c.replies
                .extend(nested.into_iter().filter(|r| seen.insert(r.id.clone())));
            c.more = combine_more(c.more.take(), continuation);
        }
        Ok((comments, more))
    }
    let (comments, mut continuation) = build(&more.parent, &more.post, &mut by_parent, 0)?;
    if !by_parent.is_empty() {
        bail!("expanded replies have missing parents");
    }
    if more.children.len() > 100 {
        let next = MoreRequest {
            children: more.children[100..].to_vec(),
            ..more.clone()
        };
        continuation = combine_more(
            continuation,
            Some(next.cursor(Some(next.children.len() as u64))),
        );
    }
    let pending = remaining_ids(continuation.as_ref());
    Ok(ReplyPage {
        order: comments
            .iter()
            .map(|c| c.id.clone())
            .chain(pending.iter().cloned())
            .collect(),
        comments,
        more: continuation,
        refreshed: Some(
            more.children
                .iter()
                .take(100)
                .filter(|id| !pending.contains(id))
                .cloned()
                .collect(),
        ),
    })
}

fn id_valid(id: &str) -> bool {
    !id.is_empty() && id.len() <= 32 && id.bytes().all(|c| c.is_ascii_alphanumeric())
}

/// The id in `/r/<sub>/comments/<id>/…`, `/user/<name>/comments/<id>/…` or `/comments/<id>`.
pub(super) fn post_id(permalink: &str) -> Option<&str> {
    match *permalink.split('/').collect::<Vec<_>>() {
        ["", "r" | "user" | "u", _, "comments", id, ..] | ["", "comments", id, ..] => Some(id),
        _ => None,
    }
    .filter(|id| id_valid(id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn merging_two_large_cursors_stays_usable() {
        let cursor = |prefix: &str| {
            MoreRequest {
                post: "p".into(),
                parent: "t3_p".into(),
                children: (0..9_000).map(|i| format!("{prefix}{i}")).collect(),
            }
            .cursor(None)
        };
        let merged = combine_more(Some(cursor("a")), Some(cursor("b"))).unwrap();
        let request: MoreRequest = serde_json::from_str(&merged.cursor).unwrap();
        request.validate().unwrap();
    }

    #[test]
    fn a_huge_more_list_still_gives_a_usable_cursor() {
        let ids: Vec<String> = (0..10_050).map(|i| format!("c{i}")).collect();
        let more = Thing {
            kind: "more".into(),
            data: json!({"count": ids.len(), "children": ids}),
        };
        let (_, cursor) = comments_from(vec![more], "p", "t3_p", 0).unwrap();
        let cursor = cursor.unwrap();
        let request: MoreRequest = serde_json::from_str(&cursor.cursor).unwrap();
        request.validate().unwrap();
        // The row offers what it can load.
        assert_eq!(cursor.count, Some(MORE_LIMIT as u64));
    }

    #[test]
    fn post_ids_come_from_the_permalink_position_not_the_first_comments_word() {
        for (permalink, id) in [
            ("/r/comments/comments/abc/x/", Some("abc")),
            ("/r/rust/comments/abc/comments/", Some("abc")),
            ("/user/me/comments/abc/x/", Some("abc")),
            ("/comments/abc", Some("abc")),
            ("/r/rust/abc/", None),
        ] {
            assert_eq!(post_id(permalink), id, "{permalink}");
        }
    }

    #[test]
    fn json_listing_thread_and_gallery_keep_the_reader_contract() {
        let list = parse_listing(include_str!("../../tests/fixtures/sub.json")).unwrap();
        assert_eq!(list.posts.len(), 1);
        let p = &list.posts[0];
        assert_eq!(
            (&*p.id, &*p.subreddit, &*p.title),
            ("1x4v4sn", "neovim", "Error with Neovim and Tree Sitter")
        );
        assert_eq!(p.score, Some(12));
        assert_eq!(p.comments, 3);
        assert_eq!(p.created.unwrap().timestamp(), 1791849600);
        assert_eq!(list.after.as_deref(), Some("t3_1x4v4sn"));
        let page = parse_post_page(include_str!("../../tests/fixtures/post.json")).unwrap();
        assert_eq!(
            page.body,
            format!(
                "Body {}secret{}.",
                crate::model::SPOILER_START,
                crate::model::SPOILER_END
            )
        );
        let comments = &page.comments[0].replies;
        assert!(comments[0].is_op);
        assert!(comments[0].replies[0].is_mod);
        let cursor: MoreRequest =
            serde_json::from_str(&comments[0].more.as_ref().unwrap().cursor).unwrap();
        assert_eq!(cursor.children, ["c3"]);
        assert_eq!(cursor.parent, "t1_c1");
        let gallery = parse_post_page(include_str!("../../tests/fixtures/gallery.json")).unwrap();
        assert_eq!(
            gallery.post.media,
            Media::Gallery {
                images: vec![
                    "https://i.redd.it/a.png".into(),
                    "https://i.redd.it/b.png".into()
                ],
                thumb: None
            }
        );
    }

    #[test]
    fn valid_empty_and_deleted_content_differ_from_errors_and_malformed_data() {
        let empty = json!({"kind":"Listing","data":{"children":[],"after":null}});
        assert!(parse_listing_value(empty).unwrap().posts.is_empty());
        for bad in [
            json!({"error":403}),
            json!({}),
            json!({"kind":"Listing","data":{}}),
            json!({"kind":"Listing","data":{"children":[{"kind":"t3","data":{}}]}}),
        ] {
            assert!(parse_listing_value(bad).is_err());
        }
        assert!(parse_post_page_value(json!([])).is_err());
        let deleted = comment(
            json!({"id":"abc","author":null,"body":"[deleted]","score_hidden":true,"score":99}),
            "post",
            0,
        )
        .unwrap();
        assert_eq!(deleted.author, "[deleted]");
        assert_eq!(deleted.body, "[deleted]");
        assert_eq!(deleted.score, None);
    }

    #[test]
    fn media_uses_hls_gif_gallery_crosspost_and_link_previews() {
        let preview = json!({"images":[{"source":{"url":"https://preview.redd.it/post.jpg?s=x"}}]});
        let video = media(
            &json!({"secure_media":{"reddit_video":{"hls_url":"https://v.redd.it/id/HLSPlaylist.m3u8"}},"preview":preview}),
        );
        assert_eq!(
            video.video_url(),
            Some("https://v.redd.it/id/HLSPlaylist.m3u8")
        );
        assert_eq!(
            video.preview_image(),
            Some("https://preview.redd.it/post.jpg?s=x")
        );
        assert_eq!(
            media(&json!({"url":"https://i.redd.it/anim.GIF"})).video_url(),
            Some("https://i.redd.it/anim.GIF")
        );
        assert_eq!(
            media(
                &json!({"crosspost_parent_list":[{"url":"https://i.redd.it/p.png","post_hint":"image"}]})
            ),
            Media::Image {
                url: "https://i.redd.it/p.png".into()
            }
        );
        let link = media(&json!({"url":"https://example.com/article","preview":preview}));
        assert_eq!(
            link.list_thumb(),
            Some("https://preview.redd.it/post.jpg?s=x")
        );
        assert_eq!(link.preview_image(), None);
        let external = json!({
            "url": "https://example.com/photo.jpg",
            "post_hint": "image",
            "preview": preview,
        });
        assert_eq!(
            media(&external).preview_image(),
            Some("https://preview.redd.it/post.jpg?s=x")
        );
    }

    #[test]
    fn when_listing_media_has_dimensions_its_selected_preview_should_preserve_them() {
        for (mut fields, expected) in [
            (
                json!({"url":"https://i.redd.it/p.jpg","preview":{"images":[{"source":{"url":"https://preview.redd.it/p.jpg","width":640,"height":320}}]}}),
                Some((640, 320)),
            ),
            (
                json!({"url":"https://i.redd.it/p.gif","preview":{"images":[{"source":{"url":"https://preview.redd.it/p.gif","width":400,"height":600}}]}}),
                Some((400, 600)),
            ),
            (
                json!({"url":"https://example.com/photo","post_hint":"image","preview":{"images":[{"source":{"url":"https://preview.redd.it/p.jpg","width":640,"height":480}}]}}),
                Some((640, 480)),
            ),
            (
                json!({"gallery_data":{"items":[{"media_id":"bad"},{"media_id":"good"}]},"media_metadata":{"bad":{"s":{"u":"https://example.com/not-allowed.jpg","x":1,"y":1}},"good":{"s":{"u":"https://i.redd.it/gallery.jpg","x":320,"y":640}}}}),
                Some((320, 640)),
            ),
            (
                json!({"gallery_data":{"items":[]},"thumbnail":"https://preview.redd.it/thumb.jpg","thumbnail_width":160,"thumbnail_height":90}),
                Some((160, 90)),
            ),
            (
                json!({"secure_media":{"reddit_video":{"hls_url":"https://v.redd.it/p/HLSPlaylist.m3u8","width":720,"height":1280}},"preview":{"images":[{"source":{"url":"https://preview.redd.it/poster.jpg","width":640,"height":360}}]}}),
                Some((640, 360)),
            ),
            (
                json!({"url":"https://example.com/article","preview":{"images":[{"source":{"url":"https://preview.redd.it/link.jpg","width":800,"height":400}}]}}),
                Some((800, 400)),
            ),
            (
                json!({"url":"https://example.com/article","thumbnail":"https://preview.redd.it/thumb.jpg","thumbnail_width":160,"thumbnail_height":120}),
                Some((160, 120)),
            ),
            (
                json!({"preview":{"images":[{"source":{"url":"https://preview.redd.it/wrong.jpg","width":1,"height":1}}]},"crosspost_parent_list":[{"url":"https://i.redd.it/parent.jpg","preview":{"images":[{"source":{"url":"https://preview.redd.it/parent.jpg","width":800,"height":1200}}]}}]}),
                Some((800, 1200)),
            ),
            (
                json!({"url":"https://i.redd.it/p.jpg","preview":{"images":[{"source":{"url":"https://preview.redd.it/p.jpg","width":0,"height":320}}]}}),
                None,
            ),
            (
                json!({"url":"https://i.redd.it/p.jpg","preview":{"images":[{"source":{"url":"https://preview.redd.it/p.jpg","width":4294967296u64,"height":320}}]}}),
                None,
            ),
        ] {
            fields.as_object_mut().unwrap().extend(json!({"id":"p","title":"preview","subreddit":"s","permalink":"/r/s/comments/p/x/"}).as_object().unwrap().clone());
            let actual = parse_listing_value(
                json!({"kind":"Listing","data":{"children":[{"kind":"t3","data":fields}]}}),
            )
            .unwrap();

            assert_eq!(actual.posts[0].preview_size, expected, "{fields}");
            assert_eq!(
                serde_json::from_value::<Listing>(serde_json::to_value(&actual).unwrap()).unwrap(),
                actual
            );
        }
    }

    #[test]
    fn flat_expansions_rebuild_tree_and_continue_after_a_hundred_children() {
        let more = MoreRequest {
            post: "p".into(),
            parent: "t3_p".into(),
            children: (0..101).map(|i| format!("c{i}")).collect(),
        };
        let response = json!({"json":{"errors":[],"data":{"things":[
            {"kind":"t1","data":{"id":"c1","parent_id":"t1_c0","body":"child"}},
            {"kind":"t1","data":{"id":"c0","parent_id":"t3_p","body":"parent"}},
            {"kind":"t1","data":{"id":"c0","parent_id":"t3_p","body":"duplicate"}},
            {"kind":"more","data":{"parent_id":"t3_p","children":["c2"],"count":1}}
        ]}}});
        let page = expanded_replies(response, &more).unwrap();
        assert_eq!(page.order, ["c0", "c2", "c100"]);
        assert_eq!(page.comments.len(), 1);
        assert_eq!(page.comments[0].id, "c0");
        assert_eq!(page.comments[0].replies[0].id, "c1");
        let next: MoreRequest = serde_json::from_str(&page.more.as_ref().unwrap().cursor).unwrap();
        assert_eq!(next.children, ["c2", "c100"]);
        let refreshed = page.refreshed.as_ref().unwrap();
        assert!(refreshed.contains(&"c0".into()) && refreshed.contains(&"c99".into()));
        assert!(!refreshed.contains(&"c2".into()) && !refreshed.contains(&"c100".into()));
        assert_eq!(
            page,
            serde_json::from_slice::<ReplyPage>(&serde_json::to_vec(&page).unwrap()).unwrap()
        );
        let last =
            expanded_replies(json!({"json":{"errors":[],"data":{"things":[]}}}), &next).unwrap();
        assert!(last.comments.is_empty() && last.more.is_none());
        assert!(expanded_replies(json!({"json":{"errors":[["BAD"]]}}), &more).is_err());
    }

    #[test]
    fn top_level_more_is_an_expandable_container_and_poll_text_remains_readable() {
        let mut page: Value =
            serde_json::from_str(include_str!("../../tests/fixtures/post.json")).unwrap();
        page[0]["data"]["children"][0]["data"]["poll_data"] =
            json!({"total_vote_count":61,"options":[{"text":"Yes"},{"text":"No"}]});
        page[1]["data"]["children"]
            .as_array_mut()
            .unwrap()
            .push(json!({"kind":"more","data":{"children":["c9"],"count":2}}));
        let page = parse_post_page_value(page).unwrap();
        assert!(page.body.ends_with("Poll: 61 votes\n• Yes\n• No"));
        assert_eq!(page.comments.last().unwrap().id, "more:t3_1x4v4sn");
        assert!(page.comments.last().unwrap().author.is_empty());
    }
}
