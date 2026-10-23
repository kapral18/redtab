//! Reading state (seen and opened posts, bookmarks) in `$XDG_STATE_HOME/redtab/state.json`.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::Post;

/// Seen/read marks not refreshed for this long are dropped on save.
const RETENTION_SECS: i64 = 90 * 24 * 60 * 60;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ListView {
    #[default]
    Card,
    Compact,
}

impl ListView {
    pub fn label(self) -> &'static str {
        match self {
            Self::Card => "Card",
            Self::Compact => "Compact",
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    list_view: ListView,
    #[serde(default)]
    list_view_changed_at: i64,
    /// Per subreddit (lowercase): post id → unix time it was last seen listed.
    #[serde(default)]
    seen: HashMap<String, HashMap<String, i64>>,
    /// Post id → unix time it was opened.
    #[serde(default)]
    read: HashMap<String, i64>,
    #[serde(default)]
    bookmarks: Vec<Bookmark>,
    /// Removed bookmarks: id → removal time, so merging an older copy can't resurrect them.
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
    pub fn list_view(&self) -> ListView {
        self.list_view
    }

    pub fn toggle_list_view(&mut self) {
        self.list_view = match self.list_view {
            ListView::Card => ListView::Compact,
            ListView::Compact => ListView::Card,
        };
        self.list_view_changed_at = chrono::Utc::now()
            .timestamp_millis()
            .max(self.list_view_changed_at + 1);
    }

    /// Invalid JSON is moved aside; an unreadable file is left alone and saving is disabled.
    pub fn load(path: &Path) -> (Self, Option<String>) {
        // Act on a symlink's target, never the link.
        let path = &resolve_link(path);
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
                // Never overwrite an earlier backup.
                let mut aside = path.with_extension("json.bad");
                if aside.exists() {
                    let stamp = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| d.as_secs());
                    aside = path.with_extension(format!("json.bad-{stamp}"));
                }
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
}

/// The file `path` names, following links even to a missing target, so writes never
/// replace a link.
fn resolve_link(path: &Path) -> std::path::PathBuf {
    if let Ok(real) = std::fs::canonicalize(path) {
        return real;
    }
    let mut current = path.to_path_buf();
    // Linux's symlink limit (macOS stops at 32), so a link loop can't spin.
    for _ in 0..40 {
        let Ok(target) = std::fs::read_link(&current) else {
            break;
        };
        current = match current.parent() {
            Some(dir) if target.is_relative() => dir.join(target),
            _ => target,
        };
    }
    current
}

/// How long a save waits for another instance's save before giving up.
const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(2);
/// A lockdir older than this is likely a crash's leftover; the error says how to clear it.
const STALE_LOCK: std::time::Duration = std::time::Duration::from_secs(30);

/// Held from reading to replacing the state file, so concurrent instances' saves take turns.
enum SaveLock {
    /// `flock(2)` on `state.json.lock`; the file stays.
    Flock(#[allow(dead_code)] std::fs::File),
    /// `state.json.lockdir` where `flock` never works (some network mounts). It can't see
    /// `flock` holders, so it's safe only when every instance falls back.
    Dir(std::path::PathBuf),
}

impl Drop for SaveLock {
    fn drop(&mut self) {
        if let SaveLock::Dir(path) = self {
            let _ = std::fs::remove_dir(&*path);
        }
    }
}

/// What a failed non-blocking `flock` means for the save.
#[derive(Debug, PartialEq, Eq)]
enum FlockFailure {
    /// Held elsewhere or interrupted: retry until the deadline.
    Busy,
    /// This filesystem has no `flock`: use the directory lock.
    Unsupported,
    Error,
}

fn flock_failure(errno: Option<i32>) -> FlockFailure {
    match errno {
        Some(libc::EWOULDBLOCK | libc::EINTR) => FlockFailure::Busy,
        // ENOTSUP and EOPNOTSUPP differ on macOS; ENOSYS comes from mounts without flock (Lustre).
        Some(code)
            if [libc::ENOTSUP, libc::EOPNOTSUPP, libc::ENOLCK, libc::ENOSYS].contains(&code) =>
        {
            FlockFailure::Unsupported
        }
        _ => FlockFailure::Error,
    }
}

fn lock(target: &Path) -> Result<SaveLock> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;

    let path = target.with_extension("json.lock");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&path)
        .with_context(|| format!("cannot open {}", path.display()))?;
    // Bounded: a stalled instance must not freeze this one's quit.
    let deadline = std::time::Instant::now() + LOCK_WAIT;
    loop {
        // SAFETY: flock on a descriptor this function owns; no memory is involved.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(SaveLock::Flock(file));
        }
        let e = std::io::Error::last_os_error();
        match flock_failure(e.raw_os_error()) {
            FlockFailure::Busy if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            FlockFailure::Busy => return Err(busy(target)),
            FlockFailure::Unsupported => return dir_lock(target),
            FlockFailure::Error => {
                return Err(e).with_context(|| format!("cannot lock {}", path.display()));
            }
        }
    }
}

/// Lock for filesystems without `flock`: the creator of `state.json.lockdir` holds it and
/// only it removes it. Never broken automatically (check-then-remove isn't atomic), so a
/// crash's leftover blocks saves until deleted.
fn dir_lock(target: &Path) -> Result<SaveLock> {
    let path = target.with_extension("json.lockdir");
    let deadline = std::time::Instant::now() + LOCK_WAIT;
    loop {
        match std::fs::create_dir(&path) {
            Ok(()) => return Ok(SaveLock::Dir(path)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e).with_context(|| format!("cannot lock {}", path.display())),
        }
        // A lock this old isn't a save in progress: fail now rather than freeze 2 s per save.
        let age = std::fs::symlink_metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok());
        if let Some(age) = age.filter(|&age| age > STALE_LOCK) {
            return Err(anyhow::anyhow!(
                "not saving: lock {} is {} s old; if no redtab is running, delete it",
                path.display(),
                age.as_secs()
            ));
        }
        if std::time::Instant::now() >= deadline {
            return Err(busy(target));
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn busy(target: &Path) -> anyhow::Error {
    anyhow::anyhow!(
        "not saving: another redtab is still saving {}",
        target.display()
    )
}

impl State {
    /// Merges other instances' saves, then writes. Refuses to overwrite a file that was
    /// or became unreadable or invalid.
    pub fn save(&mut self, path: &Path, now: i64) -> Result<()> {
        use std::io::Write;
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

        if self.read_only {
            anyhow::bail!("not saving: {} could not be read at start", path.display());
        }
        let target = resolve_link(path);
        if let Some(dir) = target.parent() {
            // Reading history is private.
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)
                .with_context(|| format!("cannot create {}", dir.display()))?;
        }
        // Hold the lock from read to rename, or another instance's save in between is lost.
        let _lock = lock(&target)?;
        match std::fs::read(&target) {
            Ok(bytes) => {
                let disk = serde_json::from_slice::<State>(&bytes).with_context(|| {
                    format!("not saving: {} is no longer valid state", target.display())
                })?;
                self.merge(disk);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("not saving: cannot read {}", target.display()));
            }
        }
        self.prune(now);
        let json = serde_json::to_vec(self)?;
        // Saves are serialized, so one temp name suffices; the next save removes a leftover.
        let tmp = target.with_extension("json.tmp");
        let write = || -> std::io::Result<()> {
            let _ = std::fs::remove_file(&tmp);
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp)?;
            // Keep the permissions the user gave the file.
            if let Ok(meta) = std::fs::metadata(&target) {
                file.set_permissions(meta.permissions())?;
            }
            file.write_all(&json)?;
            // Durable before the rename, so a crash can't leave an empty file.
            file.sync_all()?;
            std::fs::rename(&tmp, &target)
        };
        if let Err(e) = write() {
            let _ = std::fs::remove_file(&tmp);
            return Err(e).with_context(|| format!("cannot write {}", target.display()));
        }
        Ok(())
    }

    fn merge(&mut self, disk: State) {
        if disk.list_view_changed_at > self.list_view_changed_at {
            self.list_view = disk.list_view;
            self.list_view_changed_at = disk.list_view_changed_at;
        }
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
                // Same save: a spoiler tag learned by either instance sticks.
                Some(mine) if b.saved_at == mine.saved_at => mine.post.spoiler |= b.post.spoiler,
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

    /// A subreddit's first load sets the baseline; only later posts count as new.
    pub fn baseline(&mut self, subreddit: &str, posts: &[Post], now: i64) {
        let key = subreddit.to_lowercase();
        if !self.seen.contains_key(&key) {
            self.mark_seen(subreddit, posts.iter().map(|p| p.id.as_str()), now);
        }
    }

    /// Refreshing the time keeps long-pinned posts from expiring and reappearing as new.
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

    /// Listed in a load of its subreddit, or opened: a post you read is never new.
    pub fn is_seen(&self, subreddit: &str, id: &str) -> bool {
        self.read.contains_key(id)
            || self
                .seen
                .get(&subreddit.to_lowercase())
                .is_some_and(|ids| ids.contains_key(id))
    }

    pub fn unseen<'a>(&self, subreddit: &str, posts: impl IntoIterator<Item = &'a Post>) -> usize {
        let seen = self.seen.get(&subreddit.to_lowercase());
        posts
            .into_iter()
            .filter(|p| !self.is_read(&p.id) && seen.is_none_or(|ids| !ids.contains_key(&p.id)))
            .count()
    }

    pub fn mark_read(&mut self, post: &Post, now: i64) {
        self.read.insert(post.id.clone(), now);
        // Only where a baseline exists: creating the list here would skip the first
        // load's baseline.
        if let Some(ids) = self.seen.get_mut(&post.subreddit.to_lowercase()) {
            ids.insert(post.id.clone(), now);
        }
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

    /// Tags a bookmark spoiler when its post page says so; the saved listing may predate
    /// the tag. Tags are only added (merges OR them); the rest of the copy is kept.
    pub fn mark_bookmark_spoiler(&mut self, id: &str) {
        if let Some(b) = self.bookmarks.iter_mut().find(|b| b.post.id == id) {
            b.post.spoiler = true;
        }
    }

    /// Adds or removes a bookmark; true when now bookmarked. Each event is stamped after
    /// the post's previous one, so a same-second remove and re-add still order correctly.
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
            permalink: format!("/r/{sub}/comments/{id}/x/"),
            media: Media::None,
            preview_size: None,
            stickied: false,
            nsfw: false,
            spoiler: false,
        }
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("redtab-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn when_older_state_has_no_view_it_should_default_to_cards() {
        let state: State = serde_json::from_str("{\"bookmarks\":[]}").unwrap();

        assert_eq!(state.list_view(), ListView::Card);
    }

    #[test]
    fn when_a_bookmark_has_preview_geometry_it_should_survive_state_serialization() {
        let mut saved = post("p", "s");
        saved.preview_size = Some((320, 640));
        let mut state = State::default();
        state.toggle_bookmark(&saved, 1);

        let actual: State = serde_json::from_value(serde_json::to_value(state).unwrap()).unwrap();

        assert_eq!(actual.bookmarks()[0].post.preview_size, Some((320, 640)));
    }

    #[test]
    fn when_a_saved_post_predates_preview_geometry_it_should_still_load() {
        let saved = post("p", "s");
        let mut value = serde_json::to_value(&saved).unwrap();
        value.as_object_mut().unwrap().remove("preview_size");

        let actual: Post = serde_json::from_value(value).unwrap();

        assert_eq!(actual, saved);
        assert_eq!(actual.preview_size, None);
    }

    #[test]
    fn when_an_unchanged_instance_saves_it_should_keep_the_latest_list_view() {
        let dir = scratch("list-view-東京");
        let path = dir.join("state.json");
        let mut first = State::default();
        first.save(&path, 1).unwrap();
        let (mut second, _) = State::load(&path);
        second.toggle_list_view();
        second.save(&path, 2).unwrap();

        first.save(&path, 3).unwrap();
        let (saved, warning) = State::load(&path);
        std::fs::remove_dir_all(dir).unwrap();

        assert!(warning.is_none());
        assert_eq!(saved.list_view(), ListView::Compact);
        assert_eq!(first.list_view(), ListView::Compact);
    }

    #[test]
    fn when_both_instances_change_view_the_latest_choice_should_survive_a_symlink_save() {
        let dir = scratch("list-view-merge");
        let path = dir.join("state.json");
        let link = dir.join("linked.json");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        let mut first = State::default();
        first.toggle_list_view();
        first.save(&link, 1).unwrap();
        let (mut second, _) = State::load(&link);
        first.toggle_list_view();
        first.toggle_list_view();
        second.toggle_list_view();
        second.list_view_changed_at = first.list_view_changed_at + 1;
        second.save(&link, 2).unwrap();

        first.save(&link, 3).unwrap();
        let (saved, warning) = State::load(&path);
        assert!(link.is_symlink());
        std::fs::remove_dir_all(dir).unwrap();
        assert!(warning.is_none());
        assert_eq!(saved.list_view(), ListView::Card);
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
    fn a_second_corrupt_file_keeps_the_first_backup() {
        let dir = scratch("bad2");
        let path = dir.join("state.json");
        std::fs::write(&path, b"first").unwrap();
        State::load(&path);
        std::fs::write(&path, b"second").unwrap();
        State::load(&path);
        let first = std::fs::read_to_string(dir.join("state.json.bad")).unwrap();
        let backups = std::fs::read_dir(&dir).unwrap().count();
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(first, "first");
        assert_eq!(backups, 2);
    }

    #[test]
    fn a_file_corrupted_during_the_session_is_not_overwritten() {
        let dir = scratch("midbad");
        let path = dir.join("state.json");
        let mut state = State::default();
        state.save(&path, 1).unwrap();
        std::fs::write(&path, b"{\"bookmarks\":[{\"post\":1}]}").unwrap();

        let saved = state.save(&path, 2);
        let on_disk = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();

        assert!(saved.is_err());
        assert_eq!(on_disk, "{\"bookmarks\":[{\"post\":1}]}");
    }

    #[test]
    fn saving_keeps_a_symlink_and_the_file_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("link");
        let real = dir.join("real.json");
        let link = dir.join("state.json");
        std::fs::write(&real, b"{}").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let mut state = State::default();
        state.toggle_bookmark(&post("a", "s"), 1);

        state.save(&link, 1).unwrap();
        let still_link = std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink();
        let mode = std::fs::metadata(&real).unwrap().permissions().mode() & 0o777;
        let (loaded, _) = State::load(&real);
        std::fs::remove_dir_all(&dir).unwrap();

        assert!(still_link);
        assert_eq!(mode, 0o600);
        assert!(loaded.is_bookmarked("a"));
    }

    #[test]
    fn a_dangling_or_corrupt_symlinked_state_file_keeps_the_link() {
        let dir = scratch("link2");
        let link = dir.join("state.json");
        std::os::unix::fs::symlink("synced.json", &link).unwrap();
        let mut state = State::default();
        state.toggle_bookmark(&post("a", "s"), 1);
        state.save(&link, 1).unwrap();
        let dangling_kept = std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink();
        let target_written = dir.join("synced.json").exists();

        std::fs::write(dir.join("synced.json"), b"garbage").unwrap();
        State::load(&link);
        let corrupt_kept = std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink();
        let backup = std::fs::read_to_string(dir.join("synced.json.bad")).ok();
        std::fs::remove_dir_all(&dir).unwrap();

        assert!(dangling_kept && target_written);
        assert!(corrupt_kept, "the link itself was moved aside");
        assert_eq!(backup.as_deref(), Some("garbage"));
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

    #[test]
    fn opening_a_post_before_its_subreddit_loads_keeps_the_first_load_baseline() {
        let mut state = State::default();
        let listing = [post("a", "pics"), post("b", "pics"), post("x", "pics")];
        let later = post("y", "pics");

        // Opened from r/popular before r/pics ever loaded; `y` is not on its
        // first page and shows up later.
        state.mark_read(&listing[2], 1);
        state.mark_read(&later, 1);
        state.baseline("pics", &listing, 2);

        assert!(state.is_read("x"));
        assert_eq!(
            state.unseen("pics", &listing),
            0,
            "the first load is the baseline"
        );
        assert_eq!(
            state.unseen("pics", [&later]),
            0,
            "a read post is never new"
        );
    }

    #[test]
    fn a_spoiler_tag_learned_by_one_instance_survives_the_others_save() {
        let dir = scratch("spoilmerge");
        let path = dir.join("state.json");
        let mut a = State::default();
        a.toggle_bookmark(&post("p", "s"), 5);
        a.save(&path, 5).unwrap();
        let (mut b, _) = State::load(&path);
        a.mark_bookmark_spoiler("p");

        a.save(&path, 6).unwrap();
        b.save(&path, 7).unwrap();
        let (loaded, _) = State::load(&path);
        std::fs::remove_dir_all(&dir).unwrap();

        assert!(loaded.bookmarks()[0].post.spoiler);
    }

    #[test]
    fn the_directory_lock_lets_only_one_saver_in_at_a_time() {
        let dir = scratch("dirlock");
        let target = dir.join("state.json");
        let counter = dir.join("counter");
        std::fs::write(&counter, "0").unwrap();
        let workers: Vec<_> = (0..2)
            .map(|_| {
                let (target, counter) = (target.clone(), counter.clone());
                std::thread::spawn(move || {
                    for _ in 0..10 {
                        let held = dir_lock(&target).unwrap();
                        // Read, pause, write: without mutual exclusion increments are lost.
                        let n: u32 = std::fs::read_to_string(&counter).unwrap().parse().unwrap();
                        std::thread::yield_now();
                        std::fs::write(&counter, (n + 1).to_string()).unwrap();
                        drop(held);
                        // Let the other thread in, so neither waits out LOCK_WAIT.
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                })
            })
            .collect();
        for w in workers {
            w.join().unwrap();
        }
        let total = std::fs::read_to_string(&counter).unwrap();
        let lock_left = target.with_extension("json.lockdir").exists();
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(total, "20");
        assert!(!lock_left, "the lock is removed when released");
    }

    fn age(path: &Path) {
        let old = std::time::SystemTime::now() - STALE_LOCK * 2;
        std::fs::File::open(path)
            .unwrap()
            .set_modified(old)
            .unwrap();
    }

    #[test]
    fn an_old_directory_lock_is_never_broken_while_its_holder_lives() {
        let dir = scratch("oldlock");
        let target = dir.join("state.json");
        let lockdir = target.with_extension("json.lockdir");
        let stalled = dir_lock(&target).unwrap();
        age(&lockdir); // a holder stalled past STALE_LOCK, or a crash's leftover

        let started = std::time::Instant::now();
        let refused = dir_lock(&target).err().map(|e| e.to_string());
        let waited = started.elapsed();
        let still_held = lockdir.exists();
        drop(stalled);
        let after_release = dir_lock(&target).is_ok();
        std::fs::remove_dir_all(&dir).unwrap();

        let message = refused.expect("a second holder must be refused");
        assert!(
            message.contains("if no redtab is running, delete it"),
            "{message}"
        );
        assert!(still_held, "the holder's lock stays");
        assert!(
            waited < LOCK_WAIT / 2,
            "no wait for a lock this old: {waited:?}"
        );
        assert!(after_release);
    }

    /// A lock in use gives up after `LOCK_WAIT`, leaves the lock alone and does not
    /// tell the reader to delete it.
    #[test]
    fn a_lock_in_use_gives_up_after_the_wait_and_stays() {
        let dir = scratch("freshlock");
        let target = dir.join("state.json");
        let held = dir_lock(&target).unwrap();

        let started = std::time::Instant::now();
        let refused = dir_lock(&target).err().map(|e| e.to_string());
        let waited = started.elapsed();
        let still_held = target.with_extension("json.lockdir").exists();
        drop(held);
        std::fs::remove_dir_all(&dir).unwrap();

        let message = refused.expect("a second holder must be refused");
        assert!(waited >= LOCK_WAIT && waited < LOCK_WAIT * 3, "{waited:?}");
        assert!(still_held);
        assert!(!message.contains("delete"), "{message}");
    }

    #[test]
    fn flock_errors_choose_wait_fallback_or_fail() {
        assert_eq!(flock_failure(Some(libc::EWOULDBLOCK)), FlockFailure::Busy);
        assert_eq!(flock_failure(Some(libc::EINTR)), FlockFailure::Busy);
        for unsupported in [libc::ENOTSUP, libc::EOPNOTSUPP, libc::ENOLCK, libc::ENOSYS] {
            assert_eq!(flock_failure(Some(unsupported)), FlockFailure::Unsupported);
        }
        assert_eq!(flock_failure(Some(libc::EBADF)), FlockFailure::Error);
        assert_eq!(flock_failure(None), FlockFailure::Error);
    }

    #[test]
    fn a_save_gives_up_when_another_instance_holds_the_lock() {
        let dir = scratch("busy");
        let path = dir.join("state.json");
        let held = lock(&path).unwrap();
        let mut state = State::default();
        state.toggle_bookmark(&post("a", "s"), 1);

        let started = std::time::Instant::now();
        let saved = state.save(&path, 1);
        let waited = started.elapsed();
        drop(held);
        let after = state.save(&path, 1);
        std::fs::remove_dir_all(&dir).unwrap();

        assert!(saved.unwrap_err().to_string().contains("still saving"));
        assert!(waited >= LOCK_WAIT && waited < LOCK_WAIT * 3, "{waited:?}");
        assert!(after.is_ok(), "the lock is free again");
    }

    #[test]
    fn saving_through_a_chain_of_dangling_links_keeps_every_link() {
        let dir = scratch("chain");
        std::fs::create_dir(dir.join("real")).unwrap();
        std::os::unix::fs::symlink("hop.json", dir.join("state.json")).unwrap();
        std::os::unix::fs::symlink("real/state.json", dir.join("hop.json")).unwrap();
        let mut state = State::default();
        state.toggle_bookmark(&post("a", "s"), 1);

        state.save(&dir.join("state.json"), 1).unwrap();
        let is_link = |p: &str| {
            std::fs::symlink_metadata(dir.join(p))
                .unwrap()
                .file_type()
                .is_symlink()
        };
        let links = (is_link("state.json"), is_link("hop.json"));
        let written = dir.join("real/state.json").is_file();
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(links, (true, true));
        assert!(written);
    }

    #[test]
    fn simultaneous_saves_from_two_instances_lose_no_bookmark() {
        let dir = scratch("race");
        let path = dir.join("state.json");
        let savers: Vec<_> = ["a", "b"]
            .into_iter()
            .map(|who| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let mut state = State::default();
                    for i in 0..60 {
                        state.toggle_bookmark(&post(&format!("{who}{i}"), "s"), 1);
                        state.save(&path, 1).unwrap();
                    }
                })
            })
            .collect();
        for s in savers {
            s.join().unwrap();
        }

        let (loaded, _) = State::load(&path);
        let lost: Vec<String> = ["a", "b"]
            .iter()
            .flat_map(|who| (0..60).map(move |i| format!("{who}{i}")))
            .filter(|id| !loaded.is_bookmarked(id))
            .collect();
        std::fs::remove_dir_all(&dir).unwrap();

        assert!(lost.is_empty(), "lost {} bookmarks: {lost:?}", lost.len());
    }
}
