use super::*;

impl App {
    /// The settings F2 lists, keeping the selection on one of them.
    pub fn settings_rows(&mut self) -> Vec<usize> {
        let rows: Vec<usize> = (0..SETTINGS.len())
            .filter(|&index| SETTINGS[index].available(&self.settings.store.current))
            .collect();
        if !rows.contains(&self.settings.selected) {
            self.settings.selected = rows
                .iter()
                .copied()
                .find(|&index| index > self.settings.selected)
                .or_else(|| rows.last().copied())
                .unwrap_or(0);
        }
        rows
    }

    pub fn settings_key(&mut self, key: KeyEvent) {
        if self.settings.managed.is_some() {
            self.managed_save_key(key);
            return;
        }
        if self.settings.editing.is_some() {
            self.settings_edit_key(key);
            return;
        }
        if !key.modifiers.difference(KeyModifiers::SHIFT).is_empty() {
            return;
        }
        let rows = self.settings_rows();
        let position = rows
            .iter()
            .position(|&index| index == self.settings.selected)
            .unwrap_or(0);
        match key.code {
            KeyCode::Esc | KeyCode::F(2) | KeyCode::Char('q') => {
                self.overlay = self.settings.return_overlay.take()
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.settings.selected = rows[(position + 1).min(rows.len() - 1)]
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.settings.selected = rows[position.saturating_sub(1)]
            }
            KeyCode::Home => self.settings.selected = rows[0],
            KeyCode::End | KeyCode::Char('G') => self.settings.selected = rows[rows.len() - 1],
            KeyCode::Enter => {
                let field = &SETTINGS[self.settings.selected];
                let value = field.value(&self.config);
                if field.kind == Kind::Boolean {
                    self.change_setting(field.key, toml::Value::Boolean(!value.as_bool().unwrap()));
                } else {
                    let text = value.to_string();
                    self.settings.editing = Some(Field {
                        label: field.key.into(),
                        cursor: text.len(),
                        value: text,
                        multiline: false,
                        required: true,
                    });
                    self.settings.edit_base = Some(value);
                    self.settings.message = None;
                }
            }
            KeyCode::Char('r') if self.settings_allowed() => {
                if SETTINGS[self.settings.selected].scope == Scope::Reddit {
                    self.load_account_preferences(true);
                    return;
                }
                self.settings.store.save();
                self.apply_settings();
                self.settings.message = None;
                self.begin_managed_save();
            }
            KeyCode::Char('u') if self.settings_allowed() => {
                if SETTINGS[self.settings.selected].scope == Scope::Reddit {
                    self.settings.message = Some(
                        self.preference_access_message()
                            .unwrap_or("Reddit saves immediately. Toggle the preference to change it; r reloads account values.")
                            .into(),
                    );
                    return;
                }
                self.settings.store.revert();
                self.apply_settings();
            }
            _ => {}
        }
    }

    pub(super) fn managed_save_key(&mut self, key: KeyEvent) {
        if self.settings.managed_busy() || !key.modifiers.is_empty() {
            return;
        }
        match key.code {
            KeyCode::Enter | KeyCode::Char('y') => {
                if let Some(ManagedSave::Confirm(plan)) = self.settings.managed.take() {
                    let verification = plan.verification();
                    self.settings.managed = Some(ManagedSave::Applying(
                        Task::start(move || (*plan).apply()),
                        verification,
                    ));
                    self.settings.message = None;
                }
            }
            KeyCode::Esc | KeyCode::F(2) | KeyCode::Char('n' | 'q') => {
                self.settings.managed = None;
                self.settings.message = Some("Changes kept for this session.".into());
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.settings.managed_scroll = self.settings.managed_scroll.saturating_add(1)
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.settings.managed_scroll = self.settings.managed_scroll.saturating_sub(1)
            }
            KeyCode::PageDown => {
                self.settings.managed_scroll = self.settings.managed_scroll.saturating_add(8)
            }
            KeyCode::PageUp => {
                self.settings.managed_scroll = self.settings.managed_scroll.saturating_sub(8)
            }
            KeyCode::Home => self.settings.managed_scroll = 0,
            KeyCode::End => self.settings.managed_scroll = usize::MAX,
            _ => {}
        }
    }

    pub(super) fn settings_edit_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if key.code == KeyCode::Esc {
            self.settings.editing = None;
            self.settings.message = None;
            return;
        }
        if key.code == KeyCode::Enter || (ctrl && key.code == KeyCode::Char('s')) {
            let field = &SETTINGS[self.settings.selected];
            let input = &self.settings.editing.as_ref().unwrap().value;
            let parsed = toml::from_str::<toml::Table>(&format!("value = {input}"))
                .ok()
                .and_then(|mut t| {
                    if t.len() == 1 {
                        t.remove("value")
                    } else {
                        None
                    }
                });
            let Some(value) = parsed else {
                self.settings.message = Some(format!("Expected {}", field.input_hint()));
                return;
            };
            if self.settings.edit_base.as_ref() != Some(&field.value(&self.config))
                && value != field.value(&self.config)
            {
                self.settings.message = Some("This value changed on disk while editing. Esc to review it; your draft has not been applied.".into());
                return;
            }
            self.change_setting(field.key, value);
            if self.settings.message.is_none() {
                self.settings.editing = None;
            }
            return;
        }
        let input = self.settings.editing.as_mut().unwrap();
        match key.code {
            KeyCode::Char('u') if ctrl => {
                input.value.clear();
                input.cursor = 0;
            }
            KeyCode::Left => input.cursor = input.previous(),
            KeyCode::Right => input.cursor = input.next(),
            KeyCode::Home => input.cursor = 0,
            KeyCode::End => input.cursor = input.value.len(),
            KeyCode::Backspace if !key.modifiers.contains(KeyModifiers::ALT) => {
                let start = input.previous();
                input.value.replace_range(start..input.cursor, "");
                input.cursor = start;
            }
            KeyCode::Delete => {
                let end = input.next();
                input.value.replace_range(input.cursor..end, "");
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::CONTROL) =>
            {
                input.insert(&c.to_string())
            }
            _ => {}
        }
    }

    pub fn settings_paste(&mut self, text: &str) {
        if let Some(input) = &mut self.settings.editing {
            input.insert(&text.chars().filter(|c| !c.is_control()).collect::<String>());
        }
    }
}
