//! Persisted settings and their resolved account scope.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use super::{Commands, Config, Filters, Highlight, MediaSettings};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ReaderSettings {
    pub filters: Filters,
    pub highlight: Highlight,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct Document {
    all_subs_posts_per_group: usize,
    media: Media,
    commands: Commands,
    accounts: BTreeMap<String, ReaderSettings>,
    signed_out: ReaderSettings,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Media {
    image_preview: bool,
}

impl Default for Media {
    fn default() -> Self {
        Self {
            image_preview: true,
        }
    }
}

impl Default for Document {
    fn default() -> Self {
        Self::from(Config::default())
    }
}

impl From<Config> for Document {
    fn from(c: Config) -> Self {
        Self {
            all_subs_posts_per_group: c.all_subs_posts_per_group,
            media: Media {
                image_preview: c.media.image_preview,
            },
            commands: c.commands,
            accounts: c.accounts,
            signed_out: c.signed_out,
        }
    }
}

impl From<Document> for Config {
    fn from(d: Document) -> Self {
        let mut config = Self {
            all_subs_posts_per_group: d.all_subs_posts_per_group,
            media: MediaSettings {
                image_preview: d.media.image_preview,
                ..MediaSettings::default()
            },
            commands: d.commands,
            accounts: d.accounts,
            signed_out: d.signed_out,
            ..Self::default()
        };
        config.select_account(None);
        config
    }
}

impl Config {
    pub fn select_account(&mut self, username: Option<&str>) {
        self.active_account = username.map(|name| {
            self.accounts
                .keys()
                .find(|key| key.eq_ignore_ascii_case(name))
                .map_or_else(|| name.to_owned(), Clone::clone)
        });
        let reader = match &self.active_account {
            Some(id) => self.accounts.get(id).cloned().unwrap_or_default(),
            None => self.signed_out.clone(),
        };
        self.filters = reader.filters;
        self.highlight = reader.highlight;
    }

    pub(super) fn store_reader(&mut self) {
        let reader = ReaderSettings {
            filters: self.filters.clone(),
            highlight: self.highlight.clone(),
        };
        match &self.active_account {
            Some(id) => {
                self.accounts.insert(id.clone(), reader);
            }
            None => self.signed_out = reader,
        }
    }

    pub(super) fn validate_accounts(&self) -> Result<()> {
        let mut usernames = BTreeSet::new();
        for name in self.accounts.keys() {
            if !crate::model::is_username(name) {
                bail!("accounts: use Reddit usernames without u/ (letters, digits, _ and -)");
            }
            if !usernames.insert(name.to_ascii_lowercase()) {
                bail!("accounts: duplicate username {name}; account names ignore case");
            }
        }
        Ok(())
    }

    /// Earlier snapshots choose table spelling when merging the same username.
    pub(crate) fn align_account_names(configs: &mut [&mut Self]) {
        let mut names = BTreeMap::new();
        for config in configs.iter() {
            for name in config.accounts.keys() {
                names
                    .entry(name.to_ascii_lowercase())
                    .or_insert_with(|| name.clone());
            }
        }
        for config in configs {
            config.accounts = std::mem::take(&mut config.accounts)
                .into_iter()
                .map(|(name, reader)| (names[&name.to_ascii_lowercase()].clone(), reader))
                .collect();
            if let Some(name) = &mut config.active_account
                && let Some(spelling) = names.get(&name.to_ascii_lowercase())
            {
                *name = spelling.clone();
            }
        }
    }

    /// Absent account tables have empty reader rules, just like explicit empty arrays.
    pub(crate) fn same_entry(
        key: &str,
        left: Option<&toml::Value>,
        right: Option<&toml::Value>,
    ) -> bool {
        left == right
            || (key.starts_with("accounts.")
                && left
                    .into_iter()
                    .chain(right)
                    .all(|value| value.as_array().is_some_and(Vec::is_empty)))
    }

    pub(crate) fn entries(&self) -> BTreeMap<String, toml::Value> {
        fn flatten(value: toml::Value, prefix: &str, result: &mut BTreeMap<String, toml::Value>) {
            if let toml::Value::Table(table) = value {
                for (key, value) in table {
                    let key = if prefix.is_empty() {
                        key
                    } else {
                        format!("{prefix}.{key}")
                    };
                    flatten(value, &key, result);
                }
            } else {
                result.insert(prefix.to_owned(), value);
            }
        }
        let mut result = BTreeMap::new();
        flatten(
            toml::Value::try_from(self).expect("config serializes"),
            "",
            &mut result,
        );
        result
    }

    pub(crate) fn with_entries(
        &self,
        updates: impl IntoIterator<Item = (String, Option<toml::Value>)>,
    ) -> Result<Self> {
        fn update(table: &mut toml::Table, parts: &[&str], value: Option<toml::Value>) {
            let key = parts[0];
            if parts.len() == 1 {
                if let Some(value) = value {
                    table.insert(key.into(), value);
                } else {
                    table.remove(key);
                }
                return;
            }
            if value.is_none() && !table.contains_key(key) {
                return;
            }
            let child = table
                .entry(key)
                .or_insert_with(|| toml::Value::Table(Default::default()));
            let child = child.as_table_mut().expect("known config table");
            update(child, &parts[1..], value);
            if child.is_empty() {
                table.remove(key);
            }
        }
        let mut document = toml::Value::try_from(self)?;
        for (key, value) in updates {
            update(
                document.as_table_mut().unwrap(),
                &key.split('.').collect::<Vec<_>>(),
                value,
            );
        }
        let mut next: Config = document.try_into()?;
        next.path = self.path.clone();
        next.select_account(self.active_account.as_deref());
        next.show_nsfw = self.show_nsfw;
        next.media.video_autoplay = self.media.video_autoplay;
        next.validate()?;
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn when_switching_between_accounts_and_signed_out_rules_should_be_isolated() {
        let mut config:Config=toml::from_str("[accounts.first.filters]\nkeywords=[\"first\"]\n[accounts.second.filters]\nauthors=[\"bot\"]\n[signed_out.highlight]\nkeywords=[\"offline\"]\n").unwrap();
        config.select_account(Some("first"));
        assert_eq!(config.filters.keywords, ["first"]);
        config.select_account(Some("second"));
        assert!(config.filters.keywords.is_empty());
        assert_eq!(config.filters.authors, ["bot"]);
        config.select_account(None);
        assert!(config.filters.authors.is_empty());
        assert_eq!(config.highlight.keywords, ["offline"]);
        config.select_account(Some("first"));
        assert!(config.highlight.keywords.is_empty());
        assert_eq!(config.filters.keywords, ["first"]);
    }
    #[test]
    fn when_username_case_differs_rules_should_follow_the_existing_table() {
        let mut config: Config =
            toml::from_str("[accounts.TFordragon.filters]\nkeywords=[\"mine\"]\n").unwrap();
        config.select_account(Some("tfordragon"));
        assert_eq!(config.filters.keywords, ["mine"]);
        assert_eq!(config.active_account.as_deref(), Some("TFordragon"));
    }

    #[test]
    fn when_username_contains_supported_punctuation_config_should_load() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(
            &path,
            "[accounts.Reader_One-Two.filters]\nkeywords=[\"mine\"]\n",
        )
        .unwrap();
        let mut config = Config::load(Some(&path)).unwrap();
        config.select_account(Some("reader_one-two"));
        assert_eq!(config.filters.keywords, ["mine"]);
    }

    #[test]
    fn when_username_tables_differ_only_by_case_config_should_reject_them() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(
            &path,
            "[accounts.Reader.filters]\nkeywords=[]\n[accounts.reader.filters]\nkeywords=[]\n",
        )
        .unwrap();
        assert!(Config::load(Some(&path)).is_err());
    }

    #[test]
    fn when_account_name_is_not_a_username_config_load_should_reject_it() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(&path, "[accounts.\"../other\".filters]\nkeywords=[]\n").unwrap();
        assert!(Config::load(Some(&path)).is_err());
    }
}
