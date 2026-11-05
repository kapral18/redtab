use super::*;

#[derive(Default)]
pub(super) struct OutputState {
    pub(super) pending: usize,
    pub(super) command_pending: bool,
    pub(super) error: Option<io::Error>,
}

#[derive(Default)]
pub(super) struct SharedOutput {
    pub(super) state: parking_lot::Mutex<OutputState>,
    pub(super) changed: parking_lot::Condvar,
}

pub(super) enum Output {
    Frame(Vec<u8>),
    Command(Vec<u8>),
}

#[derive(Clone)]
pub struct OutputHandle {
    pub(super) shared: std::sync::Arc<SharedOutput>,
    pub(super) send: std::sync::mpsc::SyncSender<Output>,
}

pub(super) static OUTPUT: std::sync::OnceLock<OutputHandle> = std::sync::OnceLock::new();

pub fn install_output(output: OutputHandle) {
    let _ = OUTPUT.set(output);
}

pub fn drain_output() -> io::Result<()> {
    OUTPUT.get().map_or(Ok(()), OutputHandle::wait)
}

pub fn queue_command(bytes: &[u8]) -> io::Result<()> {
    OUTPUT
        .get()
        .ok_or_else(|| io::Error::other("terminal output is not initialized"))?
        .command(bytes)
}

impl OutputHandle {
    pub fn command(&self, bytes: &[u8]) -> io::Result<()> {
        let mut state = self.shared.state.lock();
        if state.command_pending {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "terminal command still pending",
            ));
        }
        if let Some(error) = state.error.take() {
            return Err(error);
        }
        if self.send.try_send(Output::Command(bytes.to_vec())).is_err() {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "terminal output stopped",
            ));
        }
        state.pending += 1;
        state.command_pending = true;
        Ok(())
    }

    pub fn ready(&self) -> io::Result<bool> {
        let mut state = self.shared.state.lock();
        match state.error.take() {
            Some(error) => Err(error),
            None => Ok(state.pending == 0),
        }
    }

    pub fn wait(&self) -> io::Result<()> {
        self.wait_for(Duration::from_secs(3))
    }

    pub(super) fn wait_for(&self, timeout: Duration) -> io::Result<()> {
        let deadline = Instant::now() + timeout;
        let mut state = self.shared.state.lock();
        while state.pending != 0 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "terminal output did not drain",
                ));
            }
            self.shared.changed.wait_for(&mut state, remaining);
        }
        match state.error.take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

pub struct AsyncWriter {
    pub(super) frame: Vec<u8>,
    pub(super) send: std::sync::mpsc::SyncSender<Output>,
    pub(super) shared: std::sync::Arc<SharedOutput>,
}

impl AsyncWriter {
    pub fn new(mut output: impl Write + Send + 'static) -> (Self, OutputHandle) {
        let shared = std::sync::Arc::new(SharedOutput::default());
        let worker = shared.clone();
        let (send, receive) = std::sync::mpsc::sync_channel::<Output>(2);
        std::thread::Builder::new()
            .name("redtab-terminal-output".into())
            .spawn(move || {
                while let Ok(message) = receive.recv() {
                    let command = matches!(message, Output::Command(_));
                    let (Output::Frame(frame) | Output::Command(frame)) = message;
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        output.write_all(&frame).and_then(|()| output.flush())
                    }))
                    .unwrap_or_else(|_| Err(io::Error::other("terminal output worker panicked")));
                    let failed = result.is_err();
                    let mut state = worker.state.lock();
                    state.pending -= 1;
                    if command {
                        state.command_pending = false;
                    }
                    state.error = result.err();
                    if failed {
                        state.pending = 0;
                        state.command_pending = false;
                    }
                    worker.changed.notify_all();
                    if failed {
                        break;
                    }
                }
            })
            .expect("cannot start terminal output");
        (
            Self {
                frame: Vec::new(),
                send: send.clone(),
                shared: shared.clone(),
            },
            OutputHandle { shared, send },
        )
    }
}

impl Write for AsyncWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.frame.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.frame.is_empty() || (self.frame.starts_with(BEGIN) && !self.frame.ends_with(END)) {
            return Ok(());
        }
        let mut state = self.shared.state.lock();
        if let Some(error) = state.error.take() {
            return Err(error);
        }
        if state.pending != 0 {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "terminal frame still pending",
            ));
        }
        state.pending = 1;
        if self
            .send
            .try_send(Output::Frame(std::mem::take(&mut self.frame)))
            .is_err()
        {
            state.pending = 0;
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "terminal output stopped",
            ));
        }
        Ok(())
    }
}
