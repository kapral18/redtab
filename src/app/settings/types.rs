use super::*;

pub struct Task<T> {
    pub(super) receiver: Receiver<Result<T, String>>,
    pub(super) thread: Option<std::thread::JoinHandle<()>>,
    pub(super) join_on_drop: bool,
}

impl<T: Send + 'static> Task<T> {
    pub(super) fn start(work: impl FnOnce() -> anyhow::Result<T> + Send + 'static) -> Self {
        let (sender, receiver) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let _ = sender.send(work().map_err(|error| format!("{error:#}")));
        });
        Self {
            receiver,
            thread: Some(thread),
            join_on_drop: true,
        }
    }

    pub(super) fn poll(&self) -> Option<Result<T, String>> {
        match self.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                Some(Err("Settings task stopped; retry saving".into()))
            }
        }
    }
}

impl<T> Drop for Task<T> {
    fn drop(&mut self) {
        // Commands are bounded; shutdown must finish an approved source/apply transaction.
        if let Some(thread) = self.thread.take().filter(|_| self.join_on_drop) {
            let _ = thread.join();
        }
    }
}

pub enum ManagedSave {
    Checking(Task<Option<Plan>>),
    Confirm(Box<Plan>),
    Applying(Task<()>, Verification),
}

#[derive(Default)]
pub struct AccountSettings {
    pub values: Option<crate::reddit::Preferences>,
    pub(super) task: Option<Task<crate::reddit::Preferences>>,
    pub(super) owner: Option<String>,
    pub writing: bool,
    pub error: Option<String>,
    pub loaded: bool,
}

pub struct Settings {
    pub store: Store,
    pub selected: usize,
    pub editing: Option<Field>,
    pub(super) edit_base: Option<toml::Value>,
    pub(super) return_overlay: Option<Overlay>,
    pub message: Option<String>,
    pub managed: Option<ManagedSave>,
    pub managed_scroll: usize,
    pub account: AccountSettings,
    pub(crate) chezmoi: Chezmoi,
    pub(super) last_poll: Instant,
}

impl AccountSettings {
    pub fn pending(&self) -> bool {
        self.task.is_some()
    }
}

impl Settings {
    pub fn new(store: Store) -> Self {
        Self {
            store,
            selected: 0,
            editing: None,
            edit_base: None,
            return_overlay: None,
            message: None,
            managed: None,
            managed_scroll: 0,
            account: AccountSettings::default(),
            chezmoi: Chezmoi::default(),
            last_poll: Instant::now(),
        }
    }

    pub fn busy(&self) -> bool {
        self.managed_busy() || self.account.task.is_some()
    }

    pub fn managed_busy(&self) -> bool {
        matches!(
            self.managed,
            Some(ManagedSave::Checking(_) | ManagedSave::Applying(..))
        )
    }

    pub fn writing(&self) -> bool {
        self.account.writing
    }

    pub fn scope_label(&self, scope: Scope, account: Option<&crate::reddit::Account>) -> String {
        let name = account.map_or_else(
            || {
                if self.store.current.active_account.as_deref() == Some("") {
                    "Unidentified account".into()
                } else {
                    "Signed out".into()
                }
            },
            |a| format!("u/{}", a.name),
        );
        match scope {
            Scope::Reddit => format!("Reddit settings · {name}"),
            Scope::Reader => format!("Reader settings · {name}"),
            Scope::Device => "This device · all accounts".into(),
        }
    }

    #[cfg(test)]
    pub fn wait_managed(&mut self) {
        let thread = match &mut self.managed {
            Some(ManagedSave::Checking(task)) => task.thread.take(),
            Some(ManagedSave::Applying(task, _)) => task.thread.take(),
            _ => None,
        };
        if let Some(thread) = thread {
            thread.join().unwrap();
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PreferenceAccess {
    SignIn,
    Reconnect,
    Readable,
}
