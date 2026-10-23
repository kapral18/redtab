//! The only transport allowed to contact Reddit. Errors deliberately omit URLs and
//! underlying library errors, which can contain credentials.

use std::io::{self, Read};
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{Result, bail};
use url::Url;
use wreq::{Client, Response};

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
    #[cfg(test)]
    pub media_origin: Option<Url>,
}

impl Transport {
    pub fn new() -> Result<Self> {
        let _enter = RUNTIME.enter();
        Ok(Self {
            client: Client::builder()
                .emulation(wreq_util::Emulation::Chrome145)
                .no_proxy()
                .redirect(wreq::redirect::Policy::none())
                .retry(wreq::retry::Policy::never())
                .connect_timeout(Duration::from_secs(10))
                .read_timeout(Duration::from_secs(20))
                .build()
                .map_err(|_| anyhow::anyhow!("cannot build HTTP transport"))?,
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

    pub fn media(&self, url: &str, range: Option<&str>) -> Result<Response> {
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
                .client
                .get(url.as_str())
                .default_headers(false)
                .header("User-Agent", super::USER_AGENT)
                .header("Accept", "*/*");
            if let Some(range) = range {
                request = request.header("Range", range);
            }
            let response = RUNTIME
                .block_on(request.send())
                .map_err(|_| network_error())?;
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
                bail!("media: HTTP {}", response.status().as_u16());
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
}

impl Reader {
    pub fn new(response: Response, limit: u64) -> Self {
        Self {
            response,
            chunk: io::Cursor::new(Vec::new()),
            left: limit,
        }
    }
}

impl Read for Reader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            let n = self.chunk.read(buf)?;
            if n > 0 {
                return Ok(n);
            }
            match RUNTIME.block_on(self.response.chunk()) {
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

pub(super) fn json(response: Response) -> Result<serde_json::Value> {
    let mut bytes = Vec::new();
    Reader::new(response, 16 * 1024 * 1024).read_to_end(&mut bytes)?;
    serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("Reddit returned invalid JSON"))
}

pub(super) fn html(response: Response) -> Result<String> {
    let mut bytes = Vec::new();
    Reader::new(response, 16 * 1024 * 1024).read_to_end(&mut bytes)?;
    String::from_utf8(bytes).map_err(|_| anyhow::anyhow!("Reddit returned invalid Home HTML"))
}
