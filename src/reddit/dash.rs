use anyhow::{Context, Result, bail};
use scraper::{ElementRef, Html, Selector};
use url::Url;

pub(crate) struct Rendition {
    pub width: u32,
    pub height: u32,
    pub bandwidth: u64,
    pub url: String,
}

pub(crate) struct Manifest {
    pub video: Vec<Rendition>,
    pub audio: Option<String>,
}

pub(super) fn parse(
    text: &str,
    base: &Url,
    mut local: impl FnMut(&str) -> Result<String>,
) -> Result<Manifest> {
    let document = Html::parse_document(text);
    let selector = |name| Selector::parse(name).expect("static DASH selector");
    let root = document
        .select(&selector("mpd"))
        .next()
        .context("missing DASH manifest")?;
    if root.value().attr("type") == Some("dynamic")
        || root.select(&selector("period")).count() != 1
        || root
            .select(&selector("contentprotection, segmenttemplate, segmentlist"))
            .next()
            .is_some()
    {
        bail!("unsupported DASH manifest");
    }
    let mut video = Vec::new();
    let mut audio = None;
    let mut audio_bandwidth = 0;
    for (index, representation) in root.select(&selector("representation")).enumerate() {
        if index >= 64 {
            bail!("too many DASH renditions");
        }
        let mime = representation.value().attr("mimetype").unwrap_or("");
        if !matches!(mime, "video/mp4" | "audio/mp4") {
            continue;
        }
        let file = representation
            .select(&selector("baseurl"))
            .find(|file| {
                file.ancestors()
                    .filter_map(ElementRef::wrap)
                    .find(|element| element.value().name() == "representation")
                    .is_some_and(|parent| parent.id() == representation.id())
            })
            .context("DASH rendition has no whole file")?
            .text()
            .collect::<String>();
        let url = base.join(file.trim()).context("invalid DASH file URL")?;
        if !url.path().ends_with(".mp4") {
            bail!("DASH rendition is not a whole MP4 file");
        }
        let url = local(url.as_str())?;
        let number = |name| {
            representation
                .value()
                .attr(name)
                .and_then(|v| v.parse().ok())
        };
        let bandwidth: u64 = number("bandwidth").unwrap_or(0);
        if mime == "audio/mp4" {
            if audio.is_none() || bandwidth > audio_bandwidth {
                audio = Some(url);
                audio_bandwidth = bandwidth;
            }
        } else {
            let width = number("width")
                .and_then(|v| u32::try_from(v).ok())
                .unwrap_or(0);
            let height = number("height")
                .and_then(|v| u32::try_from(v).ok())
                .unwrap_or(0);
            if width > 0 && height > 0 {
                video.push(Rendition {
                    width,
                    height,
                    bandwidth,
                    url,
                });
            }
        }
    }
    if video.is_empty() {
        bail!("DASH manifest has no video files");
    }
    Ok(Manifest { video, audio })
}

#[cfg(test)]
mod tests {
    use super::super::tests::{Fixture, Reply};

    #[test]
    fn cancelled_video_recovery_should_not_start_a_manifest_request() {
        let fixture = Fixture::new(|_| panic!("cancelled recovery reached HTTP"));
        let reddit = fixture.reddit();
        assert!(
            reddit
                .video_fallback(&format!("{}HLSPlaylist.m3u8", fixture.base), || true)
                .is_err()
        );
    }

    #[test]
    fn unsupported_or_unsafe_dash_should_not_supply_decoder_urls() {
        let original = include_str!("../../tests/fixtures/reddit-video-dash.mpd");
        for manifest in [
            original.replace("CMAF_360.mp4", "https://evil.test/private.mp4"),
            original.replace("type=\"static\"", "type=\"dynamic\""),
            original.replace("<Period ", "<ContentProtection/><Period "),
            original.replace("<Period ", "<SegmentTemplate/><Period "),
            original.replace("<Period ", "<SegmentList/><Period "),
        ] {
            let fixture = Fixture::new(move |_| Reply {
                status: 200,
                body: manifest.clone(),
                headers: vec![],
            });
            let reddit = fixture.reddit();
            assert!(
                reddit
                    .video_fallback(&format!("{}HLSPlaylist.m3u8", fixture.base), || false)
                    .is_err()
            );
        }
    }
}
