use super::*;

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
                self.merge(disk, now);
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

    pub(super) fn merge(&mut self, disk: State, now: i64) {
        for bookmark in self
            .bookmarks
            .iter()
            .chain(&disk.bookmarks)
            .filter(|bookmark| bookmark.post.original_deleted)
        {
            self.deleted_originals
                .entry(bookmark.post.id.clone())
                .and_modify(|observed| *observed = (*observed).max(now))
                .or_insert(now);
        }
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
        for (id, observed_at) in disk.deleted_originals {
            self.deleted_originals
                .entry(id)
                .and_modify(|mine| *mine = (*mine).max(observed_at))
                .or_insert(observed_at);
        }
        for b in disk.bookmarks {
            match self.bookmarks.iter_mut().find(|m| m.post.id == b.post.id) {
                Some(mine) if b.saved_at > mine.saved_at => *mine = b,
                Some(mine) if b.saved_at == mine.saved_at => mine.post.spoiler |= b.post.spoiler,
                Some(_) => {}
                None => self.bookmarks.push(b),
            }
        }
        // The latest event wins: a removal newer than the save drops it.
        let removed = &self.removed;
        self.bookmarks
            .retain(|b| removed.get(&b.post.id).is_none_or(|&t| t < b.saved_at));
        for bookmark in &mut self.bookmarks {
            bookmark.post.original_deleted |=
                self.deleted_originals.contains_key(&bookmark.post.id);
        }
        self.bookmarks
            .sort_by_key(|b| std::cmp::Reverse(b.saved_at));
    }

    pub(super) fn prune(&mut self, now: i64) {
        let cutoff = now - RETENTION_SECS;
        self.deleted_originals
            .retain(|_, observed_at| *observed_at >= cutoff);
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
}
