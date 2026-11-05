use super::*;

impl App {
    pub fn experience_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        if ctrl && key.code == KeyCode::Char('o') {
            self.experience_native();
            return;
        }
        if self.experience.writing() {
            return;
        }
        if let Some(Screen::Input(_)) = self.experience.screens.last() {
            if key.code == KeyCode::Esc {
                self.experience_back();
                return;
            }
            if (ctrl && key.code == KeyCode::Char('s')) || (alt && key.code == KeyCode::Enter) {
                self.submit_experience_form();
                return;
            }
            let form = self.experience.input().unwrap();
            let field_count = form.fields.len();
            let current = &mut form.fields[form.active];
            match key.code {
                KeyCode::Tab => form.active = (form.active + 1) % field_count,
                KeyCode::BackTab => form.active = (form.active + field_count - 1) % field_count,
                KeyCode::Enter if current.multiline => current.insert("\n"),
                KeyCode::Enter if form.active + 1 < field_count => form.active += 1,
                KeyCode::Enter => self.submit_experience_form(),
                KeyCode::Backspace if alt => current.delete_word(),
                KeyCode::Backspace => {
                    let start = current.previous();
                    current.value.replace_range(start..current.cursor, "");
                    current.cursor = start;
                }
                KeyCode::Delete => {
                    let end = current.next();
                    current.value.replace_range(current.cursor..end, "");
                }
                KeyCode::Left => current.cursor = current.previous(),
                KeyCode::Right => current.cursor = current.next(),
                KeyCode::Home | KeyCode::Char('a')
                    if !matches!(key.code, KeyCode::Char(_)) || ctrl =>
                {
                    current.cursor = current.value[..current.cursor]
                        .rfind('\n')
                        .map_or(0, |i| i + 1)
                }
                KeyCode::End | KeyCode::Char('e')
                    if !matches!(key.code, KeyCode::Char(_)) || ctrl =>
                {
                    current.cursor = current.value[current.cursor..]
                        .find('\n')
                        .map_or(current.value.len(), |i| current.cursor + i)
                }
                KeyCode::Char('u') if ctrl => {
                    current.value.clear();
                    current.cursor = 0;
                }
                KeyCode::Char(c) if !ctrl && !alt && !c.is_control() => {
                    current.insert(&c.to_string())
                }
                _ => {}
            }
            return;
        }
        if !(key.modifiers.difference(KeyModifiers::SHIFT).is_empty()
            || key.modifiers == KeyModifiers::CONTROL
                && matches!(key.code, KeyCode::Char('d' | 'u' | 'e' | 'y')))
        {
            return;
        }
        if matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) {
            self.experience_back();
            return;
        }
        let reading_navigation = matches!(
            key.code,
            KeyCode::Up
                | KeyCode::Down
                | KeyCode::Home
                | KeyCode::End
                | KeyCode::PageUp
                | KeyCode::PageDown
                | KeyCode::Char('j' | 'k' | 'G')
        ) || (ctrl
            && matches!(key.code, KeyCode::Char('d' | 'u' | 'e' | 'y')));
        if self.experience.pending.is_some()
            && !self.experience.refreshing
            && !(self.experience.loading_more() && reading_navigation)
        {
            return;
        }
        if reading_navigation
            && let Some(Screen::Browse { query, .. }) = self.experience.screens.last()
        {
            self.experience.prefetch = Some((query.clone(), Instant::now() + NAV_SETTLE));
        }
        if ctrl && matches!(key.code, KeyCode::Char('d' | 'u' | 'e' | 'y')) {
            if let Some(Screen::Browse { scroll, .. }) = self.experience.screens.last_mut() {
                let rows = if matches!(key.code, KeyCode::Char('d' | 'u')) {
                    5
                } else {
                    1
                };
                *scroll = if matches!(key.code, KeyCode::Char('d' | 'e')) {
                    scroll.saturating_add(rows)
                } else {
                    scroll.saturating_sub(rows)
                };
            }
            return;
        }
        let mut command = None;
        let mut read_next = None;
        match self.experience.screens.last_mut() {
            Some(Screen::Menu {
                choices, selected, ..
            }) => match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    *selected = (*selected + 1).min(choices.len().saturating_sub(1))
                }
                KeyCode::Char('k') | KeyCode::Up => *selected = selected.saturating_sub(1),
                KeyCode::Home => *selected = 0,
                KeyCode::Char('G') | KeyCode::End => *selected = choices.len().saturating_sub(1),
                KeyCode::Enter | KeyCode::Right => {
                    command = choices.get(*selected).map(|choice| choice.command.clone())
                }
                _ => {}
            },
            Some(Screen::Confirm { action, .. }) if key.code == KeyCode::Enter => {
                command = Some(Command::Write(action.clone()))
            }
            Some(Screen::Loading(query)) if key.code == KeyCode::Char('r') => {
                read_next = Some(query.clone());
            }
            Some(Screen::Browse {
                query,
                page,
                selected,
                scroll,
                ..
            }) => match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    let next = (*selected + 1).min(page.entries.len().saturating_sub(1));
                    if next != *selected {
                        *selected = next;
                        *scroll = 0;
                    } else if page.entries.is_empty() {
                        *scroll = scroll.saturating_add(1);
                    }
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    if *selected > 0 {
                        *selected -= 1;
                        *scroll = 0;
                    } else if page.entries.is_empty() {
                        *scroll = scroll.saturating_sub(1);
                    }
                }
                KeyCode::PageDown => *scroll = scroll.saturating_add(10),
                KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
                KeyCode::Home => {
                    *selected = 0;
                    *scroll = 0;
                }
                KeyCode::Char('G') | KeyCode::End => {
                    let next = page.entries.len().saturating_sub(1);
                    if page.entries.is_empty() {
                        *scroll = usize::MAX;
                    } else if next != *selected {
                        *selected = next;
                        *scroll = 0;
                    }
                }
                KeyCode::Enter => {
                    command = page.entries.get(*selected).and_then(|entry| {
                        entry
                            .query
                            .clone()
                            .map(|query| match query {
                                Query::Community(sub) => Command::Subreddit(sub),
                                Query::Profile(user) => Command::User(user),
                                query => Command::Read(query),
                            })
                            .or_else(|| {
                                entry
                                    .target
                                    .clone()
                                    .map(|target| Command::Target(target, entry.post.clone()))
                            })
                    })
                }
                KeyCode::Char('a' | 'd' | 'D') if matches!(query, Query::Multi { .. }) => {
                    self.cancel_collection_refresh();
                    self.multi_action(key.code);
                    return;
                }
                _ => {}
            },
            _ => {}
        }
        if let Some(query) = read_next {
            self.experience.screens.pop();
            self.read_experience(query, None);
            // Retrying a failed re-read after a write still restores its selection.
            if let Some(kept) = &mut self.experience.kept
                && let Some((read, _)) = &self.experience.pending
            {
                kept.0 = *read;
            }
        }
        if let Some(command) = command {
            self.cancel_collection_refresh();
            self.execute_experience(command);
        }
    }

    pub(super) fn submit_experience_form(&mut self) {
        if self.experience.pending.is_some() {
            return;
        }
        let Some(Screen::Input(form)) = self.experience.screens.last().cloned() else {
            return;
        };
        if let Some(index) = form
            .fields
            .iter()
            .position(|field| field.required && field.value.trim().is_empty())
        {
            self.experience.error = Some(format!("{} is required", form.fields[index].label));
            if let Some(current) = self.experience.input() {
                current.active = index;
            }
            return;
        }
        let v = |index: usize| form.fields[index].value.clone();
        let action = match form.kind {
            FormKind::OpenLink => {
                match link_command(&v(0)) {
                    Ok(command) => self.execute_experience(command),
                    Err(error) => self.experience.error = Some(error.to_string()),
                }
                return;
            }
            FormKind::Profile => {
                let user = v(0).trim().trim_start_matches("u/").to_string();
                if let Err(error) = name(&user) {
                    self.experience.error = Some(error.to_string());
                    return;
                }
                // The feed tab leads with the profile summary, so `gu` lands on posts and profile.
                self.clear_experience();
                self.overlay = None;
                // Already on that tab, a focused post would hide the feed `gu` asked for.
                self.close_post();
                self.focus_main();
                self.open_user(&user);
                return;
            }
            FormKind::Reply(target) => Action::Reply {
                parent: target.id,
                text: v(0),
            },
            FormKind::Edit(target) => Action::Edit { target, text: v(0) },
            FormKind::Report(target) => Action::Report {
                id: target.id,
                reason: v(0),
            },
            FormKind::Submit { link } => Action::Submit {
                subreddit: normalize_subreddit(&v(0)).into(),
                title: v(1),
                text: v(2),
                link,
            },
            FormKind::CreateMulti => Action::CreateMulti {
                name: v(0),
                description: v(1),
            },
            FormKind::MultiCommunity { owner, name, add } => Action::MultiCommunity {
                owner,
                name,
                subreddit: normalize_subreddit(&v(0)).into(),
                add,
            },
            FormKind::Wiki(subreddit) => {
                self.read_experience(
                    Query::Wiki {
                        subreddit,
                        page: v(0),
                    },
                    None,
                );
                return;
            }
            FormKind::Find => {
                self.find_comments(&v(0));
                return;
            }
        };
        self.write_experience(action);
    }

    pub fn experience_paste(&mut self, text: &str) {
        if self.experience.writing() {
            return;
        }
        if let Some(form) = self.experience.input() {
            let field = &mut form.fields[form.active];
            for c in text.chars() {
                if !c.is_control() || (field.multiline && matches!(c, '\n' | '\t')) {
                    field.insert(&c.to_string());
                } else if matches!(c, '\n' | '\r' | '\t') {
                    field.insert(" ");
                }
            }
        }
    }
}
