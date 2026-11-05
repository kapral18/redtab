use super::*;

pub struct Pool {
    pub(super) queues: Queues,
    pub(super) wanted: Arc<Wanted>,
    pub(super) cache: Option<Arc<Cache>>,
    pub(super) threads: usize,
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Clone)]
pub(super) struct Queues {
    pub(super) priority: Sender<Work>,
    pub(super) feeds: Sender<Work>,
    pub(super) selected: Sender<Work>,
    pub(super) foreground_images: Sender<Work>,
    pub(super) background: Sender<Work>,
    pub(super) group_cache: Sender<Work>,
    pub(super) images: Sender<Work>,
    pub(super) thumbs: Sender<Work>,
    pub(super) icons: Sender<Work>,
}

pub(super) struct Work {
    pub(super) job: Job,
    pub(super) force: bool,
    pub(super) cache_policy: CachePolicy,
    pub(super) image_ticket: Option<Arc<AtomicBool>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CachePolicy {
    Revalidate,
    Only,
    OrFetch,
}

impl Queues {
    pub(super) fn submit(&self, work: Work) {
        let queue = match work.job {
            Job::Thumb { .. } | Job::VideoThumb { .. } => &self.thumbs,
            Job::VideoFrame {
                background: true, ..
            } => &self.images,
            Job::VideoFrame {
                background: false, ..
            } => &self.foreground_images,
            Job::Icon { .. } => &self.icons,
            Job::Image {
                background: true, ..
            } => &self.images,
            Job::Image {
                background: false, ..
            } => &self.foreground_images,
            Job::Group { .. } if work.cache_policy == CachePolicy::Only => &self.group_cache,
            Job::Group { .. } => &self.background,
            Job::Feed { .. } => &self.feeds,
            Job::Post { .. } | Job::SortedPost { .. } | Job::MoreReplies { .. } => &self.selected,
            Job::Experience {
                request:
                    crate::reddit::experience::Request::Read {
                        query: crate::reddit::experience::Query::Thread(_),
                        ..
                    },
                ..
            } => &self.selected,
            _ => &self.priority,
        };
        let _ = queue.send(work);
    }
}

pub(super) type SharedRx = Arc<Mutex<Receiver<Work>>>;

/// URLs and pages whose queued fetches are still worth running; others are skipped.
#[derive(Default)]
pub(super) struct Wanted {
    pub(super) stopped: AtomicBool,
    /// Thumbnails drawn in the last frame.
    pub(super) thumbs: Mutex<HashSet<String>>,
    /// Full-size images of the open post.
    pub(super) images: Mutex<HashSet<String>>,
    pub(super) image_tickets: Mutex<HashMap<String, Arc<AtomicBool>>>,
    /// Generation of the page load the open post waits for.
    pub(super) post: Mutex<Option<u64>>,
    pub(super) experience: Mutex<Option<u64>>,
    pub(super) search: Mutex<Option<u64>>,
    pub(super) discovery: Mutex<Option<u64>>,
    // None until a view binds demand; each entry tracks its latest dispatched generation.
    pub(super) groups: Mutex<Option<HashMap<String, Option<u64>>>>,
    pub(super) icons: Mutex<Option<HashSet<String>>>,
    pub(super) feeds: Mutex<Option<FeedDemand>>,
}

impl Wanted {
    /// Explore's community discovery and the live list search cancel only their own kind.
    pub(super) fn search_slot(
        &self,
        query: &crate::reddit::experience::Query,
    ) -> &Mutex<Option<u64>> {
        if matches!(query, crate::reddit::experience::Query::Communities) {
            &self.discovery
        } else {
            &self.search
        }
    }
}

pub(super) struct FeedDemand {
    pub(super) active: Option<u64>,
    pub(super) requests: HashMap<u64, HashSet<u64>>,
}

impl Pool {
    pub fn stop(&self) {
        self.wanted.stopped.store(true, Ordering::Release);
    }
    pub fn start(reddit: Reddit, threads: usize, cache: Option<Cache>) -> (Self, Receiver<Msg>) {
        Self::start_shared(reddit, threads, cache.map(Arc::new))
    }

    pub fn restart(&self, reddit: Reddit) -> (Self, Receiver<Msg>) {
        Self::start_shared(reddit, self.threads, self.cache.clone())
    }

    pub(super) fn start_shared(
        reddit: Reddit,
        threads: usize,
        cache: Option<Arc<Cache>>,
    ) -> (Self, Receiver<Msg>) {
        let (priority_tx, priority_rx) = channel::<Work>();
        let (feeds_tx, feeds_rx) = channel::<Work>();
        let (selected_tx, selected_rx) = channel::<Work>();
        let (foreground_images_tx, foreground_images_rx) = channel::<Work>();
        let (background_tx, background_rx) = channel::<Work>();
        let (images_tx, images_rx) = channel::<Work>();
        let (thumbs_tx, thumbs_rx) = channel::<Work>();
        let (icons_tx, icons_rx) = channel::<Work>();
        let (msg_tx, msg_rx) = sync_channel::<Msg>(8);
        let wanted = Arc::<Wanted>::default();
        let namespace: Arc<str> = reddit.cache_namespace().into();
        let cached = |queue| {
            cache_reader(
                queue,
                msg_tx.clone(),
                Arc::clone(&wanted),
                cache.clone(),
                Arc::clone(&namespace),
            )
        };
        let queues = Queues {
            feeds: cached(feeds_tx),
            foreground_images: cached(foreground_images_tx),
            selected: cached(selected_tx),
            priority: cached(priority_tx),
            group_cache: cached(background_tx.clone()),
            background: cached(background_tx),
            images: cached(images_tx),
            thumbs: cached(thumbs_tx),
            icons: cached(icons_tx),
        };
        let background_threads = threads.saturating_sub(PRIORITY_THREADS).max(1);
        for (rx, count) in [
            (priority_rx, PRIORITY_THREADS),
            (feeds_rx, 2),
            (selected_rx, 2),
            (foreground_images_rx, 2),
            (background_rx, background_threads),
            (images_rx, 2),
            (thumbs_rx, THUMB_THREADS),
            (icons_rx, ICON_THREADS),
        ] {
            let rx: SharedRx = Arc::new(Mutex::new(rx));
            for _ in 0..count {
                spawn_worker(
                    Arc::clone(&rx),
                    msg_tx.clone(),
                    reddit.clone(),
                    Arc::clone(&wanted),
                    cache.clone(),
                    Arc::clone(&namespace),
                );
            }
        }
        (
            Self {
                queues,
                wanted,
                cache,
                threads,
            },
            msg_rx,
        )
    }

    pub fn submit(&self, job: Job) {
        self.enqueue(job, false)
    }

    pub(super) fn enqueue(&self, job: Job, force: bool) {
        self.enqueue_policy(job, force, CachePolicy::Revalidate);
    }

    pub fn cached(&self, job: Job) {
        if self.cache.is_some() {
            self.enqueue_policy(job, false, CachePolicy::Only);
        }
    }

    pub fn prefetch_group(&self, subreddit: String, generation: u64) {
        self.enqueue_policy(
            Job::Group {
                subreddit,
                generation,
            },
            false,
            CachePolicy::OrFetch,
        );
    }

    pub(super) fn enqueue_policy(&self, job: Job, force: bool, cache_policy: CachePolicy) {
        if let Job::Experience {
            generation,
            request: crate::reddit::experience::Request::Read { .. },
            ..
        } = &job
        {
            self.want_experience(Some(*generation));
        }
        if let Job::Search {
            generation, query, ..
        } = &job
        {
            self.want_search(query, Some(*generation));
        }
        let image_ticket = match &job {
            Job::Image { url, .. } | Job::VideoFrame { url, .. } => {
                self.wanted.image_tickets.lock().ok().map(|mut tickets| {
                    Arc::clone(
                        tickets
                            .entry(url.clone())
                            .or_insert_with(|| Arc::new(AtomicBool::new(false))),
                    )
                })
            }
            _ => None,
        };
        if cache_policy != CachePolicy::Only
            && let Job::Group {
                subreddit,
                generation,
            } = &job
        {
            self.want_group(subreddit, *generation);
        }
        self.queues.submit(Work {
            job,
            force,
            cache_policy,
            image_ticket,
        });
    }

    /// Explicit reload bypasses cached content, leaving the displayed view in place.
    pub fn refresh(&self, job: Job) {
        self.enqueue(job, true)
    }

    /// Record the thumbnails on screen now, so scrolled-away ones are not fetched.
    pub fn set_on_screen(&self, urls: &[&str]) {
        if let Ok(mut set) = self.wanted.thumbs.lock() {
            // Usually unchanged. Spots can repeat a URL, so compare both ways.
            if urls.iter().all(|u| set.contains(*u))
                && set.iter().all(|s| urls.contains(&s.as_str()))
            {
                return;
            }
            set.clear();
            set.extend(urls.iter().map(|u| u.to_string()));
        }
    }

    /// An image the open post shows; call before submitting its `Job::Image`.
    pub fn want_image(&self, url: &str) {
        if let Ok(mut set) = self.wanted.images.lock() {
            set.insert(url.to_string());
        }
    }

    /// An image no longer worth fetching: its queued fetch is skipped.
    pub fn unwant_image(&self, url: &str) {
        if let Ok(mut set) = self.wanted.images.lock() {
            set.remove(url);
        }
    }

    /// Replace demand atomically so shared card URLs are never briefly unwanted.
    pub fn retain_images<'a>(&self, urls: impl Iterator<Item = &'a String>) {
        if let Ok(mut set) = self.wanted.images.lock() {
            *set = urls.cloned().collect();
        }
    }

    pub fn set_visible_icons(&self, names: &[&str]) {
        if let Ok(mut icons) = self.wanted.icons.lock() {
            *icons = Some(names.iter().map(|name| name.to_ascii_lowercase()).collect());
        }
    }

    pub fn icon_wanted(&self, name: &str) -> bool {
        !icon_unwanted(&name.to_ascii_lowercase(), &self.wanted)
    }

    pub fn set_group_demand(&self, names: &[&str]) {
        if let Ok(mut groups) = self.wanted.groups.lock() {
            let previous = groups.take().unwrap_or_default();
            *groups = Some(
                names
                    .iter()
                    .map(|name| ((*name).to_string(), previous.get(*name).copied().flatten()))
                    .collect(),
            );
        }
    }

    pub fn want_group(&self, name: &str, generation: u64) {
        if let Ok(mut groups) = self.wanted.groups.lock()
            && let Some(current) = groups.as_mut().and_then(|groups| groups.get_mut(name))
        {
            *current = Some(current.unwrap_or(generation).max(generation));
        }
    }

    /// The page load the open post waits for; queued loads of other generations are
    /// dropped. `None` once no post is open.
    pub fn want_post(&self, generation: Option<u64>) {
        if let Ok(mut post) = self.wanted.post.lock() {
            *post = generation;
        }
    }

    pub fn want_experience(&self, generation: Option<u64>) {
        if let Ok(mut current) = self.wanted.experience.lock() {
            *current = generation;
        }
    }

    pub fn want_search(&self, query: &crate::reddit::experience::Query, generation: Option<u64>) {
        if let Ok(mut current) = self.wanted.search_slot(query).lock() {
            *current = generation;
        }
    }

    pub fn want_feeds(&self, active: Option<u64>, requests: HashMap<u64, HashSet<u64>>) {
        if let Ok(mut feeds) = self.wanted.feeds.lock() {
            *feeds = Some(FeedDemand { active, requests });
        }
    }
}
