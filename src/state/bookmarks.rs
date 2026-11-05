use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bookmark {
    pub post: Post,
    pub saved_at: i64,
}

impl State {
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

    pub fn mark_original_deleted(&mut self, id: &str, observed_at: i64) {
        self.deleted_originals
            .entry(id.into())
            .and_modify(|mine| *mine = (*mine).max(observed_at))
            .or_insert(observed_at);
        if let Some(bookmark) = self.bookmarks.iter_mut().find(|b| b.post.id == id) {
            bookmark.post.original_deleted = true;
        }
    }

    pub fn original_deleted(&self, id: &str) -> bool {
        self.deleted_originals.contains_key(id)
            || self
                .bookmarks
                .iter()
                .any(|bookmark| bookmark.post.id == id && bookmark.post.original_deleted)
    }

    /// Adds or removes a bookmark; true when now bookmarked. Each event is stamped after
    /// the post's previous one, so a same-second remove and re-add still order correctly.
    pub fn toggle_bookmark(&mut self, post: &Post, now: i64) -> bool {
        let original_deleted = post.original_deleted || self.original_deleted(&post.id);
        if original_deleted {
            self.mark_original_deleted(&post.id, now);
        }
        if let Some(i) = self.bookmarks.iter().position(|b| b.post.id == post.id) {
            let saved_at = self.bookmarks.remove(i).saved_at;
            self.removed.insert(post.id.clone(), now.max(saved_at + 1));
            false
        } else {
            let mut post = post.clone();
            post.original_deleted = original_deleted;
            let after = self
                .removed
                .remove(&post.id)
                .map_or(now, |t| now.max(t + 1));
            self.bookmarks.insert(
                0,
                Bookmark {
                    post,
                    saved_at: after,
                },
            );
            true
        }
    }
}
