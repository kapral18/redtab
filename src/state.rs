//! Reading history and bookmarks in `$XDG_STATE_HOME/redtab/state.json`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::cache::{Freshness, Source};
use crate::config::Config;
use crate::model::Post;

/// Observations and read marks not refreshed for this long are dropped on save.
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
    /// Compact records from fetched listings; old visit-based `seen` marks are ignored.
    #[serde(default)]
    observed: HashMap<String, ObservedCommunity>,
    /// Post id → last refresh of its opened/read mark.
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
    #[serde(skip)]
    unread_counts: RefCell<Option<UnreadCounts>>,
    #[serde(skip)]
    read_revision: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ObservedCommunity {
    observed_at: i64,
    posts: HashMap<String, ObservedPost>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ObservedPost {
    title: String,
    author: String,
    flair: Option<String>,
    nsfw: bool,
    fetched_at: i64,
    observed_at: i64,
    source: Source,
}

impl ObservedPost {
    fn compare_metadata(&self, incoming: Freshness) -> std::cmp::Ordering {
        use std::cmp::Ordering::{Greater, Less};
        if let Some(order) = incoming.source.compare_requests(self.source) {
            return order.then_with(|| self.source.cached.cmp(&incoming.source.cached));
        }
        // A live refresh supersedes history from an incomparable cache generation.
        // An unordered disk read cannot roll back this process's live observation.
        if incoming.source.cached && !self.source.cached && self.source.observed.is_some() {
            return Less;
        }
        if incoming.source.cached {
            incoming.fetched_at.cmp(&self.fetched_at)
        } else {
            Greater
        }
    }
}

#[derive(Debug)]
struct UnreadCounts {
    config: Config,
    counts: HashMap<String, usize>,
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
        Self::load_file(&resolve_link(path), false)
    }

    fn load_file(path: &Path, locked: bool) -> (Self, Option<String>) {
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
                if !locked {
                    let _lock = match lock(path) {
                        Ok(lock) => lock,
                        Err(lock) => {
                            return (
                                Self {
                                    read_only: true,
                                    ..Self::default()
                                },
                                Some(format!(
                                    "state file was invalid ({e}) and could not be locked ({lock}); not saving"
                                )),
                            );
                        }
                    };
                    // Another instance may have recovered and saved since the first read.
                    return Self::load_file(path, true);
                }
                // Never overwrite an earlier backup.
                match move_corrupt_state(path) {
                    Ok(aside) => (
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

fn move_corrupt_state(path: &Path) -> std::io::Result<std::path::PathBuf> {
    let mut aside = path.with_extension("json.bad");
    loop {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&aside)
        {
            Ok(reservation) => {
                drop(reservation);
                if let Err(error) = std::fs::rename(path, &aside) {
                    let _ = std::fs::remove_file(&aside);
                    return Err(error);
                }
                return Ok(aside);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                aside = path.with_extension(format!("json.bad-{}", uuid::Uuid::new_v4()));
            }
            Err(error) => return Err(error),
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
        self.unread_counts.get_mut().take();
        for (id, t) in disk.read {
            if !self.read.contains_key(&id) {
                self.read_revision = self.read_revision.wrapping_add(1);
            }
            self.read
                .entry(id)
                .and_modify(|x| *x = (*x).max(t))
                .or_insert(t);
        }
        for (sub, community) in disk.observed {
            let mine = self.observed.entry(sub).or_default();
            mine.observed_at = mine.observed_at.max(community.observed_at);
            for (id, mut incoming) in community.posts {
                if self.read.contains_key(&id) {
                    continue;
                }
                match mine.posts.get_mut(&id) {
                    Some(existing) => {
                        let observed_at = existing.observed_at.max(incoming.observed_at);
                        if existing
                            .compare_metadata(Freshness {
                                fetched_at: incoming.fetched_at,
                                source: Source {
                                    cached: true,
                                    ..incoming.source
                                },
                            })
                            .is_gt()
                        {
                            incoming.observed_at = observed_at;
                            *existing = incoming;
                        } else {
                            existing.observed_at = observed_at;
                        }
                    }
                    None => {
                        mine.posts.insert(id, incoming);
                    }
                }
            }
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
        self.unread_counts.get_mut().take();
        for community in self.observed.values_mut() {
            community
                .posts
                .retain(|id, post| post.observed_at >= cutoff && !self.read.contains_key(id));
        }
        self.observed
            .retain(|_, community| community.observed_at >= cutoff);
        let read_before = self.read.len();
        self.read.retain(|_, t| *t >= cutoff);
        if self.read.len() != read_before {
            self.read_revision = self.read_revision.wrapping_add(1);
        }
        self.removed.retain(|_, t| *t >= cutoff);
    }

    /// Remember fetched identities independently of the listing currently displayed.
    /// An empty successful listing establishes known zero; failures never call this.
    pub fn observe(
        &mut self,
        subreddit: Option<&str>,
        posts: &[Post],
        freshness: Freshness,
        now: i64,
    ) {
        let cache = self.unread_counts.get_mut();
        if let Some(sub) = subreddit {
            let key = sub.to_lowercase();
            let community = self.observed.entry(key.clone()).or_default();
            community.observed_at = community.observed_at.max(now);
            if let Some(cache) = cache {
                cache.counts.entry(key).or_default();
            }
        }
        for post in posts {
            if post.community().is_none() {
                if let Some(read) = self.read.get_mut(&post.id) {
                    *read = (*read).max(now);
                }
                continue;
            }
            let key = post.subreddit.to_lowercase();
            let community = self.observed.entry(key.clone()).or_default();
            community.observed_at = community.observed_at.max(now);
            if let Some(cache) = cache {
                cache.counts.entry(key.clone()).or_default();
            }
            if let Some(read) = self.read.get_mut(&post.id) {
                *read = (*read).max(now);
                community.posts.remove(&post.id);
                continue;
            }
            let old = community.posts.get_mut(&post.id);
            let old = if let Some(old) = old {
                old.observed_at = old.observed_at.max(now);
                if old.compare_metadata(freshness).is_lt() {
                    continue;
                }
                Some(old)
            } else {
                None
            };
            if let Some(cache) = cache {
                let before = old.as_ref().is_some_and(|old| {
                    !cache.config.hides_metadata(
                        &old.title,
                        &old.author,
                        old.flair.as_deref(),
                        old.nsfw,
                    )
                });
                let after = !cache.config.hides(post);
                let count = cache.counts.entry(key).or_default();
                match (before, after) {
                    (false, true) => *count += 1,
                    (true, false) => *count -= 1,
                    _ => {}
                }
            }
            if let Some(old) = old {
                old.title.clone_from(&post.title);
                old.author.clone_from(&post.author);
                old.flair.clone_from(&post.flair);
                old.nsfw = post.nsfw;
                old.fetched_at = freshness.fetched_at;
                old.source = freshness.source;
            } else {
                community.posts.insert(
                    post.id.clone(),
                    ObservedPost {
                        title: post.title.clone(),
                        author: post.author.clone(),
                        flair: post.flair.clone(),
                        nsfw: post.nsfw,
                        fetched_at: freshness.fetched_at,
                        observed_at: now,
                        source: freshness.source,
                    },
                );
            }
        }
    }

    /// Count unread fetched posts using the current filters. None means no retained observation.
    pub fn unread(&self, subreddit: &str, config: &Config) -> Option<usize> {
        let mut cache = self.unread_counts.borrow_mut();
        if cache.as_ref().is_none_or(|cache| {
            cache.config.filters != config.filters || cache.config.show_nsfw != config.show_nsfw
        }) {
            let counts = self
                .observed
                .iter()
                .map(|(sub, community)| {
                    let count = community
                        .posts
                        .iter()
                        .filter(|(id, post)| {
                            !self.read.contains_key(*id)
                                && !config.hides_metadata(
                                    &post.title,
                                    &post.author,
                                    post.flair.as_deref(),
                                    post.nsfw,
                                )
                        })
                        .count();
                    (sub.clone(), count)
                })
                .collect();
            *cache = Some(UnreadCounts {
                config: config.clone(),
                counts,
            });
        }
        cache
            .as_ref()
            .unwrap()
            .counts
            .get(&subreddit.to_lowercase())
            .copied()
    }

    pub fn mark_read(&mut self, post: &Post, now: i64) {
        let was_read = self.read.contains_key(&post.id);
        if !was_read {
            self.read_revision = self.read_revision.wrapping_add(1);
        }
        self.read
            .entry(post.id.clone())
            .and_modify(|time| *time = (*time).max(now))
            .or_insert(now);
        if let Some(key) = post.community().map(str::to_lowercase) {
            let community = self.observed.entry(key.clone()).or_default();
            community.observed_at = community.observed_at.max(now);
            if let Some(cache) = self.unread_counts.get_mut() {
                cache.counts.entry(key).or_default();
            }
        }
        if was_read {
            return;
        }
        for (subreddit, community) in &mut self.observed {
            let Some(old) = community.posts.remove(&post.id) else {
                continue;
            };
            if let Some(cache) = self.unread_counts.get_mut()
                && !cache.config.hides_metadata(
                    &old.title,
                    &old.author,
                    old.flair.as_deref(),
                    old.nsfw,
                )
            {
                *cache.counts.get_mut(subreddit).unwrap() -= 1;
            }
        }
    }

    pub fn read_revision(&self) -> u64 {
        self.read_revision
    }

    /// Clear known fetched IDs, without a cutoff that would hide future arrivals.
    pub fn mark_community_read(&mut self, subreddit: &str, now: i64) -> usize {
        let Some(community) = self.observed.get_mut(&subreddit.to_lowercase()) else {
            return 0;
        };
        community.observed_at = community.observed_at.max(now);
        let mut count = 0;
        for (id, _) in std::mem::take(&mut community.posts) {
            self.read
                .entry(id)
                .and_modify(|time| *time = (*time).max(now))
                .or_insert_with(|| {
                    count += 1;
                    now
                });
        }
        if count > 0 {
            self.read_revision = self.read_revision.wrapping_add(1);
            self.unread_counts.get_mut().take();
            for community in self.observed.values_mut() {
                community.posts.retain(|id, _| !self.read.contains_key(id));
            }
        }
        count
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

    fn cached(fetched_at: i64) -> Freshness {
        Freshness {
            fetched_at,
            source: Source {
                cached: true,
                ..Source::default()
            },
        }
    }

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
    fn fetched_posts_remain_unread_when_later_listings_omit_them() {
        let mut state = State::default();
        let config = Config::default();
        state.observe(
            Some("Neovim"),
            &[post("a", "Neovim"), post("b", "Neovim")],
            cached(100),
            100,
        );
        assert_eq!(state.unread("neovim", &config), Some(2));

        state.observe(
            Some("neovim"),
            &[post("c", "neovim"), post("a", "neovim")],
            cached(200),
            200,
        );

        assert_eq!(state.unread("NEOVIM", &config), Some(3));
    }

    #[test]
    fn reading_a_post_clears_only_that_post() {
        let mut state = State::default();
        state.observe(
            Some("vim"),
            &[post("x", "vim"), post("y", "vim")],
            cached(1),
            1,
        );
        assert_eq!(state.unread("vim", &Config::default()), Some(2));

        state.mark_read(&post("x", "vim"), 5);

        assert!(state.is_read("x"));
        assert!(!state.is_read("y"));
        assert_eq!(state.unread("vim", &Config::default()), Some(1));
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
    fn observing_a_read_pinned_post_keeps_its_read_mark_from_expiring() {
        let mut state = State::default();
        state.mark_read(&post("pinned", "s"), 0);
        state.observe(
            Some("s"),
            &[post("pinned", "s")],
            cached(RETENTION_SECS),
            RETENTION_SECS,
        );

        state.prune(RETENTION_SECS + 10);

        assert!(state.is_read("pinned"));
        assert_eq!(state.unread("s", &Config::default()), Some(0));
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
        let backups = std::fs::read_dir(&dir)
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("state.json.bad")
            })
            .count();
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(first, "first");
        assert_eq!(backups, 2);
    }

    #[test]
    fn repeated_corrupt_state_recovery_should_keep_every_backup() {
        let dir = scratch("bad-repeat");
        let path = dir.join("state.json");
        for text in ["first", "second", "third", "fourth"] {
            std::fs::write(&path, text).unwrap();
            let (_, warning) = State::load(&path);
            assert!(warning.unwrap().contains("moved to"));
        }
        let mut backups: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("state.json.bad")
            })
            .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
            .collect();
        backups.sort();
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(backups, ["first", "fourth", "second", "third"]);
    }

    #[test]
    fn when_another_instance_holds_the_save_lock_corrupt_recovery_should_leave_the_file_alone() {
        let dir = scratch("bad-locked");
        let path = dir.join("state.json");
        std::fs::write(&path, "corrupt").unwrap();
        let held = lock(&path).unwrap();

        let (state, warning) = State::load(&path);

        assert!(state.read_only);
        assert!(warning.unwrap().contains("another redtab"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "corrupt");
        drop(held);
        std::fs::remove_dir_all(&dir).unwrap();
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
    fn reading_before_observation_never_resurrects_an_unread_post() {
        let mut state = State::default();
        state.mark_read(&post("x", "pics"), 1);
        state.mark_read(&post("y", "pics"), 1);

        state.observe(
            Some("pics"),
            &[post("a", "pics"), post("b", "pics"), post("x", "pics")],
            cached(2),
            2,
        );
        state.observe(Some("pics"), &[post("y", "pics")], cached(3), 3);

        assert_eq!(state.unread("pics", &Config::default()), Some(2));
        assert!(state.is_read("x"));
        assert!(state.is_read("y"));
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
    #[test]
    fn unread_distinguishes_unknown_from_a_successful_empty_listing() {
        let mut state = State::default();
        let config = Config::default();
        assert_eq!(state.unread("s", &config), None);
        state.observe(Some("s"), &[], cached(1), 1);
        assert_eq!(state.unread("S", &config), Some(0));
        state.mark_read(&post("read", "other"), 2);
        assert_eq!(state.unread("other", &config), Some(0));
    }

    #[test]
    fn legacy_visit_marks_do_not_hide_unopened_posts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(
            &path,
            r#"{"seen":{"s":{"opened":100,"unopened":100}},"read":{"opened":100}}"#,
        )
        .unwrap();
        let (mut state, warning) = State::load(&path);
        assert!(warning.is_none());

        state.observe(
            Some("s"),
            &[post("opened", "s"), post("unopened", "s")],
            cached(101),
            101,
        );
        state.save(&path, 101).unwrap();

        assert_eq!(state.unread("s", &Config::default()), Some(1));
        assert!(state.is_read("opened"));
        assert!(!state.is_read("unopened"));
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert!(saved.get("seen").is_none());
        assert!(saved.get("unread_counts").is_none());
        assert!(saved.get("read_revision").is_none());
    }

    #[test]
    fn unread_filter_cache_tracks_both_directions_of_every_filter() {
        let mut state = State::default();
        let mut title = post("title", "s");
        title.title = "Blocked title".into();
        let mut author = post("author", "s");
        author.author = "FilteredAuthor".into();
        let mut flair = post("flair", "s");
        flair.flair = Some("FilteredFlair".into());
        let mut adult = post("adult", "s");
        adult.nsfw = true;
        state.observe(Some("s"), &[title, author, flair, adult], cached(1), 1);
        let mut config = Config::default();
        assert_eq!(state.unread("s", &config), Some(3));
        config.show_nsfw = true;
        assert_eq!(state.unread("s", &config), Some(4));
        config.filters.keywords = vec!["BLOCKED".into()];
        assert_eq!(state.unread("s", &config), Some(3));
        config.filters.keywords.clear();
        config.filters.authors = vec!["filteredauthor".into()];
        assert_eq!(state.unread("s", &config), Some(3));
        config.filters.authors.clear();
        config.filters.flairs = vec!["filteredflair".into()];
        assert_eq!(state.unread("s", &config), Some(3));
        config.filters.flairs.clear();
        config.show_nsfw = false;
        assert_eq!(state.unread("s", &config), Some(3));
    }

    #[test]
    fn merging_an_older_instance_preserves_reads_and_new_observations() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let mut first = State::default();
        first.observe(
            Some("s"),
            &[post("a", "s"), post("b", "s")],
            cached(100),
            100,
        );
        first.save(&path, 100).unwrap();
        let (mut second, _) = State::load(&path);
        assert_eq!(second.unread("s", &Config::default()), Some(2));
        first.mark_read(&post("a", "s"), 101);
        first.save(&path, 101).unwrap();
        second.observe(
            Some("s"),
            &[post("a", "s"), post("c", "s")],
            cached(100),
            102,
        );

        second.save(&path, 103).unwrap();
        first.save(&path, 104).unwrap();

        assert!(second.is_read("a"));
        assert_eq!(second.unread("s", &Config::default()), Some(2));
        assert_eq!(first.unread("s", &Config::default()), Some(2));
        assert_eq!(
            State::load(&path).0.unread("s", &Config::default()),
            Some(2)
        );
    }

    #[test]
    fn older_cached_metadata_cannot_replace_a_newer_filter_observation() {
        let mut state = State::default();
        let safe = post("a", "s");
        let mut adult = safe.clone();
        adult.nsfw = true;
        let config = Config::default();
        state.observe(Some("s"), &[safe], cached(200), 200);
        assert_eq!(state.unread("s", &config), Some(1));

        state.observe(Some("s"), std::slice::from_ref(&adult), cached(100), 300);
        assert_eq!(state.unread("s", &config), Some(1));
        state.observe(Some("s"), &[adult], cached(400), 400);
        assert_eq!(state.unread("s", &config), Some(0));
    }

    #[test]
    fn unread_history_prunes_only_expired_observations() {
        let mut state = State::default();
        state.observe(Some("expired"), &[post("old", "expired")], cached(0), 0);
        state.observe(
            Some("recent"),
            &[post("new", "recent")],
            cached(RETENTION_SECS),
            RETENTION_SECS,
        );

        state.prune(RETENTION_SECS + 1);

        assert_eq!(state.unread("expired", &Config::default()), None);
        assert_eq!(state.unread("recent", &Config::default()), Some(1));
        assert!(!state.is_read("new"));
    }
    #[test]
    fn duplicate_observations_and_global_read_ids_keep_cached_counts_consistent() {
        let mut state = State::default();
        let config = Config::default();
        state.observe(
            None,
            &[post("a", "s"), post("a", "s"), post("a", "other")],
            cached(1),
            1,
        );
        assert_eq!(state.unread("s", &config), Some(1));
        assert_eq!(state.unread("other", &config), Some(1));

        state.mark_read(&post("a", "s"), 2);
        state.observe(None, &[post("a", "s"), post("a", "other")], cached(3), 3);

        assert_eq!(state.unread("s", &config), Some(0));
        assert_eq!(state.unread("other", &config), Some(0));
    }
    fn ordered_freshness(sequence: u64, fetched_at: i64, cached: bool) -> Freshness {
        Freshness {
            fetched_at,
            source: Source {
                order: Some(crate::cache::RequestOrder {
                    generation: [1; 16],
                    sequence,
                }),
                observed: Some((sequence, sequence)),
                cached,
            },
        }
    }

    #[test]
    fn observation_metadata_keeps_request_order_across_save_merge_and_clock_rollback() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let mut newest = State::default();
        newest.observe(
            Some("s"),
            &[post("a", "s")],
            ordered_freshness(2, 1000, false),
            1000,
        );
        newest.save(&path, 1000).unwrap();
        let mut old = State::default();
        let mut adult = post("a", "s");
        adult.nsfw = true;
        old.observe(
            Some("s"),
            std::slice::from_ref(&adult),
            ordered_freshness(1, 2000, false),
            2000,
        );
        old.save(&path, 2000).unwrap();
        assert_eq!(old.unread("s", &Config::default()), Some(1));
        newest.observe(Some("s"), &[adult], ordered_freshness(3, 900, false), 2001);
        newest.save(&path, 2001).unwrap();
        old.save(&path, 2002).unwrap();
        assert_eq!(old.unread("s", &Config::default()), Some(0));
        let (loaded, warning) = State::load(&path);
        assert!(warning.is_none());
        assert_eq!(loaded.unread("s", &Config::default()), Some(0));
        let record = &loaded.observed["s"].posts["a"];
        assert_eq!(record.source.order.unwrap().sequence, 3);
        assert!(
            record.source.observed.is_none(),
            "process-local events never persist"
        );
    }

    #[test]
    fn unordered_cached_metadata_cannot_replace_a_current_live_observation() {
        let mut state = State::default();
        let mut adult = post("a", "s");
        adult.nsfw = true;
        state.observe(Some("s"), &[adult], ordered_freshness(3, 900, false), 1000);
        let disk = Freshness {
            fetched_at: 2000,
            source: Source {
                cached: true,
                observed: Some((10, 11)),
                ..Source::default()
            },
        };
        state.observe(Some("s"), &[post("a", "s")], disk, 1001);
        assert_eq!(state.unread("s", &Config::default()), Some(0));
    }
    #[test]
    fn merging_an_incomparable_disk_source_cannot_roll_back_a_current_live_observation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let mut state = State::default();
        let mut old = post("a", "s");
        old.nsfw = true;
        state.observe(Some("s"), &[old], ordered_freshness(1, 1000, false), 1000);
        state.save(&path, 1000).unwrap();
        let mut new_source = ordered_freshness(2, 900, false);
        new_source.source.order.as_mut().unwrap().generation = [2; 16];
        state.observe(Some("s"), &[post("a", "s")], new_source, 1001);
        assert_eq!(state.unread("s", &Config::default()), Some(1));
        state.save(&path, 1001).unwrap();
        assert_eq!(state.unread("s", &Config::default()), Some(1));
    }
    #[test]
    fn equivalent_disk_snapshots_cannot_discard_live_metadata_ordering() {
        for save in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("state.json");
            let mut state = State::default();
            let mut adult = post("a", "s");
            adult.nsfw = true;
            state.observe(
                Some("s"),
                std::slice::from_ref(&adult),
                ordered_freshness(3, 900, false),
                1000,
            );
            if save {
                state.save(&path, 1000).unwrap();
                state.save(&path, 1001).unwrap();
            } else {
                state.observe(Some("s"), &[adult], ordered_freshness(3, 900, true), 1001);
            }
            let old_disk = Freshness {
                fetched_at: 2000,
                source: Source {
                    cached: true,
                    observed: Some((10, 11)),
                    ..Source::default()
                },
            };
            state.observe(Some("s"), &[post("a", "s")], old_disk, 1002);
            assert_eq!(
                state.unread("s", &Config::default()),
                Some(0),
                "save={save}"
            );
        }
    }
    #[test]
    fn marking_a_community_read_should_clear_filtered_ids_and_keep_later_arrivals_unread() {
        let mut state = State::default();
        let mut adult = post("adult", "s");
        adult.nsfw = true;
        state.observe(Some("s"), &[post("a", "s"), adult], cached(100), 100);
        state.observe(Some("other"), &[post("other", "other")], cached(100), 100);
        assert_eq!(state.unread("s", &Config::default()), Some(1));

        let actual = state.mark_community_read("S", 101);

        assert_eq!(actual, 2);
        assert!(state.is_read("a") && state.is_read("adult"));
        assert_eq!(state.unread("s", &Config::default()), Some(0));
        assert_eq!(state.unread("other", &Config::default()), Some(1));
        state.observe(
            Some("s"),
            &[post("a", "s"), post("new", "s")],
            cached(102),
            102,
        );
        assert_eq!(state.unread("s", &Config::default()), Some(1));
        assert_eq!(state.mark_community_read("s", 103), 1);
        assert_eq!(state.mark_community_read("s", 104), 0);
    }

    #[test]
    fn community_read_marks_should_survive_an_older_instances_save_with_new_arrivals() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let mut first = State::default();
        first.observe(
            Some("s"),
            &[post("a", "s"), post("b", "s")],
            cached(100),
            100,
        );
        first.save(&path, 100).unwrap();
        let (mut second, _) = State::load(&path);
        first.mark_community_read("s", 101);
        first.save(&path, 101).unwrap();
        second.observe(
            Some("s"),
            &[post("a", "s"), post("c", "s")],
            cached(102),
            102,
        );

        second.save(&path, 102).unwrap();

        let (actual, warning) = State::load(&path);
        assert!(warning.is_none());
        assert!(actual.is_read("a") && actual.is_read("b"));
        assert!(!actual.is_read("c"));
        assert_eq!(actual.unread("s", &Config::default()), Some(1));
    }

    #[test]
    fn marking_an_unfetched_community_read_should_preserve_unknown() {
        let mut state = State::default();

        let actual = state.mark_community_read("unknown", 100);

        assert_eq!(actual, 0);
        assert_eq!(state.unread("unknown", &Config::default()), None);
    }
    #[test]
    fn profile_owned_posts_should_record_reads_without_creating_a_fake_community_badge() {
        let mut state = State::default();
        let mut profile = post("p", "u_blackvault");
        profile.permalink = "/user/blackvault/comments/p/title/".into();
        state.observe(None, std::slice::from_ref(&profile), cached(100), 100);
        assert_eq!(state.unread("u_blackvault", &Config::default()), None);
        state.mark_read(&profile, 101);
        assert!(state.is_read("p"));
        assert_eq!(state.unread("u_blackvault", &Config::default()), None);
    }
}
