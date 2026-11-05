use super::*;

/// What ffmpeg's log said about the stream.
#[derive(Default)]
pub(super) struct Log {
    pub(super) duration: Option<f64>,
    pub(super) audio: bool,
    /// The stream mapping was logged, so `audio` is final; a frame may arrive before it.
    pub(super) streams_read: bool,
    pub(super) error: Option<String>,
    /// The input failed (connection or demuxing), not just a frame: the file was cut.
    pub(super) input_error: bool,
    /// ffmpeg closed its log: everything it will say about this run is above.
    pub(super) ended: bool,
    /// Frames per second ffmpeg outputs, logged before its first frame.
    pub(super) rate: Option<f64>,
}

/// One ffmpeg run, from a start position to the end of the stream.
pub(super) struct Decoder {
    /// None for a decoder fed by a test.
    pub(super) child: Option<Child>,
    pub(super) frames: Receiver<Decoded>,
    pub(super) log: Arc<Mutex<Log>>,
    pub(super) progress: Arc<DecodeProgress>,
}

pub(super) struct Decoded {
    pub(super) index: u64,
    pub(super) frame: Frame,
    /// Frames per second of its run.
    pub(super) rate: f64,
}

#[derive(Default)]
pub(super) struct DecodeProgress {
    pub(super) target: AtomicU64,
    pub(super) read: AtomicU64,
}

impl Decoder {
    pub(super) fn spawn(
        url: &str,
        start: f64,
        frame_box: (u32, u32),
        headers: &[(String, String)],
    ) -> io::Result<Self> {
        let filter = video_filter(frame_box, *FPS_EXPRESSIONS);
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
            // The file's metadata would be listed among the output headings read for the
            // frame rate; the frames don't carry it anyway.
            .args(["-map_metadata", "-1", "-map_chapters", "-1"])
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
        let progress = Arc::new(DecodeProgress::default());
        let reader_progress = Arc::clone(&progress);
        let rate_log = Arc::clone(&log);
        std::thread::spawn(move || {
            read_frames(
                BufReader::with_capacity(1 << 20, stdout),
                tx,
                &reader_progress,
                || logged_rate(&rate_log, Duration::from_secs(1)),
            );
        });
        Ok(Self {
            child: Some(child),
            frames,
            log,
            progress,
        })
    }
}

/// The output rate once ffmpeg's log names it; ffmpeg logs it before writing a frame,
/// but its log is read on another thread. A log that never names it (a heading cut by
/// another thread's message) gives up after `patience`, as ffmpeg waits on the frames.
pub(super) fn logged_rate(log: &Mutex<Log>, patience: Duration) -> f64 {
    let deadline = Instant::now() + patience;
    loop {
        {
            let log = log.lock();
            if let Some(rate) = log.rate {
                return rate;
            }
            if log.ended || Instant::now() >= deadline {
                return MIN_FPS;
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

pub(super) fn read_frames(
    mut out: impl BufRead,
    tx: SyncSender<Decoded>,
    progress: &DecodeProgress,
    rate: impl Fn() -> f64,
) {
    let mut first = FirstFrame::default();
    let mut known = None;
    while let Ok(Some((width, height, rgb))) = read_ppm(&mut out) {
        let index = progress.read.fetch_add(1, Ordering::Relaxed);
        let same_as_first = first.same(&rgb);
        if first.wants() {
            first.keep(rgb.clone());
        }
        if index < progress.target.load(Ordering::Relaxed) {
            continue;
        }
        let Some(img) = image::RgbImage::from_raw(width, height, rgb) else {
            return;
        };
        let frame = Frame {
            width,
            height,
            pixels: Pixels::Raw(Arc::new(image::DynamicImage::ImageRgb8(img))),
            same_as_first,
        };
        let rate = *known.get_or_insert_with(&rate);
        if tx.send(Decoded { index, frame, rate }).is_err() {
            return;
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
pub(super) fn parse_duration(line: &str) -> Option<f64> {
    let rest = line.split_once("Duration: ")?.1;
    let stamp = rest.split(',').next()?.trim();
    let mut parts = stamp.split(':');
    let (h, m, s) = (parts.next()?, parts.next()?, parts.next()?);
    let seconds =
        h.parse::<f64>().ok()? * 3600.0 + m.parse::<f64>().ok()? * 60.0 + s.parse::<f64>().ok()?;
    (seconds > 0.0).then_some(seconds)
}

/// Read ffmpeg's log until it closes, noting what it says about the stream.
pub(super) fn read_log(stderr: impl BufRead, log: &Mutex<Log>) {
    // Input streams are listed first; the rate is the output's.
    let mut output = false;
    // Split on bytes: invalid UTF-8 must not stop the read, or ffmpeg blocks on a full pipe.
    for line in stderr.split(b'\n') {
        let Ok(line) = line else { break };
        let line = String::from_utf8_lossy(&line);
        let mut log = log.lock();
        // Only ffmpeg's own headings count: at their exact indents, on a line of their own.
        // Metadata, listed among them, can read like anything (a key can even hold a line
        // break).
        let heading = line.strip_prefix("[info] ").unwrap_or_default();
        if log.duration.is_none() && heading.starts_with("  Duration: ") {
            log.duration = parse_duration(heading);
        }
        if heading.starts_with("Output #0") {
            output = true;
        }
        if output
            && log.rate.is_none()
            && heading.starts_with("  Stream #0:")
            && heading.contains(": Video: ")
        {
            let rate = heading
                .split(", ")
                .find_map(|part| part.trim().strip_suffix(" fps")?.parse::<f64>().ok());
            log.rate = Some(rate.map_or(MIN_FPS, |rate| rate.clamp(MIN_FPS, MAX_FPS)));
        }
        if heading.starts_with("  Stream #0:") && heading.contains(": Audio: ") {
            log.audio = true;
        }
        if heading.starts_with("Stream mapping:") {
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
pub(super) fn read_ppm(r: &mut impl BufRead) -> io::Result<Option<(u32, u32, Vec<u8>)>> {
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
pub(super) fn ppm_number(r: &mut impl BufRead) -> io::Result<u32> {
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
