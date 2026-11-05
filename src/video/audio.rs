use std::io::{self, Read};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

#[derive(Default)]
struct Progress {
    sample: Option<(f64, Instant)>,
    ended: bool,
    /// Whether ffmpeg's log ended without errors. A dropped connection still exits 0.
    decoded: Option<bool>,
}

pub(super) struct Audio {
    child: Option<Child>,
    decoder: Option<Child>,
    progress: Arc<Mutex<Progress>>,
    offset: f64,
    started: Instant,
}

impl Audio {
    pub(super) fn start(
        url: &str,
        headers: &[(String, String)],
        at: f64,
        now: Instant,
    ) -> io::Result<Self> {
        let mut decoder = samples(url, headers, at)
            .spawn()
            .map_err(|e| io::Error::new(e.kind(), format!("ffmpeg: {e}")))?;
        let pcm = decoder.stdout.take().expect("piped audio samples");
        let log = decoder.stderr.take().expect("piped audio decoder log");
        let started = crate::media::local_command("ffplay")
            .args([
                "-nodisp",
                "-autoexit",
                "-loglevel",
                "quiet",
                "-stats",
                "-sync",
                "audio",
                // The pipe delivers no faster than the network; don't wait to fill a probe.
                "-f",
                "nut",
                "-probesize",
                "32",
                "-analyzeduration",
                "0",
                "pipe:0",
            ])
            .stdin(pcm)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match started {
            Ok(child) => child,
            Err(error) => {
                let _ = decoder.kill();
                let _ = decoder.wait();
                return Err(io::Error::new(error.kind(), format!("ffplay: {error}")));
            }
        };
        let stderr = child.stderr.take().expect("piped audio status");
        let progress = Arc::new(Mutex::new(Progress::default()));
        let sink = progress.clone();
        std::thread::spawn(move || read_progress(stderr, &sink));
        let sink = progress.clone();
        std::thread::spawn(move || read_decoder_log(log, &sink));
        Ok(Self {
            child: Some(child),
            decoder: Some(decoder),
            progress,
            offset: at,
            started: now,
        })
    }

    pub(super) fn position(&self, now: Instant) -> Option<f64> {
        let progress = self.progress.lock();
        let (at, observed) = progress.sample?;
        // Stats arrive every30ms. A stopped reader must not advance video forever.
        let between = if progress.ended {
            0.0
        } else {
            now.saturating_duration_since(observed)
                .min(Duration::from_millis(100))
                .as_secs_f64()
        };
        Some(self.offset + at + between)
    }

    /// Whether the sound ended normally, once it has. `duration` is the media's length.
    pub(super) fn completed(&mut self, now: Instant, duration: Option<f64>) -> Option<bool> {
        let progress = self.progress.lock();
        if progress.sample.is_none()
            && now.saturating_duration_since(self.started) >= Duration::from_secs(10)
        {
            return Some(false);
        }
        if !progress.ended {
            return None;
        }
        let played = match &mut self.child {
            Some(child) => match child.try_wait() {
                Ok(Some(status)) => status.success(),
                Ok(None) => return None,
                Err(_) => return Some(false),
            },
            None => true,
        };
        // ffplay ends normally at the end of its pipe, even when ffmpeg failed mid-stream.
        let fed = match &mut self.decoder {
            Some(decoder) if played => match (decoder.try_wait(), progress.decoded) {
                (Ok(None), _) | (_, None) => return None,
                // ffmpeg also logs a drop that `-reconnect` recovered from. Like a video
                // run, the sound was cut short only before the known end.
                (Ok(Some(status)), Some(clean)) => {
                    let heard = progress.sample.map(|(at, _)| self.offset + at);
                    status.success()
                        && (clean || duration.zip(heard).is_some_and(|(end, at)| at + 1.0 >= end))
                }
                (Err(_), _) => false,
            },
            _ => true,
        };
        Some(played && fed && progress.sample.is_some())
    }

    #[cfg(test)]
    pub(super) fn fixture(at: f64, now: Instant) -> Self {
        Self {
            child: None,
            decoder: None,
            progress: Arc::new(Mutex::new(Progress {
                decoded: Some(true),
                ..Progress::default()
            })),
            offset: at,
            started: now,
        }
    }

    #[cfg(test)]
    pub(super) fn observe(&self, at: f64, now: Instant) {
        self.progress.lock().sample = Some((at, now));
    }

    #[cfg(test)]
    pub(super) fn finish(&self) {
        self.progress.lock().ended = true;
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        for mut child in [self.child.take(), self.decoder.take()]
            .into_iter()
            .flatten()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Raw audio from exactly `at`. ffplay seeks to the nearest index entry, which can be
/// seconds early in YouTube audio; ffmpeg's input seek decodes up to `at`.
fn samples(url: &str, headers: &[(String, String)], at: f64) -> Command {
    let mut command = crate::media::local_command("ffmpeg");
    command.args(["-nostdin", "-loglevel", "error"]);
    command
        .args(super::header_args(headers))
        .args(super::input_args(url, at))
        .args(["-vn", "-sn", "-dn", "-c:a", "pcm_s16le", "-f", "nut", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn read_decoder_log(mut reader: impl Read, progress: &Mutex<Progress>) {
    let mut bytes = [0; 4096];
    let mut clean = true;
    while let Ok(count) = reader.read(&mut bytes) {
        if count == 0 {
            break;
        }
        clean &= bytes[..count].iter().all(u8::is_ascii_whitespace);
    }
    progress.lock().decoded = Some(clean);
}

fn read_progress(mut reader: impl Read, progress: &Mutex<Progress>) {
    let mut bytes = [0; 4096];
    let mut line = Vec::with_capacity(512);
    let mut overflow = false;
    while let Ok(count) = reader.read(&mut bytes) {
        if count == 0 {
            break;
        }
        for &byte in &bytes[..count] {
            if matches!(byte, b'\r' | b'\n') {
                if !overflow && let Some(at) = position(&line) {
                    progress.lock().sample = Some((at, Instant::now()));
                }
                line.clear();
                overflow = false;
            } else if line.len() < 512 {
                line.push(byte);
            } else {
                overflow = true;
            }
        }
    }
    progress.lock().ended = true;
}

fn position(line: &[u8]) -> Option<f64> {
    let mut fields = std::str::from_utf8(line).ok()?.split_ascii_whitespace();
    let at: f64 = fields.next()?.parse().ok()?;
    (at.is_finite() && at >= -0.25 && fields.next()? == "M-A:").then_some(at.max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn when_audio_exits_unsuccessfully_after_starting_its_completion_should_report_failure() {
        let now = Instant::now();
        let mut audio = Audio::fixture(0.0, now);
        audio.observe(0.25, now);
        audio.finish();
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "exit 1"])
            .spawn()
            .unwrap();
        child.wait().unwrap();
        audio.child = Some(child);

        assert_eq!(audio.completed(now, None), Some(false));
    }

    #[test]
    fn when_the_sample_decoder_fails_after_sound_started_completion_should_report_failure() {
        let now = Instant::now();
        let mut audio = Audio::fixture(0.0, now);
        audio.observe(0.25, now);
        audio.finish();
        let exited = |code: &str| {
            let mut child = std::process::Command::new("/bin/sh")
                .args(["-c", &format!("exit {code}")])
                .spawn()
                .unwrap();
            child.wait().unwrap();
            child
        };
        audio.child = Some(exited("0"));
        audio.decoder = Some(exited("1"));

        assert_eq!(audio.completed(now, None), Some(false));
    }

    #[test]
    fn when_the_decoder_logged_a_recovered_drop_sound_that_reached_the_end_should_complete() {
        let now = Instant::now();
        let exited = || {
            let mut child = std::process::Command::new("/bin/sh")
                .args(["-c", "exit 0"])
                .spawn()
                .unwrap();
            child.wait().unwrap();
            child
        };
        let heard_until = |at: f64| {
            let mut audio = Audio::fixture(5.0, now);
            audio.observe(at, now);
            audio.finish();
            audio.progress.lock().decoded = Some(false);
            audio.child = Some(exited());
            audio.decoder = Some(exited());
            audio
        };

        assert_eq!(heard_until(4.8).completed(now, Some(10.0)), Some(true));
        assert_eq!(heard_until(2.0).completed(now, Some(10.0)), Some(false));
        assert_eq!(heard_until(4.8).completed(now, None), Some(false));
    }

    #[test]
    fn when_ffplay_reports_seek_normalized_audio_time_it_should_add_the_seek_offset() {
        let now = Instant::now();
        let audio = Audio::fixture(5.0, now);
        audio.observe(0.20, now);

        assert!((audio.position(now + Duration::from_millis(30)).unwrap() - 5.23).abs() < 0.001);
    }

    #[test]
    fn when_the_index_lands_before_the_start_the_samples_should_begin_at_the_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sparse.webm");
        // Silent until 30s, then a tone; one index entry every 20s.
        let made = Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
            .arg("aevalsrc=if(gte(t\\,30)\\,0.5*sin(2*PI*440*t)\\,0):s=48000:d=40")
            .args(["-c:a", "libopus", "-cluster_time_limit", "20000"])
            .args(["-cluster_size_limit", "10000000"])
            .arg(&path)
            .status();
        // ffmpeg, and its Opus encoder, are optional dependencies.
        if !made.is_ok_and(|status| status.success()) {
            return;
        }

        let mut source = samples(path.to_str().unwrap(), &[], 30.0).spawn().unwrap();
        let pcm = Command::new("ffmpeg")
            .args(["-v", "error", "-f", "nut", "-i", "-", "-t", "0.05"])
            .args(["-f", "s16le", "-ac", "1", "-"])
            .stdin(source.stdout.take().unwrap())
            .output()
            .unwrap()
            .stdout;
        let _ = source.wait();

        let loudest = pcm
            .chunks_exact(2)
            .map(|sample| i16::from_le_bytes([sample[0], sample[1]]).unsigned_abs())
            .max();
        assert!(
            loudest > Some(8_000),
            "heard {loudest:?} instead of the tone"
        );
    }

    #[test]
    fn when_the_audio_connection_drops_mid_file_the_decoder_log_should_report_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tone.webm");
        let made = Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=f=440:d=20"])
            .args(["-c:a", "libopus"])
            .arg(&path)
            .status();
        let file = std::fs::read(&path);
        drop(dir);
        // ffmpeg, and its Opus encoder, are optional dependencies.
        let (true, Ok(file)) = (made.is_ok_and(|status| status.success()), file) else {
            return;
        };

        let decoded = |served: usize| {
            let listener = crate::reddit::tests::local_listener();
            let url = format!("http://{}/a.webm", listener.local_addr().unwrap());
            let body = file.clone();
            let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let stop = Arc::clone(&done);
            let server = std::thread::spawn(move || {
                use std::io::Write;
                listener.set_nonblocking(true).unwrap();
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let Ok((mut stream, _)) = listener.accept() else {
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    };
                    stream.set_nonblocking(false).unwrap();
                    let _ = stream.read(&mut [0; 4096]);
                    let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(&body[..served]);
                }
            });
            let mut decoder = samples(&url, &[], 3.0).spawn().unwrap();
            let mut pcm = decoder.stdout.take().unwrap();
            let drain = std::thread::spawn(move || io::copy(&mut pcm, &mut io::sink()));
            let progress = Mutex::new(Progress::default());
            read_decoder_log(decoder.stderr.take().unwrap(), &progress);
            let _ = decoder.wait();
            let _ = drain.join();
            done.store(true, std::sync::atomic::Ordering::Relaxed);
            server.join().unwrap();
            progress.into_inner().decoded
        };

        assert_eq!(decoded(file.len()), Some(true), "a complete file is clean");
        assert_eq!(decoded(file.len() / 2), Some(false));
    }

    #[test]
    fn when_status_contains_noise_or_nonfinite_positions_it_should_only_keep_audio_clock_samples() {
        let progress = Mutex::new(Progress::default());
        read_progress(
            &b"nan M-A: 0.000\rPRIVATE\n 0.25 M-A: 0.000\rinf M-A: 0.000\r"[..],
            &progress,
        );

        assert_eq!(progress.lock().sample.unwrap().0, 0.25);
        assert!(progress.lock().ended);
    }

    #[test]
    fn when_an_audio_status_line_is_oversized_it_should_resume_at_the_next_delimiter() {
        let mut bytes = vec![b'x'; 4096];
        bytes.extend_from_slice(b"\r 0.50 M-A: 0.000\r");
        let progress = Mutex::new(Progress::default());

        read_progress(&bytes[..], &progress);

        assert_eq!(progress.lock().sample.unwrap().0, 0.50);
    }
}
