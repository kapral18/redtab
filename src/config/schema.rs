//! Shared field metadata for the file template and settings screen.
use super::Config;
use anyhow::{Result, bail};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Boolean,
    Count,
    Strings,
    Command,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Reddit,
    Reader,
    Device,
}

pub struct Setting {
    pub key: &'static str,
    pub scope: Scope,
    pub label: &'static str,
    pub description: &'static str,
    pub kind: Kind,
}

pub const SETTINGS: &[Setting] = &[
    Setting {
        key: "show_nsfw",
        scope: Scope::Reddit,
        label: "Show NSFW",
        description: "Reddit mature-content setting for feeds and search. Saves to Reddit and reloads content.",
        kind: Kind::Boolean,
    },
    Setting {
        key: "media.video_autoplay",
        scope: Scope::Reddit,
        label: "Autoplay videos",
        description: "Use Reddit account autoplay for selected videos and GIFs. Saves to Reddit; p still plays manually.",
        kind: Kind::Boolean,
    },
    Setting {
        key: "filters.keywords",
        scope: Scope::Reader,
        label: "Hide title keywords",
        description: "Hide titles containing these phrases, ignoring case. Use a TOML string array, e.g. [\"spoiler\"].",
        kind: Kind::Strings,
    },
    Setting {
        key: "filters.flairs",
        scope: Scope::Reader,
        label: "Hide flairs",
        description: "Hide exact flair names, ignoring case. Use a TOML string array.",
        kind: Kind::Strings,
    },
    Setting {
        key: "filters.authors",
        scope: Scope::Reader,
        label: "Hide authors",
        description: "Hide exact usernames without u/, ignoring case. Use a TOML string array.",
        kind: Kind::Strings,
    },
    Setting {
        key: "highlight.keywords",
        scope: Scope::Reader,
        label: "Highlight keywords",
        description: "Emphasize titles containing these phrases, ignoring case. Use a TOML string array.",
        kind: Kind::Strings,
    },
    Setting {
        key: "media.image_preview",
        scope: Scope::Device,
        label: "Image previews",
        description: "Show inline images, card previews, thumbnails and community icons.",
        kind: Kind::Boolean,
    },
    Setting {
        key: "all_subs_posts_per_group",
        scope: Scope::Device,
        label: "Posts per community",
        description: "Number of posts in each All subs island (1–10).",
        kind: Kind::Count,
    },
    Setting {
        key: "commands.browser",
        scope: Scope::Device,
        label: "Browser command",
        description: "Argument array for opening URLs; URL appended. No shell expansion. Example: [\"open\"].",
        kind: Kind::Command,
    },
    Setting {
        key: "commands.video",
        scope: Scope::Device,
        label: "Video command",
        description: "Argument array for the external video player; stream URL appended. Example: [\"mpv\", \"--force-window=immediate\"].",
        kind: Kind::Command,
    },
    Setting {
        key: "commands.image",
        scope: Scope::Device,
        label: "Image command",
        description: "Argument array for opening local images; file paths appended. No shell expansion.",
        kind: Kind::Command,
    },
];

impl Setting {
    /// Reader rules belong to an identified account; without one F2 does not list them.
    pub fn available(&self, config: &Config) -> bool {
        self.scope != Scope::Reader
            || config
                .active_account
                .as_deref()
                .is_some_and(|name| !name.is_empty())
    }

    pub fn file_key(&self, config: &Config) -> String {
        match self.scope {
            Scope::Device => return self.key.into(),
            Scope::Reddit => return "Reddit account".into(),
            Scope::Reader => {}
        }
        let name = config.active_account.as_deref().unwrap_or_default();
        format!("accounts.{name}.{}", self.key)
    }
    pub fn value(&self, config: &Config) -> toml::Value {
        let strings =
            |v: &[String]| toml::Value::Array(v.iter().cloned().map(toml::Value::String).collect());
        match self.key {
            "show_nsfw" => config.show_nsfw.into(),
            "media.video_autoplay" => config.media.video_autoplay.into(),
            "media.image_preview" => config.media.image_preview.into(),
            "all_subs_posts_per_group" => (config.all_subs_posts_per_group as i64).into(),
            "filters.keywords" => strings(&config.filters.keywords),
            "filters.flairs" => strings(&config.filters.flairs),
            "filters.authors" => strings(&config.filters.authors),
            "highlight.keywords" => strings(&config.highlight.keywords),
            "commands.browser" => strings(&config.commands.browser),
            "commands.video" => strings(&config.commands.video),
            "commands.image" => strings(&config.commands.image),
            _ => unreachable!("known setting"),
        }
    }

    pub(super) fn validate_value(&self, value: &toml::Value) -> Result<()> {
        let valid = match self.kind {
            Kind::Boolean => value.is_bool(),
            Kind::Count => value.as_integer().is_some_and(|v| (1..=10).contains(&v)),
            Kind::Strings => value
                .as_array()
                .is_some_and(|v| v.iter().all(toml::Value::is_str)),
            Kind::Command => value.as_array().is_some_and(|v| {
                v.iter().all(toml::Value::is_str)
                    && v.first()
                        .and_then(toml::Value::as_str)
                        .is_some_and(|executable| !executable.trim().is_empty())
            }),
        };
        if !valid {
            bail!("{}: expected {}", self.key, self.input_hint());
        }
        Ok(())
    }

    pub fn set(&self, config: &Config, value: toml::Value) -> Result<Config> {
        if self.scope == Scope::Reader {
            match config.active_account.as_deref() {
                None => bail!("Sign in with SPC p a before editing reader settings"),
                Some("") => bail!(
                    "Reconnect with SPC p a to identify this account before editing reader settings"
                ),
                Some(_) => {}
            }
        }
        self.validate_value(&value)?;
        let mut next = config.clone();
        let strings = || {
            value
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_owned())
                .collect()
        };
        match self.key {
            "show_nsfw" => next.show_nsfw = value.as_bool().unwrap(),
            "media.video_autoplay" => next.media.video_autoplay = value.as_bool().unwrap(),
            "media.image_preview" => next.media.image_preview = value.as_bool().unwrap(),
            "all_subs_posts_per_group" => {
                next.all_subs_posts_per_group = value.as_integer().unwrap() as usize
            }
            "filters.keywords" => next.filters.keywords = strings(),
            "filters.flairs" => next.filters.flairs = strings(),
            "filters.authors" => next.filters.authors = strings(),
            "highlight.keywords" => next.highlight.keywords = strings(),
            "commands.browser" => next.commands.browser = strings(),
            "commands.video" => next.commands.video = strings(),
            "commands.image" => next.commands.image = strings(),
            _ => unreachable!("known setting"),
        }
        if self.scope == Scope::Reader {
            next.store_reader();
        }
        Ok(next)
    }

    pub fn input_hint(&self) -> &'static str {
        match self.kind {
            Kind::Boolean => "true or false",
            Kind::Count => "integer from 1 to 10",
            Kind::Strings => "TOML string array, e.g. [\"one\", \"two\"]",
            Kind::Command => "nonempty argument array, e.g. [\"mpv\", \"--fullscreen\"]",
        }
    }
}

pub fn annotated() -> String {
    let defaults = Config::default();
    let mut text = String::from(
        "# redtab settings — edit here or press F2 in the app.\n# Valid file changes apply live. UI changes save here when writable.\n# F2 offers confirmed source saves for plain chezmoi-managed configs.\n# Reddit account preferences are read from and saved to Reddit.\n# Account reader rules use [accounts.<username>.filters] and .highlight.\n\n",
    );
    for section in ["", "media", "commands"] {
        if !section.is_empty() {
            text.push_str(&format!("[{section}]\n"));
        }
        for field in SETTINGS.iter().filter(|f| {
            f.scope == Scope::Device && f.key.rsplit_once('.').map_or("", |(s, _)| s) == section
        }) {
            let key = field.key.rsplit('.').next().unwrap();
            text.push_str(&format!(
                "# {}\n{key} = {}\n\n",
                field.description,
                field.value(&defaults)
            ));
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn schema_covers_every_serialized_leaf_and_template_round_trips() {
        fn leaves(value: &toml::Value, prefix: String, keys: &mut Vec<String>) {
            if let Some(table) = value.as_table() {
                for (k, v) in table {
                    leaves(
                        v,
                        if prefix.is_empty() {
                            k.clone()
                        } else {
                            format!("{prefix}.{k}")
                        },
                        keys,
                    );
                }
            } else {
                keys.push(prefix);
            }
        }
        let mut keys = Vec::new();
        leaves(
            &toml::Value::try_from(Config::default()).unwrap(),
            String::new(),
            &mut keys,
        );
        let mut schema: Vec<_> = SETTINGS
            .iter()
            .filter(|f| f.scope == Scope::Device)
            .map(|f| f.key.to_owned())
            .collect();
        keys.sort();
        schema.sort();
        assert_eq!(keys, schema);
        let config: Config = toml::from_str(&annotated()).unwrap();
        assert_eq!(config, Config::default());
        for field in SETTINGS.iter().filter(|f| f.scope == Scope::Device) {
            assert!(annotated().contains(field.description));
        }
    }
    #[test]
    fn editing_validates_types_ranges_and_executable() {
        let config = Config {
            active_account: Some("reader".into()),
            ..Config::default()
        };
        for (key, input) in [
            ("show_nsfw", "1"),
            ("all_subs_posts_per_group", "11"),
            ("filters.authors", "[1]"),
            ("commands.video", "[]"),
            ("commands.browser", "[\"\"]"),
        ] {
            let field = SETTINGS.iter().find(|f| f.key == key).unwrap();
            let doc: toml::Table = toml::from_str(&format!("v = {input}")).unwrap();
            assert!(field.set(&config, doc["v"].clone()).is_err(), "{key}");
        }
    }
}
