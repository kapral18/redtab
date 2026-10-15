//! Exercise the real supervisor process, including pipes that outlive the TUI.

use std::ffi::CString;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

struct Server {
    child: Child,
    dir: PathBuf,
}

impl Server {
    fn start(script: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "redtab-supervisor-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        let fifo = CString::new(dir.join("gate").as_os_str().as_bytes()).unwrap();
        // SAFETY: fifo is a NUL-terminated path in this test's new directory.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        let child = Command::new(env!("CARGO_BIN_EXE_redtab"))
            .args(["--supervise-start-command", "sh", "-c", script, "server"])
            .arg(&dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap();
        Self { child, dir }
    }

    fn report(&mut self) -> serde_json::Value {
        let output = self.child.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            BufReader::new(output).read_line(&mut line).unwrap();
            tx.send(line).unwrap();
        });
        let line = rx
            .recv_timeout(Duration::from_secs(25))
            .expect("startup report");
        serde_json::from_str(&line).expect("JSON startup result")
    }

    fn release(&self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(self.dir.join("gate"))
            {
                Ok(mut fifo) => {
                    fifo.write_all(b"go\n").unwrap();
                    return;
                }
                Err(e) if e.raw_os_error() == Some(libc::ENXIO) && Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => panic!("server did not open its gate: {e}"),
            }
        }
    }

    fn finish(&mut self) -> String {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "supervisor exited {status}");
                return fs::read_to_string(self.dir.join("done"))
                    .expect("server completed its writes");
            }
            assert!(Instant::now() < deadline, "supervisor did not finish");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            // SAFETY: this still-live child leads the test's private process group.
            unsafe { libc::kill(-(self.child.id() as i32), libc::SIGKILL) };
            let _ = self.child.wait();
        }
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn a_forked_server_keeps_writing_after_its_launcher_reports_failure() {
    let mut server = Server::start(
        r#"( read -r go < "$1/gate"; head -c 262144 /dev/zero >&2 && printf done > "$1/done" ) &
           printf 'startup failed' >&2; exit 3"#,
    );

    let started = Instant::now();
    let report = server.report(); // closes the TUI's end of the report pipe
    let elapsed = started.elapsed();
    server.release();
    let completed = server.finish();

    assert!(report["Err"].as_str().unwrap().contains("startup failed"));
    assert!(elapsed < Duration::from_secs(5));
    assert_eq!(completed, "done");
}

#[test]
fn a_running_server_keeps_writing_after_the_tui_closes_its_pipe() {
    let mut server = Server::start(
        r#"read -r go < "$1/gate"; head -c 262144 /dev/zero >&2 && printf done > "$1/done""#,
    );

    let report = server.report(); // waits for the real 20-second startup deadline
    server.release();
    let completed = server.finish();

    assert_eq!(report, serde_json::json!({"Ok": "Running"}));
    assert_eq!(completed, "done");
}

#[test]
fn a_server_survives_the_tui_quitting_before_its_startup_report() {
    let mut server = Server::start(
        r#"read -r go < "$1/gate"; head -c 262144 /dev/zero >&2 && printf done > "$1/done""#,
    );

    drop(server.child.stdout.take());
    server.release();
    let completed = server.finish();

    assert_eq!(completed, "done");
}
