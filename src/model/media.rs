use super::*;

pub const PREVIEW_EDGE: u32 = 1600;

pub const PREVIEW_PIXELS: u64 = 16_000_000;

/// Post media. URLs are absolute upstream URLs; temporary media-relay URLs are never persisted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Media {
    #[default]
    None,
    Image {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        preview: Option<String>,
    },
    Video {
        /// File or HLS playlist; `None` when Reddit has no playable source.
        url: Option<String>,
        poster: Option<String>,
        #[serde(default)]
        gif: bool,
    },
    Gallery {
        images: Vec<String>,
        /// Display sources by original index; absent entries use the original.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        previews: Vec<Option<String>>,
        /// Listing thumbnail, also used when gallery entries are unavailable.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thumb: Option<String>,
    },
    Link {
        url: String,
        thumb: Option<String>,
    },
}

impl Media {
    pub fn youtube_url(&self) -> Option<&str> {
        match self {
            Self::Link { url, .. } if youtube_id(url).is_some() => Some(url),
            _ => None,
        }
    }

    pub fn tag(&self) -> Option<String> {
        if self.youtube_url().is_some() {
            return Some("YouTube".into());
        }
        match self {
            Media::None => None,
            Media::Image { .. } if self.video_url().is_some() => Some("gif".into()),
            Media::Image { .. } => Some("image".into()),
            Media::Video { gif: true, .. } => Some("gif".into()),
            Media::Video { .. } => Some("video".into()),
            Media::Gallery { images, .. } if images.is_empty() => Some("gallery".into()),
            Media::Gallery { images, .. } => Some(format!("gallery {}", images.len())),
            Media::Link { url, .. } => Some(domain_of(url).unwrap_or_else(|| "link".into())),
        }
    }

    /// Playback identity, including GIFs and YouTube page URLs resolved by the player.
    pub fn video_url(&self) -> Option<&str> {
        match self {
            Media::Video { url, .. } => url.as_deref(),
            Media::Image { url, .. } if is_gif(url) => Some(url),
            Media::Link { .. } => self.youtube_url(),
            _ => None,
        }
    }

    /// Inline still or video poster. Ordinary link previews stay list thumbnails.
    pub fn preview_image(&self) -> Option<Cow<'_, str>> {
        self.preview_image_at(0)
    }

    pub fn preview_image_at(&self, index: usize) -> Option<Cow<'_, str>> {
        let source = match self {
            Media::Image { url, preview } => Some(preview.as_deref().unwrap_or(url)),
            Media::Video { poster, url, .. } => poster.as_deref().or(url.as_deref()),
            Media::Gallery {
                images, previews, ..
            } => {
                let index = index % images.len().max(1);
                images.get(index).map(|original| {
                    previews
                        .get(index)
                        .and_then(Option::as_deref)
                        .unwrap_or(original)
                })
            }
            Media::Link { url, thumb } if self.youtube_url().is_some() => {
                if thumb.is_none() {
                    return youtube_id(url).map(|id| {
                        Cow::Owned(format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg"))
                    });
                }
                thumb.as_deref()
            }
            Media::Link { .. } | Media::None => None,
        };
        source.map(Cow::Borrowed)
    }

    /// Whether the post view shows an image or plays a video under the header.
    pub fn shows_inline(&self) -> bool {
        self.preview_image().is_some() || self.video_url().is_some()
    }

    /// GIF image posts list their first frame (the fetch stops after it).
    pub fn list_thumb(&self) -> Option<Cow<'_, str>> {
        match self {
            Media::Gallery { thumb, .. } => thumb
                .as_deref()
                .map(Cow::Borrowed)
                .or_else(|| self.preview_image()),
            Media::Image { url, preview } => Some(Cow::Borrowed(preview.as_deref().unwrap_or(url))),
            Media::Link { .. } if self.youtube_url().is_some() => self.preview_image(),
            Media::Link { thumb, .. } => thumb.as_deref().map(Cow::Borrowed),
            _ => self.preview_image(),
        }
    }
}

pub(crate) fn youtube_id(url: &str) -> Option<String> {
    if url.len() > 8192 {
        return None;
    }
    let url = url::Url::parse(url).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return None;
    }
    let path: Vec<_> = url
        .path()
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    let id = match (url.host_str()?, path.as_slice()) {
        ("youtu.be" | "www.youtu.be", [id]) => (*id).to_owned(),
        ("youtube.com" | "www.youtube.com" | "m.youtube.com" | "music.youtube.com", ["watch"]) => {
            let mut ids = url.query_pairs().filter(|(key, _)| key == "v");
            let id = ids.next()?.1.into_owned();
            if ids.next().is_some() {
                return None;
            }
            id
        }
        ("youtube.com" | "www.youtube.com" | "m.youtube.com", ["shorts" | "embed", id])
        | ("www.youtube-nocookie.com" | "youtube-nocookie.com", ["embed", id]) => (*id).to_owned(),
        _ => return None,
    };
    (id.len() == 11
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')))
    .then_some(id)
}

/// A `.gif` path, whatever its query or fragment.
pub(super) fn is_gif(url: &str) -> bool {
    url.split(['?', '#']).next().is_some_and(|path| {
        path.rsplit_once('.')
            .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("gif"))
    })
}

pub fn domain_of(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    Some(host.trim_start_matches("www.").to_string())
}
