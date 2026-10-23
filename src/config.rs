//! User configuration: `$XDG_CONFIG_HOME/redtab/config.toml`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::model::Post;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Posts per subreddit island in All subs.
    pub all_subs_posts_per_group: usize,
    pub show_nsfw: bool,
    /// Inline images, list thumbnails and subreddit icons.
    pub image_preview: bool,
    pub filters: Filters,
    pub highlight: Highlight,
    pub commands: Commands,
    /// The loaded file, empty for a config built in code. Error messages and `--check`
    /// name it.
    #[serde(skip)]
    pub path: PathBuf,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Filters {
    /// Hide posts whose title contains any of these (case-insensitive).
    pub keywords: Vec<String>,
    /// Hide posts with any of these flairs (case-insensitive, exact).
    pub flairs: Vec<String>,
    pub authors: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Highlight {
    /// Emphasize posts whose title contains any of these (case-insensitive).
    pub keywords: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Commands {
    /// Opens Reddit links and external URLs.
    pub browser: Vec<String>,
    /// Plays a stream URL in a window (`m`).
    pub video: Vec<String>,
    /// Opens local image files.
    pub image: Vec<String>,
}

fn system_opener() -> Vec<String> {
    if cfg!(target_os = "macos") {
        vec!["open".into()]
    } else {
        vec!["xdg-open".into()]
    }
}

impl Default for Commands {
    fn default() -> Self {
        Self {
            browser: system_opener(),
            video: vec!["mpv".into(), "--force-window=immediate".into()],
            image: system_opener(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            all_subs_posts_per_group: 3,
            show_nsfw: false,
            image_preview: true,
            filters: Filters::default(),
            highlight: Highlight::default(),
            commands: Commands::default(),
            path: PathBuf::new(),
        }
    }
}

pub fn config_dir() -> PathBuf {
    xdg_dir("XDG_CONFIG_HOME", ".config").join("redtab")
}

pub fn state_dir() -> PathBuf {
    xdg_dir("XDG_STATE_HOME", ".local/state").join("redtab")
}

pub fn cache_dir() -> PathBuf {
    xdg_dir("XDG_CACHE_HOME", ".cache").join("redtab")
}

fn xdg_dir(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(fallback))
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn expand_tilde(path: &Path) -> PathBuf {
    match path.strip_prefix("~") {
        Ok(rest) => home().join(rest),
        Err(_) => path.to_path_buf(),
    }
}

impl Config {
    /// Load `path`, or the default config file; a missing default file means defaults.
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let (path, required) = match path {
            Some(p) => (expand_tilde(p), true),
            None => (config_dir().join("config.toml"), false),
        };
        let mut config: Config = match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text)
                .with_context(|| format!("invalid config {}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !required => Config::default(),
            Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
        };
        config.all_subs_posts_per_group = config.all_subs_posts_per_group.clamp(1, 10);
        config
            .validate()
            .with_context(|| format!("invalid config {}", path.display()))?;
        config.path = path;
        Ok(config)
    }

    /// Catch values that would only fail later, with a vaguer message.
    fn validate(&self) -> Result<()> {
        for (key, argv) in [
            ("browser", &self.commands.browser),
            ("video", &self.commands.video),
            ("image", &self.commands.image),
        ] {
            if argv.is_empty() {
                bail!("commands.{key} is empty; remove it to use the default");
            }
        }
        Ok(())
    }

    pub fn hides(&self, post: &Post) -> bool {
        if post.nsfw && !self.show_nsfw {
            return true;
        }
        let f = &self.filters;
        contains_keyword(&post.title, &f.keywords)
            || post
                .flair
                .as_deref()
                .is_some_and(|flair| f.flairs.iter().any(|x| x.eq_ignore_ascii_case(flair)))
            || f.authors
                .iter()
                .any(|a| a.eq_ignore_ascii_case(&post.author))
    }

    pub fn highlights(&self, post: &Post) -> bool {
        contains_keyword(&post.title, &self.highlight.keywords)
    }

    pub fn visible<'a>(&self, posts: &'a [Post]) -> impl Iterator<Item = &'a Post> {
        posts.iter().filter(|p| !self.hides(p))
    }
}

fn contains_keyword(text: &str, keywords: &[String]) -> bool {
    if keywords.is_empty() {
        return false;
    }
    let text = text.to_lowercase();
    keywords
        .iter()
        .any(|k| !k.is_empty() && text.contains(&k.to_lowercase()))
}

/// A subreddit name as people write it: `/r/vim/`, `R/rust`, `r/tmux # note`,
/// with a byte order mark or spaces around; the name itself comes back.
pub fn normalize_subreddit(raw: &str) -> &str {
    let name = raw
        .trim_start_matches('\u{feff}')
        .split('#')
        .next()
        .unwrap_or_default()
        .trim()
        .trim_end_matches('/');
    let name = name.strip_prefix('/').unwrap_or(name);
    match name.get(..2) {
        Some(prefix) if prefix.eq_ignore_ascii_case("r/") => &name[2..],
        _ => name,
    }
}

pub fn is_subreddit_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 50
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Media;

    fn post(title: &str, flair: Option<&str>, author: &str) -> Post {
        Post {
            id: "x".into(),
            subreddit: "s".into(),
            title: title.into(),
            author: author.into(),
            flair: flair.map(str::to_string),
            score: None,
            comments: 0,
            created: None,
            permalink: String::new(),
            media: Media::None,
            preview_size: None,
            stickied: false,
            nsfw: false,
            spoiler: false,
        }
    }

    #[test]
    fn config_values_that_would_fail_later_are_rejected_at_load() {
        let dir = tempdir::Dir::new();
        let config_path = dir.path().join("config.toml");
        let load = |text: &str| {
            std::fs::write(&config_path, text).unwrap();
            Config::load(Some(&config_path)).map_err(|e| format!("{e:#}"))
        };

        assert!(
            load("[commands]\nimage = []\n")
                .unwrap_err()
                .contains("commands.image")
        );
    }

    #[test]
    fn when_old_subscription_keys_are_present_they_should_not_silently_curate_the_account() {
        assert!(toml::from_str::<Config>("subscriptions = [\"rust\"]").is_err());
        assert!(toml::from_str::<Config>("subscriptions_file = \"subs\"").is_err());
        assert!(toml::from_str::<Config>("home_posts_per_group = 3").is_err());
        assert_eq!(
            toml::from_str::<Config>("all_subs_posts_per_group = 4")
                .unwrap()
                .all_subs_posts_per_group,
            4
        );
    }

    #[test]
    fn filters_hide_by_keyword_flair_author_and_nsfw() {
        let config = Config {
            filters: Filters {
                keywords: vec!["Crypto".into()],
                flairs: vec!["meme".into()],
                authors: vec!["spambot".into()],
            },
            ..Config::default()
        };

        assert!(config.hides(&post("Buy crypto now", None, "a")));
        assert!(config.hides(&post("Fine title", Some("Meme"), "a")));
        assert!(config.hides(&post("Fine title", None, "SpamBot")));
        assert!(!config.hides(&post("Fine title", Some("Plugin"), "a")));
        let mut nsfw = post("Fine title", None, "a");
        nsfw.nsfw = true;
        assert!(config.hides(&nsfw));
        let shown = Config {
            show_nsfw: true,
            ..Config::default()
        };
        assert!(!shown.hides(&nsfw));
    }

    #[test]
    fn highlights_match_title_keywords_case_insensitively() {
        let config = Config {
            highlight: Highlight {
                keywords: vec!["LSP".into()],
            },
            ..Config::default()
        };

        assert!(config.highlights(&post("Faster lsp startup", None, "a")));
        assert!(!config.highlights(&post("Colorscheme", None, "a")));
    }

    #[test]
    fn config_file_rejects_unknown_keys() {
        let dir = tempdir::Dir::new();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "show_nsfw = true\nunknown = 1\n").unwrap();

        assert!(Config::load(Some(&path)).is_err());
    }

    /// Minimal self-cleaning temp dir (no extra dependency).
    mod tempdir {
        use std::path::{Path, PathBuf};
        use std::sync::atomic::{AtomicU32, Ordering};

        static N: AtomicU32 = AtomicU32::new(0);

        pub struct Dir(PathBuf);

        impl Dir {
            pub fn new() -> Self {
                let path = std::env::temp_dir().join(format!(
                    "redtab-test-{}-{}",
                    std::process::id(),
                    N.fetch_add(1, Ordering::Relaxed)
                ));
                std::fs::create_dir_all(&path).unwrap();
                Dir(path)
            }

            pub fn path(&self) -> &Path {
                &self.0
            }
        }

        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }
}
