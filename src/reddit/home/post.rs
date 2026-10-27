use anyhow::{Context, Result};
use scraper::{ElementRef, Selector};
use serde_json::{Value, json};

use crate::model::Post;

pub(super) fn parse(element: ElementRef<'_>) -> Result<Option<Post>> {
    let attr = |name| element.value().attr(name);
    if attr("is-ad").is_some() {
        return Ok(None);
    }
    let id = attr("id")
        .and_then(|id| id.strip_prefix("t3_"))
        .context("invalid Home post identity")?;
    let mut data = json!({
        "id": id,
        "title": attr("post-title").context("Home post has no title")?,
        "subreddit": attr("subreddit-name").context("Home post has no community")?,
        "permalink": attr("permalink").context("Home post has no location")?,
        "author": attr("author"),
        "score": attr("score").and_then(|s| s.parse::<i64>().ok()),
        "num_comments": attr("comment-count").and_then(|s| s.parse::<u64>().ok()).unwrap_or(0),
        "created_utc": attr("created-timestamp").and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()).map(|d|d.timestamp()),
        "over_18": attr("nsfw").is_some(),
        "spoiler": attr("spoiler").is_some(),
        "stickied": find(element, ".stickied-status:not(.hidden):not([hidden])").is_some(),
        "link_flair_text": find(element, "shreddit-post-flair").map(|f|f.text().collect::<String>()),
        "url_overridden_by_dest": attr("content-href"),
        "is_self": attr("post-type") == Some("text"),
    });
    if let Some(prefixed) = attr("subreddit-prefixed-name").filter(|s| s.starts_with("u/")) {
        data["subreddit_type"] = json!("user");
        data["subreddit_name_prefixed"] = json!(prefixed);
    }
    if let Some(telemetry) = find(element, "shreddit-media-lightbox-listener")
        .and_then(|el| el.value().attr("telemetry"))
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
    {
        data["over_18"] = json!(data["over_18"] == true || telemetry["post"]["nsfw"] == true);
        data["stickied"] = json!(data["stickied"] == true || telemetry["post"]["pinned"] == true);
    }
    media(element, &mut data);
    super::super::parse::post(data).map(|(post, _)| Some(post))
}

fn find<'a>(element: ElementRef<'a>, selector: &str) -> Option<ElementRef<'a>> {
    element
        .select(&Selector::parse(selector).expect("static selector"))
        .next()
}

fn media(element: ElementRef<'_>, data: &mut Value) {
    if let Some(gallery) = find(element, "gallery-carousel") {
        let originals = Selector::parse("zoomable-img img").expect("static selector");
        let previews = Selector::parse("img[width][height]:not([role=presentation])")
            .expect("static selector");
        let previews: Vec<_> = gallery.select(&previews).collect();
        let mut items = Vec::new();
        for (index, original) in gallery.select(&originals).enumerate() {
            let key = index.to_string();
            let preview = previews.get(index).copied();
            data["media_metadata"][&key] = json!({
                "s": { "u": original.value().attr("src"), "x": preview.and_then(|e|number(e,"width")), "y": preview.and_then(|e|number(e,"height")) },
                "p": preview.map(image_previews).unwrap_or_default(),
            });
            // The model's gallery parser expects Reddit's x/y/u names.
            if let Some(candidates) = data["media_metadata"][&key]["p"].as_array_mut() {
                for candidate in candidates {
                    *candidate = json!({"u":candidate["url"],"x":candidate["width"],"y":candidate["height"]});
                }
            }
            items.push(json!({"media_id":key}));
        }
        data["gallery_data"] = json!({"items":items});
    }
    if let Some(player) = find(element, "shreddit-player") {
        data["media"] = json!({"reddit_video":{
            "hls_url": player.value().attr("src"), "is_gif":player.value().attr("gif").is_some()
        }});
        let size = player
            .value()
            .attr("packaged-media-json")
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .and_then(|v| {
                v["playbackMp4s"]["permutations"]
                    .as_array()?
                    .last()
                    .cloned()
            })
            .map(|v| v["source"]["dimensions"].clone())
            .unwrap_or_default();
        data["preview"] = json!({"images":[{"source":{
            "url":player.value().attr("poster"),"width":size["width"],"height":size["height"]
        }}]});
        return;
    }
    let preview = find(element, "img[data-post-media-primary]")
        .or_else(|| {
            find(
                element,
                "gallery-carousel img[width][height]:not([role=presentation])",
            )
        })
        .or_else(|| find(element, "[slot=thumbnail] img"));
    if let Some(image) = preview {
        // Native image width/height describe its preview, not necessarily the original.
        // Leave original dimensions unknown so the shared parser chooses a bounded preview.
        data["preview"] = json!({"images":[{"source":{"url":image.value().attr("src")},"resolutions":image_previews(image)}]});
    }
    if element.value().attr("post-type") == Some("image") {
        data["post_hint"] = json!("image");
    }
}

fn number(element: ElementRef<'_>, key: &str) -> Option<u32> {
    element.value().attr(key)?.parse().ok().filter(|n| *n > 0)
}

fn image_previews(image: ElementRef<'_>) -> Vec<Value> {
    let attr = |name| image.value().attr(name);
    let size = number(image, "width").zip(number(image, "height"));
    let Some((width, height)) = size else {
        return Vec::new();
    };
    let mut candidates: Vec<_> = attr("srcset")
        .or_else(|| attr("data-lazy-srcset"))
        .unwrap_or_default()
        .split(',')
        .filter_map(|entry| {
            let (url, descriptor) = entry.trim().rsplit_once(char::is_whitespace)?;
            let w: u32 = descriptor.strip_suffix('w')?.parse().ok()?;
            let h = u64::from(w)
                .checked_mul(u64::from(height))?
                .checked_div(u64::from(width))?;
            Some(json!({"url":url.trim(),"width":w,"height":h}))
        })
        .collect();
    if let Some(url) = attr("src").or_else(|| attr("data-lazy-src")) {
        let actual_width = url::Url::parse(url)
            .ok()
            .and_then(|u| {
                u.query_pairs()
                    .find(|(k, _)| k == "width")
                    .and_then(|(_, v)| v.parse::<u32>().ok())
            })
            .unwrap_or(width);
        candidates.push(json!({"url":url,"width":actual_width,"height":u64::from(actual_width)*u64::from(height)/u64::from(width)}));
    }
    candidates
}
