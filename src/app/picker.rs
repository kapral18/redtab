use super::*;

#[derive(Debug, Default)]
pub struct PickerState {
    pub input: String,
    pub selected: usize,
}

pub enum Overlay {
    Settings,
    Experience,
    Auth,
    Membership {
        target: MembershipTarget,
        join: bool,
    },
    Picker(PickerState),
    Sort {
        selected: usize,
    },
    Search {
        input: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerEntry {
    Tab { index: usize, title: String },
    Open(String),
    OpenUser(String),
}

impl App {
    /// Picker rows: tabs starting with the input, then "open r/<input>", then tabs
    /// containing it, then fuzzy matches. Row 0 is the default, so a new name isn't
    /// hijacked by a longer subscription containing it.
    pub fn picker_entries(&self, input: &str) -> Vec<PickerEntry> {
        use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
        use nucleo_matcher::{Config as MatcherConfig, Matcher};
        let tab_entry = |(index, t): (usize, &Tab)| PickerEntry::Tab {
            index,
            title: t.title(),
        };
        let raw = input.trim().trim_start_matches('/');
        let user_prefix = raw
            .get(..2)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("u/"));
        let sub_prefix = raw
            .get(..2)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("r/"));
        let query = if user_prefix {
            raw[2..].trim_end_matches('/')
        } else {
            normalize_subreddit(input)
        };
        let eligible = |tab: &Tab| {
            (!user_prefix || matches!(tab.kind, TabKind::User { .. }))
                && (!sub_prefix || matches!(tab.kind, TabKind::Subreddit { .. }))
        };
        if query.is_empty() {
            return self.tabs.iter().enumerate().map(tab_entry).collect();
        }
        let lower = query.to_lowercase();
        let name_of = |t: &Tab| {
            t.title()
                .trim_start_matches("r/")
                .trim_start_matches("u/")
                .to_lowercase()
        };
        let mut prefix: Vec<(usize, &Tab)> = Vec::new();
        let mut inside: Vec<(usize, &Tab)> = Vec::new();
        for (i, t) in self
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, tab)| eligible(tab))
        {
            let name = name_of(t);
            if name.starts_with(&lower) {
                prefix.push((i, t));
            } else if name.contains(&lower) {
                inside.push((i, t));
            }
        }
        prefix.sort_by_key(|(_, t)| name_of(t) != lower);
        let mut entries: Vec<PickerEntry> = prefix.into_iter().map(tab_entry).collect();
        // Only an exact subreddit tab makes "open r/<query>" redundant; Home and Bookmarks
        // aren't subreddits.
        let exact = self.tabs.iter().any(|t| {
            (if user_prefix {
                matches!(t.kind, TabKind::User { .. })
            } else {
                matches!(t.kind, TabKind::Subreddit { .. })
            }) && name_of(t) == lower
        });
        if !exact && user_prefix && crate::model::is_username(query) {
            entries.push(PickerEntry::OpenUser(query.to_string()));
        }
        if !exact && !user_prefix && is_subreddit_name(query) {
            entries.push(PickerEntry::Open(query.to_string()));
        }
        entries.extend(inside.into_iter().map(tab_entry));
        let mut matcher = Matcher::new(MatcherConfig::DEFAULT);
        let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
        let titles: Vec<String> = self.tabs.iter().map(Tab::title).collect();
        for (title, _) in pattern.match_list(titles.iter().map(String::as_str), &mut matcher) {
            if let Some(index) = titles.iter().position(|t| t == title)
                && eligible(&self.tabs[index])
                && !entries
                    .iter()
                    .any(|e| matches!(e, PickerEntry::Tab { index: i, .. } if *i == index))
            {
                entries.push(PickerEntry::Tab {
                    index,
                    title: title.to_string(),
                });
            }
        }
        entries
    }

    pub fn picker_accept(&mut self, state: &PickerState) {
        match self
            .picker_entries(&state.input)
            .into_iter()
            .nth(state.selected)
        {
            Some(PickerEntry::Tab { index, .. }) => self.switch_tab(index),
            Some(PickerEntry::Open(name)) => self.open_subreddit(&name),
            Some(PickerEntry::OpenUser(name)) => self.open_user(&name),
            None => {}
        }
    }
}
