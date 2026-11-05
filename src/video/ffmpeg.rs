use super::*;

pub fn first_frame_preview(
    url: &str,
    cancelled: impl Fn() -> bool,
) -> Result<image::DynamicImage, String> {
    let decoder = Decoder::spawn(url, 0.0, (640, 640), &[]).map_err(|error| error.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if cancelled() {
            return Err("Video preview no longer visible".into());
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err("Video preview timed out".into());
        }
        match decoder
            .frames
            .recv_timeout(left.min(Duration::from_millis(50)))
        {
            Ok(decoded) => {
                return match decoded.frame.pixels {
                    Pixels::Raw(image) => Ok((*image).clone()),
                    Pixels::Png(bytes) => {
                        image::load_from_memory(&bytes).map_err(|error| error.to_string())
                    }
                };
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(error) => return Err(format!("Video preview unavailable: {error}")),
        }
    }
}

/// ffmpeg's options up to and including the input.
///
/// A plain GIF is read as a stream: when it can seek, ffmpeg's GIF reader first scans
/// the whole file for its duration and then reads it again from the start, which for a
/// 76 MB GIF took 16 s before the first frame (0.6 s streamed). Its start is reached by
/// decoding past earlier frames (`-ss` after `-i`), not by seeking the input; it shows
/// no total time. A stream cannot reconnect where it stopped (ffmpeg would start the
/// file over mid-GIF), so a dropped connection ends the run with an error and the
/// player goes on from there. Other files keep seeking and reconnect after a long
/// pause instead of ending early: a non-seekable MP4 whose index sits at the end could
/// not play at all.
pub(super) fn input_args(url: &str, start: f64) -> Vec<String> {
    let start = format!("{start:.3}");
    let http = url.starts_with("http://") || url.starts_with("https://");
    let mut args: Vec<String> = Vec::new();
    if http {
        args.extend(["-rw_timeout".into(), "10000000".into()]);
    }
    if plain_gif(url) {
        if http {
            args.extend(["-seekable".into(), "0".into()]);
        }
        args.extend(["-i".into(), url.into()]);
        if start != "0.000" {
            args.extend(["-ss".into(), start]);
        }
    } else {
        // Extracted YouTube files already identify the video track. Avoid reading
        // seconds of compressed frames just to estimate its frame rate again.
        // googlevideo paces one open-ended response near playback speed; ranged
        // requests arrive at full speed, so a seek's preroll doesn't stall playback.
        if url::Url::parse(url).is_ok_and(|url| {
            url.scheme() == "https"
                && url.host_str().is_some_and(|host| {
                    host == "googlevideo.com" || host.ends_with(".googlevideo.com")
                })
        }) {
            args.extend(["-probesize".into(), "32768".into()]);
            if *RANGED_REQUESTS {
                args.extend(
                    ["-request_size", "1048576", "-multiple_requests", "1"].map(String::from),
                );
            }
        }
        if http {
            args.extend(["-reconnect".into(), "1".into()]);
        }
        args.extend(["-ss".into(), start, "-i".into(), url.into()]);
    }
    args
}

/// Whether ffmpeg's fps filter takes an expression of the source's rate (FFmpeg 5.0+).
/// Older builds take only a fixed rate.
pub(super) static FPS_EXPRESSIONS: LazyLock<bool> = LazyLock::new(|| {
    crate::media::local_command("ffmpeg")
        .args(["-hide_banner", "-h", "filter=fps"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .is_ok_and(|out| supports_fps_expressions(&String::from_utf8_lossy(&out.stdout)))
});

pub(super) fn supports_fps_expressions(fps_help: &str) -> bool {
    fps_help.lines().any(|line| {
        line.trim_start()
            .strip_prefix("fps ")
            .is_some_and(|rest| rest.trim_start().starts_with("<string>"))
    })
}

/// Whether ffmpeg's HTTP reader can split a file into ranged requests (FFmpeg 8.1+).
/// Older builds reject the option and would fail to open the input.
pub(super) static RANGED_REQUESTS: LazyLock<bool> = LazyLock::new(|| {
    crate::media::local_command("ffmpeg")
        .args(["-hide_banner", "-h", "protocol=http"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .is_ok_and(|out| supports_ranged(&String::from_utf8_lossy(&out.stdout)))
});

pub(super) fn supports_ranged(http_help: &str) -> bool {
    http_help
        .lines()
        .any(|line| line.trim_start().starts_with("-request_size "))
}

pub(super) fn header_args(headers: &[(String, String)]) -> Vec<String> {
    if headers.is_empty() {
        return Vec::new();
    }
    vec![
        "-headers".into(),
        headers
            .iter()
            .map(|(key, value)| format!("{key}: {value}\r\n"))
            .collect(),
    ]
}

/// A `.gif` file served as itself: Reddit's `.gif?format=mp4` previews are MP4.
pub(crate) fn plain_gif(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    parsed
        .path()
        .rsplit_once('.')
        .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("gif"))
        && !parsed.query_pairs().any(|(key, _)| key == "format")
}

/// A run's first picture, kept until a frame differs from it: then the run moves.
#[derive(Default)]
pub(super) struct FirstFrame {
    pub(super) pixels: Option<Vec<u8>>,
    pub(super) moved: bool,
}

impl FirstFrame {
    /// Whether `rgb` shows the run's first picture; true for the first frame itself.
    pub(super) fn same(&mut self, rgb: &[u8]) -> bool {
        match &self.pixels {
            _ if self.moved => false,
            None => true,
            Some(first) if first.as_slice() == rgb => true,
            Some(_) => {
                self.moved = true;
                self.pixels = None;
                false
            }
        }
    }

    /// The first frame's pixels are still wanted.
    pub(super) fn wants(&self) -> bool {
        !self.moved && self.pixels.is_none()
    }

    /// Keep the first frame's pixels; later frames are ignored.
    pub(super) fn keep(&mut self, rgb: Vec<u8>) {
        if self.wants() {
            self.pixels = Some(rgb);
        }
    }
}

/// Full document geometry, independent of its visible slice. The pixel budget in
/// the decoder filter bounds memory even for tall reservations.
pub(super) fn frame_box(area: Size, font: FontSize) -> (u32, u32) {
    (
        (u32::from(area.width) * u32::from(font.width)).clamp(2, MAX_FRAME_WIDTH),
        (u32::from(area.height) * u32::from(font.height)).max(2),
    )
}

/// Preserve source detail up to the displayed size and shared pixel budget, at the
/// source's own rate within MIN_FPS..=MAX_FPS: resampling 30 fps to another rate drops
/// or repeats frames unevenly, which shows as judder.
/// eof_action=pass retains one-frame GIFs shorter than one output frame.
pub(super) fn video_filter((w, h): (u32, u32), expressions: bool) -> String {
    let scale = format!("min(1,min(min({w}/iw,{h}/ih),sqrt({MAX_VIDEO_PIXELS}/(iw*ih))))");
    let fps = if expressions {
        format!("'if(gt(source_fps,0),clip(source_fps,{MIN_FPS},{MAX_FPS}),{MIN_FPS})'")
    } else {
        MIN_FPS.to_string()
    };
    format!(
        "fps={fps}:eof_action=pass,scale=w='max(2,trunc(iw*{scale}))':h='max(2,trunc(ih*{scale}))':flags=bilinear"
    )
}

pub(super) fn prepare_playback(
    resolve: impl FnOnce() -> Result<Sources, String>,
    prepare: impl FnOnce() + Send,
) -> Result<Sources, String> {
    std::thread::scope(|scope| {
        let preparation = scope.spawn(prepare);
        let result = resolve();
        preparation.join().unwrap();
        result
    })
}
