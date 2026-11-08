//! An in-memory server for tests. A test receives each request and answers when it
//! chooses; answers stream like a socket would, stalls and
//! early ends included, yet the test binary opens no socket a host firewall could hold.

use std::io::{self, Read};
use std::pin::Pin;
use std::sync::{Mutex, mpsc};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;

pub(crate) struct Server {
    pub(super) fixture: super::Fixture,
    pub(super) requests: Mutex<mpsc::Receiver<Request>>,
}

impl Server {
    /// The host the transport answers from this server, as `host` in `http://host/`.
    pub(crate) fn server_addr(&self) -> &str {
        &self.fixture.host
    }

    pub(crate) fn recv_timeout(&self, timeout: Duration) -> io::Result<Option<Request>> {
        match self.requests.lock().unwrap().recv_timeout(timeout) {
            Ok(request) => Ok(Some(request)),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(io::ErrorKind::BrokenPipe.into()),
        }
    }
}

pub(crate) struct Header {
    pub(crate) field: String,
    pub(crate) value: String,
}

impl Header {
    pub(crate) fn new(field: &str, value: &str) -> Self {
        Self {
            field: field.into(),
            value: value.into(),
        }
    }
}

pub(crate) struct Request {
    pub(super) url: String,
    pub(super) method: String,
    pub(super) headers: Vec<Header>,
    pub(super) reply: mpsc::Sender<Response>,
}

impl Request {
    pub(crate) fn url(&self) -> &str {
        &self.url
    }

    pub(crate) fn method(&self) -> &str {
        &self.method
    }

    pub(crate) fn headers(&self) -> &[Header] {
        &self.headers
    }

    pub(crate) fn respond(self, response: Response) -> io::Result<()> {
        self.reply
            .send(response)
            .map_err(|_| io::ErrorKind::BrokenPipe.into())
    }

    /// Answers 200, announcing `len` bytes when given; the body is what the test writes,
    /// and dropping the writer ends it, short of `len` included.
    pub(crate) fn stream(self, len: Option<usize>) -> Writer {
        let (tx, rx) = mpsc::channel();
        let response = Response {
            status: 200,
            headers: Vec::new(),
            body: Box::new(Piped {
                chunks: rx,
                chunk: io::Cursor::new(Vec::new()),
            }),
            len,
        };
        let _ = self.respond(response);
        Writer(tx)
    }
}

pub(crate) struct Writer(mpsc::Sender<Vec<u8>>);

impl io::Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .send(bytes.to_vec())
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct Piped {
    chunks: mpsc::Receiver<Vec<u8>>,
    chunk: io::Cursor<Vec<u8>>,
}

impl Read for Piped {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let n = self.chunk.read(buf)?;
            if n > 0 || buf.is_empty() {
                return Ok(n);
            }
            match self.chunks.recv() {
                Ok(chunk) => self.chunk = io::Cursor::new(chunk),
                Err(_) => return Ok(0),
            }
        }
    }
}

pub(crate) struct Response {
    pub(super) status: u16,
    pub(super) headers: Vec<(String, String)>,
    pub(super) body: Box<dyn Read + Send>,
    /// The length the answer announces; a body that ends sooner fails the read.
    pub(super) len: Option<usize>,
}

impl Response {
    pub(crate) fn new(
        status: u16,
        headers: Vec<Header>,
        body: impl Read + Send + 'static,
        len: Option<usize>,
    ) -> Self {
        Self {
            status,
            headers: headers.into_iter().map(|h| (h.field, h.value)).collect(),
            body: Box::new(body),
            len,
        }
    }

    pub(crate) fn empty(status: u16) -> Self {
        Self::from_data(Vec::new()).with_status_code(status)
    }

    pub(crate) fn from_string(body: impl Into<String>) -> Self {
        Self::from_data(body.into().into_bytes())
    }

    pub(crate) fn from_data(body: impl Into<Vec<u8>>) -> Self {
        let body = body.into();
        Self {
            status: 200,
            headers: Vec::new(),
            len: Some(body.len()),
            body: Box::new(io::Cursor::new(body)),
        }
    }

    pub(crate) fn with_status_code(mut self, status: u16) -> Self {
        self.status = status;
        self
    }

    pub(crate) fn with_header(mut self, header: Header) -> Self {
        self.headers.push((header.field, header.value));
        self
    }

    /// The answer as the transport receives it: the body streams from a reader thread.
    pub(super) fn into_http(self, uri: wreq::Uri) -> wreq::Response {
        use wreq::ResponseBuilderExt;
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let (mut body, len) = (self.body, self.len);
        std::thread::spawn(move || {
            let mut sent = 0;
            let mut chunk = [0; 8192];
            loop {
                match body.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        sent += n;
                        if tx.send(Ok(Bytes::copy_from_slice(&chunk[..n]))).is_err() {
                            return;
                        }
                    }
                    Err(error) => {
                        let _ = tx.send(Err(error));
                        return;
                    }
                }
            }
            if len.is_some_and(|len| sent < len) {
                let _ = tx.send(Err(io::ErrorKind::UnexpectedEof.into()));
            }
        });
        let mut response = http::Response::builder().status(self.status).uri(uri);
        if let Some(len) = len {
            response = response.header("content-length", len);
        }
        for (name, value) in self.headers {
            response = response.header(name, value);
        }
        wreq::Response::from(response.body(wreq::Body::wrap(Streamed(rx))).unwrap())
    }
}

struct Streamed(tokio::sync::mpsc::UnboundedReceiver<io::Result<Bytes>>);

impl http_body::Body for Streamed {
    type Data = Bytes;
    type Error = io::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<io::Result<http_body::Frame<Bytes>>>> {
        self.0
            .poll_recv(cx)
            .map(|chunk| chunk.map(|chunk| chunk.map(http_body::Frame::data)))
    }
}
