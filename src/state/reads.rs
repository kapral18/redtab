use super::*;

#[derive(Debug, Default, Serialize, Deserialize)]
pub(super) struct ObservedCommunity {
    pub(super) observed_at: i64,
    pub(super) posts: HashMap<String, ObservedPost>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct ObservedPost {
    pub(super) title: String,
    pub(super) author: String,
    pub(super) flair: Option<String>,
    pub(super) nsfw: bool,
    pub(super) fetched_at: i64,
    pub(super) observed_at: i64,
    pub(super) source: Source,
}

impl ObservedPost {
    pub(super) fn compare_metadata(&self, incoming: Freshness) -> std::cmp::Ordering {
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
pub(super) struct UnreadCounts {
    pub(super) config: Config,
    pub(super) counts: HashMap<String, usize>,
}

impl State {
    /// Remember fetched identities independently of the listing currently displayed.
    /// An empty successful listing establishes known zero; failures never call this.
    pub fn observe(
        &mut self,
        subreddit: Option<&str>,
        posts: &[Post],
        freshness: Freshness,
        now: i64,
    ) {
        for post in posts.iter().filter(|post| post.original_deleted) {
            self.mark_original_deleted(&post.id, now);
        }
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
        if let Some(key) = post.community().map(str::to_lowercase) {
            let community = self.observed.entry(key.clone()).or_default();
            community.observed_at = community.observed_at.max(now);
            if let Some(cache) = self.unread_counts.get_mut() {
                cache.counts.entry(key).or_default();
            }
        }
        self.mark_read_id(&post.id, now);
    }

    /// Marks a post read by id alone, for a post no longer at hand.
    pub fn mark_read_id(&mut self, id: &str, now: i64) {
        let was_read = self.read.contains_key(id);
        if !was_read {
            self.read_revision = self.read_revision.wrapping_add(1);
        }
        self.read
            .entry(id.to_owned())
            .and_modify(|time| *time = (*time).max(now))
            .or_insert(now);
        if was_read {
            return;
        }
        for (subreddit, community) in &mut self.observed {
            let Some(old) = community.posts.remove(id) else {
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
}
