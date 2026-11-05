use super::*;

/// Cache hits remain usable while HTTP is busy, with media decoding isolated.
pub(super) fn cache_reader(
    next: Sender<Work>,
    tx: SyncSender<Msg>,
    wanted: Arc<Wanted>,
    cache: Option<Arc<Cache>>,
    namespace: Arc<str>,
) -> Sender<Work> {
    let Some(cache) = cache else { return next };
    let (sender, receiver) = channel::<Work>();
    thread::Builder::new()
        .name("redtab-cache".into())
        .spawn(move || {
            while let Ok(work) = receiver.recv() {
                if wanted.stopped.load(Ordering::Acquire) {
                    return;
                }
                let job = &work.job;
                if superseded_post(job, &wanted) {
                    if matches!(job, Job::Post { .. })
                        && tx
                            .send(job.failed(crate::reddit::REQUEST_CANCELLED.into()))
                            .is_err()
                    {
                        return;
                    }
                    continue;
                }
                if work
                    .image_ticket
                    .as_ref()
                    .is_some_and(|ticket| ticket.load(Ordering::Acquire))
                {
                    continue;
                }
                if work.cache_policy != CachePolicy::Only && background_unwanted(job, &wanted) {
                    if tx
                        .send(job.failed(crate::reddit::REQUEST_CANCELLED.into()))
                        .is_err()
                    {
                        return;
                    }
                    continue;
                }
                {
                    let mut claim = ImageClaim {
                        wanted: &wanted,
                        url: match job {
                            Job::Image { url, .. } | Job::VideoFrame { url, .. } => Some(url),
                            _ => None,
                        },
                        ticket: work.image_ticket.as_ref(),
                        claimed: false,
                    };
                    if let Some(message) = skipped_image(job, &wanted) {
                        if !claim.finish(&tx, message) {
                            return;
                        }
                        continue;
                    }
                    if !work.force
                        && cached_reply(
                            job,
                            Some(&cache),
                            &namespace,
                            &tx,
                            &wanted,
                            &mut claim,
                            work.cache_policy,
                        )
                    {
                        continue;
                    }
                }
                if work.cache_policy == CachePolicy::Only {
                    continue;
                }
                // A miss/stale image stays unclaimed until HTTP starts, allowing promotion.
                if next.send(work).is_err() {
                    return;
                }
            }
        })
        .expect("cannot start cache reader");
    sender
}

/// Cache readers deliver snapshots before dispatching misses/stale refreshes to HTTP.
pub(super) fn cached_reply(
    job: &Job,
    cache: Option<&Cache>,
    namespace: &str,
    tx: &SyncSender<Msg>,
    wanted: &Wanted,
    claim: &mut ImageClaim<'_>,
    cache_policy: CachePolicy,
) -> bool {
    let Some(cache) = cache else {
        return false;
    };
    let Some((key, ttl)) = job.cache_key(namespace) else {
        return false;
    };
    let now = crate::app::now();
    let read_start = NEXT_EVENT.fetch_add(1, Ordering::Relaxed);
    let Some(entry) = cache.load(&key, now) else {
        return false;
    };
    let fresh = now.saturating_sub(entry.fetched_at) < ttl;
    if matches!(job, Job::Experience { .. }) && !fresh {
        return false;
    }
    if matches!(job, Job::ResolveMedia { .. }) && !fresh {
        return false;
    }
    let Some(message) = job.cached_message(&entry.bytes) else {
        return false;
    };
    if let Some(skipped) = skipped_image(job, wanted) {
        let _ = claim.finish(tx, skipped);
        return true;
    }
    let source = Source {
        order: Some(entry.order),
        observed: Some((read_start, NEXT_EVENT.fetch_add(1, Ordering::Relaxed))),
        cached: true,
    };
    let refreshing = cache_policy == CachePolicy::Revalidate
        && (!fresh
            || matches!(job, Job::MoreReplies { minimum: Some(minimum), .. } if !source.may_replace(*minimum)));
    let message = Msg::Snapshot {
        message: Box::new(message),
        fetched_at: entry.fetched_at,
        refreshing,
        source,
    };
    let delivered = if refreshing {
        tx.send(message).is_ok()
    } else {
        claim.finish(tx, message)
    };
    if !delivered {
        return true;
    }
    if !refreshing {
        cache.touch(&key, now);
    }
    !refreshing
}

impl Job {
    /// Whether this job can be answered by a cached snapshot.
    pub(super) fn uses_cache(&self) -> bool {
        matches!(
            self,
            Job::Experience {
                request: crate::reddit::experience::Request::Read {
                    query: crate::reddit::experience::Query::Thread(_),
                    ..
                },
                ..
            }
        ) || matches!(
            self,
            Job::Subscriptions { .. }
                | Job::Feed { .. }
                | Job::Group { .. }
                | Job::Post { .. }
                | Job::ResolveMedia { .. }
                | Job::MoreReplies { .. }
                | Job::Image { .. }
                | Job::VideoFrame { .. }
                | Job::Thumb { .. }
                | Job::VideoThumb { .. }
                | Job::Icon { .. }
        )
    }

    pub(super) fn cache_key(&self, namespace: &str) -> Option<(String, i64)> {
        let (request, ttl) = match self {
            Job::Subscriptions { .. } | Job::Membership { .. } => {
                ("subscriptions-v3".to_string(), CONTENT_TTL)
            }
            Job::Feed {
                target,
                query,
                after,
                ..
            } => (query.cache_request(target, after.as_deref())?, CONTENT_TTL),
            Job::Group { subreddit, .. } => (
                FeedQuery::DEFAULT_LISTING
                    .cache_request(&FeedTarget::Subreddit(subreddit.clone()), None)?,
                CONTENT_TTL,
            ),
            Job::Experience {
                request:
                    crate::reddit::experience::Request::Read {
                        query: crate::reddit::experience::Query::Thread(link),
                        ..
                    },
                ..
            } => (post_cache_request(link)?, CONTENT_TTL),
            Job::Post { permalink, .. } | Job::ResolveMedia { permalink } => {
                (post_cache_request(permalink)?, CONTENT_TTL)
            }
            Job::MoreReplies { cursor, .. } => (
                serde_json::to_string(&("replies-v3", cursor)).ok()?,
                CONTENT_TTL,
            ),
            Job::Image { url, .. } | Job::VideoFrame { url, .. } => (
                serde_json::to_string(&("image", MAX_IMAGE_EDGE, url)).ok()?,
                IMAGE_TTL,
            ),
            Job::Thumb { url } | Job::VideoThumb { url } => {
                (serde_json::to_string(&("thumb", url)).ok()?, IMAGE_TTL)
            }
            Job::Icon { subreddit } => (
                serde_json::to_string(&("icon", subreddit.to_ascii_lowercase())).ok()?,
                IMAGE_TTL,
            ),
            _ => return None,
        };
        Some((Cache::key(namespace, &request), ttl))
    }

    pub(super) fn cached_message(&self, bytes: &[u8]) -> Option<Msg> {
        Some(match self {
            Job::Experience {
                generation,
                request:
                    crate::reddit::experience::Request::Read {
                        query: crate::reddit::experience::Query::Thread(link),
                        ..
                    },
                ..
            } => {
                let page: PostPage = serde_json::from_slice(bytes).ok()?;
                let (_, comment) = crate::reddit::experience::thread_location(link).ok()?;
                fn contains(comments: &[crate::model::Comment], id: &str) -> bool {
                    comments
                        .iter()
                        .any(|comment| comment.id == id || contains(&comment.replies, id))
                }
                if comment
                    .as_ref()
                    .is_some_and(|id| !contains(&page.comments, id))
                {
                    return None;
                }
                Msg::Experience {
                    generation: *generation,
                    result: Ok(crate::reddit::experience::Response::Thread(Box::new(page))),
                }
            }
            Job::Subscriptions { generation } => Msg::Subscriptions {
                generation: *generation,
                result: Ok(serde_json::from_slice(bytes).ok()?),
            },
            Job::Feed {
                tab,
                generation,
                after,
                ..
            } => Msg::Feed {
                tab: *tab,
                generation: *generation,
                append: after.is_some(),
                result: Ok(serde_json::from_slice(bytes).ok()?),
            },
            Job::Group {
                subreddit,
                generation,
            } => Msg::Group {
                generation: *generation,
                subreddit: subreddit.clone(),
                result: Ok(serde_json::from_slice(bytes).ok()?),
            },
            Job::Post { generation, .. } => Msg::Post {
                generation: *generation,
                result: Ok(serde_json::from_slice(bytes).ok()?),
            },
            Job::MoreReplies {
                generation,
                comment_id,
                ..
            } => Msg::MoreReplies {
                generation: *generation,
                comment_id: comment_id.clone(),
                result: Ok(serde_json::from_slice(bytes).ok()?),
            },
            Job::ResolveMedia { permalink } => Msg::MediaResolved {
                permalink: permalink.clone(),
                result: Ok(serde_json::from_slice::<PostPage>(bytes).ok()?.post.media),
            },
            Job::Thumb { url } | Job::VideoThumb { url } => Msg::Thumb {
                url: url.clone(),
                result: Ok(image::load_from_memory(bytes).ok()?),
            },
            Job::Image { url, .. } | Job::VideoFrame { url, .. } => Msg::Image {
                url: url.clone(),
                result: Ok(image::load_from_memory(bytes).ok()?.into()),
            },
            Job::Icon { subreddit } => Msg::Icon {
                subreddit: subreddit.clone(),
                result: Ok(serde_json::from_slice(bytes).ok()?),
            },
            _ => return None,
        })
    }
}

impl Job {
    pub(super) fn post_generation(&self) -> Option<u64> {
        match self {
            Self::Post { generation, .. }
            | Self::SortedPost { generation, .. }
            | Self::MoreReplies { generation, .. } => Some(*generation),
            _ => None,
        }
    }
}
