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

pub struct Setting {
    pub key: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub kind: Kind,
}

pub const SETTINGS: &[Setting] = &[
    Setting {
        key: "show_nsfw",
        label: "Show NSFW",
        description: "Include adult posts. Changing this reloads content immediately.",
        kind: Kind::Boolean,
    },
    Setting {
        key: "media.image_preview",
        label: "Image previews",
        description: "Show inline images, card previews, thumbnails and community icons.",
        kind: Kind::Boolean,
    },
    Setting {
        key: "media.video_autoplay",
        label: "Autoplay videos",
        description: "Play selected videos and GIFs muted after navigation settles. P still plays manually.",
        kind: Kind::Boolean,
    },
    Setting {
        key: "all_subs_posts_per_group",
        label: "Posts per community",
        description: "Number of posts in each All subs island (1–10).",
        kind: Kind::Count,
    },
    Setting {
        key: "filters.keywords",
        label: "Hide title keywords",
        description: "Hide titles containing these phrases, ignoring case. Use a TOML string array, e.g. [\"spoiler\"].",
        kind: Kind::Strings,
    },
    Setting {
        key: "filters.flairs",
        label: "Hide flairs",
        description: "Hide exact flair names, ignoring case. Use a TOML string array.",
        kind: Kind::Strings,
    },
    Setting {
        key: "filters.authors",
        label: "Hide authors",
        description: "Hide exact usernames without u/, ignoring case. Use a TOML string array.",
        kind: Kind::Strings,
    },
    Setting {
        key: "highlight.keywords",
        label: "Highlight keywords",
        description: "Emphasize titles containing these phrases, ignoring case. Use a TOML string array.",
        kind: Kind::Strings,
    },
    Setting {
        key: "commands.browser",
        label: "Browser command",
        description: "Argument array for opening URLs; URL appended. No shell expansion. Example: [\"open\"].",
        kind: Kind::Command,
    },
    Setting {
        key: "commands.video",
        label: "Video command",
        description: "Argument array for the external video player; stream URL appended. Example: [\"mpv\", \"--force-window=immediate\"].",
        kind: Kind::Command,
    },
    Setting {
        key: "commands.image",
        label: "Image command",
        description: "Argument array for opening local images; file paths appended. No shell expansion.",
        kind: Kind::Command,
    },
];

impl Setting {
    pub fn value(&self, config: &Config) -> toml::Value {
        let value = toml::Value::try_from(config).expect("config serializes");
        self.key
            .split('.')
            .fold(&value, |value, key| &value[key])
            .clone()
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
        self.validate_value(&value)?;
        let mut root = toml::Value::try_from(config)?;
        let mut target = &mut root;
        for key in self.key.split('.') {
            target = &mut target[key];
        }
        *target = value;
        let mut next: Config = root.try_into()?;
        next.path = config.path.clone();
        next.validate()?;
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
        "# redtab settings — edit here or press F2 in the app.\n# Valid file changes apply live. UI changes save here when writable.\n# F2 offers confirmed source saves for plain chezmoi-managed configs.\n\n",
    );
    for section in ["", "media", "filters", "highlight", "commands"] {
        if !section.is_empty() {
            text.push_str(&format!("[{section}]\n"));
        }
        for field in SETTINGS
            .iter()
            .filter(|f| f.key.rsplit_once('.').map_or("", |(s, _)| s) == section)
        {
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
        let mut schema: Vec<_> = SETTINGS.iter().map(|f| f.key.to_string()).collect();
        keys.sort();
        schema.sort();
        assert_eq!(keys, schema);
        let config: Config = toml::from_str(&annotated()).unwrap();
        assert_eq!(config, Config::default());
        for field in SETTINGS {
            assert!(annotated().contains(field.description));
        }
    }
    #[test]
    fn editing_validates_types_ranges_and_executable() {
        for (key, input) in [
            ("show_nsfw", "1"),
            ("all_subs_posts_per_group", "11"),
            ("filters.authors", "[1]"),
            ("commands.video", "[]"),
            ("commands.browser", "[\"\"]"),
        ] {
            let field = SETTINGS.iter().find(|f| f.key == key).unwrap();
            let doc: toml::Table = toml::from_str(&format!("v = {input}")).unwrap();
            assert!(
                field.set(&Config::default(), doc["v"].clone()).is_err(),
                "{key}"
            );
        }
    }
}
