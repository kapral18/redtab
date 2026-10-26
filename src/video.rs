//! Inline video: ffmpeg decodes raw frames for the scrolling tile renderer. One
//! player owns playback; ffplay plays optional sound in its own process.

mod youtube;

use std::io::{self, BufRead, BufReader};
use std::process::{Child, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, sync_channel};
use std::time::{Duration, Instant};

use base64::Engine;
use parking_lot::Mutex;
use ratatui::buffer::{Buffer, CellDiffOption};
use ratatui::layout::{Position, Rect, Size};
use ratatui::style::Color;
use ratatui_image::FontSize;

const FPS: f64 = 24.0;
/// Decode at most this width; the pixel budget bounds tall frames.
const MAX_FRAME_WIDTH: u32 = 1280;
/// Pixel budget shared by decoder and tiled/native output; portrait frames receive
/// the same source detail as landscape frames.
pub(crate) const MAX_VIDEO_PIXELS: f64 = 921_600.0;
/// A kitty frame comes from a rendition at least a third larger than it, so the stream's
/// own compression shows less in the picture the terminal enlarges.
const RENDITION_HEADROOM: f64 = 0.75;
/// Frames decoded ahead of the one on screen.
const AHEAD: usize = 3;
/// A frame this late means a stall: resume from it instead of racing through late frames.
const STALL: f64 = 0.25;
/// How far `,` and `.` seek, in seconds.
pub const SEEK_STEP: f64 = 5.0;

pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub pixels: Pixels,
    /// The same picture as its run's first frame (true for that frame itself).
    pub same_as_first: bool,
}

pub enum Pixels {
    /// Lossless PNG, ready for the kitty protocol.
    Png(Vec<u8>),
    /// Unencoded frames use scrolling tiles on every terminal protocol.
    Raw(Arc<image::DynamicImage>),
}

/// What ffmpeg's log said about the stream.
#[derive(Default)]
struct Log {
    duration: Option<f64>,
    audio: bool,
    /// The stream mapping was logged, so `audio` is final; a frame may arrive before it.
    streams_read: bool,
    error: Option<String>,
    /// The input failed (connection or demuxing), not just a frame: the file was cut.
    input_error: bool,
    /// ffmpeg closed its log: everything it will say about this run is above.
    ended: bool,
}

/// One ffmpeg run, from a start position to the end of the stream.
struct Decoder {
    /// None for a decoder fed by a test.
    child: Option<Child>,
    frames: Receiver<Frame>,
    log: Arc<Mutex<Log>>,
}

impl Decoder {
    fn spawn(
        url: &str,
        start: f64,
        frame_box: (u32, u32),
        headers: &[(String, String)],
    ) -> io::Result<Self> {
        let filter = video_filter(frame_box);
        let mut child = crate::media::local_command("ffmpeg")
            .args([
                "-nostdin",
                "-hide_banner",
                "-nostats",
                "-loglevel",
                "level+info",
            ])
            .args(header_args(headers))
            .args(input_args(url, start))
            .args(["-an", "-sn", "-dn", "-vf", &filter])
            // 8-bit RGB even for 10-bit sources.
            .args(["-pix_fmt", "rgb24", "-f", "image2pipe", "-c:v", "ppm", "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let log = Arc::new(Mutex::new(Log::default()));
        let sink = Arc::clone(&log);
        std::thread::spawn(move || read_log(BufReader::new(stderr), &sink));
        let (tx, frames) = sync_channel(AHEAD);
        std::thread::spawn(move || {
            let mut out = BufReader::with_capacity(1 << 20, stdout);
            let mut first = FirstFrame::default();
            while let Ok(Some((width, height, rgb))) = read_ppm(&mut out) {
                let same_as_first = first.same(&rgb);
                if first.wants() {
                    first.keep(rgb.clone());
                }
                let Some(img) = image::RgbImage::from_raw(width, height, rgb) else {
                    return;
                };
                let pixels = Pixels::Raw(Arc::new(image::DynamicImage::ImageRgb8(img)));
                let frame = Frame {
                    width,
                    height,
                    pixels,
                    same_as_first,
                };
                if tx.send(frame).is_err() {
                    return;
                }
            }
        });
        Ok(Self {
            child: Some(child),
            frames,
            log,
        })
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
fn input_args(url: &str, start: f64) -> Vec<String> {
    let start = format!("{start:.3}");
    let http = url.starts_with("http://") || url.starts_with("https://");
    let mut args: Vec<String> = Vec::new();
    if plain_gif(url) {
        if http {
            args.extend(["-seekable".into(), "0".into()]);
        }
        args.extend(["-i".into(), url.into()]);
        if start != "0.000" {
            args.extend(["-ss".into(), start]);
        }
    } else {
        if http {
            args.extend(["-reconnect".into(), "1".into()]);
        }
        args.extend(["-ss".into(), start, "-i".into(), url.into()]);
    }
    args
}

fn header_args(headers: &[(String, String)]) -> Vec<String> {
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
fn plain_gif(url: &str) -> bool {
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
struct FirstFrame {
    pixels: Option<Vec<u8>>,
    moved: bool,
}

impl FirstFrame {
    /// Whether `rgb` shows the run's first picture; true for the first frame itself.
    fn same(&mut self, rgb: &[u8]) -> bool {
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
    fn wants(&self) -> bool {
        !self.moved && self.pixels.is_none()
    }

    /// Keep the first frame's pixels; later frames are ignored.
    fn keep(&mut self, rgb: Vec<u8>) {
        if self.wants() {
            self.pixels = Some(rgb);
        }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// `Duration: 00:01:02.50,` in ffmpeg's log, in seconds.
fn parse_duration(line: &str) -> Option<f64> {
    let rest = line.split_once("Duration: ")?.1;
    let stamp = rest.split(',').next()?.trim();
    let mut parts = stamp.split(':');
    let (h, m, s) = (parts.next()?, parts.next()?, parts.next()?);
    let seconds =
        h.parse::<f64>().ok()? * 3600.0 + m.parse::<f64>().ok()? * 60.0 + s.parse::<f64>().ok()?;
    (seconds > 0.0).then_some(seconds)
}

/// Read ffmpeg's log until it closes, noting what it says about the stream.
fn read_log(stderr: impl BufRead, log: &Mutex<Log>) {
    // Split on bytes: invalid UTF-8 must not stop the read, or ffmpeg blocks on a full pipe.
    for line in stderr.split(b'\n') {
        let Ok(line) = line else { break };
        let line = String::from_utf8_lossy(&line);
        let mut log = log.lock();
        if log.duration.is_none() {
            log.duration = parse_duration(&line);
        }
        if line.contains("Stream #0:") && line.contains(": Audio: ") {
            log.audio = true;
        }
        if line.contains("Stream mapping:") {
            log.streams_read = true;
        }
        // `[http @ 0x7f] [error] …`, or nested: `[vist#0:0/gif @ 0x1] [dec:gif @ 0x2] [error] …`.
        let mut message: &str = &line;
        let mut input = false;
        while let Some((component, rest)) = message
            .strip_prefix('[')
            .and_then(|rest| rest.split_once("] "))
            .filter(|(component, _)| component.contains(" @ "))
        {
            let name = component.split(" @ ").next().unwrap_or_default();
            input |= matches!(name, "http" | "https" | "tls" | "tcp") || name.starts_with("in#");
            message = rest;
        }
        if let Some(text) = message
            .strip_prefix("[error] ")
            .or_else(|| message.strip_prefix("[fatal] "))
        {
            log.error = Some(text.trim().to_string());
            log.input_error |= input;
        }
    }
    log.lock().ended = true;
}

/// The next binary PPM (`P6`, 8-bit) frame: width, height and RGB bytes. None at
/// the end of the stream between frames; an error for a cut or malformed frame.
fn read_ppm(r: &mut impl BufRead) -> io::Result<Option<(u32, u32, Vec<u8>)>> {
    if r.fill_buf()?.is_empty() {
        return Ok(None);
    }
    let mut magic = [0; 2];
    r.read_exact(&mut magic)?;
    if &magic != b"P6" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a PPM frame",
        ));
    }
    let width = ppm_number(r)?;
    let height = ppm_number(r)?;
    let max = ppm_number(r)?;
    if width == 0 || height == 0 || max != 255 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported PPM frame",
        ));
    }
    let len = width as usize * height as usize * 3;
    let mut rgb = vec![0; len];
    r.read_exact(&mut rgb)?;
    Ok(Some((width, height, rgb)))
}

/// A header number: skips the whitespace before it and eats the one byte after it.
fn ppm_number(r: &mut impl BufRead) -> io::Result<u32> {
    let mut byte = [0; 1];
    loop {
        r.read_exact(&mut byte)?;
        if !byte[0].is_ascii_whitespace() {
            break;
        }
    }
    let mut value: u32 = 0;
    loop {
        if !byte[0].is_ascii_digit() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "bad PPM header"));
        }
        value = value
            .checked_mul(10)
            .and_then(|v| v.checked_add(u32::from(byte[0] - b'0')))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bad PPM header"))?;
        r.read_exact(&mut byte)?;
        if byte[0].is_ascii_whitespace() {
            return Ok(value);
        }
    }
}

/// Media time against wall time.
#[derive(Debug, Clone, Copy)]
struct Clock {
    /// Media time at `anchor`.
    at: f64,
    anchor: Instant,
    running: bool,
}

impl Clock {
    fn new(at: f64, now: Instant) -> Self {
        Self {
            at,
            anchor: now,
            running: true,
        }
    }

    fn position(&self, now: Instant) -> f64 {
        if self.running {
            self.at + now.saturating_duration_since(self.anchor).as_secs_f64()
        } else {
            self.at
        }
    }

    fn set(&mut self, at: f64, now: Instant) {
        self.at = at;
        self.anchor = now;
    }

    fn stop(&mut self, now: Instant) {
        self.set(self.position(now), now);
        self.running = false;
    }

    fn start(&mut self, now: Instant) {
        self.anchor = now;
        self.running = true;
    }

    /// When media time `at` comes, if the clock runs.
    fn when(&self, at: f64, now: Instant) -> Option<Instant> {
        let ahead = at - self.position(now);
        self.running
            .then(|| now + Duration::from_secs_f64(ahead.max(0.0)))
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Sources {
    video: String,
    audio: Option<String>,
    /// `audio` is the video's own file; whether it has sound shows once ffmpeg reads it.
    audio_in_file: bool,
    headers: Vec<(String, String)>,
    audio_headers: Vec<(String, String)>,
}

impl Sources {
    fn file(url: &str) -> Self {
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
fn resolve(
    url: &str,
    frame_box: (u32, u32),
    tall: u32,
    pixels: Option<f64>,
) -> Result<Sources, String> {
    let base = url::Url::parse(url).map_err(|e| format!("bad video URL: {e}"))?;
    if !base.path().ends_with(".m3u8") {
        return Ok(Sources::file(url));
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        // Inputs come from the loopback relay; upstream routing belongs to it.
        .proxy(None)
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .into();
    let fetch = |at: &url::Url| -> Result<String, String> {
        agent
            .get(at.as_str())
            .call()
            .and_then(|mut r| r.body_mut().with_config().limit(1 << 20).read_to_string())
            .map_err(|e| format!("video playlist: {e}"))
    };
    let join = |path: &str| {
        base.join(path)
            .map_err(|e| format!("bad playlist entry: {e}"))
    };
    let master = fetch(&base)?;
    let Some((video, audio)) = pick_rendition(&master, frame_box, tall, pixels) else {
        return Ok(Sources::file(url));
    };
    // A playlist's whole file, or the playlist itself when it has none.
    let file = |playlist: url::Url| -> Result<String, String> {
        let text = fetch(&playlist)?;
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
fn pick_rendition(
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
    // The scale the decoder will apply to it: down or kept, never up.
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
    let size = |r: &(u32, u32, u64, String, Option<String>)| (u64::from(r.0) * u64::from(r.1), r.2);
    let chosen = renditions
        .iter()
        .filter(|r| covers(r.0, r.1))
        .min_by_key(|r| (size(r).0, std::cmp::Reverse(r.2)))
        .or_else(|| renditions.iter().max_by_key(|r| size(r)))?;
    let audio = chosen.4.as_ref().and_then(|group| {
        audio_groups
            .iter()
            .find(|(g, _)| g == group)
            .map(|(_, uri)| uri.clone())
    });
    Some((chosen.3.clone(), audio))
}

/// The file a media playlist's segments are cut from (`#EXT-X-MAP`), when every
/// segment is a byte range of that one file.
fn media_file(playlist: &str) -> Option<String> {
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

/// Starts a decoder run at a position: ffmpeg, or frames a test feeds.
type Spawn = Box<dyn FnMut(&Sources, f64) -> io::Result<Decoder>>;

/// Where a player starts: position, pause and sound.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Resume {
    pub at: f64,
    pub paused: bool,
    pub sound: bool,
}

pub struct Player {
    pub url: String,
    /// The area, in cells, the frames are sized for.
    pub area: Size,
    /// Resolving the stream to its files; `sources` holds the answer.
    resolving: Option<Receiver<Result<Sources, String>>>,
    resolve_cancel: Option<Arc<youtube::Cancellation>>,
    sources: Option<Sources>,
    spawn: Spawn,
    decoder: Option<Decoder>,
    /// Media time of the current run's first frame.
    start: f64,
    shown: u64,
    next: Option<Frame>,
    frame: Option<Frame>,
    /// Counts frames put on screen; the drawing side sends a frame once per number.
    pub frame_number: u64,
    clock: Clock,
    paused: bool,
    hidden: bool,
    /// Stopped after a seek or a paused start: show one frame at the new position.
    show_one: bool,
    /// Every frame of this run so far was the same picture.
    unchanged: bool,
    /// A silent stream that never changed (a still GIF): it stays on screen and
    /// nothing plays.
    still: bool,
    /// No more retries for runs before the next frame: one was used, or a resume just
    /// failed and this run from the start must not retry either.
    retried: bool,
    /// Where resuming after a cut failed even when retried, as when the server always
    /// cuts the file at the same byte: a cut there is treated as the end.
    unresumable: Option<f64>,
    /// Sound starts with this run's first frame, so a slow start can't start it twice.
    sound_waits: bool,
    sound: bool,
    audio: Option<Child>,
    duration: Option<f64>,
    error: Option<String>,
    /// A message for the reader (no sound, no ffplay), and whether it is an error.
    notice: Option<(String, bool)>,
}

/// Full document geometry, independent of its visible slice. The pixel budget in
/// the decoder filter bounds memory even for tall reservations.
fn frame_box(area: Size, font: FontSize) -> (u32, u32) {
    (
        (u32::from(area.width) * u32::from(font.width)).clamp(2, MAX_FRAME_WIDTH),
        (u32::from(area.height) * u32::from(font.height)).max(2),
    )
}

/// Preserve source detail up to the displayed size and shared pixel budget.
/// eof_action=pass retains one-frame GIFs shorter than one output frame.
fn video_filter((w, h): (u32, u32)) -> String {
    let scale = format!("min(1,min(min({w}/iw,{h}/ih),sqrt({MAX_VIDEO_PIXELS}/(iw*ih))))");
    format!(
        "fps={FPS}:eof_action=pass,scale=w='max(2,trunc(iw*{scale}))':h='max(2,trunc(ih*{scale}))':flags=bilinear"
    )
}

impl Player {
    /// Plays `url` (absolute) for an `area` cells big at `font`, from `resume`.
    pub fn start(url: String, area: Size, font: FontSize, resume: Resume, now: Instant) -> Self {
        let frame_box = frame_box(area, font);
        let (tx, rx) = sync_channel(1);
        let target = url.clone();
        let cancelled = Arc::new(youtube::Cancellation::default());
        let cancel = Arc::clone(&cancelled);
        std::thread::spawn(move || {
            let result = if crate::model::youtube_id(&target).is_some() {
                youtube::resolve(&target, &cancel)
            } else {
                resolve(&target, frame_box, frame_box.1, Some(MAX_VIDEO_PIXELS))
            };
            let _ = tx.send(result);
        });
        let spawn: Spawn = Box::new(move |sources: &Sources, at| {
            Decoder::spawn(&sources.video, at, frame_box, &sources.headers)
        });
        let mut player = Self::new(url, area, Some(rx), None, spawn, resume, now);
        player.resolve_cancel = Some(cancelled);
        player
    }

    fn new(
        url: String,
        area: Size,
        resolving: Option<Receiver<Result<Sources, String>>>,
        sources: Option<Sources>,
        spawn: Spawn,
        resume: Resume,
        now: Instant,
    ) -> Self {
        let mut clock = Clock::new(resume.at, now);
        if resume.paused {
            clock.stop(now);
        }
        let mut player = Self {
            url,
            area,
            resolving,
            resolve_cancel: None,
            sources,
            spawn,
            decoder: None,
            start: resume.at,
            shown: 0,
            next: None,
            frame: None,
            frame_number: 0,
            clock,
            paused: resume.paused,
            hidden: false,
            show_one: resume.paused,
            unchanged: true,
            still: false,
            retried: false,
            unresumable: None,
            sound_waits: false,
            sound: resume.sound,
            audio: None,
            duration: None,
            error: None,
            notice: None,
        };
        if player.sources.is_some() {
            player.run_from(resume.at, now);
        }
        player
    }

    fn playing(&self) -> bool {
        !self.paused && !self.hidden && self.error.is_none()
    }

    /// When the next frame of this run is due, in media time.
    fn due(&self) -> f64 {
        self.start + self.shown as f64 / FPS
    }

    /// Start a decoder run (and the sound) at `at` seconds.
    fn run_from(&mut self, at: f64, now: Instant) {
        self.stop_audio();
        self.decoder = None;
        self.next = None;
        self.start = at;
        self.shown = 0;
        self.unchanged = true;
        self.still = false;
        self.clock.set(at, now);
        let Some(sources) = &self.sources else {
            return;
        };
        match (self.spawn)(sources, at) {
            Ok(decoder) => self.decoder = Some(decoder),
            Err(e) => {
                self.error = Some(if e.kind() == io::ErrorKind::NotFound {
                    "ffmpeg is not installed".into()
                } else {
                    format!("ffmpeg: {e}")
                });
                return;
            }
        }
        if self.playing() && self.sound {
            self.play_sound(now);
        }
    }

    /// Advance to the frame due now; true when the picture or status line changed.
    pub fn tick(&mut self, now: Instant) -> bool {
        let mut changed = false;
        if let Some(resolving) = &self.resolving {
            let answer = match resolving.try_recv() {
                Ok(answer) => answer,
                Err(TryRecvError::Empty) => return false,
                Err(TryRecvError::Disconnected) => Err("the video could not be resolved".into()),
            };
            self.resolving = None;
            changed = true;
            match answer {
                Ok(sources) => {
                    self.sources = Some(sources);
                    self.run_from(self.start, now);
                }
                Err(e) => self.error = Some(e),
            }
        }
        if self.error.is_some() {
            return changed;
        }
        if !self.playing() {
            if self.show_one && self.receive(now, &mut changed) {
                self.show_one = false;
                self.show_next();
                changed = true;
            }
            return changed;
        }
        while self.receive(now, &mut changed) {
            let due = self.due();
            let position = self.clock.position(now);
            if position < due {
                break;
            }
            if position - due > STALL {
                self.clock.set(due, now);
                // The sound played on through the stall: start it again with the picture.
                if self.audio.is_some() {
                    self.start_audio(now);
                }
            }
            self.show_next();
            if self.sound_waits && self.sound {
                self.start_audio(now);
            }
            changed = true;
        }
        changed
    }

    /// Ensure a frame waits in `next`; false if none is ready. A run that ends is judged
    /// once ffmpeg's log has ended:
    /// - cut short (before the known duration, or, with none, on an input error such as
    ///   a dropped connection), it goes on from there;
    /// - an opened input cut before its first frame is retried once; a resume that still
    ///   fails restarts from the start, and a later cut there counts as the end;
    /// - a silent run from the start that never changed is a still and stays;
    /// - any other at the end, or seeked past it, loops; one with no frame is an error.
    fn receive(&mut self, now: Instant, changed: &mut bool) -> bool {
        if self.next.is_some() {
            return true;
        }
        let Some(decoder) = &self.decoder else {
            return false;
        };
        let received = decoder.frames.try_recv();
        let log = Arc::clone(&decoder.log);
        let mut log = log.lock();
        match received {
            Ok(frame) => {
                self.next = Some(frame);
                if self.duration.is_none() {
                    self.duration = log.duration;
                }
                let audio = log.streams_read.then_some(log.audio);
                drop(log);
                if let Some(audio) = audio {
                    self.learn_audio(audio);
                }
                true
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                // The frames can end before the log says why: wait for the rest of it.
                if !log.ended {
                    return false;
                }
                *changed = true;
                let error = log.error.take();
                let input_failed = log.input_error;
                // A failure before the stream mapping is a link that never opened.
                let opened = log.streams_read;
                // A short still can end before any of its frames learned there is no sound.
                let audio = log.streams_read.then_some(log.audio);
                drop(log);
                if let Some(audio) = audio {
                    self.learn_audio(audio);
                }
                let end = self.due();
                // Cut short: before the known end, or (no duration, as for a streamed GIF)
                // when the input failed, such as a dropped connection. Go on from there,
                // unless resuming there already failed.
                let cut = self.duration.map_or(input_failed, |d| end + 1.0 < d)
                    && !self.unresumable.is_some_and(|at| (at - end).abs() < 0.5);
                if self.shown > 0 && cut {
                    self.run_from(end, now);
                } else if self.shown == 0 && input_failed && opened && !self.retried {
                    // Cut before the first frame (for a resumed run, while decoding up to
                    // its start): once more from there.
                    self.retried = true;
                    self.run_from(self.start, now);
                } else if self.shown == 0 && self.start > 0.0 && input_failed {
                    self.unresumable = Some(self.start);
                    self.run_from(0.0, now);
                } else if self.start == 0.0
                    && self.shown > 0
                    && self.unchanged
                    && self.sources.as_ref().is_some_and(|s| s.audio.is_none())
                {
                    // A silent picture that never changed has nothing to loop: restarting
                    // it would refetch and decode the file over and over. A still GIF
                    // drawn in several zero-delay pieces changes, so it still loops.
                    self.decoder = None;
                    self.still = true;
                } else if self.shown > 0 || self.start > 0.0 {
                    self.run_from(0.0, now);
                } else {
                    self.decoder = None;
                    self.stop_audio();
                    self.error = Some(error.unwrap_or_else(|| "the stream has no video".into()));
                }
                false
            }
        }
    }

    /// Learn from ffmpeg's log whether the video's own file has sound.
    fn learn_audio(&mut self, has_audio: bool) {
        let Some(sources) = &mut self.sources else {
            return;
        };
        if !sources.audio_in_file {
            return;
        }
        sources.audio_in_file = false;
        if !has_audio {
            sources.audio = None;
            if self.sound {
                self.stop_audio();
                self.sound = false;
                self.notice = Some(("This video has no sound".into(), false));
            }
        }
    }

    fn show_next(&mut self) {
        if let Some(frame) = self.next.take() {
            self.unchanged &= frame.same_as_first;
            self.retried = false;
            self.frame = Some(frame);
            self.shown += 1;
            self.frame_number += 1;
            // Played past where resuming failed: that was an outage, not the file's end.
            if self.unresumable.is_some_and(|at| self.due() > at + 0.5) {
                self.unresumable = None;
            }
        }
    }

    /// How long until the next frame is due; None while nothing will change.
    pub fn wake_in(&self, now: Instant) -> Option<Duration> {
        if self.error.is_some() || self.still {
            return None;
        }
        if self.resolving.is_some() {
            return Some(Duration::from_millis(50));
        }
        if !(self.playing() || self.show_one) {
            return None;
        }
        let poll = Duration::from_millis(10);
        if self.next.is_none() || !self.playing() {
            return Some(poll);
        }
        self.clock
            .when(self.due(), now)
            .map(|at| at.saturating_duration_since(now))
    }

    /// Off screen (scrolled away, an overlay, a dialog): stop, and go on when back.
    pub fn set_hidden(&mut self, hidden: bool, now: Instant) {
        if self.hidden != hidden {
            let was = self.playing();
            self.hidden = hidden;
            self.follow(was, now);
        }
    }

    pub fn toggle_pause(&mut self, now: Instant) {
        let was = self.playing();
        self.paused = !self.paused;
        self.follow(was, now);
    }

    /// Start or stop the clock and the sound after a change of `paused` or `hidden`.
    fn follow(&mut self, was: bool, now: Instant) {
        match (was, self.playing()) {
            (true, false) => {
                self.clock.stop(now);
                self.stop_audio();
            }
            (false, true) => {
                self.clock.start(now);
                self.show_one = false;
                if self.sound {
                    self.play_sound(now);
                }
            }
            _ => {}
        }
    }

    /// Toggles sound; without a sound stream or ffplay it stays off with a notice.
    pub fn toggle_sound(&mut self, now: Instant) {
        self.sound = !self.sound;
        if !self.sound {
            self.stop_audio();
        } else if self.sources.as_ref().is_some_and(|s| s.audio.is_none()) {
            self.sound = false;
            self.notice = Some(("This video has no sound".into(), false));
        } else if self.playing() && self.sources.is_some() {
            self.play_sound(now);
        }
    }

    pub fn sound(&self) -> bool {
        self.sound
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    /// The stream is resolved: seeking works.
    pub fn ready(&self) -> bool {
        self.sources.is_some()
    }

    /// Jump `by` seconds from the current position, within the video.
    pub fn seek(&mut self, by: f64, now: Instant) {
        if self.error.is_some() || self.sources.is_none() {
            return;
        }
        let mut to = (self.position(now) + by).max(0.0);
        // Past the end, or past a point the server always cuts at: stop just before it.
        if let Some(end) = self.duration.or(self.unresumable) {
            to = to.min((end - 1.0).max(0.0));
        }
        let was = self.playing();
        // A new start: its own retry.
        self.retried = false;
        self.run_from(to, now);
        if !was {
            self.clock.stop(now);
            self.show_one = true;
        }
    }

    fn start_audio(&mut self, now: Instant) {
        self.stop_audio();
        let Some(url) = self.sources.as_ref().and_then(|s| s.audio.as_deref()) else {
            self.sound = false;
            self.notice = Some(("This video has no sound".into(), false));
            return;
        };
        let at = format!("{:.3}", self.clock.position(now));
        let started = crate::media::local_command("ffplay")
            .args(["-nodisp", "-vn", "-autoexit", "-loglevel", "quiet"])
            .args(header_args(&self.sources.as_ref().unwrap().audio_headers))
            .args(["-ss", &at, url])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        match started {
            Ok(child) => self.audio = Some(child),
            Err(e) => {
                self.sound = false;
                let text = if e.kind() == io::ErrorKind::NotFound {
                    "Sound needs ffplay (part of ffmpeg)".to_string()
                } else {
                    format!("ffplay: {e}")
                };
                self.notice = Some((text, true));
            }
        }
    }

    /// Start sound now if this run shows frames, else with its first frame.
    fn play_sound(&mut self, now: Instant) {
        if self.shown == 0 {
            self.sound_waits = true;
        } else {
            self.start_audio(now);
        }
    }

    fn stop_audio(&mut self) {
        self.sound_waits = false;
        if let Some(mut child) = self.audio.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// The message for the reader, once: text and whether it is an error.
    pub fn take_notice(&mut self) -> Option<(String, bool)> {
        self.notice.take()
    }

    /// Where a player for a new area takes over: the run's start until its first frame,
    /// not the clock that ran while loading.
    pub fn resume(&self, now: Instant) -> Resume {
        Resume {
            at: if self.shown == 0 {
                self.start
            } else {
                self.position(now)
            },
            paused: self.paused,
            sound: self.sound,
        }
    }

    pub fn position(&self, now: Instant) -> f64 {
        if self.still {
            self.start
        } else {
            self.clock.position(now)
        }
    }

    pub fn duration(&self) -> Option<f64> {
        self.duration
    }

    pub fn frame(&self) -> Option<&Frame> {
        self.frame.as_ref()
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref().map(|error| {
            if crate::model::youtube_id(&self.url).is_some()
                && self.sources.is_some()
                && error != "ffmpeg is not installed"
            {
                "YouTube playback failed; m opens browser"
            } else {
                error
            }
        })
    }

    pub fn resolving(&self) -> bool {
        self.resolving.is_some()
    }

    #[cfg(test)]
    pub fn hidden(&self) -> bool {
        self.hidden
    }

    /// A player showing `frame`, with nothing to decode.
    #[cfg(test)]
    pub fn showing(url: &str, frame: Frame) -> Self {
        let spawn: Spawn = Box::new(|_, _| Err(io::Error::other("no decoder in tests")));
        let mut player = Self::new(
            url.into(),
            Size::new(10, 5),
            None,
            None,
            spawn,
            Resume::default(),
            Instant::now(),
        );
        player.show(frame);
        player
    }

    /// The next frame on screen, as if it came due.
    #[cfg(test)]
    pub fn show(&mut self, frame: Frame) {
        self.next = Some(frame);
        self.show_next();
    }

    /// A player for a video without sound, sized for `area`, whose run never gives
    /// a frame.
    #[cfg(test)]
    pub fn silent(url: &str, area: Size) -> Self {
        let spawn: Spawn = Box::new(|_, _| {
            let (tx, frames) = sync_channel(1);
            std::mem::forget(tx);
            Ok(Decoder {
                child: None,
                frames,
                log: Arc::new(Mutex::new(Log::default())),
            })
        });
        let sources = Sources {
            video: url.into(),
            audio: None,
            audio_in_file: false,
            headers: Vec::new(),
            audio_headers: Vec::new(),
        };
        Self::new(
            url.into(),
            area,
            None,
            Some(sources),
            spawn,
            Resume::default(),
            Instant::now(),
        )
    }

    #[cfg(test)]
    pub fn unavailable(url: &str, area: Size) -> Self {
        Self::new(
            url.into(),
            area,
            None,
            Some(Sources {
                video: url.into(),
                audio: None,
                audio_in_file: false,
                headers: Vec::new(),
                audio_headers: Vec::new(),
            }),
            Box::new(|_, _| Err(io::Error::other("fixture decoder unavailable"))),
            Resume::default(),
            Instant::now(),
        )
    }

    /// ffplay failing to start, as the sound would see it.
    #[cfg(test)]
    pub fn fail_sound(&mut self) {
        self.sound = false;
        self.notice = Some(("Sound needs ffplay (part of ffmpeg)".into(), true));
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        if let Some(cancelled) = &self.resolve_cancel {
            cancelled.cancel();
        }
        if self.error.is_some() {
            youtube::invalidate(&self.url);
        }
        self.stop_audio();
    }
}

/// kitty's row and column marks (its `gen/rowcolumn-diacritics.txt`, in order); they cap a
/// placement's height.
#[rustfmt::skip]
const DIACRITICS: [char; 297] = [
    '\u{0305}', '\u{030D}', '\u{030E}', '\u{0310}', '\u{0312}', '\u{033D}', '\u{033E}',
    '\u{033F}', '\u{0346}', '\u{034A}', '\u{034B}', '\u{034C}', '\u{0350}', '\u{0351}',
    '\u{0352}', '\u{0357}', '\u{035B}', '\u{0363}', '\u{0364}', '\u{0365}', '\u{0366}',
    '\u{0367}', '\u{0368}', '\u{0369}', '\u{036A}', '\u{036B}', '\u{036C}', '\u{036D}',
    '\u{036E}', '\u{036F}', '\u{0483}', '\u{0484}', '\u{0485}', '\u{0486}', '\u{0487}',
    '\u{0592}', '\u{0593}', '\u{0594}', '\u{0595}', '\u{0597}', '\u{0598}', '\u{0599}',
    '\u{059C}', '\u{059D}', '\u{059E}', '\u{059F}', '\u{05A0}', '\u{05A1}', '\u{05A8}',
    '\u{05A9}', '\u{05AB}', '\u{05AC}', '\u{05AF}', '\u{05C4}', '\u{0610}', '\u{0611}',
    '\u{0612}', '\u{0613}', '\u{0614}', '\u{0615}', '\u{0616}', '\u{0617}', '\u{0657}',
    '\u{0658}', '\u{0659}', '\u{065A}', '\u{065B}', '\u{065D}', '\u{065E}', '\u{06D6}',
    '\u{06D7}', '\u{06D8}', '\u{06D9}', '\u{06DA}', '\u{06DB}', '\u{06DC}', '\u{06DF}',
    '\u{06E0}', '\u{06E1}', '\u{06E2}', '\u{06E4}', '\u{06E7}', '\u{06E8}', '\u{06EB}',
    '\u{06EC}', '\u{0730}', '\u{0732}', '\u{0733}', '\u{0735}', '\u{0736}', '\u{073A}',
    '\u{073D}', '\u{073F}', '\u{0740}', '\u{0741}', '\u{0743}', '\u{0745}', '\u{0747}',
    '\u{0749}', '\u{074A}', '\u{07EB}', '\u{07EC}', '\u{07ED}', '\u{07EE}', '\u{07EF}',
    '\u{07F0}', '\u{07F1}', '\u{07F3}', '\u{0816}', '\u{0817}', '\u{0818}', '\u{0819}',
    '\u{081B}', '\u{081C}', '\u{081D}', '\u{081E}', '\u{081F}', '\u{0820}', '\u{0821}',
    '\u{0822}', '\u{0823}', '\u{0825}', '\u{0826}', '\u{0827}', '\u{0829}', '\u{082A}',
    '\u{082B}', '\u{082C}', '\u{082D}', '\u{0951}', '\u{0953}', '\u{0954}', '\u{0F82}',
    '\u{0F83}', '\u{0F86}', '\u{0F87}', '\u{135D}', '\u{135E}', '\u{135F}', '\u{17DD}',
    '\u{193A}', '\u{1A17}', '\u{1A75}', '\u{1A76}', '\u{1A77}', '\u{1A78}', '\u{1A79}',
    '\u{1A7A}', '\u{1A7B}', '\u{1A7C}', '\u{1B6B}', '\u{1B6D}', '\u{1B6E}', '\u{1B6F}',
    '\u{1B70}', '\u{1B71}', '\u{1B72}', '\u{1B73}', '\u{1CD0}', '\u{1CD1}', '\u{1CD2}',
    '\u{1CDA}', '\u{1CDB}', '\u{1CE0}', '\u{1DC0}', '\u{1DC1}', '\u{1DC3}', '\u{1DC4}',
    '\u{1DC5}', '\u{1DC6}', '\u{1DC7}', '\u{1DC8}', '\u{1DC9}', '\u{1DCB}', '\u{1DCC}',
    '\u{1DD1}', '\u{1DD2}', '\u{1DD3}', '\u{1DD4}', '\u{1DD5}', '\u{1DD6}', '\u{1DD7}',
    '\u{1DD8}', '\u{1DD9}', '\u{1DDA}', '\u{1DDB}', '\u{1DDC}', '\u{1DDD}', '\u{1DDE}',
    '\u{1DDF}', '\u{1DE0}', '\u{1DE1}', '\u{1DE2}', '\u{1DE3}', '\u{1DE4}', '\u{1DE5}',
    '\u{1DE6}', '\u{1DFE}', '\u{20D0}', '\u{20D1}', '\u{20D4}', '\u{20D5}', '\u{20D6}',
    '\u{20D7}', '\u{20DB}', '\u{20DC}', '\u{20E1}', '\u{20E7}', '\u{20E9}', '\u{20F0}',
    '\u{2CEF}', '\u{2CF0}', '\u{2CF1}', '\u{2DE0}', '\u{2DE1}', '\u{2DE2}', '\u{2DE3}',
    '\u{2DE4}', '\u{2DE5}', '\u{2DE6}', '\u{2DE7}', '\u{2DE8}', '\u{2DE9}', '\u{2DEA}',
    '\u{2DEB}', '\u{2DEC}', '\u{2DED}', '\u{2DEE}', '\u{2DEF}', '\u{2DF0}', '\u{2DF1}',
    '\u{2DF2}', '\u{2DF3}', '\u{2DF4}', '\u{2DF5}', '\u{2DF6}', '\u{2DF7}', '\u{2DF8}',
    '\u{2DF9}', '\u{2DFA}', '\u{2DFB}', '\u{2DFC}', '\u{2DFD}', '\u{2DFE}', '\u{2DFF}',
    '\u{A66F}', '\u{A67C}', '\u{A67D}', '\u{A6F0}', '\u{A6F1}', '\u{A8E0}', '\u{A8E1}',
    '\u{A8E2}', '\u{A8E3}', '\u{A8E4}', '\u{A8E5}', '\u{A8E6}', '\u{A8E7}', '\u{A8E8}',
    '\u{A8E9}', '\u{A8EA}', '\u{A8EB}', '\u{A8EC}', '\u{A8ED}', '\u{A8EE}', '\u{A8EF}',
    '\u{A8F0}', '\u{A8F1}', '\u{AAB0}', '\u{AAB2}', '\u{AAB3}', '\u{AAB7}', '\u{AAB8}',
    '\u{AABE}', '\u{AABF}', '\u{AAC1}', '\u{FE20}', '\u{FE21}', '\u{FE22}', '\u{FE23}',
    '\u{FE24}', '\u{FE25}', '\u{FE26}', '\u{10A0F}', '\u{10A38}', '\u{1D185}', '\u{1D186}',
    '\u{1D187}', '\u{1D188}', '\u{1D189}', '\u{1D1AA}', '\u{1D1AB}', '\u{1D1AC}', '\u{1D1AD}',
    '\u{1D242}', '\u{1D243}', '\u{1D244}',
];
const PLACEHOLDER: char = '\u{10EEEE}';

pub const KITTY_MAX_ROWS: u16 = DIACRITICS.len() as u16;

/// Most bytes in one tmux passthrough: tmux resets the terminal's modes, showing the
/// cursor, after each one, so a frame goes in as few as fit well under tmux's default
/// 1 MiB `input-buffer-size` (longer sequences are dropped).
const PASSTHROUGH_MAX: usize = 256 * 1024;

/// Sends `frame` as kitty image `id`, replacing the last, with a virtual `cols`×`rows`
/// placement; inside tmux the chunks go in as few passthroughs as `PASSTHROUGH_MAX` allows.
pub fn kitty_transmit(id: u32, frame: &Frame, cols: u16, rows: u16, tmux: bool) -> String {
    let Pixels::Png(data) = &frame.pixels else {
        return String::new();
    };
    let escape = if tmux { "\x1b\x1b" } else { "\x1b" };
    // 3072 input bytes encode to the protocol's 4096-byte payload limit.
    let chunks = data.chunks(3072);
    let count = chunks.len();
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4 + count * 32 + 96);
    let mut chunk_escape = String::with_capacity(4096 + 128);
    // Bytes in the open passthrough; 0 when none is open.
    let mut open = 0;
    for (i, chunk) in chunks.enumerate() {
        chunk_escape.clear();
        chunk_escape.push_str(escape);
        chunk_escape.push_str("_G");
        if i == 0 {
            // p=1: each frame replaces the one virtual placement. Without an id Ghostty
            // kept the first one's size, so the next post's video showed in the last's box.
            chunk_escape.push_str(&format!(
                "a=T,U=1,i={id},p=1,f=100,t=d,s={},v={},c={cols},r={rows},q=2,",
                frame.width, frame.height
            ));
        }
        chunk_escape.push_str(if i + 1 < count { "m=1;" } else { "m=0;" });
        base64::engine::general_purpose::STANDARD.encode_string(chunk, &mut chunk_escape);
        chunk_escape.push_str(escape);
        chunk_escape.push('\\');
        if tmux {
            if open > 0 && open + chunk_escape.len() + 2 > PASSTHROUGH_MAX {
                out.push_str("\x1b\\");
                open = 0;
            }
            if open == 0 {
                out.push_str("\x1bPtmux;");
                open = "\x1bPtmux;".len();
            }
            open += chunk_escape.len();
        }
        out.push_str(&chunk_escape);
    }
    if open > 0 {
        out.push_str("\x1b\\");
    }
    out
}

/// Fills `area` with kitty placeholder cells; `transmit` rides in the first cell so it
/// goes out with the draw.
pub fn kitty_cells(
    buf: &mut Buffer,
    area: Rect,
    id: u32,
    source: Position,
    transmit: Option<String>,
) {
    let [extra, r, g, b] = id.to_be_bytes();
    let rows = area.height.min(KITTY_MAX_ROWS.saturating_sub(source.y));
    let cols = area.width.min(KITTY_MAX_ROWS.saturating_sub(source.x));
    let mut transmit = transmit;
    let mut symbol = String::new();
    for y in 0..rows {
        for x in 0..cols {
            let Some(cell) = buf.cell_mut((area.x + x, area.y + y)) else {
                continue;
            };
            let mut first = transmit.take();
            symbol.clear();
            let text = match first.as_mut() {
                Some(seq) => seq,
                None => &mut symbol,
            };
            text.push(PLACEHOLDER);
            // Later cells of a row continue from the first one.
            if x == 0 {
                text.push(DIACRITICS[usize::from(source.y + y)]);
                text.push(DIACRITICS[usize::from(source.x)]);
                text.push(DIACRITICS[usize::from(extra)]);
            }
            cell.set_symbol(text)
                .set_fg(Color::Rgb(r, g, b))
                .set_diff_option(CellDiffOption::ForcedWidth(std::num::NonZeroU16::MIN));
        }
    }
}

/// `m:ss` for a time in seconds.
pub fn clock_text(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn when_a_resolving_player_is_dropped_it_should_cancel_without_waiting_for_the_worker() {
        let (_sender, resolving) = sync_channel(1);
        let cancelled = Arc::new(youtube::Cancellation::default());
        let mut player = Player::new(
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ".into(),
            Size::new(20, 10),
            Some(resolving),
            None,
            Box::new(|_, _| Err(io::Error::other("unused"))),
            Resume::default(),
            Instant::now(),
        );
        player.resolve_cancel = Some(cancelled.clone());
        assert!(player.resolving());
        let started = Instant::now();

        drop(player);

        assert!(cancelled.is_cancelled());
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn when_youtube_decoding_fails_it_should_hide_stream_tokens_and_keep_install_guidance() {
        let mut player = Player::unavailable(
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            Size::new(20, 10),
        );
        player.error = Some("failed https://rr1.googlevideo.com/file?token=PRIVATE".into());
        assert_eq!(
            player.error(),
            Some("YouTube playback failed; m opens browser")
        );

        player.error = Some("ffmpeg is not installed".into());

        assert_eq!(player.error(), Some("ffmpeg is not installed"));
    }

    #[test]
    fn when_stream_headers_are_forwarded_they_should_precede_the_input_as_http_header_lines() {
        let actual = header_args(&[
            ("User-Agent".into(), "fixture-agent".into()),
            ("Referer".into(), "https://www.youtube.com/".into()),
        ]);

        assert_eq!(
            actual,
            [
                "-headers",
                "User-Agent: fixture-agent\r\nReferer: https://www.youtube.com/\r\n"
            ]
        );
    }
    use std::io::Read;

    /// Hands out one byte per read, so every header field and frame crosses reads.
    struct Trickle<'a>(&'a [u8]);

    impl Read for Trickle<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            match (self.0.split_first(), buf.first_mut()) {
                (Some((&b, rest)), Some(slot)) => {
                    *slot = b;
                    self.0 = rest;
                    Ok(1)
                }
                _ => Ok(0),
            }
        }
    }

    #[test]
    fn ppm_frames_are_read_whole_across_short_reads() {
        let mut stream = b"P6\n2 1\n255\n".to_vec();
        stream.extend([1, 2, 3, 4, 5, 6]);
        stream.extend(b"P6 1 2 255 ");
        stream.extend([7, 8, 9, 10, 11, 12]);
        let mut r = BufReader::with_capacity(1, Trickle(&stream));
        assert_eq!(
            read_ppm(&mut r).unwrap(),
            Some((2, 1, vec![1, 2, 3, 4, 5, 6]))
        );
        assert_eq!(
            read_ppm(&mut r).unwrap(),
            Some((1, 2, vec![7, 8, 9, 10, 11, 12]))
        );
        assert_eq!(read_ppm(&mut r).unwrap(), None);
    }

    #[test]
    fn a_cut_ppm_frame_is_an_error_not_a_frame() {
        let mut r = BufReader::new(&b"P6\n2 1\n255\n\x01\x02\x03"[..]);
        assert!(read_ppm(&mut r).is_err());
    }

    #[test]
    fn when_a_log_line_is_not_utf8_the_rest_of_the_log_should_still_be_read() {
        let log = Mutex::new(Log::default());
        let stderr = b"[info] title: caf\xe9\n  Duration: 00:00:10.00, start: 0\nStream mapping:\n";

        read_log(&stderr[..], &log);

        let log = log.lock();
        assert_eq!(log.duration, Some(10.0));
        assert!(log.streams_read);
    }

    #[test]
    fn when_ffmpeg_logs_an_error_from_a_component_it_should_be_noted() {
        // Lines ffmpeg 9 printed for a GIF whose connection dropped.
        let stderr =
            b"[http @ 0x7951404000] [error] Stream ends prematurely at 60000, should be 2802623\n\
[in#0/gif @ 0x7950808000] [error] Error during demuxing: Input/output error\n";
        let log = Mutex::new(Log::default());

        read_log(&stderr[..], &log);

        let log = log.lock();
        assert_eq!(
            log.error.as_deref(),
            Some("Error during demuxing: Input/output error")
        );
        assert!(log.input_error, "a dropped connection is an input failure");
        assert!(log.ended);
    }

    #[test]
    fn when_ffmpeg_logs_a_bad_frame_it_should_not_count_as_a_cut_input() {
        // Lines ffmpeg 9 printed for a GIF with a corrupt frame, read to its end.
        let stderr = b"[gif @ 0x7950854e00] [error] LZW decode failed\n\
[vist#0:0/gif @ 0x1] [dec:gif @ 0x2] [error] Error submitting packet to decoder: Invalid data\n";
        let log = Mutex::new(Log::default());

        read_log(&stderr[..], &log);

        let log = log.lock();
        assert_eq!(
            log.error.as_deref(),
            Some("Error submitting packet to decoder: Invalid data"),
            "nested components are read through"
        );
        assert!(!log.input_error);
    }

    #[test]
    fn ffmpeg_duration_line_gives_seconds() {
        let line = "[info]   Duration: 00:01:02.50, start: 0.000000, bitrate: N/A";
        assert_eq!(parse_duration(line), Some(62.5));
        assert_eq!(parse_duration("[info]   Duration: N/A, start: 0"), None);
    }

    #[test]
    fn the_clock_stands_still_while_stopped_and_resumes_from_there() {
        let t0 = Instant::now();
        let mut clock = Clock::new(3.0, t0);
        let t1 = t0 + Duration::from_secs(2);
        assert_eq!(clock.position(t1), 5.0);
        clock.stop(t1);
        let t2 = t1 + Duration::from_secs(10);
        assert_eq!(clock.position(t2), 5.0);
        assert_eq!(clock.when(6.0, t2), None);
        clock.start(t2);
        assert_eq!(clock.position(t2 + Duration::from_secs(1)), 6.0);
        assert_eq!(clock.when(6.0, t2), Some(t2 + Duration::from_secs(1)));
    }

    /// The first frame ffmpeg decodes from a one-frame `w`×`h` GIF, or None without ffmpeg
    /// (an optional dependency).
    fn first_frame(w: u16, h: u16, frame_box: (u32, u32)) -> Option<(u32, u32)> {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return None;
        }
        // One directory per call: the tests run in parallel.
        let dir = std::env::temp_dir().join(format!(
            "redtab-filter-{}-{w}x{h}-{}x{}",
            std::process::id(),
            frame_box.0,
            frame_box.1
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("frame.gif");
        let mut file = std::fs::File::create(&path).unwrap();
        let mut encoder = gif::Encoder::new(&mut file, w, h, &[]).unwrap();
        let pixels = vec![0; usize::from(w) * usize::from(h)];
        encoder
            .write_frame(&gif::Frame::from_palette_pixels(
                w,
                h,
                pixels,
                [90, 90, 90],
                None,
            ))
            .unwrap();
        drop(encoder);
        let decoder = Decoder::spawn(path.to_str().unwrap(), 0.0, frame_box, &[]).unwrap();
        let frame = decoder
            .frames
            .recv_timeout(Duration::from_secs(20))
            .unwrap();
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
        Some((frame.width, frame.height))
    }

    #[test]
    fn video_width_regression_card_decode_should_retain_display_resolution() {
        let Some(actual) = first_frame(1280, 720, (1280, 720)) else {
            return;
        };
        assert_eq!(actual, (1280, 720));
    }

    #[test]
    fn video_frames_should_keep_source_detail_up_to_the_display_size() {
        let decoded = |w, h| first_frame(w, h, (856, 1170));
        if decoded(2, 2).is_none() {
            return;
        }

        assert_eq!(decoded(1280, 720), Some((856, 481)));
        assert_eq!(decoded(540, 674), Some((540, 674)));
        assert_eq!(decoded(900, 900), Some((856, 856)));
        // Smaller than that: as it is; the terminal enlarges it.
        assert_eq!(decoded(100, 80), Some((100, 80)));
    }

    #[test]
    fn video_frames_should_keep_portrait_width_for_scrolling() {
        let Some(actual) = first_frame(1080, 1920, (608, 1088)) else {
            return;
        };
        assert_eq!(actual, (608, 1080));
    }

    #[test]
    fn video_decode_box_should_follow_full_document_height() {
        assert_eq!(
            frame_box(Size::new(100, 100), FontSize::new(16, 36)),
            (1280, 3600)
        );
    }

    #[test]
    fn kitty_frames_go_out_in_chunks_and_through_tmux() {
        let frame = Frame {
            width: 2,
            height: 1,
            pixels: Pixels::Png(vec![7; 5000]),
            same_as_first: false,
        };
        let plain = kitty_transmit(42, &frame, 3, 2, false);
        let escapes: Vec<&str> = plain.split("\x1b\\").filter(|s| !s.is_empty()).collect();
        assert_eq!(escapes.len(), 2, "5000 bytes are 6668 base64 characters");
        assert!(
            escapes[0].starts_with("\x1b_Ga=T,U=1,i=42,p=1,f=100,t=d,s=2,v=1,c=3,r=2,q=2,m=1;")
        );
        assert!(escapes[1].starts_with("\x1b_Gm=0;"));
        assert!(
            escapes
                .iter()
                .all(|e| e.split_once(';').unwrap().1.len() <= 4096)
        );
        let payload: String = escapes
            .iter()
            .map(|escape| escape.split_once(';').unwrap().1)
            .collect();
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(payload)
                .unwrap(),
            vec![7; 5000]
        );

        // Inside tmux: the escapes, ESCs doubled, in one passthrough.
        let tmux = kitty_transmit(42, &frame, 3, 2, true);
        let wrapped = format!("\x1bPtmux;{}\x1b\\", plain.replace('\x1b', "\x1b\x1b"));
        assert_eq!(tmux, wrapped);

        // A frame too big for one passthrough splits between whole escapes.
        let big = Frame {
            pixels: Pixels::Png(vec![7; 600_000]),
            ..frame
        };
        let plain = kitty_transmit(42, &big, 3, 2, false);
        let tmux = kitty_transmit(42, &big, 3, 2, true);
        let passthroughs: Vec<&str> = tmux.split_inclusive("\x1b\\\x1bPtmux;").collect();
        assert_eq!(
            passthroughs.len(),
            4,
            "800,000 base64 bytes in 256 KiB pieces"
        );
        assert!(passthroughs.iter().all(|p| p.len() <= PASSTHROUGH_MAX + 9));
        let unwrapped = tmux
            .strip_prefix("\x1bPtmux;")
            .and_then(|t| t.strip_suffix("\x1b\\"))
            .unwrap()
            .replace("\x1b\\\x1bPtmux;", "")
            .replace("\x1b\x1b", "\x1b");
        assert_eq!(unwrapped, plain);
    }

    #[test]
    fn kitty_cells_should_encode_the_full_id_like_the_upstream_protocol() {
        use ratatui::widgets::Widget;
        let id = 0xab12_3456;
        let protocol = ratatui_image::protocol::Protocol::Kitty(
            ratatui_image::protocol::kitty::Kitty::new(
                image::DynamicImage::new_rgba8(1, 1),
                Size::new(1, 1),
                id,
                false,
                false,
            )
            .unwrap(),
        );
        let mut reference = Buffer::empty(Rect::new(0, 0, 1, 1));
        ratatui_image::Image::new(&protocol).render(reference.area, &mut reference);
        let mut actual = Buffer::empty(reference.area);
        kitty_cells(&mut actual, reference.area, id, Position::ORIGIN, None);
        let (_, suffix) = reference[(0, 0)].symbol().split_once('\u{10EEEE}').unwrap();
        assert_eq!(actual[(0, 0)].symbol(), format!("\u{10EEEE}{suffix}"));
        assert_eq!(actual[(0, 0)].fg, reference[(0, 0)].fg);
    }

    #[test]
    fn kitty_cells_carry_the_id_the_rows_and_the_frame_once() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 3));
        let id = 0x0001_0203;
        kitty_cells(
            &mut buf,
            Rect::new(1, 0, 2, 2),
            id,
            Position::ORIGIN,
            Some("DATA".into()),
        );
        let cell = |x, y| buf.cell((x, y)).unwrap();

        assert_eq!(cell(1, 0).symbol(), "DATA\u{10EEEE}\u{305}\u{305}\u{305}");
        assert_eq!(cell(2, 0).symbol(), "\u{10EEEE}");
        assert_eq!(cell(1, 1).symbol(), "\u{10EEEE}\u{30D}\u{305}\u{305}");
        assert_eq!(cell(2, 1).symbol(), "\u{10EEEE}");
        for (x, y) in [(1, 0), (2, 0), (1, 1), (2, 1)] {
            assert_eq!(
                cell(x, y).fg,
                Color::Rgb(1, 2, 3),
                "every cell names the image"
            );
            assert_eq!(
                cell(x, y).diff_option,
                CellDiffOption::ForcedWidth(std::num::NonZeroU16::MIN),
                "one column each, whatever the data in it"
            );
        }
        assert_eq!(cell(0, 0).symbol(), " ", "outside the area");
        assert_eq!(cell(1, 2).symbol(), " ", "below the placement");
    }

    /// Each decoder run a test player started: its start position and the sender
    /// that feeds it frames.
    type Runs = std::rc::Rc<std::cell::RefCell<Vec<(f64, std::sync::mpsc::SyncSender<Frame>)>>>;

    /// Decoder runs whose frames the test feeds, with what their log says.
    fn fed_runs(duration: Option<f64>, audio: bool) -> (Spawn, Runs) {
        fed_runs_logging(duration, audio, None)
    }

    /// As `fed_runs`, with every run's log ending on `error`: its message and whether
    /// the input failed (a dropped connection) rather than a frame. The input opened
    /// (its stream mapping was logged) unless the message says it could not.
    fn fed_runs_logging(
        duration: Option<f64>,
        audio: bool,
        error: Option<(&'static str, bool)>,
    ) -> (Spawn, Runs) {
        let runs = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let record = std::rc::Rc::clone(&runs);
        let spawn: Spawn = Box::new(move |_, at| {
            let (tx, frames) = sync_channel(64);
            record.borrow_mut().push((at, tx));
            let log = Log {
                duration,
                audio,
                streams_read: !error
                    .is_some_and(|(message, _)| message.starts_with("Error opening")),
                error: error.map(|(message, _)| message.into()),
                input_error: error.is_some_and(|(_, input)| input),
                ended: true,
            };
            Ok(Decoder {
                child: None,
                frames,
                log: Arc::new(Mutex::new(log)),
            })
        });
        (spawn, runs)
    }

    /// A playing silent player on fed runs.
    fn fed_silent_player(spawn: Spawn, at: f64, now: Instant) -> Player {
        Player::new(
            "u".into(),
            Size::new(10, 5),
            None,
            Some(no_sound()),
            spawn,
            Resume {
                at,
                ..Resume::default()
            },
            now,
        )
    }

    const DROPPED: Option<(&str, bool)> = Some(("Error during demuxing: Input/output error", true));

    fn no_sound() -> Sources {
        Sources {
            video: "v".into(),
            audio: None,
            audio_in_file: false,
            headers: Vec::new(),
            audio_headers: Vec::new(),
        }
    }

    /// A playing player whose decoder runs are channels the test feeds.
    fn fed_player(duration: Option<f64>, now: Instant) -> (Player, Runs) {
        let (spawn, runs) = fed_runs(duration, false);
        (fed_silent_player(spawn, 0.0, now), runs)
    }

    /// A frame of moving video.
    fn blank() -> Frame {
        Frame {
            width: 1,
            height: 1,
            pixels: Pixels::Png(Vec::new()),
            same_as_first: false,
        }
    }

    /// A frame showing the same picture as its run's first.
    fn repeated() -> Frame {
        Frame {
            same_as_first: true,
            ..blank()
        }
    }

    fn after(t0: Instant, seconds: f64) -> Instant {
        t0 + Duration::from_secs_f64(seconds)
    }

    #[test]
    fn frames_show_when_due_and_wait_while_paused() {
        let t0 = Instant::now();
        let (mut player, runs) = fed_player(None, t0);
        for _ in 0..4 {
            runs.borrow()[0].1.send(blank()).unwrap();
        }

        assert!(player.tick(t0));
        assert_eq!(player.frame_number, 1, "the first frame is due at once");
        assert!(!player.tick(after(t0, 0.02)), "the second is due at 1/24 s");
        assert!(player.tick(after(t0, 0.05)));
        assert_eq!(player.frame_number, 2);

        player.toggle_pause(after(t0, 0.05));
        assert!(!player.tick(after(t0, 5.0)));
        assert_eq!(player.frame_number, 2, "paused: no frame comes due");
        assert_eq!(player.position(after(t0, 5.0)), 0.05);

        player.toggle_pause(after(t0, 5.0));
        assert!(player.tick(after(t0, 5.04)));
        assert_eq!(
            player.frame_number, 3,
            "playing again goes on from the pause"
        );
    }

    #[test]
    fn a_seek_while_paused_shows_one_frame_and_playing_ends_that() {
        let t0 = Instant::now();
        let (mut player, runs) = fed_player(Some(60.0), t0);
        player.toggle_pause(t0);
        player.seek(SEEK_STEP, t0);
        assert_eq!(
            runs.borrow()[1].0,
            SEEK_STEP,
            "a new run starts at the target"
        );
        for _ in 0..2 {
            runs.borrow()[1].1.send(blank()).unwrap();
        }
        assert!(player.tick(after(t0, 1.0)));
        assert!(!player.tick(after(t0, 2.0)));
        assert_eq!(player.frame_number, 1, "one frame shows the new place");
        assert_eq!(player.position(after(t0, 2.0)), SEEK_STEP);

        // Seek again and play before that run's first frame, then pause: the
        // pending one-frame show is over, so nothing moves while paused.
        player.seek(SEEK_STEP, after(t0, 2.0));
        player.toggle_pause(after(t0, 2.0));
        player.toggle_pause(after(t0, 2.0));
        runs.borrow()[2].1.send(blank()).unwrap();
        player.tick(after(t0, 3.0));
        assert_eq!(player.frame_number, 1);
    }

    #[test]
    fn a_run_that_ends_loops_at_the_end_and_resumes_when_cut_short() {
        let t0 = Instant::now();
        // Duration unknown: an ended run is the end, play again from the start.
        let (mut player, runs) = fed_player(None, t0);
        let first = runs.borrow_mut().remove(0).1;
        first.send(repeated()).unwrap();
        first.send(blank()).unwrap();
        drop(first);
        player.tick(t0);
        player.tick(after(t0, 0.05));
        player.tick(after(t0, 0.15));
        assert_eq!(runs.borrow()[0].0, 0.0);

        // Known to last 60 s: a run cut after 2 frames goes on from there.
        let (mut player, runs) = fed_player(Some(60.0), t0);
        let first = runs.borrow_mut().remove(0).1;
        first.send(blank()).unwrap();
        first.send(blank()).unwrap();
        drop(first);
        player.tick(after(t0, 0.1));
        player.tick(after(t0, 0.2));
        assert_eq!(runs.borrow()[0].0, 2.0 / FPS);
    }

    #[test]
    fn when_a_run_of_unknown_length_ends_on_an_error_it_should_go_on_from_there() {
        // A streamed GIF has no known duration; a dropped connection ends it with an error.
        let t0 = Instant::now();
        let (spawn, runs) = fed_runs_logging(None, false, DROPPED);
        let mut player = fed_silent_player(spawn, 0.0, t0);
        let first = runs.borrow_mut().remove(0).1;
        first.send(repeated()).unwrap();
        first.send(blank()).unwrap();
        drop(first);

        player.tick(t0);
        player.tick(after(t0, 0.05));
        player.tick(after(t0, 0.15));

        assert_eq!(runs.borrow()[0].0, 2.0 / FPS, "not back to 0:00");
    }

    #[test]
    fn when_a_run_ends_after_a_bad_frame_it_should_loop_not_resume() {
        // `[gif @ …] [error] LZW decode failed`: the file was read to its end.
        let t0 = Instant::now();
        let (spawn, runs) = fed_runs_logging(None, false, Some(("LZW decode failed", false)));
        let mut player = fed_silent_player(spawn, 0.0, t0);
        let first = runs.borrow_mut().remove(0).1;
        first.send(repeated()).unwrap();
        first.send(blank()).unwrap();
        drop(first);

        player.tick(t0);
        player.tick(after(t0, 0.05));
        player.tick(after(t0, 0.15));

        assert_eq!(runs.borrow()[0].0, 0.0);
    }

    #[test]
    fn when_a_resumed_run_is_cut_before_its_first_frame_it_should_retry_once() {
        let t0 = Instant::now();
        let (spawn, runs) = fed_runs_logging(None, false, DROPPED);
        let mut player = fed_silent_player(spawn, 5.0, t0);

        drop(runs.borrow_mut().remove(0));
        player.tick(t0);
        let retried = runs.borrow()[0].0;
        drop(runs.borrow_mut().remove(0));
        player.tick(after(t0, 0.1));
        let then = runs.borrow()[0].0;

        assert_eq!(retried, 5.0, "cut on the way to 0:05: once more from there");
        assert_eq!(then, 0.0, "cut again: no endless retries");
    }

    #[test]
    fn when_resuming_at_a_cut_keeps_failing_the_cut_should_count_as_the_end() {
        // A server that always ends the file at the same byte: every run is cut at 2
        // frames and resuming there gives nothing.
        let t0 = Instant::now();
        let (spawn, runs) = fed_runs_logging(None, false, DROPPED);
        let mut player = fed_silent_player(spawn, 0.0, t0);
        let play_to_the_cut = |runs: &Runs| {
            let run = runs.borrow_mut().remove(0).1;
            run.send(repeated()).unwrap();
            run.send(blank()).unwrap();
        };
        play_to_the_cut(&runs);
        player.tick(t0);
        player.tick(after(t0, 0.05));
        player.tick(after(t0, 0.15));
        for at in [0.2, 0.25] {
            // The resume and its retry: no frames.
            drop(runs.borrow_mut().remove(0));
            player.tick(after(t0, at));
        }
        play_to_the_cut(&runs);
        player.tick(after(t0, 0.3));
        player.tick(after(t0, 0.35));
        player.tick(after(t0, 0.45));

        assert_eq!(
            runs.borrow()[0].0,
            0.0,
            "after a failed resume there, the cut loops"
        );
    }

    #[test]
    fn when_a_still_on_a_cutting_server_settles_it_should_hold_its_frame() {
        // As above, for a still GIF: once resuming at the cut failed, the still holds.
        let t0 = Instant::now();
        let (spawn, runs) = fed_runs_logging(None, false, DROPPED);
        let mut player = fed_silent_player(spawn, 0.0, t0);
        let play_to_the_cut = |runs: &Runs| {
            let run = runs.borrow_mut().remove(0).1;
            run.send(repeated()).unwrap();
            run.send(repeated()).unwrap();
        };
        play_to_the_cut(&runs);
        player.tick(t0);
        player.tick(after(t0, 0.05));
        player.tick(after(t0, 0.15));
        for at in [0.2, 0.25] {
            drop(runs.borrow_mut().remove(0));
            player.tick(after(t0, at));
        }
        play_to_the_cut(&runs);
        player.tick(after(t0, 0.3));
        player.tick(after(t0, 0.35));
        player.tick(after(t0, 0.45));

        assert!(runs.borrow().is_empty(), "no more downloads");
        assert_eq!(player.wake_in(after(t0, 0.5)), None, "held");
    }

    #[test]
    fn when_a_run_from_the_start_is_cut_before_its_first_frame_it_should_retry_once() {
        let t0 = Instant::now();
        let (spawn, runs) = fed_runs_logging(None, false, DROPPED);
        let mut player = fed_silent_player(spawn, 0.0, t0);

        drop(runs.borrow_mut().remove(0));
        player.tick(t0);
        let retried = runs.borrow().len();
        drop(runs.borrow_mut().remove(0));
        player.tick(after(t0, 0.1));

        assert_eq!(retried, 1, "one more try");
        assert!(runs.borrow().is_empty(), "then no more");
        assert!(player.error().is_some());
    }

    #[test]
    fn when_a_seek_follows_a_used_retry_the_new_start_should_get_its_own() {
        let t0 = Instant::now();
        let (spawn, runs) = fed_runs_logging(None, false, DROPPED);
        let mut player = fed_silent_player(spawn, 5.0, t0);
        drop(runs.borrow_mut().remove(0));
        player.tick(t0); // cut on the way to 0:05: the retry is used
        runs.borrow_mut().clear();

        player.seek(SEEK_STEP, t0);
        drop(runs.borrow_mut().remove(0));
        player.tick(after(t0, 0.1));

        assert_eq!(runs.borrow()[0].0, 10.0, "the seek target is tried again");
    }

    #[test]
    fn when_a_cut_point_failed_seeks_should_stop_before_it_until_playback_passes_it() {
        let t0 = Instant::now();
        let (spawn, runs) = fed_runs(None, false);
        let mut player = fed_silent_player(spawn, 0.0, t0);
        player.unresumable = Some(3.0);
        runs.borrow_mut().clear();

        player.seek(SEEK_STEP, t0);
        let clamped = runs.borrow()[0].0;
        let run = runs.borrow_mut().remove(0).1;
        for i in 0..40 {
            run.send(blank()).unwrap();
            player.tick(after(t0, f64::from(i) / FPS));
        }

        assert_eq!(clamped, 2.0, "a second before the failed point");
        assert_eq!(player.unresumable, None, "played past it: it was an outage");
    }

    #[test]
    fn when_a_link_cannot_be_opened_it_should_be_an_error_without_a_retry() {
        let t0 = Instant::now();
        let error = Some(("Error opening input: Connection refused", true));
        let (spawn, runs) = fed_runs_logging(None, false, error);
        let mut player = fed_silent_player(spawn, 0.0, t0);

        drop(runs.borrow_mut().remove(0));
        player.tick(t0);

        assert!(runs.borrow().is_empty());
        assert_eq!(
            player.error(),
            Some("Error opening input: Connection refused")
        );
    }

    #[test]
    fn when_frames_end_before_the_log_it_should_wait_for_the_log() {
        let t0 = Instant::now();
        let log = Arc::new(Mutex::new(Log {
            input_error: true,
            error: Some("Stream ends prematurely".into()),
            ..Log::default()
        }));
        let shared = Arc::clone(&log);
        let runs: Runs = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let record = std::rc::Rc::clone(&runs);
        let spawn: Spawn = Box::new(move |_, at| {
            let (tx, frames) = sync_channel(64);
            record.borrow_mut().push((at, tx));
            Ok(Decoder {
                child: None,
                frames,
                log: Arc::clone(&shared),
            })
        });
        let mut player = fed_silent_player(spawn, 0.0, t0);
        let first = runs.borrow_mut().remove(0).1;
        first.send(repeated()).unwrap();
        first.send(blank()).unwrap();
        drop(first);

        player.tick(t0);
        player.tick(after(t0, 0.15));
        let while_logging = runs.borrow().len();
        log.lock().ended = true;
        player.tick(after(t0, 0.2));

        assert_eq!(while_logging, 0, "nothing decided before the log ends");
        assert_eq!(
            runs.borrow()[0].0,
            2.0 / FPS,
            "then it goes on from the cut"
        );
    }

    #[test]
    fn a_run_past_the_end_or_at_it_plays_again_from_the_start() {
        let t0 = Instant::now();
        // A run started past the last frame gives none: back to the start.
        let (spawn, runs) = fed_runs(None, false);
        let mut player = fed_silent_player(spawn, 5.0, t0);
        drop(runs.borrow_mut().remove(0));
        player.tick(t0);
        assert_eq!(runs.borrow()[0].0, 0.0);
        assert!(player.error().is_none());

        // A 1 s video that ran to its end loops rather than resuming at 1 s.
        let (mut player, runs) = fed_player(Some(1.0), t0);
        let first = runs.borrow_mut().remove(0).1;
        first.send(repeated()).unwrap();
        for _ in 1..24 {
            first.send(blank()).unwrap();
        }
        drop(first);
        for i in 0..=25 {
            player.tick(after(t0, f64::from(i) / FPS));
        }
        assert_eq!(runs.borrow()[0].0, 0.0);
    }

    #[test]
    fn when_a_plain_gif_plays_it_should_stream_without_seeking_its_input() {
        let gif = "http://127.0.0.1:1/k/https/i.redd.it/a.gif";
        let mp4 = "http://127.0.0.1:1/k/https/preview.redd.it/a.gif?format=mp4&s=x";

        let args = |url: &str, at: f64| input_args(url, at).join(" ");

        assert_eq!(args(gif, 0.0), format!("-seekable 0 -i {gif}"));
        assert_eq!(
            args(gif, 5.0),
            format!("-seekable 0 -i {gif} -ss 5.000"),
            "a later start decodes past earlier frames"
        );
        assert_eq!(
            args(mp4, 5.0),
            format!("-reconnect 1 -ss 5.000 -i {mp4}"),
            "an MP4 behind a .gif name still seeks"
        );
    }

    #[test]
    fn a_run_moves_once_a_frame_differs_from_its_first() {
        let mut first = FirstFrame::default();
        let mut same = |rgb: &[u8]| {
            let same = first.same(rgb);
            first.keep(rgb.to_vec());
            same
        };

        let actual = [
            same(&[1, 2, 3]),
            same(&[1, 2, 3]),
            same(&[1, 2]),
            same(&[1, 2, 3]),
        ];

        assert_eq!(
            actual,
            [true, true, false, false],
            "a change, even back, moves it"
        );
    }

    /// Decoder runs started over 1 s of 1 ms ticks when every run sends `frames`
    /// copies of `frame` and ends.
    fn runs_in_a_second(
        player: &mut Player,
        runs: &Runs,
        frames: usize,
        frame: fn() -> Frame,
        t0: Instant,
    ) -> usize {
        let mut started = 0;
        for ms in 0..=1000 {
            let pending: Vec<_> = runs.borrow_mut().drain(..).collect();
            started += pending.len();
            for (_, tx) in pending {
                for _ in 0..frames {
                    tx.send(frame()).unwrap();
                }
            }
            player.tick(after(t0, f64::from(ms) / 1000.0));
        }
        started
    }

    #[test]
    fn when_a_silent_run_never_changes_it_should_keep_its_frame_without_restarting() {
        // ffmpeg's fps filter turns one GIF frame into 1, 2 or many copies, depending on
        // the frame delay; the duration may be unknown.
        for (frames, duration) in [(1, Some(0.04)), (2, None), (12, Some(0.5))] {
            let t0 = Instant::now();
            let (mut player, runs) = fed_player(duration, t0);

            let started = runs_in_a_second(&mut player, &runs, frames, repeated, t0);

            assert_eq!(started, 1, "{frames} frames: decoder runs started in 1 s");
            assert!(player.frame().is_some(), "the still frame stays on screen");
            assert_eq!(
                player.wake_in(after(t0, 1.0)),
                None,
                "nothing waits to play"
            );
            assert!(player.error().is_none());
        }
    }

    #[test]
    fn when_a_still_picture_has_sound_it_should_keep_looping() {
        let t0 = Instant::now();
        let (spawn, runs) = fed_runs(Some(0.5), true);
        let sources = Sources {
            video: "v".into(),
            audio: Some("a".into()),
            audio_in_file: false,
            headers: Vec::new(),
            audio_headers: Vec::new(),
        };
        let mut player = Player::new(
            "u".into(),
            Size::new(10, 5),
            None,
            Some(sources),
            spawn,
            Resume::default(),
            t0,
        );

        let started = runs_in_a_second(&mut player, &runs, 12, repeated, t0);

        assert!(
            started > 1,
            "the picture plays again with its sound: {started} runs"
        );
    }

    /// The log arrives on its own thread: a frame before the stream mapping decides
    /// nothing about sound.
    #[test]
    fn sound_is_decided_only_once_the_log_lists_the_streams() {
        let t0 = Instant::now();
        let log = Arc::new(Mutex::new(Log::default()));
        let shared = Arc::clone(&log);
        let senders = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let record = std::rc::Rc::clone(&senders);
        let spawn: Spawn = Box::new(move |_, _| {
            let (tx, frames) = sync_channel(8);
            record.borrow_mut().push(tx);
            Ok(Decoder {
                child: None,
                frames,
                log: Arc::clone(&shared),
            })
        });
        let sources = Sources {
            video: "v".into(),
            audio: Some("v".into()),
            audio_in_file: true,
            headers: Vec::new(),
            audio_headers: Vec::new(),
        };
        let mut player = Player::new(
            "u".into(),
            Size::new(10, 5),
            None,
            Some(sources),
            spawn,
            Resume::default(),
            t0,
        );

        senders.borrow()[0].send(blank()).unwrap();
        player.tick(t0);
        assert_eq!(
            player.sources.as_ref().unwrap().audio.as_deref(),
            Some("v"),
            "not known yet"
        );

        {
            let mut log = log.lock();
            log.audio = true;
            log.streams_read = true;
        }
        senders.borrow()[0].send(blank()).unwrap();
        player.tick(after(t0, 0.05));
        let sources = player.sources.as_ref().unwrap();
        assert_eq!(sources.audio.as_deref(), Some("v"), "it has sound");
        assert!(!sources.audio_in_file, "decided");
    }

    #[test]
    fn a_file_shows_at_its_first_frame_whether_it_has_sound() {
        let t0 = Instant::now();
        let in_file = || Sources {
            video: "v".into(),
            audio: Some("v".into()),
            audio_in_file: true,
            headers: Vec::new(),
            audio_headers: Vec::new(),
        };
        let with_sound = Resume {
            sound: true,
            ..Resume::default()
        };

        let (spawn, runs) = fed_runs(None, false);
        let mut silent = Player::new(
            "u".into(),
            Size::new(10, 5),
            None,
            Some(in_file()),
            spawn,
            with_sound,
            t0,
        );
        runs.borrow()[0].1.send(blank()).unwrap();
        silent.tick(t0);
        assert!(!silent.sound(), "no sound to play");
        assert!(silent.take_notice().is_some());
        assert_eq!(silent.sources.as_ref().unwrap().audio, None);

        let (spawn, runs) = fed_runs(None, true);
        let mut loud = Player::new(
            "u".into(),
            Size::new(10, 5),
            None,
            Some(in_file()),
            spawn,
            Resume::default(),
            t0,
        );
        runs.borrow()[0].1.send(blank()).unwrap();
        loud.tick(t0);
        assert_eq!(loud.sources.as_ref().unwrap().audio.as_deref(), Some("v"));
    }

    /// Sound starts with a run's first frame; a takeover before that frame starts where
    /// the run starts.
    #[test]
    fn the_sound_and_a_takeover_wait_for_the_first_frame() {
        let t0 = Instant::now();
        let (spawn, runs) = fed_runs(None, false);
        let with_sound = Resume {
            at: 3.0,
            paused: false,
            sound: true,
        };
        // No sound file: trying to start the sound says so, which shows when it tried.
        let mut player = Player::new(
            "u".into(),
            Size::new(10, 5),
            None,
            Some(no_sound()),
            spawn,
            with_sound,
            t0,
        );
        assert!(player.take_notice().is_none(), "no sound before a frame");
        assert_eq!(player.resume(after(t0, 1.0)).at, 3.0);

        runs.borrow()[0].1.send(blank()).unwrap();
        player.tick(after(t0, 1.0));
        assert!(
            player.take_notice().is_some(),
            "the sound starts with the frame"
        );
    }

    #[test]
    fn a_stream_that_resolves_but_cannot_decode_is_an_error_on_screen() {
        let t0 = Instant::now();
        let (tx, rx) = sync_channel(1);
        tx.send(Ok(no_sound())).unwrap();
        let spawn: Spawn = Box::new(|_, _| Err(io::Error::other("broken")));
        let mut player = Player::new(
            "u".into(),
            Size::new(10, 5),
            Some(rx),
            None,
            spawn,
            Resume::default(),
            t0,
        );

        assert!(player.tick(t0), "the error needs a draw");
        assert_eq!(player.error(), Some("ffmpeg: broken"));
    }

    #[test]
    fn a_stream_without_a_frame_is_an_error_on_screen() {
        let t0 = Instant::now();
        let (mut player, runs) = fed_player(None, t0);
        runs.borrow_mut().clear();

        assert!(player.tick(t0), "the error needs a draw");
        assert!(player.error().is_some());
        assert!(runs.borrow().is_empty(), "no run starts again");
        assert_eq!(player.wake_in(after(t0, 1.0)), None);
    }

    /// Reddit's master playlist, shortened: two sound groups, renditions repeated
    /// per group.
    const MASTER: &str = r#"#EXTM3U
#EXT-X-MEDIA:URI="CMAF_AUDIO_64.m3u8",TYPE=AUDIO,GROUP-ID="5",NAME="audio 0",DEFAULT=YES
#EXT-X-MEDIA:URI="CMAF_AUDIO_128.m3u8",TYPE=AUDIO,GROUP-ID="6",NAME="audio 0",DEFAULT=YES
#EXT-X-STREAM-INF:BANDWIDTH=1619363,RESOLUTION=1280x720,FRAME-RATE=30,CODECS="avc1.4d401f,mp4a.40.2",AUDIO="6"
CMAF_720.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=160794,RESOLUTION=392x220,CODECS="avc1.4d401e,mp4a.40.2",AUDIO="5"
CMAF_220.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=500006,RESOLUTION=640x360,CODECS="avc1.4d401e,mp4a.40.2",AUDIO="5"
CMAF_360.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=520000,RESOLUTION=640x360,CODECS="avc1.4d401e,mp4a.40.2",AUDIO="6"
CMAF_360.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=1555722,RESOLUTION=1280x720,CODECS="avc1.4d401f,mp4a.40.2",AUDIO="5"
CMAF_720.m3u8
"#;

    #[test]
    fn the_rendition_is_the_smallest_that_needs_no_scaling_up() {
        let pick =
            |frame_box: (u32, u32)| pick_rendition(MASTER, frame_box, frame_box.1, None).unwrap();
        // A 600×300 box fits a 16:9 picture 533 wide: 640×360 covers it, and of the
        // two the one with the better sound.
        assert_eq!(
            pick((600, 300)),
            ("CMAF_360.m3u8".into(), Some("CMAF_AUDIO_128.m3u8".into()))
        );
        // Taller than 360 but narrow: the width limits the fit, 640 covers it.
        assert_eq!(pick((600, 2000)).0, "CMAF_360.m3u8");
        // Larger than every rendition: the largest.
        assert_eq!(pick((4000, 3000)).0, "CMAF_720.m3u8");
        // The display budget retains enough detail to choose the 720p rendition.
        let bounded = |master| {
            pick_rendition(master, (856, 1170), 1170, Some(MAX_VIDEO_PIXELS))
                .unwrap()
                .0
        };
        assert_eq!(bounded(MASTER), "CMAF_720.m3u8");
        assert_eq!(pick((856, 1170)).0, "CMAF_720.m3u8");
        // If both streams are smaller than the display, choose the larger one.
        let with_270 = "#EXTM3U\n\
            #EXT-X-STREAM-INF:BANDWIDTH=300000,RESOLUTION=480x270\nCMAF_270.m3u8\n\
            #EXT-X-STREAM-INF:BANDWIDTH=500000,RESOLUTION=640x360\nCMAF_360.m3u8\n";
        assert_eq!(bounded(with_270), "CMAF_360.m3u8");
    }

    #[test]
    fn a_tall_video_drawn_at_its_decoded_size_should_fetch_only_what_its_rows_show() {
        let portrait = "#EXTM3U\n\
            #EXT-X-STREAM-INF:BANDWIDTH=400000,RESOLUTION=360x640\nCMAF_360.m3u8\n\
            #EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=480x854\nCMAF_480.m3u8\n\
            #EXT-X-STREAM-INF:BANDWIDTH=1600000,RESOLUTION=720x1280\nCMAF_720.m3u8\n";
        // A 784×720 box whose tall frames get 400 px: 360×640 already covers 225×400.
        let tall = pick_rendition(portrait, (784, 720), 400, None).unwrap();
        assert_eq!(tall.0, "CMAF_360.m3u8");
    }

    #[test]
    fn a_byte_range_playlist_plays_its_whole_file() {
        let ranged = "#EXTM3U\n#EXT-X-MAP:URI=\"CMAF_720.mp4\",BYTERANGE=\"893@0\"\n#EXTINF:4,\n#EXT-X-BYTERANGE:677420@1033\nCMAF_720.mp4\n#EXTINF:4,\n#EXT-X-BYTERANGE:614215\nCMAF_720.mp4\n";
        assert_eq!(media_file(ranged).as_deref(), Some("CMAF_720.mp4"));
        // Segments in files of their own: the playlist itself is the stream.
        let split =
            "#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:4,\nseg1.m4s\n#EXTINF:4,\nseg2.m4s\n";
        assert_eq!(media_file(split), None);
    }
    #[test]
    fn when_video_waits_for_graphics_the_input_poll_should_not_spin_on_overdue_frames() {
        use crate::app::App;
        use crate::card_image::Key;
        use crate::config::Config;
        use crate::reddit::Reddit;
        use crate::worker::Pool;
        use ratatui_image::picker::Picker;
        let directory = tempfile::tempdir().unwrap();
        let start = Instant::now() - Duration::from_secs(1);
        let (mut player, runs) = fed_player(Some(10.0), start);
        runs.borrow()[0].1.send(blank()).unwrap();
        runs.borrow()[0].1.send(blank()).unwrap();
        player.tick(start);
        assert_eq!(player.wake_in(Instant::now()), Some(Duration::ZERO));
        let reddit = Reddit::test_at("http://127.0.0.1:9", false);
        let (pool, _messages) = Pool::start(reddit.clone(), 1, None);
        let picker = Picker::halfblocks();
        let mut app = App::new(
            Config::default(),
            pool,
            reddit,
            directory.path().join("state.json"),
            picker.clone(),
        );
        app.video = Some(player);
        let image = Arc::new(image::DynamicImage::new_rgb8(16, 16));
        let key = Key::new("lag-probe", &picker, Size::new(2, 1), (16, 16), 0, 1, 0);
        app.card_renderer.begin();
        app.card_renderer.tile(
            key,
            &image,
            &picker,
            crate::card_image::Preparation::Refresh,
        );
        app.card_renderer.finish();
        assert!(app.card_renderer.pending());
        assert!(
            app.wake_in() >= Duration::from_millis(10),
            "cannot advance a frame while graphics are pending; an overdue deadline must not spin"
        );
        let (input, events) = std::sync::mpsc::channel();
        input
            .send(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('q'),
                crossterm::event::KeyModifiers::NONE,
            ))
            .unwrap();
        let key = events.recv_timeout(app.wake_in()).unwrap();
        crate::keys::handle(&mut app, key);
        assert!(app.quit, "graphics preparation must not consume quit input");
        app.card_renderer.clear();
        assert_eq!(
            app.wake_in(),
            Duration::ZERO,
            "ready graphics allow the overdue frame immediately"
        );
    }
}
