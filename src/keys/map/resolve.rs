use super::*;

/// The key hint of a pending chord: due after `HINT_DELAY`, then shown until it ends.
#[derive(Default, Clone, Copy)]
pub(super) enum Hint {
    #[default]
    Off,
    Due(Instant),
    Shown,
}

#[derive(Default)]
pub struct State {
    pub prefix: String,
    pub scroll: usize,
    pub help_g: bool,
    pub hint_rows: usize,
    pub help_rows: usize,
    pub(super) scope: Option<Scope>,
    pub(super) hint: Hint,
}

impl State {
    pub fn clear(&mut self) {
        self.prefix.clear();
        self.scroll = 0;
        self.scope = None;
        self.help_g = false;
        self.hint_rows = 0;
        self.hint = Hint::Off;
    }

    pub fn hint_visible(&self) -> bool {
        matches!(self.hint, Hint::Shown)
    }

    pub(crate) fn schedule_hint(&mut self, now: Instant) {
        if !self.hint_visible() {
            self.hint = Hint::Due(now);
        }
    }

    pub(crate) fn tick(&mut self, now: Instant) -> bool {
        match self.hint {
            Hint::Due(since) if now.saturating_duration_since(since) >= HINT_DELAY => {
                self.hint = Hint::Shown;
                true
            }
            _ => false,
        }
    }

    pub(crate) fn wake_in(&self, now: Instant) -> Option<Duration> {
        match self.hint {
            Hint::Due(since) => {
                Some(HINT_DELAY.saturating_sub(now.saturating_duration_since(since)))
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
pub enum Target {
    Action(Action),
    Menu(usize),
}

pub struct Candidate {
    pub keys: String,
    pub description: String,
    pub target: Target,
}

pub fn sync(app: &mut App) {
    if !app.keymap.prefix.is_empty() && app.keymap.scope.as_ref() != Some(&scope(app)) {
        app.keymap.clear();
    }
}

pub fn candidates(app: &App) -> Vec<Candidate> {
    let mut result: Vec<_> = BINDINGS
        .iter()
        .filter(|binding| available(app, binding.action))
        .map(|binding| Candidate {
            keys: binding.keys.into(),
            description: description(app, binding).into(),
            target: Target::Action(binding.action),
        })
        .collect();
    if context(app) == Context::Menu
        && let Some(Screen::Menu { choices, .. }) = app.experience.screens.last()
    {
        result.extend(
            choices
                .iter()
                .enumerate()
                .filter(|(_, choice)| !choice.shortcut.is_empty())
                .map(|(index, choice)| Candidate {
                    keys: choice.shortcut.into(),
                    description: choice.label.clone(),
                    target: Target::Menu(index),
                }),
        );
    }
    result
}

pub fn resolve(app: &mut App, key: KeyEvent) -> Option<Target> {
    sync(app);
    let c = match key.code {
        KeyCode::Char(c) if key.modifiers == KeyModifiers::CONTROL && c.is_ascii_lowercase() => {
            char::from_u32(c as u32 - 96)?
        }
        KeyCode::Char(c)
            if !key.modifiers.intersects(
                KeyModifiers::CONTROL
                    | KeyModifiers::ALT
                    | KeyModifiers::SUPER
                    | KeyModifiers::HYPER
                    | KeyModifiers::META,
            ) =>
        {
            c
        }
        _ => {
            app.keymap.clear();
            return None;
        }
    };
    if app.keymap.prefix.is_empty() {
        app.keymap.scope = Some(scope(app));
    }
    app.keymap.prefix.push(c);
    let candidates = candidates(app);
    let exact = candidates
        .iter()
        .find(|candidate| candidate.keys == app.keymap.prefix)
        .map(|candidate| candidate.target);
    if exact.is_some()
        || !candidates
            .iter()
            .any(|candidate| candidate.keys.starts_with(&app.keymap.prefix))
    {
        app.keymap.clear();
    } else {
        app.keymap.schedule_hint(Instant::now());
    }
    exact
}
