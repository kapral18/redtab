//! Inline video in the post view. ffmpeg decodes the stream into a pipe of PPM
//! frames, sized for the video area; the UI shows each frame when it is due. In a
//! kitty-graphics terminal (Ghostty, kitty) a frame goes out as one kitty image under
//! a fixed id, scaled by the terminal; other terminals get a ratatui-image protocol,
//! encoded off the UI thread. ffplay plays the sound in its own process, started at
//! the video's position.

use std::io::{self, BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, TryRecvError, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use ratatui::buffer::{Buffer, CellDiffOption};
use ratatui::layout::{Rect, Size};
use ratatui::style::Color;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::Protocol;
use ratatui_image::{FontSize, Resize};

/// Frames per second the decoder emits.
const FPS: f64 = 24.0;
/// Largest decoded frame. kitty scales it up to the area; the other protocols draw
/// it at its size. Each frame crosses the terminal connection, so more pixels cost
/// more than they show.
const MAX_FRAME: (u32, u32) = (1280, 720);
/// Largest frame for sixel, whose encoding takes about 45 ms at 1280×720, more
/// than a frame's time: small enough to keep up.
const MAX_SIXEL_FRAME: (u32, u32) = (640, 360);
/// Frames decoded ahead of the one on screen.
const AHEAD: usize = 3;
/// A frame due longer ago than this means the stream stalled (network): play on from
/// it instead of racing through the frames that were late.
const STALL: f64 = 0.25;
/// How far `,` and `.` seek, in seconds.
pub const SEEK_STEP: f64 = 5.0;

/// One decoded frame.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub pixels: Pixels,
}

pub enum Pixels {
    /// zlib-compressed RGB, ready for the kitty protocol.
    Zlib(Vec<u8>),
    /// Encoded for the terminal's image protocol, sized for the area.
    Image(Protocol),
}

/// How frames reach the terminal.
#[derive(Clone)]
pub enum Output {
    Kitty,
    /// Any other protocol, for an area this many cells big.
    Image(Picker, Size),
}

/// What ffmpeg's log said about the stream.
#[derive(Default)]
struct Log {
    duration: Option<f64>,
    /// The input has a sound stream.
    audio: bool,
    /// ffmpeg logged all input streams (its stream mapping): `audio` is final. The
    /// log and the frames come on separate threads, so a frame can come first.
    streams_read: bool,
    error: Option<String>,
}

/// One ffmpeg run, from a start position to the end of the stream.
struct Decoder {
    /// None for a decoder fed by a test.
    child: Option<Child>,
    frames: Receiver<Frame>,
    log: Arc<Mutex<Log>>,
}

impl Decoder {
    fn spawn(url: &str, start: f64, frame_box: (u32, u32), output: Output) -> io::Result<Self> {
        let (width, height) = frame_box;
        let filter = format!(
            "fps={FPS},scale=w={width}:h={height}:force_original_aspect_ratio=decrease:flags=bilinear"
        );
        let mut command = Command::new("ffmpeg");
        command.args([
            "-nostdin",
            "-hide_banner",
            "-nostats",
            "-loglevel",
            "level+info",
        ]);
        // A long pause leaves the connection idle until the server drops it: pick the
        // file up again where it broke rather than end early.
        if url.starts_with("http://") || url.starts_with("https://") {
            command.args(["-reconnect", "1"]);
        }
        let mut child = command
            .args(["-ss", &format!("{start:.3}"), "-i", url])
            .args(["-an", "-sn", "-dn", "-vf", &filter])
            // 8 bits per channel whatever the source (10-bit video makes 16-bit PPM).
            .args(["-pix_fmt", "rgb24", "-f", "image2pipe", "-c:v", "ppm", "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other("ffmpeg pipes missing"));
        };
        let log = Arc::new(Mutex::new(Log::default()));
        let sink = Arc::clone(&log);
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { return };
                let Ok(mut log) = sink.lock() else { return };
                if log.duration.is_none() {
                    log.duration = parse_duration(&line);
                }
                if line.contains("Stream #0:") && line.contains(": Audio: ") {
                    log.audio = true;
                }
                if line.contains("Stream mapping:") {
                    log.streams_read = true;
                }
                if let Some(text) = line
                    .strip_prefix("[error] ")
                    .or_else(|| line.strip_prefix("[fatal] "))
                {
                    log.error = Some(text.trim().to_string());
                }
            }
        });
        let (tx, frames) = sync_channel(AHEAD);
        std::thread::spawn(move || {
            let mut out = BufReader::with_capacity(1 << 20, stdout);
            while let Ok(Some((width, height, rgb))) = read_ppm(&mut out) {
                let pixels = match &output {
                    Output::Kitty => Pixels::Zlib(zlib(&rgb)),
                    Output::Image(picker, cells) => {
                        let Some(img) = image::RgbImage::from_raw(width, height, rgb) else {
                            return;
                        };
                        let image = image::DynamicImage::ImageRgb8(img);
                        match picker.new_protocol(image, *cells, Resize::Fit(None)) {
                            Ok(protocol) => Pixels::Image(protocol),
                            Err(_) => return,
                        }
                    }
                };
                let frame = Frame {
                    width,
                    height,
                    pixels,
                };
                // The player stopped this run.
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

fn zlib(raw: &[u8]) -> Vec<u8> {
    use std::io::Write as _;
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    // Writing into a Vec cannot fail.
    let _ = enc.write_all(raw);
    enc.finish().unwrap_or_default()
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

/// Where the picture and the sound come from.
#[derive(Debug, PartialEq)]
struct Sources {
    video: String,
    audio: Option<String>,
    /// `audio` is the video's own file: whether it holds sound shows once ffmpeg
    /// reads it.
    audio_in_file: bool,
}

impl Sources {
    /// One file for both.
    fn file(url: &str) -> Self {
        Self {
            video: url.to_string(),
            audio: Some(url.to_string()),
            audio_in_file: true,
        }
    }
}

/// The files to decode for `url`. Reddit's HLS playlists cut each rendition out of
/// one fragmented MP4 by byte ranges, and ffmpeg's HLS reader stops after the first
/// range; the whole file plays and seeks well. So for a playlist: the rendition that
/// fills `frame_box` without scaling up (else the largest), its MP4, and the MP4 of
/// its sound. Anything else plays as it is.
fn resolve(url: &str, frame_box: (u32, u32)) -> Result<Sources, String> {
    let base = url::Url::parse(url).map_err(|e| format!("bad video URL: {e}"))?;
    if !base.path().ends_with(".m3u8") {
        return Ok(Sources::file(url));
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
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
    let Some((video, audio)) = pick_rendition(&master, frame_box) else {
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
        // Sound that fails to resolve leaves the picture playing, without sound.
        Some(audio) => Sources {
            video,
            audio: join(&audio).and_then(file).ok(),
            audio_in_file: false,
        },
        None => Sources::file(&video),
    })
}

/// From a master playlist: the video rendition's playlist and its sound's. The
/// smallest rendition at least as large as `frame_box` in one dimension that limits
/// the fit (no scaling up), else the largest; between equal sizes the richer one.
fn pick_rendition(master: &str, frame_box: (u32, u32)) -> Option<(String, Option<String>)> {
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
    // Shown at the box's size, the rendition is scaled down or kept, never up.
    let covers = |w: u32, h: u32| {
        w > 0
            && h > 0
            && (f64::from(frame_box.0) / f64::from(w)).min(f64::from(frame_box.1) / f64::from(h))
                <= 1.0
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

/// An inline video: decoding, timing, pause, sound and seeking.
pub struct Player {
    /// The post's stream URL, as the post gives it.
    pub url: String,
    /// The area, in cells, the frames are sized for.
    pub area: Size,
    /// Resolving the stream to its files; `sources` holds the answer.
    resolving: Option<Receiver<Result<Sources, String>>>,
    sources: Option<Sources>,
    spawn: Spawn,
    decoder: Option<Decoder>,
    /// Media time of the current run's first frame.
    start: f64,
    /// Frames this run has shown.
    shown: u64,
    next: Option<Frame>,
    frame: Option<Frame>,
    /// Counts frames put on screen; the drawing side sends a frame once per number.
    pub frame_number: u64,
    clock: Clock,
    paused: bool,
    hidden: bool,
    /// While stopped (paused, hidden) after a seek or a paused start: show the
    /// first frame at the new position.
    show_one: bool,
    /// The sound starts with this run's first frame, so a slow start does not
    /// start it twice.
    sound_waits: bool,
    sound: bool,
    audio: Option<Child>,
    duration: Option<f64>,
    error: Option<String>,
    /// A message for the reader (no sound, no ffplay), and whether it is an error.
    notice: Option<(String, bool)>,
}

/// The largest frame, in pixels, for an `area` cells big at `font`: the area's
/// pixels, within the cap for the output.
fn frame_box(area: Size, font: FontSize, output: &Output) -> (u32, u32) {
    let max = match output {
        Output::Image(picker, _) if picker.protocol_type() == ProtocolType::Sixel => {
            MAX_SIXEL_FRAME
        }
        _ => MAX_FRAME,
    };
    (
        (u32::from(area.width) * u32::from(font.width)).clamp(2, max.0),
        (u32::from(area.height) * u32::from(font.height)).clamp(2, max.1),
    )
}

impl Player {
    /// Plays `url` (absolute) for an `area` cells big at `font`, from `resume`.
    pub fn start(
        url: String,
        area: Size,
        font: FontSize,
        output: Output,
        resume: Resume,
        now: Instant,
    ) -> Self {
        let frame_box = frame_box(area, font, &output);
        let (tx, rx) = sync_channel(1);
        let target = url.clone();
        std::thread::spawn(move || {
            let _ = tx.send(resolve(&target, frame_box));
        });
        let spawn: Spawn = Box::new(move |sources: &Sources, at| {
            Decoder::spawn(&sources.video, at, frame_box, output.clone())
        });
        Self::new(url, area, Some(rx), None, spawn, resume, now)
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

    /// Advance to the frame due now. Returns true when the picture or the status
    /// line changed.
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

    /// Make sure a frame waits in `next`. False when none is ready. A run that
    /// ends short of the known duration (a dropped connection) goes on from where it
    /// broke; one that reached the end, or a seek past it, plays again from the start;
    /// one that never gave a frame is an error.
    fn receive(&mut self, now: Instant, changed: &mut bool) -> bool {
        if self.next.is_some() {
            return true;
        }
        let Some(decoder) = &self.decoder else {
            return false;
        };
        let received = decoder.frames.try_recv();
        let log = Arc::clone(&decoder.log);
        let mut log = log.lock().ok();
        match received {
            Ok(frame) => {
                self.next = Some(frame);
                if self.duration.is_none() {
                    self.duration = log.as_ref().and_then(|l| l.duration);
                }
                let audio = log.as_ref().filter(|l| l.streams_read).map(|l| l.audio);
                drop(log);
                if let Some(audio) = audio {
                    self.learn_audio(audio);
                }
                true
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                *changed = true;
                let error = log.as_mut().and_then(|l| l.error.take());
                drop(log);
                let end = self.due();
                if self.shown > 0 && self.duration.is_some_and(|d| end + 1.0 < d) {
                    self.run_from(end, now);
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

    /// A frame of a file that may carry its own sound, once ffmpeg's log has told
    /// whether it has sound.
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
            self.frame = Some(frame);
            self.shown += 1;
            self.frame_number += 1;
        }
    }

    /// How long until the next frame is due; None while nothing will change.
    pub fn wake_in(&self, now: Instant) -> Option<Duration> {
        if self.error.is_some() {
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
                // Playing shows the frames from here on.
                self.show_one = false;
                if self.sound {
                    self.play_sound(now);
                }
            }
            _ => {}
        }
    }

    /// Turn the sound on or off. A video without sound, or a missing ffplay, leaves
    /// it off with a notice.
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
        if let Some(duration) = self.duration {
            to = to.min((duration - 1.0).max(0.0));
        }
        let was = self.playing();
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
        let started = Command::new("ffplay")
            .args(["-nodisp", "-vn", "-autoexit", "-loglevel", "quiet"])
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

    /// Sound with the picture: now when this run shows frames, else with its first.
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

    /// Where a new player for another area takes over from this one: before this
    /// run shows a frame, where the run starts, not the clock that ran while loading.
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

    /// Seconds into the video.
    pub fn position(&self, now: Instant) -> f64 {
        self.clock.position(now)
    }

    pub fn duration(&self) -> Option<f64> {
        self.duration
    }

    pub fn frame(&self) -> Option<&Frame> {
        self.frame.as_ref()
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
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

    /// ffplay failing to start, as the sound would see it.
    #[cfg(test)]
    pub fn fail_sound(&mut self) {
        self.sound = false;
        self.notice = Some(("Sound needs ffplay (part of ffmpeg)".into(), true));
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop_audio();
    }
}

/// The cells a `width`×`height` frame fills inside `area`, keeping its shape:
/// at the top left of `area`, as large as fits.
pub fn fit(width: u32, height: u32, area: Rect, font: FontSize) -> Rect {
    let (fw, fh) = (f64::from(font.width.max(1)), f64::from(font.height.max(1)));
    let (w, h) = (f64::from(width.max(1)), f64::from(height.max(1)));
    let scale = (f64::from(area.width) * fw / w).min(f64::from(area.height) * fh / h);
    let cols = ((w * scale / fw).round() as u16).clamp(1, area.width.max(1));
    let rows = ((h * scale / fh).round() as u16).clamp(1, area.height.max(1));
    Rect::new(area.x, area.y, cols, rows)
}

/// The kitty-protocol rows-and-columns marks; a placement here is at most this tall.
const DIACRITICS: [char; 32] = [
    '\u{305}', '\u{30D}', '\u{30E}', '\u{310}', '\u{312}', '\u{33D}', '\u{33E}', '\u{33F}',
    '\u{346}', '\u{34A}', '\u{34B}', '\u{34C}', '\u{350}', '\u{351}', '\u{352}', '\u{357}',
    '\u{35B}', '\u{363}', '\u{364}', '\u{365}', '\u{366}', '\u{367}', '\u{368}', '\u{369}',
    '\u{36A}', '\u{36B}', '\u{36C}', '\u{36D}', '\u{36E}', '\u{36F}', '\u{483}', '\u{484}',
];
const PLACEHOLDER: char = '\u{10EEEE}';

/// The most rows a kitty video placement can take.
pub const KITTY_MAX_ROWS: u16 = DIACRITICS.len() as u16;

/// The kitty image id of this process's video. Several redtabs in one terminal
/// (tmux panes) must not replace each other's frames; the high byte stays 0, so
/// the cells carry the id in their color alone.
pub fn kitty_id() -> u32 {
    (std::process::id() & 0x00FF_FFFF).max(1)
}

/// Sends `frame` as kitty image `id`, replacing the last frame, with a virtual
/// placement `cols`×`rows` cells big that the placeholder cells show. Inside tmux each
/// chunk is wrapped for passthrough.
pub fn kitty_transmit(id: u32, frame: &Frame, cols: u16, rows: u16, tmux: bool) -> String {
    let Pixels::Zlib(data) = &frame.pixels else {
        return String::new();
    };
    let (start, escape, end) = if tmux {
        ("\x1bPtmux;", "\x1b\x1b", "\x1b\\")
    } else {
        ("", "\x1b", "")
    };
    let encoded = base64::engine::general_purpose::STANDARD.encode(data);
    // The protocol takes at most 4096 bytes of base64 per escape.
    let chunks: Vec<&[u8]> = encoded.as_bytes().chunks(4096).collect();
    let mut out = String::with_capacity(encoded.len() + chunks.len() * 32 + 96);
    for (i, chunk) in chunks.iter().enumerate() {
        out.push_str(start);
        out.push_str(escape);
        out.push_str("_G");
        if i == 0 {
            out.push_str(&format!(
                "a=T,U=1,i={id},f=24,o=z,t=d,s={},v={},c={cols},r={rows},q=2,",
                frame.width, frame.height
            ));
        }
        out.push_str(if i + 1 < chunks.len() { "m=1;" } else { "m=0;" });
        // Base64 is ASCII.
        out.push_str(std::str::from_utf8(chunk).unwrap_or_default());
        out.push_str(escape);
        out.push('\\');
        out.push_str(end);
    }
    out
}

/// Fill `area` with the placeholder cells of kitty image `id`; `transmit` (the
/// frame's data) rides in the first cell, so it goes out with the draw.
pub fn kitty_cells(buf: &mut Buffer, area: Rect, id: u32, transmit: Option<String>) {
    let [_, r, g, b] = id.to_be_bytes();
    let rows = area.height.min(KITTY_MAX_ROWS);
    let mut transmit = transmit;
    let mut symbol = String::new();
    for y in 0..rows {
        for x in 0..area.width {
            let Some(cell) = buf.cell_mut((area.x + x, area.y + y)) else {
                continue;
            };
            // The frame's data takes the first cell's string as it is.
            let mut first = transmit.take();
            symbol.clear();
            let text = match first.as_mut() {
                Some(seq) => seq,
                None => &mut symbol,
            };
            text.push(PLACEHOLDER);
            // Later cells of a row continue from the first one.
            if x == 0 {
                text.push(DIACRITICS[usize::from(y)]);
                text.push(DIACRITICS[0]);
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

    #[test]
    #[allow(deprecated)]
    fn frames_are_sized_for_the_area_and_only_sixel_kept_small() {
        let font = FontSize::new(16, 36);
        let area = Size::new(100, 20);
        let image = |protocol| {
            let mut picker = Picker::from_fontsize(font);
            picker.set_protocol_type(protocol);
            Output::Image(picker, area)
        };

        assert_eq!(
            frame_box(Size::new(10, 5), font, &Output::Kitty),
            (160, 180)
        );
        assert_eq!(frame_box(area, font, &Output::Kitty), (1280, 720));
        assert_eq!(
            frame_box(area, font, &image(ProtocolType::Iterm2)),
            (1280, 720)
        );
        assert_eq!(
            frame_box(area, font, &image(ProtocolType::Halfblocks)),
            (1280, 720)
        );
        assert_eq!(
            frame_box(area, font, &image(ProtocolType::Sixel)),
            (640, 360)
        );
    }

    #[test]
    fn a_frame_fits_the_area_keeping_its_shape() {
        let font = FontSize::new(10, 20);
        let area = Rect::new(4, 2, 80, 10);
        // A 9:16 portrait frame: as tall as the area, narrow.
        assert_eq!(fit(360, 640, area, font), Rect::new(4, 2, 11, 10));
        // A wide frame: as wide as the area, short.
        assert_eq!(fit(1600, 80, area, font), Rect::new(4, 2, 80, 2));
    }

    #[test]
    fn kitty_frames_go_out_in_chunks_and_through_tmux() {
        let frame = Frame {
            width: 2,
            height: 1,
            pixels: Pixels::Zlib(vec![7; 5000]),
        };
        let plain = kitty_transmit(42, &frame, 3, 2, false);
        let escapes: Vec<&str> = plain.split("\x1b\\").filter(|s| !s.is_empty()).collect();
        assert_eq!(escapes.len(), 2, "5000 bytes are 6668 base64 characters");
        assert!(escapes[0].starts_with("\x1b_Ga=T,U=1,i=42,f=24,o=z,t=d,s=2,v=1,c=3,r=2,q=2,m=1;"));
        assert!(escapes[1].starts_with("\x1b_Gm=0;"));
        assert!(
            escapes
                .iter()
                .all(|e| e.split_once(';').unwrap().1.len() <= 4096)
        );

        // Inside tmux: each escape whole in its own passthrough, its ESCs doubled.
        let tmux = kitty_transmit(42, &frame, 3, 2, true);
        let wrapped: String = plain
            .split_inclusive("\x1b\\")
            .map(|esc| format!("\x1bPtmux;{}\x1b\\", esc.replace('\x1b', "\x1b\x1b")))
            .collect();
        assert_eq!(tmux, wrapped);
    }

    #[test]
    fn kitty_cells_carry_the_id_the_rows_and_the_frame_once() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 3));
        let id = 0x0001_0203;
        kitty_cells(&mut buf, Rect::new(1, 0, 2, 2), id, Some("DATA".into()));
        let cell = |x, y| buf.cell((x, y)).unwrap();

        assert_eq!(cell(1, 0).symbol(), "DATA\u{10EEEE}\u{305}\u{305}");
        assert_eq!(cell(2, 0).symbol(), "\u{10EEEE}");
        assert_eq!(cell(1, 1).symbol(), "\u{10EEEE}\u{30D}\u{305}");
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
        let runs = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let record = std::rc::Rc::clone(&runs);
        let spawn: Spawn = Box::new(move |_, at| {
            let (tx, frames) = sync_channel(64);
            record.borrow_mut().push((at, tx));
            let log = Log {
                duration,
                audio,
                streams_read: true,
                error: None,
            };
            Ok(Decoder {
                child: None,
                frames,
                log: Arc::new(Mutex::new(log)),
            })
        });
        (spawn, runs)
    }

    fn no_sound() -> Sources {
        Sources {
            video: "v".into(),
            audio: None,
            audio_in_file: false,
        }
    }

    /// A playing player whose decoder runs are channels the test feeds.
    fn fed_player(duration: Option<f64>, now: Instant) -> (Player, Runs) {
        let (spawn, runs) = fed_runs(duration, false);
        let player = Player::new(
            "u".into(),
            Size::new(10, 5),
            None,
            Some(no_sound()),
            spawn,
            Resume::default(),
            now,
        );
        (player, runs)
    }

    fn blank() -> Frame {
        Frame {
            width: 1,
            height: 1,
            pixels: Pixels::Zlib(Vec::new()),
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
        first.send(blank()).unwrap();
        drop(first);
        player.tick(t0);
        player.tick(after(t0, 0.1));
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
    fn a_run_past_the_end_or_at_it_plays_again_from_the_start() {
        let t0 = Instant::now();
        // A run started past the last frame gives none: back to the start.
        let (spawn, runs) = fed_runs(None, false);
        let resume = Resume {
            at: 5.0,
            ..Resume::default()
        };
        let mut player = Player::new(
            "u".into(),
            Size::new(10, 5),
            None,
            Some(no_sound()),
            spawn,
            resume,
            t0,
        );
        drop(runs.borrow_mut().remove(0));
        player.tick(t0);
        assert_eq!(runs.borrow()[0].0, 0.0);
        assert!(player.error().is_none());

        // A 1 s video that ran to its end loops rather than resuming at 1 s.
        let (mut player, runs) = fed_player(Some(1.0), t0);
        let first = runs.borrow_mut().remove(0).1;
        for _ in 0..24 {
            first.send(blank()).unwrap();
        }
        drop(first);
        for i in 0..=25 {
            player.tick(after(t0, f64::from(i) / FPS));
        }
        assert_eq!(runs.borrow()[0].0, 0.0);
    }

    /// The log comes on its own thread: a frame that arrives before ffmpeg logged
    /// its streams decides nothing about the sound; a later frame does.
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
            let mut log = log.lock().unwrap();
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

    /// The sound of a run starts with its first frame, not before (a slow start
    /// would otherwise start it twice); a player taking over before that frame
    /// starts where the run starts, not where the clock ran on to.
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
        let pick = |frame_box| pick_rendition(MASTER, frame_box).unwrap();
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
}
