//! The only transport allowed to contact Reddit. Errors deliberately omit URLs and
//! underlying library errors, which can contain credentials.

use std::future::Future;
use std::io::{self, Read};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use anyhow::{Result, bail};
use url::Url;
use wreq::{Client, Response};

pub(super) type Cancellation = Arc<dyn Fn() -> bool + Send + Sync>;
pub(super) const CANCEL_POLL: Duration = Duration::from_millis(50);

#[derive(Debug)]
pub(super) struct MediaStatus(pub u16);

impl std::fmt::Display for MediaStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "media: HTTP {}", self.0)
    }
}

impl std::error::Error for MediaStatus {}

pub(super) type Sending = std::pin::Pin<Box<dyn Future<Output = Result<Response, ()>> + Send>>;

/// Sends `request`. In tests fixture hosts and live relays answer in memory and anything
/// else fails: the test binary opens no socket, which a host firewall would hold for every
/// newly built test binary.
pub(super) fn send(request: wreq::RequestBuilder, timeout: Option<Duration>) -> Sending {
    #[cfg(test)]
    return super::tests::answer(request, timeout);
    #[cfg(not(test))]
    {
        let request = match timeout {
            Some(timeout) => request.timeout(timeout),
            None => request,
        };
        Box::pin(async move { request.send().await.map_err(|_| ()) })
    }
}

/// Keep one HTTP future across polls; dropping it releases an obsolete request.
pub(super) fn wait<F: Future>(future: F, cancelled: Option<&Cancellation>) -> Result<F::Output> {
    RUNTIME.block_on(async {
        let mut future = std::pin::pin!(future);
        let Some(cancelled) = cancelled else {
            return Ok(future.await);
        };
        loop {
            if cancelled() {
                bail!(super::REQUEST_CANCELLED);
            }
            if let Ok(output) = tokio::time::timeout(CANCEL_POLL, future.as_mut()).await {
                return Ok(output);
            }
        }
    })
}

pub(super) static RUNTIME: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("HTTP runtime")
});

#[derive(Clone)]
pub(super) struct Transport {
    pub client: Client,
    /// Media over HTTP/1.1, one connection per request. On one shared HTTP/2 connection,
    /// videos read at playback pace hold its receive window and stall every other
    /// request until it times out (measured: playlists failing after 20 s).
    media_client: Client,
    #[cfg(test)]
    pub media_origin: Option<Url>,
}

impl Transport {
    pub fn new() -> Result<Self> {
        let _enter = RUNTIME.enter();
        let client = |http1: bool| {
            let builder = Client::builder().emulation(wreq_util::Emulation::Chrome145);
            if http1 { builder.http1_only() } else { builder }
                .no_proxy()
                .redirect(wreq::redirect::Policy::none())
                .retry(wreq::retry::Policy::never())
                .connect_timeout(Duration::from_secs(10))
                .read_timeout(Duration::from_secs(20))
                .build()
                .map_err(|_| anyhow::anyhow!("cannot build HTTP transport"))
        };
        Ok(Self {
            client: client(false)?,
            media_client: client(true)?,
            #[cfg(test)]
            media_origin: None,
        })
    }

    pub fn media_allowed(&self, url: &Url) -> bool {
        #[cfg(test)]
        if self
            .media_origin
            .as_ref()
            .is_some_and(|origin| origin.origin() == url.origin())
        {
            return true;
        }
        media_allowed(url)
    }

    pub fn media(
        &self,
        url: &str,
        range: Option<&str>,
        cancelled: Option<&Cancellation>,
    ) -> Result<Response> {
        let mut url = Url::parse(url).map_err(|_| anyhow::anyhow!("invalid media URL"))?;
        for hop in 0..6 {
            #[cfg(test)]
            if self
                .media_origin
                .as_ref()
                .is_some_and(|origin| origin.origin() != url.origin())
            {
                bail!("fixture client refuses non-local media");
            }
            if !self.media_allowed(&url) {
                if hop > 0 {
                    bail!("media redirected to an unsupported host or protocol");
                }
                bail!("unsupported media host or protocol");
            }
            // Without the browser defaults: media hosts send a page load (`Accept:
            // text/html`, `sec-fetch-mode: navigate`) to a reddit.com viewer page.
            let mut request = self
                .media_client
                .get(url.as_str())
                .default_headers(false)
                .header("User-Agent", super::USER_AGENT)
                .header("Accept", "*/*");
            if let Some(range) = range {
                request = request.header("Range", range);
            }
            let response = wait(send(request, None), cancelled)?.map_err(|_| network_error())?;
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| anyhow::anyhow!("media redirect has no location"))?;
                url = url
                    .join(location)
                    .map_err(|_| anyhow::anyhow!("invalid media redirect"))?;
                continue;
            }
            if !response.status().is_success() {
                return Err(MediaStatus(response.status().as_u16()).into());
            }
            return Ok(response);
        }
        bail!("too many media redirects")
    }
}

pub(super) fn network_error() -> anyhow::Error {
    anyhow::anyhow!("Reddit connection failed")
}

pub(super) fn media_allowed(url: &Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port_or_known_default() == Some(443)
        && url.host_str().is_some_and(|host| {
            matches!(
                host,
                "i.redd.it"
                    | "i.ytimg.com"
                    | "v.redd.it"
                    | "preview.redd.it"
                    | "external-preview.redd.it"
                    | "styles.redditmedia.com"
                    | "emoji.redditmedia.com"
                    | "a.thumbs.redditmedia.com"
                    | "b.thumbs.redditmedia.com"
                    | "www.redditstatic.com"
                    | "reddit-econ-prod-assets-permanent.s3.amazonaws.com"
            )
        })
}

/// Pull one chunk at a time so decoding the first GIF frame cancels the rest and
/// streaming a video never buffers the full file. Read errors contain no URL.
pub(super) struct Reader {
    response: Response,
    chunk: io::Cursor<Vec<u8>>,
    left: u64,
    cancelled: Option<Cancellation>,
}

impl Reader {
    pub fn new(response: Response, limit: u64, cancelled: Option<Cancellation>) -> Self {
        Self {
            response,
            chunk: io::Cursor::new(Vec::new()),
            left: limit,
            cancelled,
        }
    }
}

impl Read for Reader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.cancelled.as_ref().is_some_and(|cancel| cancel()) {
            return Err(io::Error::other(super::REQUEST_CANCELLED));
        }
        loop {
            let n = self.chunk.read(buf)?;
            if n > 0 {
                return Ok(n);
            }
            match wait(self.response.chunk(), self.cancelled.as_ref())
                .map_err(|_| io::Error::other(super::REQUEST_CANCELLED))?
            {
                Ok(Some(chunk)) => {
                    self.left = self
                        .left
                        .checked_sub(chunk.len() as u64)
                        .ok_or_else(|| io::Error::other("response exceeds size limit"))?;
                    self.chunk = io::Cursor::new(chunk.to_vec());
                }
                Ok(None) => return Ok(0),
                Err(_) => return Err(io::Error::other("response stream interrupted")),
            }
        }
    }
}

pub(super) fn access_denied(
    response: Response,
    cancelled: Option<Cancellation>,
) -> Result<super::AccessDenied> {
    let mut bytes = Vec::new();
    let read = Reader::new(response, 64 * 1024, cancelled.clone()).read_to_end(&mut bytes);
    if read.is_err() && cancelled.as_ref().is_some_and(|cancel| cancel()) {
        bail!(super::REQUEST_CANCELLED);
    }
    let value = read
        .ok()
        .and_then(|_| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
    if value
        .as_ref()
        .and_then(|v| v.get("reason"))
        .and_then(|v| v.as_str())
        == Some("private")
    {
        Ok(super::AccessDenied::PrivateCommunity)
    } else {
        Ok(super::AccessDenied::Forbidden)
    }
}

pub(super) fn json(
    response: Response,
    cancelled: Option<Cancellation>,
) -> Result<serde_json::Value> {
    let mut bytes = Vec::new();
    Reader::new(response, 16 * 1024 * 1024, cancelled).read_to_end(&mut bytes)?;
    serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("Reddit returned invalid JSON"))
}

pub(super) fn listing(
    response: Response,
    cancelled: Cancellation,
    progress: &mut dyn FnMut(&crate::model::Post),
) -> Result<crate::model::Listing> {
    super::parse::stream_listing(
        Reader::new(response, 16 * 1024 * 1024, Some(cancelled.clone())),
        progress,
    )
    .map_err(|_| {
        if cancelled() {
            anyhow::anyhow!(super::REQUEST_CANCELLED)
        } else {
            anyhow::anyhow!("Reddit returned an incomplete or invalid listing")
        }
    })
}
