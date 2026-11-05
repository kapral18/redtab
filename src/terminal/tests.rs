use super::*;

#[test]
fn cursor_barrier_should_require_a_successful_reply_and_fail_closed() {
    let (tx, rx) = std::sync::mpsc::channel::<CursorQuery>();
    let worker = std::thread::spawn(move || {
        rx.recv()
            .unwrap()
            .send(Err(std::io::Error::other("query failed")))
            .unwrap();
    });
    assert!(cursor_barrier(&tx).is_err());
    worker.join().unwrap();
    let (tx, rx) = std::sync::mpsc::channel::<CursorQuery>();
    drop(rx);
    assert_eq!(
        cursor_barrier(&tx).unwrap_err().kind(),
        std::io::ErrorKind::BrokenPipe
    );
    let (tx, _rx) = std::sync::mpsc::channel::<CursorQuery>();
    assert_eq!(
        cursor_barrier(&tx).unwrap_err().kind(),
        std::io::ErrorKind::TimedOut
    );
}

#[test]
fn startup_panics_restore_raw_mode_before_output_exists() {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::process::{Command, Stdio};
    const CHILD: &str = "REDTAB_TEST_EARLY_PANIC";
    if std::env::var_os(CHILD).is_some() {
        install_panic_hook();
        let result = std::thread::Builder::new()
            .name("main".into())
            .spawn(|| {
                crossterm::terminal::enable_raw_mode().unwrap();
                panic!("early startup fixture");
            })
            .unwrap()
            .join();
        assert!(result.is_err());
        return;
    }
    let mut master = -1;
    let mut slave = -1;
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        },
        0
    );
    let _master = unsafe { OwnedFd::from_raw_fd(master) };
    let slave = unsafe { OwnedFd::from_raw_fd(slave) };
    let mut before = unsafe { std::mem::zeroed::<libc::termios>() };
    assert_eq!(
        unsafe { libc::tcgetattr(slave.as_raw_fd(), &mut before) },
        0
    );
    let mut pipe = [-1; 2];
    assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
    let _reader = unsafe { OwnedFd::from_raw_fd(pipe[0]) };
    let writer = unsafe { OwnedFd::from_raw_fd(pipe[1]) };
    let flags = unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_GETFL) };
    assert_eq!(
        unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) },
        0
    );
    let bytes = [b'x'; 4096];
    loop {
        if unsafe { libc::write(writer.as_raw_fd(), bytes.as_ptr().cast(), bytes.len()) } < 0 {
            assert_eq!(
                std::io::Error::last_os_error().kind(),
                std::io::ErrorKind::WouldBlock
            );
            break;
        }
    }
    assert_eq!(
        unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_SETFL, flags) },
        0
    );
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            &format!(
                "{}::startup_panics_restore_raw_mode_before_output_exists",
                module_path!().split_once("::").unwrap().1
            ),
            "--nocapture",
        ])
        .env(CHILD, "1")
        .env("RUST_BACKTRACE", "0")
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::null())
        .stderr(Stdio::from(writer))
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        let status = child.try_wait().unwrap();
        if status.is_some() || Instant::now() >= deadline {
            break status;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let mut after = unsafe { std::mem::zeroed::<libc::termios>() };
    assert_eq!(unsafe { libc::tcgetattr(slave.as_raw_fd(), &mut after) }, 0);
    if status.is_none() {
        child.kill().unwrap();
        child.wait().unwrap();
    }
    assert!(
        status.is_some_and(|status| status.success()),
        "panic handling must finish with blocked stderr"
    );
    assert_eq!(
        after.c_lflag & (libc::ICANON | libc::ECHO),
        before.c_lflag & (libc::ICANON | libc::ECHO)
    );
}
