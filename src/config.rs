//! User configuration: `$XDG_CONFIG_HOME/redtab/config.toml` plus an optional
//! subscriptions file (one subreddit per line, `#` comments).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::model::Post;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Base URL of the Redlib instance.
    pub redlib_url: String,
    /// Subreddits listed inline, in tab order.
    pub subscriptions: Vec<String>,
    /// File with more subreddits; defaults to `<config dir>/subscriptions`.
    pub subscriptions_file: Option<PathBuf>,
    /// Posts per subreddit group on the Home wall.
    pub home_posts_per_group: usize,
    pub show_nsfw: bool,
    /// Show images inline in the post view (kitty/iTerm2/sixel, else half-blocks).
    pub image_preview: bool,
    /// Command offered when Redlib is down, e.g. `["docker", "start", "redlib"]`.
    pub redlib_start_command: Option<Vec<String>>,
    pub filters: Filters,
    pub highlight: Highlight,
    pub commands: Commands,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Filters {
    /// Hide posts whose title contains any of these (case-insensitive).
    pub keywords: Vec<String>,
    /// Hide posts with any of these flairs (case-insensitive, exact).
    pub flairs: Vec<String>,
    /// Hide posts by these authors.
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
    /// Plays videos; receives the stream URL.
    pub video: Vec<String>,
    /// Opens full-size images; receives one or more local image file paths.
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
            redlib_url: "http://127.0.0.1:8080".into(),
            subscriptions: Vec::new(),
            subscriptions_file: None,
            home_posts_per_group: 3,
            show_nsfw: false,
            image_preview: true,
            redlib_start_command: None,
            filters: Filters::default(),
            highlight: Highlight::default(),
            commands: Commands::default(),
        }
    }
}

pub fn config_dir() -> PathBuf {
    xdg_dir("XDG_CONFIG_HOME", ".config").join("redtab")
}

pub fn state_dir() -> PathBuf {
    xdg_dir("XDG_STATE_HOME", ".local/state").join("redtab")
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
        config.subscriptions_file = config.subscriptions_file.as_deref().map(expand_tilde);
        config.home_posts_per_group = config.home_posts_per_group.clamp(1, 10);
        Ok(config)
    }

    /// Inline subscriptions followed by the subscriptions file, deduplicated.
    pub fn subscriptions(&self) -> Result<Vec<String>> {
        let (path, required) = match &self.subscriptions_file {
            Some(p) => (p.clone(), true),
            None => (config_dir().join("subscriptions"), false),
        };
        let file_text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !required => String::new(),
            Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
        };
        let mut names = Vec::new();
        for raw in self
            .subscriptions
            .iter()
            .map(String::as_str)
            .chain(file_text.lines())
        {
            let name = raw.split('#').next().unwrap_or_default().trim();
            let name = name
                .strip_prefix("r/")
                .or_else(|| name.strip_prefix("/r/"))
                .unwrap_or(name);
            if name.is_empty() {
                continue;
            }
            if !is_subreddit_name(name) {
                bail!(
                    "{}: {name:?} is not a subreddit name (letters, digits, _ only)",
                    path.display()
                );
            }
            if !names.iter().any(|n: &String| n.eq_ignore_ascii_case(name)) {
                names.push(name.to_string());
            }
        }
        Ok(names)
    }

    pub fn hides(&self, post: &Post) -> bool {
        if post.nsfw && !self.show_nsfw {
            return true;
        }
        let title = post.title.to_lowercase();
        let f = &self.filters;
        f.keywords
            .iter()
            .any(|k| !k.is_empty() && title.contains(&k.to_lowercase()))
            || post
                .flair
                .as_deref()
                .is_some_and(|flair| f.flairs.iter().any(|x| x.eq_ignore_ascii_case(flair)))
            || f.authors
                .iter()
                .any(|a| a.eq_ignore_ascii_case(&post.author))
    }

    pub fn highlights(&self, post: &Post) -> bool {
        let title = post.title.to_lowercase();
        self.highlight
            .keywords
            .iter()
            .any(|k| !k.is_empty() && title.contains(&k.to_lowercase()))
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
            age: String::new(),
            permalink: String::new(),
            media: Media::None,
            stickied: false,
            nsfw: false,
        }
    }

    fn config_with_file(text: &str, inline: &[&str]) -> (tempdir::Dir, Config) {
        let dir = tempdir::Dir::new();
        let file = dir.path().join("subs");
        std::fs::write(&file, text).unwrap();
        let config = Config {
            subscriptions: inline.iter().map(|s| s.to_string()).collect(),
            subscriptions_file: Some(file),
            ..Config::default()
        };
        (dir, config)
    }

    #[test]
    fn subscriptions_merge_inline_and_file_in_order_without_duplicates() {
        let (_dir, config) = config_with_file(
            "# mine\nNeovim\n\n  tmux  # terminal\nr/vim\nrust\n",
            &["rust", "neovim"],
        );

        assert_eq!(
            config.subscriptions().unwrap(),
            ["rust", "neovim", "tmux", "vim"]
        );
    }

    #[test]
    fn subscriptions_reject_a_bad_name_and_name_it() {
        let (_dir, config) = config_with_file("neovim\nr/vim/new\n", &[]);

        let err = config.subscriptions().unwrap_err().to_string();

        assert!(err.contains("\"vim/new\" is not a subreddit name"), "{err}");
    }

    #[test]
    fn explicit_subscriptions_file_must_exist() {
        let config = Config {
            subscriptions_file: Some(PathBuf::from("/nonexistent/redtab-subs")),
            ..Config::default()
        };

        assert!(config.subscriptions().is_err());
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
        std::fs::write(&path, "redlib_url = \"http://x\"\nunknown = 1\n").unwrap();

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
