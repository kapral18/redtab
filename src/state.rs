//! Local reading state: which posts you have seen or opened, and bookmarks.
//! Stored as JSON in `$XDG_STATE_HOME/redtab/state.json`.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::Post;

/// Seen/read marks not refreshed for this long are dropped on save.
const RETENTION_SECS: i64 = 90 * 24 * 60 * 60;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    /// Per subreddit (lowercase): post id → unix time it was last seen listed.
    #[serde(default)]
    seen: HashMap<String, HashMap<String, i64>>,
    /// Post id → unix time it was opened.
    #[serde(default)]
    read: HashMap<String, i64>,
    #[serde(default)]
    bookmarks: Vec<Bookmark>,
    /// Removed bookmarks: post id → removal time. Persisted so a merge with
    /// another instance's older copy does not bring a removed bookmark back.
    #[serde(default)]
    removed: HashMap<String, i64>,
    /// Set when the file exists but cannot be read: never overwrite it.
    #[serde(skip)]
    read_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bookmark {
    pub post: Post,
    pub saved_at: i64,
}

impl State {
    /// Load state. A file that is not valid state JSON is moved aside; a file
    /// that cannot be read at all is left alone and saving is turned off.
    pub fn load(path: &Path) -> (Self, Option<String>) {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Self::default(), None),
            Err(e) => {
                let state = Self {
                    read_only: true,
                    ..Self::default()
                };
                return (
                    state,
                    Some(format!(
                        "cannot read {}: {e}; this session will not save state",
                        path.display()
                    )),
                );
            }
        };
        match serde_json::from_slice(&bytes) {
            Ok(state) => (state, None),
            Err(e) => {
                let aside = path.with_extension("json.bad");
                match std::fs::rename(path, &aside) {
                    Ok(()) => (
                        Self::default(),
                        Some(format!(
                            "state file was invalid ({e}); moved to {}",
                            aside.display()
                        )),
                    ),
                    Err(rename) => (
                        Self {
                            read_only: true,
                            ..Self::default()
                        },
                        Some(format!(
                            "state file was invalid ({e}) and could not be moved aside ({rename}); not saving"
                        )),
                    ),
                }
            }
        }
    }

    /// Write state, first merging what other redtab instances saved meanwhile.
    /// Returns true when the merge changed the bookmark list. Fails without
    /// touching the file when it could not be read at start.
    pub fn save(&mut self, path: &Path, now: i64) -> Result<bool> {
        if self.read_only {
            anyhow::bail!("not saving: {} could not be read at start", path.display());
        }
        let before: Vec<String> = self.bookmarks.iter().map(|b| b.post.id.clone()).collect();
        if let Ok(bytes) = std::fs::read(path)
            && let Ok(disk) = serde_json::from_slice::<State>(&bytes)
        {
            self.merge(disk);
        }
        let changed = self.bookmarks.iter().map(|b| &b.post.id).ne(before.iter());
        self.prune(now);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("cannot create {}", dir.display()))?;
        }
        let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
        std::fs::write(&tmp, serde_json::to_vec(self)?)
            .with_context(|| format!("cannot write {}", tmp.display()))?;
        std::fs::rename(&tmp, path)
            .with_context(|| format!("cannot replace {}", path.display()))?;
        Ok(changed)
    }

    fn merge(&mut self, disk: State) {
        for (sub, ids) in disk.seen {
            let mine = self.seen.entry(sub).or_default();
            for (id, t) in ids {
                mine.entry(id).and_modify(|x| *x = (*x).max(t)).or_insert(t);
            }
        }
        for (id, t) in disk.read {
            self.read
                .entry(id)
                .and_modify(|x| *x = (*x).max(t))
                .or_insert(t);
        }
        for (id, t) in disk.removed {
            self.removed
                .entry(id)
                .and_modify(|x| *x = (*x).max(t))
                .or_insert(t);
        }
        for b in disk.bookmarks {
            match self.bookmarks.iter_mut().find(|m| m.post.id == b.post.id) {
                Some(mine) if b.saved_at > mine.saved_at => *mine = b,
                Some(_) => {}
                None => self.bookmarks.push(b),
            }
        }
        // The latest event wins: a removal newer than the save drops it.
        let removed = &self.removed;
        self.bookmarks
            .retain(|b| removed.get(&b.post.id).is_none_or(|&t| t < b.saved_at));
        self.bookmarks
            .sort_by_key(|b| std::cmp::Reverse(b.saved_at));
    }

    fn prune(&mut self, now: i64) {
        let cutoff = now - RETENTION_SECS;
        for ids in self.seen.values_mut() {
            ids.retain(|_, t| *t >= cutoff);
        }
        self.seen.retain(|_, ids| !ids.is_empty());
        self.read.retain(|_, t| *t >= cutoff);
        self.removed.retain(|_, t| *t >= cutoff);
    }

    /// First time a subreddit loads, its current posts become the baseline,
    /// so only posts that appear later count as new.
    pub fn baseline(&mut self, subreddit: &str, posts: &[Post], now: i64) {
        let key = subreddit.to_lowercase();
        if !self.seen.contains_key(&key) {
            self.mark_seen(subreddit, posts.iter().map(|p| p.id.as_str()), now);
        }
    }

    /// Record posts as seen; refreshing the time keeps long-pinned posts from
    /// expiring and later reappearing as new.
    pub fn mark_seen<'a>(
        &mut self,
        subreddit: &str,
        ids: impl IntoIterator<Item = &'a str>,
        now: i64,
    ) {
        let entry = self.seen.entry(subreddit.to_lowercase()).or_default();
        for id in ids {
            entry.insert(id.to_string(), now);
        }
    }

    pub fn is_seen(&self, subreddit: &str, id: &str) -> bool {
        self.seen
            .get(&subreddit.to_lowercase())
            .is_some_and(|ids| ids.contains_key(id))
    }

    pub fn unseen<'a>(&self, subreddit: &str, posts: impl IntoIterator<Item = &'a Post>) -> usize {
        posts
            .into_iter()
            .filter(|p| !self.is_seen(subreddit, &p.id))
            .count()
    }

    pub fn mark_read(&mut self, post: &Post, now: i64) {
        self.read.insert(post.id.clone(), now);
        self.mark_seen(&post.subreddit, [post.id.as_str()], now);
    }

    pub fn is_read(&self, id: &str) -> bool {
        self.read.contains_key(id)
    }

    pub fn bookmarks(&self) -> &[Bookmark] {
        &self.bookmarks
    }

    pub fn is_bookmarked(&self, id: &str) -> bool {
        self.bookmarks.iter().any(|b| b.post.id == id)
    }

    /// Add or remove a bookmark; returns true when the post is now bookmarked.
    /// Each event is stamped strictly after the previous one for the post, so
    /// remove/re-add within one second still orders correctly across instances.
    pub fn toggle_bookmark(&mut self, post: &Post, now: i64) -> bool {
        if let Some(i) = self.bookmarks.iter().position(|b| b.post.id == post.id) {
            let saved_at = self.bookmarks.remove(i).saved_at;
            self.removed.insert(post.id.clone(), now.max(saved_at + 1));
            false
        } else {
            let after = self
                .removed
                .remove(&post.id)
                .map_or(now, |t| now.max(t + 1));
            self.bookmarks.insert(
                0,
                Bookmark {
                    post: post.clone(),
                    saved_at: after,
                },
            );
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Media;

    fn post(id: &str, sub: &str) -> Post {
        Post {
            id: id.into(),
            subreddit: sub.into(),
            title: id.into(),
            author: "a".into(),
            flair: None,
            score: None,
            comments: 0,
            created: None,
            age: String::new(),
            permalink: format!("/r/{sub}/comments/{id}/x/"),
            media: Media::None,
            stickied: false,
            nsfw: false,
        }
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("redtab-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn first_load_is_a_baseline_and_later_posts_count_as_new() {
        let mut state = State::default();
        let first = [post("a", "Neovim"), post("b", "Neovim")];
        state.baseline("Neovim", &first, 100);
        assert_eq!(state.unseen("neovim", &first), 0);

        let later = [post("c", "neovim"), post("a", "neovim")];
        state.baseline("neovim", &later, 200);

        assert_eq!(state.unseen("NEOVIM", &later), 1);
    }

    #[test]
    fn reading_a_post_marks_it_seen_and_read() {
        let mut state = State::default();
        state.baseline("vim", &[], 1);
        let p = post("x", "vim");

        state.mark_read(&p, 5);

        assert!(state.is_read("x"));
        assert!(state.is_seen("vim", "x"));
    }

    #[test]
    fn bookmarks_toggle_newest_first() {
        let mut state = State::default();

        assert!(state.toggle_bookmark(&post("a", "s"), 1));
        assert!(state.toggle_bookmark(&post("b", "s"), 2));
        assert_eq!(state.bookmarks()[0].post.id, "b");
        assert!(!state.toggle_bookmark(&post("a", "s"), 3));

        assert!(!state.is_bookmarked("a"));
        assert!(state.is_bookmarked("b"));
    }

    #[test]
    fn save_round_trips_and_prunes_stale_marks_but_keeps_bookmarks() {
        let dir = scratch("roundtrip");
        let path = dir.join("state.json");
        let mut state = State::default();
        let old = post("old", "s");
        state.mark_read(&old, 0);
        state.toggle_bookmark(&old, 0);
        state.mark_read(&post("new", "s"), RETENTION_SECS + 10);

        state.save(&path, RETENTION_SECS + 20).unwrap();
        let (loaded, warning) = State::load(&path);
        std::fs::remove_dir_all(&dir).unwrap();

        assert!(warning.is_none());
        assert!(!loaded.is_read("old"));
        assert!(loaded.is_read("new"));
        assert!(loaded.is_bookmarked("old"));
    }

    #[test]
    fn seeing_a_post_again_keeps_it_from_expiring() {
        let mut state = State::default();
        state.mark_seen("s", ["pinned"], 0);
        state.mark_seen("s", ["pinned"], RETENTION_SECS);

        state.prune(RETENTION_SECS + 10);

        assert!(state.is_seen("s", "pinned"));
    }

    #[test]
    fn two_instances_keep_each_others_bookmarks_and_removals() {
        let dir = scratch("merge");
        let path = dir.join("state.json");
        let mut first = State::default();
        first.toggle_bookmark(&post("keep", "s"), 1);
        first.toggle_bookmark(&post("drop", "s"), 2);
        first.save(&path, 10).unwrap();

        let (mut second, _) = State::load(&path);
        first.toggle_bookmark(&post("mine", "s"), 3);
        second.toggle_bookmark(&post("theirs", "s"), 4);
        second.toggle_bookmark(&post("drop", "s"), 5);
        second.save(&path, 10).unwrap();
        first.save(&path, 10).unwrap();
        let (merged, _) = State::load(&path);
        std::fs::remove_dir_all(&dir).unwrap();

        let ids: Vec<&str> = merged
            .bookmarks()
            .iter()
            .map(|b| b.post.id.as_str())
            .collect();
        assert!(
            ids.contains(&"keep") && ids.contains(&"mine") && ids.contains(&"theirs"),
            "{ids:?}"
        );
        // `second` removed "drop" after `first` saved it; `first` still had it in
        // memory, but the newer removal wins no matter which instance saves last.
        assert!(!ids.contains(&"drop"), "{ids:?}");
    }

    #[test]
    fn re_adding_a_removed_bookmark_in_the_same_second_keeps_it() {
        let dir = scratch("readd");
        let path = dir.join("state.json");
        let mut state = State::default();
        let p = post("x", "s");
        state.toggle_bookmark(&p, 7);
        state.toggle_bookmark(&p, 7);
        state.save(&path, 7).unwrap();
        state.toggle_bookmark(&p, 7);
        state.save(&path, 7).unwrap();
        let (loaded, _) = State::load(&path);
        std::fs::remove_dir_all(&dir).unwrap();

        assert!(loaded.is_bookmarked("x"));
    }

    #[test]
    fn corrupt_state_is_moved_aside() {
        let dir = scratch("bad");
        let path = dir.join("state.json");
        std::fs::write(&path, b"\xff\xfe not json").unwrap();

        let (state, warning) = State::load(&path);
        let moved = dir.join("state.json.bad").exists();
        std::fs::remove_dir_all(&dir).unwrap();

        assert!(state.bookmarks().is_empty());
        assert!(warning.unwrap().contains("moved to"));
        assert!(moved);
    }

    #[test]
    fn unreadable_state_is_never_overwritten() {
        let dir = scratch("unreadable");
        let path = dir.join("state.json");
        std::fs::create_dir(&path).unwrap(); // reading a directory fails

        let (mut state, warning) = State::load(&path);
        state.toggle_bookmark(&post("a", "s"), 1);
        let saved = state.save(&path, 1);
        let still_dir = path.is_dir();
        std::fs::remove_dir_all(&dir).unwrap();

        assert!(warning.unwrap().contains("will not save"));
        assert!(saved.is_err(), "a skipped save must not report success");
        assert!(still_dir);
    }
}
