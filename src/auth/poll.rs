use super::*;

impl Controller {
    /// Polling never accesses browsers, the network or the credential store.
    pub fn poll(&mut self, show_nsfw: bool) -> (bool, Option<Reddit>) {
        self.poll_with(show_nsfw, commit_with)
    }

    pub(super) fn poll_with(
        &mut self,
        show_nsfw: bool,
        save: impl FnOnce(Request, &Attempt) -> Result<()> + Send + 'static,
    ) -> (bool, Option<Reddit>) {
        let Some(receiver) = &self.pending else {
            return (false, None);
        };
        let event = match receiver.try_recv() {
            Ok(event) => event,
            Err(std::sync::mpsc::TryRecvError::Empty) => return (false, None),
            Err(_) => {
                self.fail("Sign-in stopped unexpectedly; choose a profile to retry".into());
                return (true, None);
            }
        };
        self.pending = None;
        match event {
            Event::Accounts(Ok(accounts)) => {
                self.accounts = accounts;
                self.selected = self
                    .active_id
                    .as_ref()
                    .and_then(|id| self.accounts.iter().position(|account| &account.id == id))
                    .unwrap_or(0);
                self.phase = Phase::Accounts;
                if self.accounts.is_empty() {
                    self.discover();
                }
            }
            Event::Sources(Ok(sources)) => {
                self.sources = sources;
                self.selected = self
                    .preferred
                    .as_ref()
                    .and_then(|p| self.sources.iter().position(|s| s.same_profile(p)))
                    .unwrap_or(0);
                self.phase = Phase::Choose;
                if self.sources.is_empty() {
                    self.message = Some("No supported browsers found. Install Chrome/Chromium or Firefox, or pipe a cookie export to redtab --signin.".into());
                }
                if let Some((browser, profile)) = self.cli_selection.take() {
                    let matches: Vec<_> = self.sources.iter().enumerate().filter_map(|(i,s)| matches!(s, Source::Browser { browser: b, profile: p, label } if b == &browser && profile.as_ref().is_none_or(|q| p == q || label.rsplit(" · ").next() == Some(q.as_str()))).then_some(i)).collect();
                    if matches.len() == 1 {
                        self.selected = matches[0];
                        self.select(show_nsfw);
                    } else {
                        self.message = Some("Select a detected profile; the requested browser/profile was missing or ambiguous.".into());
                    }
                }
            }
            Event::Candidate(Ok(candidate)) => {
                let switching = self.phase == Phase::Switching;
                self.candidate = Some(candidate);
                self.phase = Phase::Confirm;
                if switching {
                    self.confirm_with(save);
                }
            }
            Event::Saved(Ok(reddit)) => {
                self.phase = Phase::Idle;
                return (true, Some(reddit));
            }
            Event::Removed(Ok(())) => {
                self.accounts
                    .retain(|account| Some(&account.id) != self.removal_id.as_ref());
                self.selected = self.selected.min(self.accounts.len());
                self.phase = Phase::Accounts;
                self.message = None;
            }
            Event::Accounts(Err(error))
            | Event::Sources(Err(error))
            | Event::Candidate(Err(error))
            | Event::Saved(Err(error))
            | Event::Removed(Err(error)) => self.fail(error),
        }
        (true, None)
    }
}
