use super::*;

pub(super) fn media_url(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|s| url::Url::parse(s).is_ok_and(|u| super::super::transport::media_allowed(&u)))
        .map(str::to_string)
}

pub(super) fn media_source(value: &Value) -> &Value {
    value["crosspost_parent_list"]
        .as_array()
        .and_then(|p| p.first())
        .unwrap_or(value)
}

pub(super) fn dimensions(value: &Value, width: &str, height: &str) -> Option<(u32, u32)> {
    let width = u32::try_from(value[width].as_u64()?).ok()?;
    let height = u32::try_from(value[height].as_u64()?).ok()?;
    (width > 0 && height > 0).then_some((width, height))
}

pub(super) fn bounded_preview(
    source: &Value,
    previews: &Value,
    keys: (&str, &str, &str),
) -> Option<String> {
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

pub(super) fn preview_size(value: &Value, media: &Media) -> Option<(u32, u32)> {
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
                == Some(url.as_ref())
            {
                return dimensions(image, "x", "y");
            }
            if let Some(previews) = source["media_metadata"][id]["p"].as_array() {
                for preview in previews {
                    if media_url(&preview["u"]).as_deref() == Some(url.as_ref()) {
                        return dimensions(preview, "x", "y");
                    }
                }
            }
        }
    }
    let preview = &source["preview"]["images"][0]["source"];
    if let Some(previews) = source["preview"]["images"][0]["resolutions"].as_array() {
        for preview in previews {
            if media_url(&preview["url"]).as_deref() == Some(url.as_ref()) {
                return dimensions(preview, "width", "height");
            }
        }
    }
    if (matches!(media, Media::Image { .. }) || preview["url"].as_str() == Some(url.as_ref()))
        && let Some(size) = dimensions(preview, "width", "height")
    {
        return Some(size);
    }
    (source["thumbnail"].as_str() == Some(url.as_ref()))
        .then(|| dimensions(source, "thumbnail_width", "thumbnail_height"))
        .flatten()
}

pub(super) fn media(value: &Value) -> Media {
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
        super::super::transport::media_allowed(&u)
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
