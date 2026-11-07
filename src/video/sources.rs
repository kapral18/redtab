use super::*;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Sources {
    pub(super) video: String,
    pub(super) audio: Option<String>,
    /// `audio` is the video's own file; whether it has sound shows once ffmpeg reads it.
    pub(super) audio_in_file: bool,
    pub(super) headers: Vec<(String, String)>,
    pub(super) audio_headers: Vec<(String, String)>,
}

impl Sources {
    pub(super) fn file(url: &str) -> Self {
        Self {
            video: url.to_string(),
            audio: Some(url.to_string()),
            audio_in_file: true,
            headers: Vec::new(),
            audio_headers: Vec::new(),
        }
    }
}

/// Files to decode for `url`. Reddit's HLS playlists cut renditions from one fragmented
/// MP4 by byte range and ffmpeg's HLS reader stops after the first range, so play whole
/// MP4s: the rendition that fills `frame_box` without upscaling (else the largest) and its sound.
/// `tall` caps a frame not wider than tall, `pixels` the decoded frame (kitty): a rendition
/// need only cover what is decoded.
pub(super) fn resolve(
    url: &str,
    frame_box: (u32, u32),
    tall: u32,
    pixels: Option<f64>,
    fallback: impl FnOnce() -> Result<crate::reddit::VideoManifest, String>,
) -> Result<Sources, String> {
    let base = url::Url::parse(url).map_err(|e| format!("bad video URL: {e}"))?;
    if !base.path().ends_with(".m3u8") {
        return Ok(Sources::file(url));
    }
    #[cfg(not(test))]
    let agent: ureq::Agent = ureq::Agent::config_builder()
        // Inputs come from the loopback relay; upstream routing belongs to it.
        .proxy(None)
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .into();
    let fetch = |at: &url::Url| {
        #[cfg(test)]
        return crate::reddit::tests::fetch_text(at.as_str());
        #[cfg(not(test))]
        agent
            .get(at.as_str())
            .call()
            .and_then(|mut r| r.body_mut().with_config().limit(1 << 20).read_to_string())
    };
    let explain = |e| match e {
        ureq::Error::StatusCode(403) => "Reddit denied access to this video (HTTP 403)".into(),
        ureq::Error::StatusCode(status @ (404 | 410)) => {
            format!("Video is no longer available (HTTP {status})")
        }
        _ => format!("video playlist: {e}"),
    };
    let master = match fetch(&base) {
        Ok(master) => master,
        Err(error @ ureq::Error::StatusCode(403)) => {
            if let Ok(manifest) = fallback() {
                let dimensions: Vec<_> = manifest
                    .video
                    .iter()
                    .map(|r| (r.width, r.height, r.bandwidth))
                    .collect();
                if let Some(index) = choose_rendition(&dimensions, frame_box, tall, pixels) {
                    let video = manifest.video.into_iter().nth(index).unwrap().url;
                    return Ok(match manifest.audio {
                        Some(audio) => Sources {
                            video,
                            audio: Some(audio),
                            audio_in_file: false,
                            headers: Vec::new(),
                            audio_headers: Vec::new(),
                        },
                        None => Sources::file(&video),
                    });
                }
            }
            return Err(explain(error));
        }
        Err(error) => return Err(explain(error)),
    };
    let join = |path: &str| {
        base.join(path)
            .map_err(|e| format!("bad playlist entry: {e}"))
    };
    let Some((video, audio)) = pick_rendition(&master, frame_box, tall, pixels) else {
        return Ok(Sources::file(url));
    };
    // A playlist's whole file, or the playlist itself when it has none.
    let file = |playlist: url::Url| -> Result<String, String> {
        let text = fetch(&playlist).map_err(explain)?;
        Ok(match media_file(&text) {
            Some(path) => playlist
                .join(&path)
                .map_err(|e| format!("bad playlist entry: {e}"))?
                .into(),
            None => playlist.into(),
        })
    };
    let video = file(join(&video)?)?;
    Ok(match audio {
        // Sound that fails to resolve leaves a silent picture.
        Some(audio) => Sources {
            video,
            audio: join(&audio).and_then(file).ok(),
            audio_in_file: false,
            headers: Vec::new(),
            audio_headers: Vec::new(),
        },
        None => Sources::file(&video),
    })
}

/// From a master playlist, the video and sound playlists: the smallest rendition that
/// needs no upscaling to fit `frame_box` (`tall` high when not wider than tall; within
/// `pixels`, with `RENDITION_HEADROOM`, when frames are capped), else the largest; ties go
/// to the higher bandwidth (on Reddit, the richer sound).
pub(super) fn pick_rendition(
    master: &str,
    frame_box: (u32, u32),
    tall: u32,
    pixels: Option<f64>,
) -> Option<(String, Option<String>)> {
    let attr = |line: &str, key: &str| -> Option<String> {
        let at = line.find(&format!("{key}="))? + key.len() + 1;
        let rest = &line[at..];
        Some(match rest.strip_prefix('"') {
            Some(quoted) => quoted.split('"').next()?.to_string(),
            None => rest.split(',').next()?.to_string(),
        })
    };
    let mut audio_groups = Vec::new();
    let mut renditions = Vec::new();
    let mut lines = master.lines().map(str::trim);
    while let Some(line) = lines.next() {
        if line.starts_with("#EXT-X-MEDIA:") && attr(line, "TYPE").as_deref() == Some("AUDIO") {
            if let (Some(group), Some(uri)) = (attr(line, "GROUP-ID"), attr(line, "URI")) {
                audio_groups.push((group, uri));
            }
        } else if line.starts_with("#EXT-X-STREAM-INF:") {
            let (w, h) = attr(line, "RESOLUTION")
                .and_then(|r| {
                    let (w, h) = r.split_once('x')?;
                    Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?))
                })
                .unwrap_or((0, 0));
            let bandwidth = attr(line, "BANDWIDTH")
                .and_then(|b| b.parse::<u64>().ok())
                .unwrap_or(0);
            let group = attr(line, "AUDIO");
            if let Some(uri) = lines.find(|l| !l.is_empty() && !l.starts_with('#')) {
                renditions.push((w, h, bandwidth, uri.to_string(), group));
            }
        }
    }
    let dimensions: Vec<_> = renditions.iter().map(|r| (r.0, r.1, r.2)).collect();
    let chosen = &renditions[choose_rendition(&dimensions, frame_box, tall, pixels)?];
    let audio = chosen.4.as_ref().and_then(|group| {
        audio_groups
            .iter()
            .find(|(g, _)| g == group)
            .map(|(_, uri)| uri.clone())
    });
    Some((chosen.3.clone(), audio))
}

pub(super) fn choose_rendition(
    renditions: &[(u32, u32, u64)],
    frame_box: (u32, u32),
    tall: u32,
    pixels: Option<f64>,
) -> Option<usize> {
    let covers = |w: u32, h: u32| {
        let (w, h) = (f64::from(w), f64::from(h));
        let high = if w > h { frame_box.1 } else { tall };
        let fit = (f64::from(frame_box.0) / w).min(f64::from(high) / h);
        let (scale, most) = match pixels {
            Some(pixels) => (fit.min((pixels / (w * h)).sqrt()), RENDITION_HEADROOM),
            None => (fit, 1.0),
        };
        w > 0.0 && h > 0.0 && scale <= most
    };
    let size = |r: &(u32, u32, u64)| (u64::from(r.0) * u64::from(r.1), r.2);
    renditions
        .iter()
        .enumerate()
        .filter(|(_, r)| covers(r.0, r.1))
        .min_by_key(|(_, r)| (size(r).0, std::cmp::Reverse(r.2)))
        .or_else(|| renditions.iter().enumerate().max_by_key(|(_, r)| size(r)))
        .map(|(index, _)| index)
}

/// The file a media playlist's segments are cut from (`#EXT-X-MAP`), when every
/// segment is a byte range of that one file.
pub(super) fn media_file(playlist: &str) -> Option<String> {
    let mut map = None;
    for line in playlist.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("#EXT-X-MAP:") {
            let uri = rest.split("URI=\"").nth(1)?.split('"').next()?;
            map = Some(uri.to_string());
        } else if !line.is_empty() && !line.starts_with('#') && Some(line) != map.as_deref() {
            // A segment in its own file: the playlist is the stream.
            return None;
        }
    }
    map
}
