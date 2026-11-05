use super::*;

impl Reddit {
    pub fn disconnected(show_nsfw: bool) -> Result<Self> {
        Self::with_transport(
            Transport::new()?,
            show_nsfw,
            API,
            session::Session::disconnected(),
        )
    }

    pub fn from_export(text: &str, show_nsfw: bool) -> Result<Self> {
        Self::with_transport(
            Transport::new()?,
            show_nsfw,
            API,
            session::Session::parse(text, session::now())?,
        )
    }

    pub fn account(&self) -> Option<Account> {
        self.0.account.lock().clone()
    }

    pub(crate) fn saved_id(&self) -> Option<String> {
        self.0.saved_key.lock().clone()
    }

    pub(crate) fn from_saved(text: &str, show_nsfw: bool) -> Result<Self> {
        Self::with_transport(
            Transport::new()?,
            show_nsfw,
            API,
            session::Session::stored(text, session::now())?,
        )
    }

    pub fn source(&self) -> Option<crate::auth::Source> {
        self.0.source.lock().clone()
    }

    pub fn session_fingerprint(&self) -> &str {
        &self.0.session.fingerprint
    }

    pub fn needs_sign_in(&self) -> bool {
        !self.0.session.valid() || self.0.status.lock().rejected
    }

    pub fn retire(&self) {
        self.0.retired.store(true, Ordering::Release);
        self.0.rate.wake();
    }

    pub(crate) fn for_worker(
        &self,
        priority: rate::Priority,
        retry_reads: bool,
        cancelled: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Self {
        let cancelled: transport::Cancellation = Arc::new(cancelled);
        let demand = Arc::clone(&cancelled);
        let since = Mutex::new(None);
        let read_aborted = Arc::new(AtomicBool::new(false));
        let aborted = Arc::clone(&read_aborted);
        let active_cancelled: transport::Cancellation = Arc::new(move || {
            if aborted.load(Ordering::Acquire) {
                return true;
            }
            let obsolete = demand();
            let mut since = since.lock();
            if !obsolete {
                *since = None;
                return false;
            }
            let now = Instant::now();
            let expired =
                now.saturating_duration_since(*since.get_or_insert(now)) >= READ_REUSE_GRACE;
            if expired {
                aborted.store(true, Ordering::Release);
            }
            expired
        });
        Self(
            Arc::clone(&self.0),
            self.1,
            RequestContext {
                priority,
                retry_reads,
                cancelled: Some(cancelled),
                active_cancelled: Some(active_cancelled),
                read_aborted,
            },
        )
    }

    pub(crate) fn read_aborted(&self) -> bool {
        self.2.read_aborted.load(Ordering::Acquire)
    }

    pub fn pause(&self) -> Option<Pause> {
        self.0.rate.pause()
    }

    pub(super) fn read_cancellation(&self) -> transport::Cancellation {
        let inner = Arc::clone(&self.0);
        let cancelled = self.2.active_cancelled.clone();
        Arc::new(move || {
            inner.retired.load(Ordering::Acquire)
                || cancelled.as_ref().is_some_and(|cancel| cancel())
        })
    }

    pub fn verify_account(&self) -> Result<Account> {
        let me = self.get(self.0.api.join("api/me.json")?)?;
        let account = Account {
            id: me["data"]["id"].as_str().unwrap_or_default().into(),
            name: me["data"]["name"].as_str().unwrap_or_default().into(),
        };
        if !account.valid() {
            bail!(
                "Reddit did not confirm a signed-in account; sign in in your browser and try again"
            );
        }
        *self.0.account.lock() = Some(account.clone());
        Ok(account)
    }

    pub(crate) fn verify_saved_account(&self) -> Result<()> {
        let expected = self.account();
        let actual = self.verify_account()?;
        if expected.is_some_and(|account| account.id != actual.id) {
            bail!("Saved session belongs to a different account; reconnect it from your browser");
        }
        Ok(())
    }

    pub fn save_connection(&self, source: Option<crate::auth::Source>) -> Result<()> {
        self.save_connection_with(source, crate::auth::save_export)
    }

    pub(crate) fn save_connection_with(
        &self,
        source: Option<crate::auth::Source>,
        save: impl FnOnce(&str) -> Result<()>,
    ) -> Result<()> {
        let account = self
            .account()
            .context("Verify the Reddit account before saving")?;
        save(&self.0.session.export(session::Connection {
            account: account.clone(),
            source: source.clone(),
        })?)?;
        *self.0.source.lock() = source;
        *self.0.saved_key.lock() = Some(account.id);
        Ok(())
    }

    pub fn new(show_nsfw: bool) -> Result<Self> {
        let session = session::Session::load()?;
        Self::with_transport(Transport::new()?, show_nsfw, API, session)
    }

    pub(super) fn with_transport(
        transport: Transport,
        show_nsfw: bool,
        api: &str,
        session: session::Session,
    ) -> Result<Self> {
        let account = Mutex::new(session.connection.as_ref().map(|c| c.account.clone()));
        let source = Mutex::new(session.connection.as_ref().and_then(|c| c.source.clone()));
        let saved_key =
            Mutex::new((session.fingerprint != "disconnected").then(|| session.saved_id()));
        Ok(Self(
            Arc::new(Inner {
                transport,
                api: Url::parse(api)?,
                classic: Url::parse(if api == API { CLASSIC_API } else { api })?,
                old: Url::parse(if api == API { OLD_API } else { api })?,
                session,
                account,
                source,
                saved_key,
                retired: AtomicBool::new(false),
                status: Mutex::new(Status::default()),
                rate: rate::Rate::new(if cfg!(test) {
                    Duration::ZERO
                } else {
                    Duration::from_secs(1)
                }),
                relay: OnceLock::new(),
            }),
            show_nsfw,
            RequestContext::default(),
        ))
    }

    /// Local HTTP fixtures exercise the same client/parser without contacting Reddit.
    #[cfg(test)]
    pub fn test_at(base: &str, show_nsfw: bool) -> Self {
        let mut transport = Transport::new().unwrap();
        transport.media_origin = Some(Url::parse(base).unwrap());
        Self::with_transport(
            transport,
            show_nsfw,
            &format!("{}/", base.trim_end_matches('/')),
            session::Session::parse(
                ".reddit.com\tTRUE\t/\tTRUE\t0\treddit_session\tfixture-session\n",
                0,
            )
            .unwrap(),
        )
        .unwrap()
    }

    #[cfg(test)]
    pub fn test_at_paced(base: &str, interval: Duration) -> Self {
        let mut reddit = Self::test_at(base, false);
        Arc::get_mut(&mut reddit.0).unwrap().rate = rate::Rate::new(interval);
        reddit
    }

    /// Reuse the same account, transport and health without rereading credentials.
    pub fn with_nsfw(&self, show_nsfw: bool) -> Self {
        Self(Arc::clone(&self.0), show_nsfw, self.2.clone())
    }

    pub fn cache_namespace(&self) -> String {
        let identity = self.account().map_or_else(
            || self.0.session.fingerprint.clone(),
            |a| format!("account:{}", a.id),
        );
        format!(
            "reddit-web-v1\n{}\nnsfw={}\nsession={}",
            self.0.api, self.1, identity
        )
    }
}
