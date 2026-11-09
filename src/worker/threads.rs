use super::*;

/// Worker thread name; the panic hook leaves the screen alone on non-main threads.
pub(super) const WORKER_THREAD: &str = "redtab-worker";

pub(super) fn spawn_worker(
    rx: SharedRx,
    tx: SyncSender<Msg>,
    reddit: Reddit,
    wanted: Arc<Wanted>,
    cache: Option<Arc<Cache>>,
    namespace: Arc<str>,
) {
    thread::Builder::new()
        .name(WORKER_THREAD.into())
        .spawn(move || {
            loop {
                let Work {
                    job,
                    force,
                    cache_policy: _,
                    image_ticket,
                } = {
                    let Ok(guard) = rx.lock() else { return };
                    match guard.recv() {
                        Ok(job) => job,
                        Err(_) => return,
                    }
                };
                if wanted.stopped.load(Ordering::Acquire) {
                    return;
                }
                let mut claim = ImageClaim {
                    wanted: &wanted,
                    url: match &job {
                        Job::Image { url, .. } | Job::VideoFrame { url, .. } => Some(url),
                        _ => None,
                    },
                    ticket: image_ticket.as_ref().map(ImageTicket::claim),
                    claimed: false,
                };
                if !claim.acquire() {
                    continue;
                }
                if superseded_post(&job, &wanted) {
                    if matches!(job, Job::Post { .. })
                        && tx
                            .send(job.failed(crate::reddit::REQUEST_CANCELLED.into()))
                            .is_err()
                    {
                        return;
                    }
                    continue;
                }
                if background_unwanted(&job, &wanted) {
                    if tx
                        .send(job.failed(crate::reddit::REQUEST_CANCELLED.into()))
                        .is_err()
                    {
                        return;
                    }
                    continue;
                }
                if let Some(msg) = skipped_image(&job, &wanted) {
                    if !claim.finish(&tx, msg) {
                        return;
                    }
                    continue;
                }
                // Reserved as the request starts, so a later start orders after it.
                let request_order = cache
                    .as_deref()
                    .filter(|_| job.uses_cache() && !matches!(job, Job::ResolveMedia { .. }))
                    .and_then(|cache| match cache.reserve_request() {
                        Ok(order) => Some(order),
                        Err(e) => {
                            let _ =
                                tx.send(Msg::CacheWarning(format!("Could not prepare cache: {e}")));
                            None
                        }
                    });
                let started = NEXT_EVENT.fetch_add(1, Ordering::Relaxed);
                // A job's panic becomes its error instead of killing the worker.
                let msg = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let generation = job.post_generation();
                    let experience = match &job {
                        Job::Experience {
                            generation,
                            request: crate::reddit::experience::Request::Read { .. },
                            ..
                        } => Some(*generation),
                        _ => None,
                    };
                    let search = match &job {
                        Job::Search {
                            generation, query, ..
                        } => Some((query.clone(), *generation)),
                        _ => None,
                    };
                    let group = match &job {
                        Job::Group {
                            subreddit,
                            generation,
                        } => Some((subreddit.clone(), *generation)),
                        _ => None,
                    };
                    let icon = match &job {
                        Job::Icon { subreddit } => Some(subreddit.clone()),
                        _ => None,
                    };
                    let cancelled = Arc::clone(&wanted);
                    let image = match &job {
                        Job::Image { url, .. } | Job::VideoFrame { url, .. } => Some(url.clone()),
                        _ => None,
                    };
                    let feed = match &job {
                        Job::Feed {
                            tab, generation, ..
                        } => Some((*tab, *generation)),
                        _ => None,
                    };
                    let retry_reads = !matches!(
                        &job,
                        Job::Membership { .. }
                            | Job::Experience {
                                request: crate::reddit::experience::Request::Write(_),
                                ..
                            }
                    );
                    let scoped = reddit.for_worker(
                        match job {
                            Job::Experience {
                                request:
                                    crate::reddit::experience::Request::Read {
                                        query: crate::reddit::experience::Query::Thread(_),
                                        ..
                                    },
                                ..
                            }
                            | Job::Post { .. }
                            | Job::Search { .. }
                            | Job::SortedPost { .. }
                            | Job::MoreReplies { .. }
                            | Job::Image {
                                background: false, ..
                            }
                            | Job::VideoFrame {
                                background: false, ..
                            } => Priority::Selected,
                            Job::Icon { .. } => Priority::Background,
                            Job::Subscriptions { .. } if !force => Priority::Background,
                            Job::Feed { .. } => Priority::Listing,
                            Job::Group { .. } => Priority::Browsing,
                            _ => Priority::Foreground,
                        },
                        retry_reads,
                        move || {
                            cancelled.stopped.load(Ordering::Acquire)
                                || experience.is_some_and(|generation| {
                                    cancelled
                                        .experience
                                        .lock()
                                        .is_ok_and(|current| *current != Some(generation))
                                })
                                || search.as_ref().is_some_and(|(query, generation)| {
                                    cancelled
                                        .search_slot(query)
                                        .lock()
                                        .is_ok_and(|current| *current != Some(*generation))
                                })
                                || feed.is_some_and(|(tab, generation)| {
                                    feed_unwanted(tab, generation, &cancelled)
                                })
                                || group.as_ref().is_some_and(|(name, generation)| {
                                    group_unwanted(name, *generation, &cancelled)
                                })
                                || image.as_ref().is_some_and(|url| {
                                    cancelled
                                        .images
                                        .lock()
                                        .is_ok_and(|urls| !urls.contains(url))
                                })
                                || icon
                                    .as_ref()
                                    .is_some_and(|name| icon_unwanted(name, &cancelled))
                                || generation.is_some_and(|generation| {
                                    cancelled
                                        .post
                                        .lock()
                                        .is_ok_and(|current| *current != Some(generation))
                                })
                        },
                    );
                    let source = Source {
                        order: request_order,
                        observed: Some((started, started)),
                        cached: false,
                    };
                    let mut fallback = None;
                    let msg = run(&scoped, &job, &wanted, &mut |event| {
                        if let (
                            Job::Group {
                                subreddit,
                                generation,
                            },
                            ListingEvent::Post(post),
                        ) = (&job, &event)
                        {
                            let _ = tx.send(Msg::GroupProgress {
                                subreddit: subreddit.clone(),
                                generation: *generation,
                                post: Box::new((*post).clone()),
                                source,
                            });
                        }
                        if let Job::Feed {
                            tab, generation, ..
                        } = &job
                        {
                            match event {
                                ListingEvent::Post(post) => {
                                    let _ = tx.send(Msg::FeedProgress {
                                        tab: *tab,
                                        generation: *generation,
                                        post: Box::new(post.clone()),
                                        source,
                                    });
                                }
                                ListingEvent::ContinuationFailed { cursor, error } => {
                                    fallback = Some((cursor.to_string(), error.to_string()));
                                }
                            }
                        }
                    });
                    let msg = match (msg, fallback) {
                        (
                            Msg::Feed {
                                tab,
                                generation,
                                result: Ok(listing),
                                ..
                            },
                            Some((cursor, error)),
                        ) => Msg::FeedFallback {
                            tab,
                            generation,
                            listing,
                            cursor,
                            error,
                        },
                        (msg, _) => msg,
                    };
                    let msg = match &job {
                        Job::Image { url, .. } | Job::VideoFrame { url, .. }
                            if scoped.read_aborted() =>
                        {
                            Msg::ImageSkipped { url: url.clone() }
                        }
                        _ => msg,
                    };
                    if wanted.stopped.load(Ordering::Acquire) {
                        return job.failed("Connection replaced".into());
                    }
                    cache_result(msg, &job, cache.as_deref(), &namespace, &tx, source)
                }))
                .unwrap_or_else(|_| {
                    // "panicked at file:line:col:\nmessage" → one status line.
                    let detail = take_panic().unwrap_or_default().replace('\n', " ");
                    job.failed(format!("internal error in redtab, please report: {detail}"))
                });
                if !claim.finish(&tx, msg) {
                    return;
                }
            }
        })
        .expect("cannot start a worker thread");
}

/// Promoted foreground work and its old queued copy share one claim.
pub(super) struct ImageClaim<'a> {
    pub(super) wanted: &'a Wanted,
    pub(super) url: Option<&'a String>,
    pub(super) ticket: Option<&'a Arc<AtomicBool>>,
    pub(super) claimed: bool,
}

impl ImageClaim<'_> {
    pub(super) fn acquire(&mut self) -> bool {
        if self.claimed {
            return true;
        }
        if self
            .ticket
            .is_some_and(|ticket| ticket.swap(true, Ordering::AcqRel))
        {
            return false;
        }
        self.claimed = true;
        true
    }

    /// A receiver may immediately retry this URL, before this worker runs again.
    pub(super) fn finish(&mut self, tx: &SyncSender<Msg>, message: Msg) -> bool {
        if !self.acquire() {
            return true;
        }
        self.release();
        tx.send(message).is_ok()
    }

    pub(super) fn release(&mut self) {
        if self.claimed
            && let (Some(url), Some(ticket)) = (self.url, self.ticket)
            && let Ok(mut tickets) = self.wanted.image_tickets.lock()
            && tickets
                .get(url)
                .is_some_and(|current| Arc::ptr_eq(current, ticket))
        {
            tickets.remove(url);
        }
        self.ticket = None;
        self.claimed = false;
    }
}

impl Drop for ImageClaim<'_> {
    fn drop(&mut self) {
        self.release();
    }
}
