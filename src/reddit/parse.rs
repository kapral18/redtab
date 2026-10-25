//! Reddit JSON to the reader's persisted models. Bodies come from Reddit's rendered HTML
//! when present, else the raw text; see `text`.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
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

#[cfg(test)]
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

pub(super) fn stream_listing(
    reader: impl std::io::Read,
    progress: &mut dyn FnMut(&Post),
) -> serde_json::Result<Listing> {
    let mut state = ListingStream {
        listing: Listing::default(),
        kind: None,
        data: false,
        children: false,
        after: false,
        emitted: 0,
        seen: HashSet::new(),
        progress,
    };
    let mut decoder = serde_json::Deserializer::from_reader(std::io::BufReader::new(reader));
    ListingSeed {
        state: &mut state,
        field: ListingField::Envelope,
    }
    .deserialize(&mut decoder)?;
    decoder.end()?;
    Ok(state.listing)
}

struct ListingStream<'a> {
    listing: Listing,
    kind: Option<String>,
    data: bool,
    children: bool,
    after: bool,
    emitted: usize,
    seen: HashSet<String>,
    progress: &'a mut dyn FnMut(&Post),
}

impl ListingStream<'_> {
    fn publish(&mut self) {
        if self.kind.as_deref() == Some("Listing") {
            for post in &self.listing.posts[self.emitted..] {
                (self.progress)(post);
            }
            self.emitted = self.listing.posts.len();
        }
    }
}

#[derive(Clone, Copy)]
enum ListingField {
    Envelope,
    Data,
    Children,
}

struct ListingSeed<'a, 'b> {
    state: &'a mut ListingStream<'b>,
    field: ListingField,
}

impl<'de> DeserializeSeed<'de> for ListingSeed<'_, '_> {
    type Value = ();

    fn deserialize<D: de::Deserializer<'de>>(
        self,
        decoder: D,
    ) -> std::result::Result<(), D::Error> {
        match self.field {
            ListingField::Children => decoder.deserialize_seq(self),
            _ => decoder.deserialize_map(self),
        }
    }
}

impl<'de> Visitor<'de> for ListingSeed<'_, '_> {
    type Value = ();

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Reddit listing")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> std::result::Result<(), M::Error> {
        while let Some(key) = map.next_key::<String>()? {
            match (self.field, key.as_str()) {
                (ListingField::Envelope, "kind") => {
                    if self.state.kind.is_some() {
                        return Err(de::Error::duplicate_field("kind"));
                    }
                    let kind: String = map.next_value()?;
                    if kind != "Listing" {
                        return Err(de::Error::custom("expected Reddit Listing"));
                    }
                    self.state.kind = Some(kind);
                    self.state.publish();
                }
                (ListingField::Envelope, "data") => {
                    if self.state.data {
                        return Err(de::Error::duplicate_field("data"));
                    }
                    self.state.data = true;
                    map.next_value_seed(ListingSeed {
                        state: self.state,
                        field: ListingField::Data,
                    })?;
                }
                (ListingField::Data, "children") => {
                    if self.state.children {
                        return Err(de::Error::duplicate_field("children"));
                    }
                    self.state.children = true;
                    map.next_value_seed(ListingSeed {
                        state: self.state,
                        field: ListingField::Children,
                    })?;
                }
                (ListingField::Data, "after") => {
                    if self.state.after {
                        return Err(de::Error::duplicate_field("after"));
                    }
                    self.state.after = true;
                    self.state.listing.after = map
                        .next_value::<Option<String>>()?
                        .filter(|s| !s.is_empty());
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        match self.field {
            ListingField::Envelope if self.state.kind.is_none() || !self.state.data => {
                Err(de::Error::custom("incomplete Reddit listing"))
            }
            ListingField::Data if !self.state.children => Err(de::Error::missing_field("children")),
            _ => Ok(()),
        }
    }

    fn visit_seq<S: SeqAccess<'de>>(self, mut seq: S) -> std::result::Result<(), S::Error> {
        while let Some(thing) = seq.next_element::<Thing>()? {
            if thing.kind == "t3" {
                let post = post(thing.data)
                    .map_err(|_| de::Error::custom("invalid Reddit post"))?
                    .0;
                if self.state.seen.insert(post.id.clone()) {
                    self.state.listing.posts.push(post);
                    self.state.publish();
                }
            }
        }
        Ok(())
    }
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

pub(super) fn post(value: Value) -> Result<(Post, String)> {
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
                    if let Some(votes) = option["vote_count"].as_u64() {
                        body.push_str(&format!(" — {votes} votes"));
                    }
                }
            }
        }
    }
    if let Some(original) = value["crosspost_parent_list"]
        .as_array()
        .and_then(|items| items.first())
    {
        body.push_str(&format!(
            "\n\nCrossposted from r/{} by u/{}: {}\n{}",
            clean(original["subreddit"].as_str().unwrap_or_default()),
            clean(original["author"].as_str().unwrap_or_default()),
            clean(original["title"].as_str().unwrap_or_default()),
            clean(original["permalink"].as_str().unwrap_or_default())
        ));
    }
    if let Some(items) = media_source(&value)["gallery_data"]["items"].as_array() {
        for (index, item) in items.iter().enumerate() {
            let caption = item["caption"].as_str().unwrap_or_default();
            let link = item["outbound_url"].as_str().unwrap_or_default();
            if !caption.is_empty() || !link.is_empty() {
                body.push_str(&format!(
                    "\n\nImage {}: {}\n{}",
                    index + 1,
                    clean(caption),
                    clean(link)
                ));
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

fn bounded_preview(source: &Value, previews: &Value, keys: (&str, &str, &str)) -> Option<String> {
    let (url, width, height) = keys;
    let pixels = |(width, height): (u32, u32)| u64::from(width) * u64::from(height);
    if dimensions(source, width, height)
        .is_some_and(|size| pixels(size) <= crate::model::PREVIEW_PIXELS)
    {
        return None;
    }
    previews
        .as_array()?
        .iter()
        .filter_map(|candidate| {
            let size = dimensions(candidate, width, height)?;
            if pixels(size) > crate::model::PREVIEW_PIXELS {
                return None;
            }
            Some((media_url(&candidate[url])?, size))
        })
        .min_by_key(|(_, size)| {
            let sufficient = size.0.max(size.1) >= crate::model::PREVIEW_EDGE;
            (
                !sufficient,
                if sufficient {
                    pixels(*size)
                } else {
                    u64::MAX - pixels(*size)
                },
            )
        })
        .map(|(url, _)| url)
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
            if let Some(previews) = source["media_metadata"][id]["p"].as_array() {
                for preview in previews {
                    if media_url(&preview["u"]).as_deref() == Some(url) {
                        return dimensions(preview, "x", "y");
                    }
                }
            }
        }
    }
    let preview = &source["preview"]["images"][0]["source"];
    if let Some(previews) = source["preview"]["images"][0]["resolutions"].as_array() {
        for preview in previews {
            if media_url(&preview["url"]).as_deref() == Some(url) {
                return dimensions(preview, "width", "height");
            }
        }
    }
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
    let image = &source["preview"]["images"][0];
    let preview = bounded_preview(
        &image["source"],
        &image["resolutions"],
        ("url", "width", "height"),
    );
    let thumb = preview
        .clone()
        .or_else(|| media_url(&image["source"]["url"]))
        .or_else(|| media_url(&source["thumbnail"]));
    if let Some(items) = source["gallery_data"]["items"].as_array() {
        let (images, mut previews): (Vec<_>, Vec<_>) = items
            .iter()
            .filter_map(|item| {
                let id = item["media_id"].as_str()?;
                let data = &source["media_metadata"][id];
                let original =
                    media_url(&data["s"]["u"]).or_else(|| media_url(&data["s"]["gif"]))?;
                let preview = bounded_preview(&data["s"], &data["p"], ("u", "x", "y"));
                Some((original, preview))
            })
            .unzip();
        if previews.iter().all(Option::is_none) {
            previews.clear();
        }
        return Media::Gallery {
            images,
            previews,
            thumb,
        };
    }
    for video in [
        &source["secure_media"]["reddit_video"],
        &source["media"]["reddit_video"],
        &source["preview"]["reddit_video_preview"],
    ] {
        if video.is_object() {
            return Media::Video {
                gif: video["is_gif"].as_bool().unwrap_or(false),
                url: media_url(&video["hls_url"]).or_else(|| media_url(&video["fallback_url"])),
                poster: thumb,
            };
        }
    }
    if let Some(url) =
        media_url(&source["preview"]["images"][0]["variants"]["mp4"]["source"]["url"])
    {
        return Media::Video {
            gif: true,
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
            preview,
        };
    }
    // External image posts have a Reddit-hosted preview; link-post previews stay
    // thumbnails, while this one is the image itself.
    if source["post_hint"].as_str() == Some("image")
        && let Some(url) =
            media_url(&image["source"]["url"]).or_else(|| media_url(&source["thumbnail"]))
    {
        return Media::Image { url, preview };
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
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sort: String,
}

pub(super) fn set_more_sort(more: &mut Option<MoreReplies>, sort: &str) -> Result<()> {
    if let Some(more) = more {
        let mut request: MoreRequest = serde_json::from_str(&more.cursor)?;
        request.sort = sort.into();
        more.cursor = serde_json::to_string(&request)?;
    }
    Ok(())
}

pub(super) fn set_reply_sort(comments: &mut [Comment], sort: &str) -> Result<()> {
    for comment in comments {
        set_more_sort(&mut comment.more, sort)?;
        set_reply_sort(&mut comment.replies, sort)?;
    }
    Ok(())
}

/// Ids one reply cursor keeps; replies past them are not offered.
const MORE_LIMIT: usize = 10_000;
const EXPANSION_BATCH: usize = 4;

impl MoreRequest {
    pub fn validate(&self) -> Result<()> {
        if !matches!(
            self.sort.as_str(),
            "" | "confidence" | "top" | "new" | "old" | "controversial" | "qa"
        ) || !id_valid(&self.post)
            || !self
                .parent
                .strip_prefix("t1_")
                .or_else(|| self.parent.strip_prefix("t3_"))
                .is_some_and(id_valid)
            || self.children.len() > MORE_LIMIT
            || self.children.iter().any(|id| !id_valid(id))
            || (self.parent.starts_with("t3_") && self.parent != format!("t3_{}", self.post))
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
            sort: String::new(),
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
    if page.post.id != more.post {
        bail!("reply post identity does not match");
    }
    let parent = if more.parent.starts_with("t3_") {
        format!("{THREAD_ROOT}{}", more.parent)
    } else {
        more.parent.trim_start_matches("t1_").to_string()
    };
    let c = find(page.comments, &parent).context("reply parent missing")?;
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

/// Preserve pending replies from partial trees, excluding replies already loaded.
pub(crate) fn merge_reply_continuations(
    previous: Option<MoreReplies>,
    incoming: Option<MoreReplies>,
    loaded: &[Comment],
) -> Option<MoreReplies> {
    fn request(more: &MoreReplies) -> Result<MoreRequest> {
        let request: MoreRequest = serde_json::from_str(&more.cursor)?;
        request.validate()?;
        Ok(request)
    }
    let mut cursors = previous.into_iter().chain(incoming);
    let first = cursors.next()?;
    let Ok(mut left) = request(&first) else {
        return Some(first);
    };
    let mut seen = HashSet::new();
    let mut stack: Vec<_> = loaded.iter().collect();
    while let Some(comment) = stack.pop() {
        seen.insert(comment.id.clone());
        stack.extend(&comment.replies);
    }
    let mut focused = left.children.is_empty();
    left.children.retain(|id| seen.insert(id.clone()));
    if let Some(next) = cursors.next() {
        let Ok(right) = request(&next) else {
            return Some(first);
        };
        if (left.post.as_str(), left.parent.as_str(), left.sort.as_str())
            != (
                right.post.as_str(),
                right.parent.as_str(),
                right.sort.as_str(),
            )
        {
            return Some(first);
        }
        focused |= right.children.is_empty();
        for id in right.children {
            if seen.insert(id.clone()) {
                if left.children.len() == MORE_LIMIT {
                    focused = true;
                    break;
                }
                left.children.push(id);
            }
        }
    }
    if focused {
        // A parent-focused read retains access when an ID union exceeds the bound.
        left.children.clear();
    } else if left.children.is_empty() {
        return None;
    }
    Some(left.cursor(None))
}

pub(super) fn expanded_replies(
    more: &MoreRequest,
    mut fetch: impl FnMut(&str) -> Result<Value>,
) -> Result<ReplyPage> {
    let mut things: Vec<Thing> = Vec::new();
    let attempted: Vec<_> = more
        .children
        .iter()
        .take(EXPANSION_BATCH)
        .cloned()
        .collect();
    for id in &attempted {
        if !has_thing(&things, id) {
            add_focused_things(&mut things, fetch(id)?, more)?;
        }
        while has_thing(&things, id) {
            let Some(parent) = missing_reply_ancestor(&things, id, more)? else {
                break;
            };
            add_focused_things(&mut things, fetch(&parent)?, more)?;
            if !has_thing(&things, &parent) {
                bail!("Reply ancestor is unavailable");
            }
        }
    }
    let loaded: HashSet<_> = things
        .iter()
        .filter(|t| t.kind == "t1")
        .filter_map(|t| t.data["id"].as_str().map(str::to_owned))
        .collect();
    let mut by_parent: HashMap<String, Vec<Thing>> = HashMap::new();
    for mut thing in things {
        if thing.kind == "more"
            && let Some(children) = thing.data["children"].as_array_mut()
        {
            let had_ids = !children.is_empty();
            let before = children.len();
            children.retain(|id| {
                id.as_str().is_some_and(|id| {
                    !loaded.contains(id) && !attempted.iter().any(|done| done == id)
                })
            });
            if had_ids && children.is_empty() {
                continue;
            }
            let removed = before - children.len();
            if let Some(count) = thing.data["count"].as_u64() {
                thing.data["count"] = Value::from(count.saturating_sub(removed as u64));
            }
        }
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
            c.more = merge_reply_continuations(c.more.take(), continuation, &c.replies);
        }
        Ok((comments, more))
    }
    let (comments, mut continuation) = build(&more.parent, &more.post, &mut by_parent, 0)?;
    set_more_sort(&mut continuation, &more.sort)?;
    fn returned_ids(comments: &[Comment], ids: &mut Vec<String>) {
        for comment in comments {
            ids.push(comment.id.clone());
            returned_ids(&comment.replies, ids);
        }
    }
    let mut returned = Vec::new();
    returned_ids(&comments, &mut returned);
    let remaining: Vec<_> = more
        .children
        .iter()
        .filter(|id| !returned.contains(id) && !attempted.contains(id))
        .cloned()
        .collect();
    if !remaining.is_empty() {
        let next = MoreRequest {
            children: remaining,
            ..more.clone()
        };
        continuation = merge_reply_continuations(
            continuation,
            Some(next.cursor(Some(next.children.len() as u64))),
            &comments,
        );
    }
    let pending = remaining_ids(continuation.as_ref());
    let mut refreshed = attempted;
    for id in returned {
        if !refreshed.contains(&id) {
            refreshed.push(id);
        }
    }
    Ok(ReplyPage {
        order: comments
            .iter()
            .map(|c| c.id.clone())
            .chain(pending.iter().cloned())
            .collect(),
        comments,
        more: continuation,
        refreshed: Some(refreshed),
    })
}

fn has_thing(things: &[Thing], id: &str) -> bool {
    things.iter().any(|t| t.kind == "t1" && t.data["id"] == id)
}

fn missing_reply_ancestor(
    things: &[Thing],
    id: &str,
    more: &MoreRequest,
) -> Result<Option<String>> {
    let mut parent = format!("t1_{id}");
    let mut seen = HashSet::new();
    while parent != more.parent {
        if !seen.insert(parent.clone()) || seen.len() > 64 {
            bail!("invalid reply ancestry");
        }
        let id = parent
            .strip_prefix("t1_")
            .context("Reply is outside the requested parent")?;
        let Some(thing) = things.iter().find(|t| t.kind == "t1" && t.data["id"] == id) else {
            return Ok(Some(id.to_string()));
        };
        parent = thing.data["parent_id"]
            .as_str()
            .context("reply has no parent")?
            .into();
    }
    Ok(None)
}

fn add_focused_things(things: &mut Vec<Thing>, value: Value, more: &MoreRequest) -> Result<()> {
    let page = parse_post_page_value(value.clone())?;
    if page.post.id != more.post {
        bail!("reply post identity does not match");
    }
    let children = listing(value[1].clone())?.children;
    flatten_focused(children, None, &more.post, things)
}

fn flatten_focused(
    children: Vec<Thing>,
    parent: Option<&str>,
    post: &str,
    things: &mut Vec<Thing>,
) -> Result<()> {
    for mut thing in children {
        if thing.kind == "more" {
            let parent = parent.context("focused reply contains an unresolved placeholder")?;
            if thing.data["parent_id"]
                .as_str()
                .is_some_and(|id| id != parent)
            {
                bail!("reply parent identity does not match");
            }
            thing.data["parent_id"] = Value::from(parent);
            things.push(thing);
            continue;
        }
        if thing.kind != "t1" {
            bail!("invalid focused reply type");
        }
        let id = thing.data["id"]
            .as_str()
            .context("reply has no identity")?
            .to_string();
        let fullname = format!("t1_{id}");
        let actual_parent = thing.data["parent_id"]
            .as_str()
            .context("reply has no parent")?;
        let valid_parent = actual_parent.strip_prefix("t1_").is_some_and(id_valid)
            || actual_parent == format!("t3_{post}");
        if !valid_parent
            || parent.is_some_and(|parent| parent != actual_parent)
            || thing.data["link_id"]
                .as_str()
                .is_some_and(|link| link != format!("t3_{post}"))
            || thing.data["name"]
                .as_str()
                .is_some_and(|name| name != fullname)
        {
            bail!("reply parent or content identity does not match");
        }
        let replies = if thing.data["replies"].is_object() {
            listing(thing.data["replies"].take())?.children
        } else {
            Vec::new()
        };
        thing.data["replies"] = Value::String(String::new());
        if let Some(existing) = things
            .iter_mut()
            .find(|t| t.kind == "t1" && t.data["id"] == id)
        {
            if existing.data["parent_id"] != thing.data["parent_id"] {
                bail!("reply parent changed");
            }
            *existing = thing;
        } else {
            things.push(thing);
        }
        flatten_focused(replies, Some(&fullname), post, things)?;
    }
    Ok(())
}

fn id_valid(id: &str) -> bool {
    !id.is_empty() && id.len() <= 32 && id.bytes().all(|c| c.is_ascii_alphanumeric())
}

/// The id in `/r/<sub>/comments/<id>/…`, `/user/<name>/comments/<id>/…` or `/comments/<id>`.
pub(crate) fn post_id(permalink: &str) -> Option<&str> {
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
    fn sorted_expansion_should_keep_both_returned_and_unattempted_continuations() {
        let request = MoreRequest {
            post: "p".into(),
            parent: "t1_parent".into(),
            children: ["a", "b", "c", "d", "tail"].map(str::to_string).into(),
            sort: "new".into(),
        };
        let page = expanded_replies(&request, |id| Ok(json!([
            {"kind":"Listing","data":{"children":[{"kind":"t3","data":{"id":"p","title":"post","subreddit":"s","permalink":"/comments/p/"}}]}},
            {"kind":"Listing","data":{"children":[{"kind":"t1","data":{"id":"parent","parent_id":"t3_p","body":"parent","replies":{"kind":"Listing","data":{"children":[
                {"kind":"t1","data":{"id":id,"parent_id":"t1_parent","body":"reply"}},
                {"kind":"more","data":{"parent_id":"t1_parent","children":["next"],"count":1}}
            ]}}}}]}}
        ]))).unwrap();
        let next: MoreRequest = serde_json::from_str(&page.more.unwrap().cursor).unwrap();
        assert_eq!(next.children, ["next", "tail"]);
        assert_eq!(next.sort, "new");
    }

    #[test]
    fn merging_two_large_cursors_stays_usable() {
        let cursor = |prefix: &str| {
            MoreRequest {
                post: "p".into(),
                parent: "t3_p".into(),
                children: (0..9_000).map(|i| format!("{prefix}{i}")).collect(),
                sort: String::new(),
            }
            .cursor(None)
        };
        let merged = merge_reply_continuations(Some(cursor("a")), Some(cursor("b")), &[]).unwrap();
        let request: MoreRequest = serde_json::from_str(&merged.cursor).unwrap();
        request.validate().unwrap();
        assert!(
            request.children.is_empty(),
            "large unions use a parent-focused read"
        );

        let pending = |children: &[&str]| {
            MoreRequest {
                post: "p".into(),
                parent: "t3_p".into(),
                children: children.iter().map(|id| (*id).to_string()).collect(),
                sort: String::new(),
            }
            .cursor(None)
        };
        let loaded = comment(json!({"id":"loaded", "body":"already visible"}), "p", 0).unwrap();
        let merged = merge_reply_continuations(
            Some(pending(&["loaded", "old", "old"])),
            Some(pending(&["new", "loaded"])),
            std::slice::from_ref(&loaded),
        )
        .unwrap();
        let request: MoreRequest = serde_json::from_str(&merged.cursor).unwrap();
        assert_eq!(request.children, ["old", "new"]);
        assert!(merge_reply_continuations(None, Some(pending(&["loaded"])), &[loaded]).is_none());
        for (a, b) in [
            (pending(&[]), pending(&["new"])),
            (pending(&["old"]), pending(&[])),
        ] {
            let merged = merge_reply_continuations(Some(a), Some(b), &[]).unwrap();
            let request: MoreRequest = serde_json::from_str(&merged.cursor).unwrap();
            assert!(
                request.children.is_empty(),
                "parent-focused continuation survives"
            );
        }
        let wrong_scope = MoreRequest {
            post: "q".into(),
            parent: "t3_q".into(),
            children: vec!["new".into()],
            sort: String::new(),
        }
        .cursor(None);
        assert_eq!(
            merge_reply_continuations(Some(pending(&["old"])), Some(wrong_scope), &[]),
            Some(pending(&["old"]))
        );
        let invalid = MoreReplies {
            cursor: "invalid cursor".into(),
            count: None,
        };
        assert_eq!(
            merge_reply_continuations(Some(invalid.clone()), Some(pending(&["new"])), &[]),
            Some(invalid)
        );
        assert!(merge_reply_continuations(None, None, &[]).is_none());
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
    fn when_listing_fields_arrive_in_either_order_streaming_should_keep_the_reader_contract() {
        let value: Value =
            serde_json::from_str(include_str!("../../tests/fixtures/sub.json")).unwrap();
        for input in [
            include_str!("../../tests/fixtures/sub.json").to_string(),
            serde_json::to_string(&value).unwrap(),
        ] {
            let mut published = Vec::new();

            let actual =
                stream_listing(input.as_bytes(), &mut |post| published.push(post.clone())).unwrap();

            assert_eq!(actual, parse_listing(&input).unwrap());
            assert_eq!(published, actual.posts);
        }
    }

    #[test]
    fn when_listing_structure_is_invalid_the_stream_should_not_complete() {
        for input in [
            "",
            "[]",
            "{}",
            r#"{"kind":"Listing"}"#,
            r#"{"kind":"Other","data":{"children":[]}}"#,
            r#"{"data":{"children":[]},"kind":"Other"}"#,
            r#"{"kind":"Listing","data":{}}"#,
            r#"{"kind":"Listing","data":{"children":null}}"#,
            r#"{"kind":"Listing","kind":"Listing","data":{"children":[]}}"#,
            r#"{"kind":"Listing","data":{"children":[]},"data":{"children":[]}}"#,
            r#"{"kind":"Listing","data":{"children":[],"children":[]}}"#,
            r#"{"kind":"Listing","data":{"children":[],"after":null,"after":null}}"#,
            r#"{"kind":"Listing","data":{"children":[]}} trailing"#,
            r#"{"kind":"Listing","data":{"children":[{"kind":"t3","data":{"id":"a"}}]}}"#,
        ] {
            assert!(
                stream_listing(input.as_bytes(), &mut |_| {}).is_err(),
                "{input}"
            );
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
                previews: Vec::new(),
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
                url: "https://i.redd.it/p.png".into(),
                preview: None
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
    fn when_an_image_is_huge_its_inline_preview_should_use_a_detailed_safe_resolution() {
        let fields = json!({
            "id":"large", "title":"large guide", "subreddit":"coolguides",
            "permalink":"/r/coolguides/comments/large/guide/",
            "url":"https://i.redd.it/guide.png",
            "preview":{"images":[{
                "source":{"url":"https://preview.redd.it/guide.png", "width":12000, "height":18000},
                "resolutions":[
                    {"url":"https://preview.redd.it/guide.png?width=320", "width":320, "height":480},
                    {"url":"https://preview.redd.it/guide.png?width=1080", "width":1080, "height":1620},
                    {"url":"https://preview.redd.it/guide.png?width=2160", "width":2160, "height":3240}
                ]
            }]}
        });

        let actual: Post =
            serde_json::from_value(serde_json::to_value(post(fields).unwrap().0).unwrap()).unwrap();

        assert_eq!(
            actual.media.preview_image(),
            Some("https://preview.redd.it/guide.png?width=1080")
        );
        assert_eq!(actual.preview_size, Some((1080, 1620)));
        assert!(
            matches!(actual.media, Media::Image { url, .. } if url == "https://i.redd.it/guide.png")
        );
    }

    #[test]
    fn when_a_large_image_is_externally_hosted_its_original_handoff_should_keep_the_full_reddit_source()
     {
        let fields = json!({
            "id":"external", "title":"external guide", "subreddit":"coolguides",
            "permalink":"/r/coolguides/comments/external/guide/", "post_hint":"image",
            "url":"https://example.com/original.png",
            "preview":{"images":[{
                "source":{"url":"https://external-preview.redd.it/full.png","width":12000,"height":18000},
                "resolutions":[{"url":"https://external-preview.redd.it/sized.png","width":1080,"height":1620}]
            }]}
        });

        let actual = post(fields).unwrap().0;

        assert_eq!(
            actual.media.preview_image(),
            Some("https://external-preview.redd.it/sized.png")
        );
        assert_eq!(actual.preview_size, Some((1080, 1620)));
        assert!(
            matches!(actual.media, Media::Image { url, .. } if url == "https://external-preview.redd.it/full.png")
        );
    }

    #[test]
    fn when_preview_sizes_cross_the_limit_selection_should_preserve_quality_and_reject_bad_candidates()
     {
        for (width, height, candidates, expected) in [
            (
                4000,
                4000,
                json!([{"url":"https://preview.redd.it/small.png","width":800,"height":800}]),
                "https://i.redd.it/original.png",
            ),
            (
                4000,
                4001,
                json!([
                    {"url":"https://preview.redd.it/small.png","width":320,"height":320},
                    {"url":"https://preview.redd.it/best.png","width":800,"height":800}
                ]),
                "https://preview.redd.it/best.png",
            ),
            (
                12000,
                18000,
                json!([
                    {"url":"https://example.com/untrusted.png","width":1080,"height":1620},
                    {"url":"https://preview.redd.it/zero.png","width":0,"height":1620},
                    {"url":"https://preview.redd.it/huge.png","width":12000,"height":18000},
                    {"url":"https://preview.redd.it/overflow.png","width":4294967296u64,"height":1}
                ]),
                "https://i.redd.it/original.png",
            ),
        ] {
            let fields = json!({
                "id":"p", "title":"image", "subreddit":"s", "permalink":"/r/s/comments/p/",
                "url":"https://i.redd.it/original.png",
                "preview":{"images":[{"source":{"url":"https://preview.redd.it/original.png","width":width,"height":height},"resolutions":candidates}]}
            });

            let actual = post(fields).unwrap().0;

            assert_eq!(actual.media.preview_image(), Some(expected));
        }
    }

    #[test]
    fn when_a_gallery_has_invalid_entries_its_safe_previews_should_stay_with_their_originals() {
        let fields = json!({
            "id":"gallery", "title":"guides", "subreddit":"coolguides",
            "permalink":"/r/coolguides/comments/gallery/guides/",
            "gallery_data":{"items":[{"media_id":"bad"},{"media_id":"large"},{"media_id":"small"}]},
            "media_metadata":{
                "bad":{"s":{"u":"https://example.com/not-allowed.png","x":1,"y":1}},
                "large":{
                    "s":{"u":"https://i.redd.it/large.png","x":12000,"y":18000},
                    "p":[{"u":"https://preview.redd.it/large.png?width=1080","x":1080,"y":1620}]
                },
                "small":{"s":{"u":"https://i.redd.it/small.png","x":600,"y":400}}
            }
        });

        let actual = post(fields).unwrap().0;

        assert_eq!(
            actual.media.preview_image(),
            Some("https://preview.redd.it/large.png?width=1080")
        );
        assert_eq!(actual.preview_size, Some((1080, 1620)));
        assert!(
            matches!(actual.media, Media::Gallery { images, .. } if images == ["https://i.redd.it/large.png", "https://i.redd.it/small.png"])
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

    fn focused_thread(post: &str, children: Value) -> Value {
        json!([
            {"kind":"Listing","data":{"children":[{"kind":"t3","data":{
                "id":post,"title":"post","subreddit":"rust","permalink":format!("/comments/{post}/")
            }}]}},
            {"kind":"Listing","data":{"children":children}}
        ])
    }

    #[test]
    fn focused_expansions_should_preserve_nested_replies_and_skip_returned_or_deleted_ids() {
        let more = MoreRequest {
            post: "p".into(),
            parent: "t3_p".into(),
            children: ["c0", "c1", "gone", "c2", "tail"].map(str::to_owned).into(),
            sort: "new".into(),
        };
        let actual = expanded_replies(&more, |id| Ok(focused_thread("p", match id {
            "c0" => json!([{"kind":"t1","data":{"id":"c0","parent_id":"t3_p","body":"first","replies":{
                "kind":"Listing","data":{"children":[
                    {"kind":"t1","data":{"id":"c1","parent_id":"t1_c0","body":"nested"}},
                    {"kind":"more","data":{"parent_id":"t1_c0","children":["c1","deeper"],"count":2}}
                ]}
            }}}]),
            "c2" => json!([{"kind":"t1","data":{"id":"c2","parent_id":"t3_p","body":"second"}}]),
            "gone" => json!([]),
            _ => panic!("returned descendants and later pages must not be fetched"),
        }))).unwrap();
        assert_eq!(actual.order, ["c0", "c2", "tail"]);
        assert_eq!(actual.comments[0].replies[0].id, "c1");
        let nested: MoreRequest =
            serde_json::from_str(&actual.comments[0].more.as_ref().unwrap().cursor).unwrap();
        assert_eq!(nested.children, ["deeper"]);
        let next: MoreRequest =
            serde_json::from_str(&actual.more.as_ref().unwrap().cursor).unwrap();
        assert_eq!(next.children, ["tail"]);
        assert_eq!(next.sort, "new");
        assert!(actual.refreshed.as_ref().unwrap().contains(&"gone".into()));
        assert!(!actual.refreshed.as_ref().unwrap().contains(&"tail".into()));
        assert_eq!(
            actual,
            serde_json::from_slice::<ReplyPage>(&serde_json::to_vec(&actual).unwrap()).unwrap()
        );
    }

    #[test]
    fn a_focused_descendant_should_fetch_its_missing_ancestor_without_reparenting_it() {
        let more = MoreRequest {
            post: "p".into(),
            parent: "t3_p".into(),
            children: vec!["leaf".into()],
            sort: String::new(),
        };
        let actual = expanded_replies(&more, |id| Ok(focused_thread("p", match id {
            "leaf" => json!([{"kind":"t1","data":{"id":"leaf","parent_id":"t1_branch","body":"leaf"}}]),
            "branch" => json!([{"kind":"t1","data":{"id":"branch","parent_id":"t3_p","body":"branch"}}]),
            _ => panic!("unexpected ancestor"),
        }))).unwrap();
        assert_eq!(actual.comments[0].id, "branch");
        assert_eq!(actual.comments[0].replies[0].id, "leaf");
        assert!(actual.more.is_none());
        assert!(actual.refreshed.is_some());
    }

    #[test]
    fn the_last_focused_batch_should_keep_partial_coverage_for_prior_siblings() {
        let more = MoreRequest {
            post: "p".into(),
            parent: "t1_parent".into(),
            children: vec!["child".into()],
            sort: String::new(),
        };
        let actual = expanded_replies(&more, |_| {
            Ok(focused_thread(
                "p",
                json!([
                    {"kind":"t1","data":{"id":"child","parent_id":"t1_parent","body":"reply"}}
                ]),
            ))
        })
        .unwrap();
        assert_eq!(actual.comments[0].id, "child");
        assert_eq!(actual.refreshed, Some(vec!["child".into()]));
        assert!(actual.more.is_none());
    }

    #[test]
    fn a_focused_comment_from_another_post_or_parent_should_be_rejected() {
        let more = MoreRequest {
            post: "p".into(),
            parent: "t1_parent".into(),
            children: vec!["child".into()],
            sort: String::new(),
        };
        assert!(expanded_replies(&more, |_| Ok(focused_thread("other", json!([])))).is_err());
        assert!(
            expanded_replies(&more, |_| Ok(focused_thread(
                "p",
                json!([
                    {"kind":"t1","data":{"id":"child","parent_id":"t3_p","body":"outside"}}
                ])
            )))
            .is_err()
        );
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
