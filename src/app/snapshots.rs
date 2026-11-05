use super::*;

impl App {
    pub(super) fn on_snapshot(&mut self, message: Msg, freshness: Freshness, refreshing: bool) {
        match message {
            Msg::Experience { generation, result } => {
                self.on_experience(generation, result, freshness)
            }
            Msg::Search { generation, result } => self.on_search(generation, result),
            Msg::FeedFallback {
                tab,
                generation,
                listing,
                cursor,
                error,
            } => self.on_feed_fallback(tab, generation, listing, cursor, error, freshness),
            Msg::Subscriptions { generation, result } => {
                self.on_subscriptions(generation, result, freshness, refreshing)
            }
            Msg::Feed {
                tab,
                generation,
                append,
                result,
            } => {
                self.on_feed(tab, generation, append, result, freshness, refreshing);
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab)
                    && tab.feed.accepts(generation)
                    && refreshing
                {
                    tab.feed.refreshing.insert(generation);
                }
            }
            Msg::Group {
                subreddit,
                generation,
                result,
            } => {
                if !self
                    .all_subs
                    .groups
                    .iter()
                    .any(|g| g.subreddit == subreddit && g.generation == generation)
                {
                    return;
                }
                self.on_group(&subreddit, generation, result, freshness, refreshing);
            }
            Msg::Post { generation, result } => {
                let succeeded = result.is_ok();
                self.on_result(
                    Msg::Post { generation, result },
                    freshness.source,
                    refreshing,
                );
                if let Some(view) = &mut self.post_view
                    && view.generation == generation
                {
                    if succeeded {
                        view.freshness = Some(freshness);
                    }
                    view.loading = refreshing;
                }
            }
            Msg::MoreReplies {
                generation,
                comment_id,
                result,
            } => {
                // The post may have refreshed after this expansion was submitted.
                let revalidate = if freshness.source.cached && !refreshing {
                    self.post_view
                        .as_mut()
                        .filter(|view| view.generation == generation)
                        .and_then(|view| view.page.as_mut())
                        .and_then(|page| find_comment_mut(&mut page.comments, &comment_id))
                        .and_then(|comment| {
                            let minimum = comment.sources?.more;
                            let more = comment.more.as_ref()?;
                            (!freshness.source.may_replace(minimum))
                                .then(|| (more.cursor.clone(), minimum))
                        })
                } else {
                    None
                };
                let refreshing = refreshing || revalidate.is_some();
                if let Some((cursor, minimum)) = revalidate {
                    self.pool.refresh(Job::MoreReplies {
                        generation,
                        comment_id: comment_id.clone(),
                        cursor,
                        minimum: Some(minimum),
                    });
                }
                self.on_result(
                    Msg::MoreReplies {
                        generation,
                        comment_id: comment_id.clone(),
                        result,
                    },
                    freshness.source,
                    refreshing,
                );
                if refreshing
                    && let Some(view) = &mut self.post_view
                    && view.generation == generation
                {
                    view.loading_more.insert(comment_id);
                }
            }
            other => self.on_result(other, freshness.source, refreshing),
        }
    }

    pub(super) fn observe_community_access(
        &mut self,
        name: &str,
        denied: Option<AccessDenied>,
        source: Source,
    ) {
        let key = name.to_ascii_lowercase();
        let replace = self.community_access.get(&key).is_none_or(|old| {
            if denied.is_some() {
                old.source.compare_requests(source) != Some(std::cmp::Ordering::Greater)
            } else {
                source.may_replace(old.source)
            }
        });
        if replace {
            self.community_access
                .insert(key, CommunityAccess { denied, source });
        }
        self.sync_community_access();
    }

    // Feed and All subs retain separate listings, but share the latest access observation.
    pub(super) fn sync_community_access(&mut self) {
        for tab in &mut self.tabs {
            let Some(name) = tab.subreddit() else {
                continue;
            };
            let Some(access) = self.community_access.get(&name.to_ascii_lowercase()) else {
                continue;
            };
            tab.feed
                .failures
                .retain(|_, failure| failure.error.access().is_none());
            if let Some(denied) = access.denied {
                tab.feed.failures.insert(
                    None,
                    FeedFailure {
                        error: FeedError::Access(denied),
                        source: access.source,
                    },
                );
            }
        }
        for group in &mut self.all_subs.groups {
            let Some(access) = self
                .community_access
                .get(&group.subreddit.to_ascii_lowercase())
            else {
                continue;
            };
            if let Some(denied) = access.denied {
                group.error = Some(FeedError::Access(denied));
            } else if group.error.as_ref().and_then(FeedError::access).is_some() {
                group.error = None;
            }
        }
        self.settle_arrivals();
    }
}
