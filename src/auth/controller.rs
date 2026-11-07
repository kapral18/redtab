use super::*;

pub struct Controller {
    pub phase: Phase,
    pub sources: Vec<Source>,
    pub accounts: Vec<crate::reddit::SavedAccount>,
    pub active_id: Option<String>,
    pub active_name: Option<String>,
    pub removal_name: Option<String>,
    pub removing_current: bool,
    pub(super) removal_id: Option<String>,
    pub selected: usize,
    pub message: Option<String>,
    pub(super) candidate: Option<Candidate>,
    pub(super) accounts_view: bool,
    pub(super) pending: Option<Receiver<Event>>,
    pub(super) cancelled: Arc<Attempt>,
    pub(super) preferred: Option<Source>,
    pub cli_selection: Option<(String, Option<String>)>,
}

impl Default for Controller {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            sources: Vec::new(),
            accounts: Vec::new(),
            active_id: None,
            active_name: None,
            removal_name: None,
            removing_current: true,
            removal_id: None,
            selected: 0,
            message: None,
            candidate: None,
            accounts_view: false,
            pending: None,
            cancelled: Arc::default(),
            preferred: None,
            cli_selection: None,
        }
    }
}

impl Controller {
    pub fn open(&mut self, preferred: Option<Source>) {
        if matches!(self.phase, Phase::Saving | Phase::Disconnecting) {
            return;
        }
        self.cancel();
        self.preferred = preferred;
        if self.cli_selection.is_some() {
            self.discover();
            return;
        }
        self.accounts_view = true;
        self.phase = Phase::LoadingAccounts;
        self.message = None;
        let cancelled = self.new_attempt();
        let (tx, rx) = channel();
        self.pending = Some(rx);
        std::thread::spawn(move || {
            let result = run_helper(Request::Accounts, &cancelled, Duration::from_secs(30))
                .and_then(|reply| match reply {
                    Reply::Accounts(accounts) => Ok(accounts),
                    _ => bail!("Could not list saved accounts"),
                })
                .map_err(|error| error.to_string());
            let _ = tx.send(Event::Accounts(result));
        });
    }

    pub fn discover(&mut self) {
        if self.busy() {
            return;
        }
        self.cancel();
        self.accounts_view = false;
        self.phase = Phase::Discovering;
        self.message = None;
        let cancelled = self.new_attempt();
        let (tx, rx) = channel();
        self.pending = Some(rx);
        std::thread::spawn(move || {
            let result = run_helper(Request::Discover, &cancelled, Duration::from_secs(40))
                .and_then(|reply| match reply {
                    Reply::Sources(sources) => Ok(sources),
                    _ => bail!("Could not discover browser profiles"),
                })
                .map_err(|e| e.to_string());
            let _ = tx.send(Event::Sources(result));
        });
    }

    pub fn account_name(&self) -> Option<String> {
        self.candidate.as_ref()?.reddit.account().map(|a| a.name)
    }

    pub fn busy(&self) -> bool {
        matches!(
            self.phase,
            Phase::LoadingAccounts
                | Phase::Switching
                | Phase::Discovering
                | Phase::Importing
                | Phase::Saving
                | Phase::Disconnecting
        )
    }

    pub(super) fn new_attempt(&mut self) -> Arc<Attempt> {
        self.cancelled = Arc::default();
        Arc::clone(&self.cancelled)
    }

    pub fn cancel(&mut self) {
        if matches!(self.phase, Phase::Saving | Phase::Disconnecting) {
            return;
        }
        self.cancelled.store(true, Ordering::Release);
        self.pending = None;
        self.candidate = None;
        self.phase = if self.accounts_view {
            Phase::Accounts
        } else {
            Phase::Choose
        };
    }

    pub fn choices(&self) -> usize {
        if self.phase == Phase::Accounts {
            self.accounts.len() + 1
        } else {
            self.sources.len()
        }
    }

    pub fn add(&mut self) {
        self.preferred = None;
        self.discover();
    }

    pub fn reconnect(&mut self) {
        if self.busy() {
            return;
        }
        if self.phase == Phase::Accounts {
            self.preferred = self
                .accounts
                .get(self.selected)
                .and_then(|account| account.source.clone());
        }
        self.discover();
    }

    pub fn select(&mut self, show_nsfw: bool) {
        if self.phase == Phase::Accounts {
            let Some(account) = self.accounts.get(self.selected).cloned() else {
                self.add();
                return;
            };
            if account.expired {
                self.message = Some(
                    "Saved sign-in expired; press r to reconnect this account from your browser"
                        .into(),
                );
                return;
            }
            self.phase = Phase::Switching;
            self.message = None;
            let cancelled = self.new_attempt();
            let (tx, rx) = channel();
            self.pending = Some(rx);
            std::thread::spawn(move || {
                let result = prepare_saved(&account.id, show_nsfw, &cancelled)
                    .map_err(|error| error.to_string());
                if !cancelled.load(Ordering::Acquire) {
                    let _ = tx.send(Event::Candidate(result));
                }
            });
            return;
        }
        if self.phase != Phase::Choose {
            return;
        }
        let Some(source) = self.sources.get(self.selected).cloned() else {
            return;
        };
        self.phase = Phase::Importing;
        self.message = None;
        let cancelled = self.new_attempt();
        let (tx, rx) = channel();
        self.pending = Some(rx);
        std::thread::spawn(move || {
            let result = prepare(source, show_nsfw, &cancelled).map_err(|e| e.to_string());
            if !cancelled.load(Ordering::Acquire) {
                let _ = tx.send(Event::Candidate(result));
            }
        });
    }

    pub fn request_disconnect(&mut self) {
        if !self.can_remove() {
            return;
        }
        if self.phase == Phase::Accounts {
            let Some(account) = self.accounts.get(self.selected) else {
                return;
            };
            self.removal_id = Some(account.id.clone());
            self.removal_name = Some(account.name.clone());
            self.removing_current = self.active_id.as_ref() == Some(&account.id);
        } else {
            self.removal_id = self.active_id.clone();
            self.removal_name = self.active_name.as_ref().map(|name| format!("u/{name}"));
            self.removing_current = true;
        }
        self.cancel();
        self.message = None;
        self.phase = Phase::DisconnectConfirm;
    }

    pub fn disconnect(&mut self, show_nsfw: bool) {
        let Some(id) = self.removal_id.clone() else {
            return;
        };
        self.disconnect_with(show_nsfw, move |attempt| remove_export_with(&id, attempt));
    }

    pub fn can_remove(&self) -> bool {
        match self.phase {
            Phase::DisconnectConfirm | Phase::Saving | Phase::Disconnecting => false,
            Phase::Accounts => self.selected < self.accounts.len(),
            _ => self.active_id.is_some(),
        }
    }

    pub(super) fn disconnect_with(
        &mut self,
        show_nsfw: bool,
        remove: impl FnOnce(&Attempt) -> Result<()> + Send + 'static,
    ) {
        if self.phase != Phase::DisconnectConfirm {
            return;
        }
        let cancelled = self.new_attempt();
        self.phase = Phase::Disconnecting;
        let (tx, rx) = channel();
        self.pending = Some(rx);
        let disconnect = self.removing_current;
        std::thread::spawn(move || {
            let event = if disconnect {
                let result = Reddit::disconnected(show_nsfw).and_then(|reddit| {
                    remove(&cancelled)?;
                    Ok(reddit)
                });
                Event::Saved(result.map_err(|error| error.to_string()))
            } else {
                Event::Removed(remove(&cancelled).map_err(|error| error.to_string()))
            };
            let _ = tx.send(event);
        });
    }

    pub fn confirm(&mut self) {
        self.confirm_with(commit_with);
    }

    pub(super) fn confirm_with(
        &mut self,
        save: impl FnOnce(Request, &Attempt) -> Result<()> + Send + 'static,
    ) {
        if self.phase != Phase::Confirm {
            return;
        }
        let Some(Candidate {
            reddit,
            source,
            activation,
        }) = self.candidate.take()
        else {
            return;
        };
        self.phase = Phase::Saving;
        let (tx, rx) = channel();
        self.pending = Some(rx);
        let cancelled = Arc::clone(&self.cancelled);
        std::thread::spawn(move || {
            let result = reddit
                .save_connection_with(source, |text| {
                    let request = match activation {
                        Some((id, expected)) => Request::Activate {
                            id,
                            expected,
                            text: text.into(),
                        },
                        None => Request::Store(text.into()),
                    };
                    save(request, &cancelled)
                })
                .map(|()| reddit)
                .map_err(|e| e.to_string());
            let _ = tx.send(Event::Saved(result));
        });
    }

    pub(super) fn fail(&mut self, error: String) {
        self.message = Some(error);
        self.phase = if self.accounts_view {
            Phase::Accounts
        } else {
            Phase::Choose
        };
        self.candidate = None;
        self.pending = None;
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}
