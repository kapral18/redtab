use super::*;

impl Reddit {
    pub fn health(&self) -> Health {
        if self.needs_sign_in() {
            return Health::SignedOut;
        }
        let status = self.0.status.lock();
        match status.up {
            _ if status.rejected => Health::Down,
            Some(false) => Health::Down,
            _ if self.pause().is_some() => Health::Paused,
            Some(true) => Health::Up,
            None => Health::Unknown,
        }
    }

    pub(super) fn record(&self, up: bool) {
        self.0.status.lock().up = Some(up);
    }

    /// Asks Reddit for one listing, for `--check`, and returns the health it showed.
    pub fn check(&self) -> Health {
        let listing = self.home(Sort::Best, TimeRange::Day, None);
        match (listing, self.health()) {
            (Err(_), Health::Up) => Health::Down,
            (_, health) => health,
        }
    }

    pub(super) fn get(&self, url: Url) -> Result<Value> {
        transport::json(
            self.request(url, "application/json")?,
            Some(self.read_cancellation()),
        )
    }

    /// Non-www hosts (Posts search, community listings, r/all) get only `.reddit.com` cookies; when those are missing
    /// from a live session, only that request fails.
    pub(super) fn cookie(&self, url: &Url, www: bool) -> Result<wreq::header::HeaderValue> {
        let header = self.0.session.header(url.path(), www, session::now());
        if www {
            header
        } else {
            header.map_err(|error| {
                if self.0.session.valid() {
                    anyhow::anyhow!(CLASSIC_SESSION)
                } else {
                    error
                }
            })
        }
    }

    pub(super) fn request(&self, url: Url, accept: &str) -> Result<wreq::Response> {
        self.send(url, accept, |client, url| client.get(url))
    }

    pub(super) fn send(
        &self,
        url: Url,
        accept: &str,
        request: impl FnOnce(&wreq::Client, &str) -> wreq::RequestBuilder,
    ) -> Result<wreq::Response> {
        self.send_with_timeout(url, accept, Duration::from_secs(20), request)
            .map(|(response, _)| response)
    }

    pub(super) fn send_with_timeout(
        &self,
        url: Url,
        accept: &str,
        timeout: Duration,
        request: impl FnOnce(&wreq::Client, &str) -> wreq::RequestBuilder,
    ) -> Result<(wreq::Response, Duration)> {
        if self.0.retired.load(Ordering::Acquire) {
            bail!("This Reddit connection was replaced");
        }
        let www = url.origin() == self.0.api.origin();
        if !www && url.origin() != self.0.classic.origin() && url.origin() != self.0.old.origin() {
            bail!("refusing to send Reddit cookies to another origin");
        }
        {
            let status = self.0.status.lock();
            if status.rejected {
                bail!(session::SIGN_IN);
            }
        }
        let cookie = self.cookie(&url, www).inspect_err(|_| {
            if www {
                self.record(false);
            }
        })?;
        let mut request = request(&self.0.transport.client, url.as_str())
            .header("Cookie", cookie)
            .header("Accept", accept)
            .header("User-Agent", USER_AGENT)
            .timeout(timeout);
        if accept == home::ACCEPT {
            request = request.header("Referer", self.0.api.as_str());
        }
        let request = request.build().map_err(|_| transport::network_error())?;
        let safe = request.method() == wreq::Method::GET;
        let retry = self.2.retry_reads && safe;
        let cancelled = safe.then(|| self.read_cancellation());
        let mut pending = Some(request);
        let mut waited = Duration::ZERO;
        let mut network = Duration::ZERO;
        let response = loop {
            let waiting = Instant::now();
            let admission = self.0.rate.admit(&url, self.2.priority, retry, || {
                self.0.retired.load(Ordering::Acquire)
                    || self.2.cancelled.as_ref().is_some_and(|cancel| cancel())
            })?;
            waited += waiting.elapsed();
            if self.needs_sign_in() {
                bail!(session::SIGN_IN);
            }
            let mut outgoing = if retry {
                pending.as_ref().and_then(wreq::Request::try_clone)
            } else {
                pending.take()
            }
            .context("Cannot repeat this Reddit request")?;
            outgoing
                .headers_mut()
                .insert("Cookie", self.cookie(&url, www)?);
            let available = timeout.saturating_sub(network);
            if available.is_zero() {
                bail!("Reddit request timed out; retry when ready");
            }
            let sent = Instant::now();
            let response = transport::wait(
                transport::send(
                    wreq::RequestBuilder::from_parts(self.0.transport.client.clone(), outgoing),
                    Some(available),
                ),
                cancelled.as_ref(),
            )?
            .map_err(|_| {
                self.record(false);
                transport::network_error()
            })?;
            network += sent.elapsed();
            self.0
                .rate
                .observe(&response, &url, sent, admission.sequence, admission.older);
            let code = response.status().as_u16();
            if www || code != 401 {
                self.record(!matches!(code, 401 | 500..));
            }
            if response.status().as_u16() == 429 && retry {
                continue;
            }
            break response;
        };
        let status = response.status().as_u16();
        match status {
            200..=299 => Ok((response, waited)),
            401 if www => {
                self.0.status.lock().rejected = true;
                bail!(session::SIGN_IN);
            }
            401 => bail!(CLASSIC_REFUSED),
            403 => Err(transport::access_denied(response, cancelled)?.into()),
            404 => Err(NotFound.into()),
            429 => bail!("Reddit rate limit reached; wait before retrying"),
            300..=399 => bail!(
                "Reddit redirected the request (HTTP {status}); reconnect through Accounts or redtab --signin"
            ),
            _ => bail!("Reddit: HTTP {status}"),
        }
    }
}
