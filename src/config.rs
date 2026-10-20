//! User configuration: `$XDG_CONFIG_HOME/redtab/config.toml` plus an optional
//! subscriptions file (one subreddit per line, `#` comments).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::model::Post;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Inline subreddits, in tab order.
    pub subscriptions: Vec<String>,
    /// File with more subreddits; defaults to `$XDG_STATE_HOME/redtab/subscriptions`.
    pub subscriptions_file: Option<PathBuf>,
    /// Posts per subreddit group on the Home wall.
    pub home_posts_per_group: usize,
    pub show_nsfw: bool,
    /// Inline images, list thumbnails and subreddit icons.
    pub image_preview: bool,
    pub filters: Filters,
    pub highlight: Highlight,
    pub commands: Commands,
    /// The loaded file, empty for a config built in code. Error messages and `--check`
    /// name it; relative `subscriptions_file` overrides resolve next to it.
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
            subscriptions: Vec::new(),
            subscriptions_file: None,
            home_posts_per_group: 3,
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
        // A relative subscriptions file sits next to the config file.
        config.subscriptions_file = config.subscriptions_file.as_deref().map(|p| {
            let p = expand_tilde(p);
            match path.parent() {
                Some(dir) if p.is_relative() => dir.join(p),
                _ => p,
            }
        });
        config.home_posts_per_group = config.home_posts_per_group.clamp(1, 10);
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

    pub fn subscriptions_path(&self) -> PathBuf {
        self.subscriptions_file
            .clone()
            .unwrap_or_else(|| state_dir().join("subscriptions"))
    }

    /// Inline subscriptions followed by the subscriptions file, deduplicated.
    pub fn subscriptions(&self) -> Result<Vec<String>> {
        let path = self.subscriptions_path();
        let required = self.subscriptions_file.is_some();
        let file_text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !required => String::new(),
            Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
        };
        let inline = self.subscriptions.iter().map(|n| (n.as_str(), true));
        let from_file = file_text.lines().map(|n| (n, false));
        let mut names = Vec::new();
        for (raw, is_inline) in inline.chain(from_file) {
            let name = normalize_subreddit(raw);
            if name.is_empty() {
                continue;
            }
            if !is_subreddit_name(name) {
                let source = if is_inline {
                    format!("{} (subscriptions)", self.path.display())
                } else {
                    path.display().to_string()
                };
                bail!("{source}: {name:?} is not a subreddit name (letters, digits, _ only)");
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
            stickied: false,
            nsfw: false,
            spoiler: false,
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
    fn subscriptions_accept_common_spellings() {
        let (_dir, config) = config_with_file("\u{feff}neovim\n/r/vim/\nR/rust\nr/tmux/\n", &[]);

        assert_eq!(
            config.subscriptions().unwrap(),
            ["neovim", "vim", "rust", "tmux"]
        );
    }

    #[test]
    fn a_bad_inline_subscription_names_the_config_file() {
        let (_dir, mut config) = config_with_file("", &["bad-name"]);
        config.path = PathBuf::from("/etc/redtab.toml");

        let err = config.subscriptions().unwrap_err().to_string();

        assert!(err.starts_with("/etc/redtab.toml"), "{err}");
    }

    #[test]
    fn an_explicit_relative_subscriptions_file_sits_next_to_a_given_config() {
        let dir = tempdir::Dir::new();
        let config_path = dir.path().join("config.toml");
        std::fs::write(dir.path().join("subs"), "neovim\n").unwrap();
        std::fs::write(&config_path, "subscriptions_file = \"subs\"\n").unwrap();

        let config = Config::load(Some(&config_path)).unwrap();

        assert_eq!(config.subscriptions().unwrap(), ["neovim"]);
    }

    #[test]
    fn when_config_is_default_subscriptions_should_merge_inline_with_private_state() {
        with_subscription_home(
            "when_config_is_default_subscriptions_should_merge_inline_with_private_state",
            Some("état"),
            |home| {
                std::fs::write(
                    home.join("config/redtab/config.toml"),
                    "subscriptions = [\"rust\", \"neovim\"]\n",
                )
                .unwrap();
                std::fs::write(home.join("config/redtab/subscriptions"), "old-place\n").unwrap();
                std::fs::write(home.join("état/redtab/subscriptions"), "RUST\ntmux\n").unwrap();

                let config = Config::load(None).unwrap();

                assert_eq!(config.subscriptions().unwrap(), ["rust", "neovim", "tmux"]);
            },
        );
    }

    #[test]
    fn when_config_is_elsewhere_subscriptions_should_still_use_private_state() {
        with_subscription_home(
            "when_config_is_elsewhere_subscriptions_should_still_use_private_state",
            Some("état"),
            |home| {
                let path = home.join("custom.toml");
                std::fs::write(&path, "").unwrap();
                std::fs::write(home.join("subscriptions"), "old-place\n").unwrap();
                std::fs::write(home.join("état/redtab/subscriptions"), "rust\n").unwrap();

                let config = Config::load(Some(&path)).unwrap();

                assert_eq!(config.subscriptions().unwrap(), ["rust"]);
            },
        );
    }

    #[test]
    fn when_xdg_state_home_is_unset_subscriptions_should_use_home_state() {
        with_subscription_home(
            "when_xdg_state_home_is_unset_subscriptions_should_use_home_state",
            None,
            |home| {
                std::fs::write(home.join(".local/state/redtab/subscriptions"), "rust\n").unwrap();

                assert_eq!(Config::default().subscriptions().unwrap(), ["rust"]);
            },
        );
    }

    #[test]
    fn when_private_subscriptions_change_the_config_should_read_the_current_list() {
        with_subscription_home(
            "when_private_subscriptions_change_the_config_should_read_the_current_list",
            Some("état"),
            |home| {
                let path = home.join("état/redtab/subscriptions");
                let mut config = Config {
                    subscriptions: vec!["rust".into()],
                    ..Config::load(None).unwrap()
                };

                assert_eq!(config.subscriptions().unwrap(), ["rust"]);
                std::fs::write(&path, "").unwrap();
                assert_eq!(config.subscriptions().unwrap(), ["rust"]);
                std::fs::write(&path, "RUST\nvim\nvim\n").unwrap();
                assert_eq!(config.subscriptions().unwrap(), ["rust", "vim"]);
                config.subscriptions = vec!["tmux".into()];
                assert_eq!(config.subscriptions().unwrap(), ["tmux", "RUST", "vim"]);
                std::fs::remove_file(&path).unwrap();
                assert_eq!(config.subscriptions().unwrap(), ["tmux"]);
                std::fs::write(&path, "neovim\n").unwrap();
                assert_eq!(config.subscriptions().unwrap(), ["tmux", "neovim"]);
            },
        );
    }

    #[cfg(unix)]
    #[test]
    fn when_private_subscriptions_is_a_symlink_it_should_read_the_target() {
        with_subscription_home(
            "when_private_subscriptions_is_a_symlink_it_should_read_the_target",
            Some("état"),
            |home| {
                let target = home.join("abonnements");
                std::fs::write(&target, "rust\n").unwrap();
                std::os::unix::fs::symlink(&target, home.join("état/redtab/subscriptions"))
                    .unwrap();

                assert_eq!(
                    Config::load(None).unwrap().subscriptions().unwrap(),
                    ["rust"]
                );
            },
        );
    }

    fn with_subscription_home(name: &str, state_subdir: Option<&str>, check: impl FnOnce(&Path)) {
        const CHILD_HOME: &str = "REDTAB_SUBSCRIPTIONS_TEST_HOME";
        if let Some(home) = std::env::var_os(CHILD_HOME) {
            check(Path::new(&home));
            return;
        }
        let dir = tempdir::Dir::new();
        let home = dir.path();
        std::fs::create_dir_all(home.join("config/redtab")).unwrap();
        std::fs::create_dir_all(
            home.join(state_subdir.unwrap_or(".local/state"))
                .join("redtab"),
        )
        .unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .args(["--exact", &format!("config::tests::{name}"), "--nocapture"])
            .env(CHILD_HOME, home)
            .env("HOME", home)
            .env("XDG_CONFIG_HOME", home.join("config"));
        match state_subdir {
            Some(subdir) => child.env("XDG_STATE_HOME", home.join(subdir)),
            None => child.env_remove("XDG_STATE_HOME"),
        };
        let output = child.output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success() && stdout.contains("test result: ok. 1 passed;"),
            "{stdout}\n{stderr}"
        );
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
