use super::*;

mod audio_master_sync;
mod clock;
mod controls;
mod decoder;
mod ffmpeg;
mod kitty;
mod playback_loop;
mod player;
mod sources;
mod youtube_cancellation;

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

/// Frames a `rate` fps source decodes to in its first second, and their rate; None
/// without ffmpeg (an optional dependency). Its metadata names another rate the way
/// ffmpeg's log would.
fn decoded_second(rate: u32) -> Option<(usize, f64)> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(format!("{rate}.nut"));
    let made = std::process::Command::new("ffmpeg")
        .args(["-nostdin", "-loglevel", "error", "-f", "lavfi", "-i"])
        .arg(format!("testsrc2=size=64x36:rate={rate}:duration=1"))
        .args(["-c:v", "ffv1", "-metadata"])
        .arg("k\n[info]   Stream #0:0: Video: x, 27 fps, y=v")
        .arg(&path)
        .status()
        .is_ok_and(|status| status.success());
    if !made {
        return None;
    }
    let decoder = Decoder::spawn(path.to_str().unwrap(), 0.0, (64, 36), &[]).unwrap();
    let mut rates = Vec::new();
    while let Ok(decoded) = decoder.frames.recv_timeout(Duration::from_secs(20)) {
        rates.push(decoded.rate);
    }
    Some((rates.len(), rates[0]))
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
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("frame.gif");
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
    Some((frame.frame.width, frame.frame.height))
}

struct FrameSender {
    tx: SyncSender<Decoded>,
    index: std::cell::Cell<u64>,
    rate: std::cell::Cell<f64>,
}

impl FrameSender {
    fn send(&self, frame: Frame) -> Result<(), std::sync::mpsc::SendError<Decoded>> {
        let index = self.index.get();
        let rate = self.rate.get();
        self.tx.send(Decoded { index, frame, rate })?;
        self.index.set(index + 1);
        Ok(())
    }
}

fn frame_channel(capacity: usize) -> (FrameSender, Receiver<Decoded>) {
    let (tx, rx) = sync_channel(capacity);
    (
        FrameSender {
            tx,
            index: std::cell::Cell::new(0),
            rate: std::cell::Cell::new(MIN_FPS),
        },
        rx,
    )
}

type Runs = std::rc::Rc<std::cell::RefCell<Vec<(f64, FrameSender)>>>;

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
        let (tx, frames) = frame_channel(64);
        record.borrow_mut().push((at, tx));
        let log = Log {
            duration,
            audio,
            streams_read: !error.is_some_and(|(message, _)| message.starts_with("Error opening")),
            error: error.map(|(message, _)| message.into()),
            input_error: error.is_some_and(|(_, input)| input),
            ended: true,
            rate: None,
        };
        Ok(Decoder {
            child: None,
            frames,
            log: Arc::new(Mutex::new(log)),
            progress: Arc::new(DecodeProgress::default()),
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
