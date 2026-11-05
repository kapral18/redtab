use super::*;

impl App {
    pub fn open_in_browser(&mut self) {
        if self.focus == Focus::Sidebar
            && matches!(
                self.active_tab().kind,
                TabKind::Subreddit { .. } | TabKind::User { .. }
            )
        {
            self.open_source_in_browser();
            return;
        }
        if let Some((name, _)) = self.restricted_community() {
            self.open_reddit_link(&format!("/r/{name}/"));
            return;
        }
        if let Some(target) = self.comment_target() {
            self.open_reddit_link(&target.permalink);
            return;
        }
        let Some(post) = self.current_post() else {
            self.open_source_in_browser();
            return;
        };
        match media::spawn_detached(&self.config.commands.browser, &[post.reddit_url()]) {
            Ok(()) => self.info("Opened on reddit.com"),
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    pub(super) fn copy_url(&mut self, url: &str, comment: bool) {
        let label = if comment { "comment link" } else { "link" };
        if self.clipboard.is_some() {
            self.info("Wait for the pending copy");
            return;
        }
        self.clipboard = Some((media::CopyTask::start(url.to_string()), label));
        self.info(format!("Copying {label}…"));
    }

    pub(crate) fn copy_link_target(&self) -> anyhow::Result<(String, bool)> {
        if let Some(target) = self.comment_target() {
            Ok((target.url()?, true))
        } else if let Some(post) = self.current_post() {
            Ok((post.reddit_url(), false))
        } else if let Some(path) = self.search_row_path() {
            Ok((format!("https://www.reddit.com{path}"), false))
        } else {
            anyhow::bail!("No link selected")
        }
    }

    pub fn copy_link(&mut self) {
        match self.copy_link_target() {
            Ok((url, comment)) => self.copy_url(&url, comment),
            Err(error) => self.error(format!("copy failed: {error:#}")),
        }
    }

    pub fn open_media(&mut self) {
        let Some(post) = self.current_post() else {
            return;
        };
        self.hand_off_media(&post.permalink, post.media);
    }

    pub(super) fn hand_off_media(&mut self, permalink: &str, media: Media) {
        let commands = &self.config.commands;
        let video = media.video_url().is_some();
        let result = if let Some(url) = media.youtube_url() {
            media::spawn_detached(&commands.browser, &[url.to_owned()])
        } else if let Some(url) = media.video_url() {
            self.reddit
                .media_url(url)
                .and_then(|url| media::spawn_media(&commands.video, &[url]))
        } else {
            match media {
                Media::None => {
                    self.info("This post has no media");
                    return;
                }
                Media::Image { url, .. } => {
                    self.download_images(permalink, vec![url]);
                    return;
                }
                Media::Gallery { images, .. } if !images.is_empty() => {
                    self.download_images(permalink, images);
                    return;
                }
                Media::Link { url, .. } => media::spawn_detached(&commands.browser, &[url]),
                Media::Gallery { .. } | Media::Video { .. } => {
                    if self.pending_media.insert(permalink.to_string()) {
                        self.pool.submit(Job::ResolveMedia {
                            permalink: permalink.to_string(),
                        });
                    }
                    self.info("Fetching media…");
                    return;
                }
            }
        };
        if video
            && result.is_ok()
            && let Some(player) = &mut self.video
            && !player.paused()
        {
            player.toggle_pause(Instant::now());
        }
        match result {
            Ok(()) => self.info("Opened media"),
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    /// One image download per post at a time, so a second media-open action while it runs can't open
    /// the viewer twice.
    pub(super) fn download_images(&mut self, permalink: &str, urls: Vec<String>) {
        if !self.pending_media.insert(permalink.to_string()) {
            self.info("Already downloading…");
            return;
        }
        self.info(match urls.len() {
            1 => "Downloading image…".to_string(),
            n => format!("Downloading {n} images…"),
        });
        self.pool.submit(Job::DownloadImages {
            permalink: permalink.to_string(),
            urls,
        });
    }

    pub fn media_loading(&self) -> bool {
        !self.pending_media.is_empty()
    }
}
