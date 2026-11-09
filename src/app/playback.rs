use super::*;

pub(super) struct CardPlayback {
    pub(super) id: String,
    pub(super) url: String,
    pub(super) since: Instant,
    pub(super) shown: bool,
}

/// The one live Player and the post that owns it, in a card or in the post view.
pub(super) struct ActiveVideo {
    pub(super) player: Player,
    pub(super) post: String,
    pub(super) card: bool,
}

impl ActiveVideo {
    pub(super) fn plays(&self, post: &str, card: bool, url: &str) -> bool {
        self.post == post && self.card == card && self.player.url == url
    }
}

pub(super) struct VideoPreview {
    pub(super) post: String,
    pub(super) url: String,
    pub(super) resume: video::Resume,
    pub(super) frame: Option<video::Frame>,
    pub(super) player: Option<Player>,
    pub(super) card: bool,
}

impl VideoPreview {
    pub(super) fn bytes(&self) -> usize {
        match self.frame.as_ref().map(|frame| &frame.pixels) {
            Some(video::Pixels::Raw(pixels)) => pixels.as_bytes().len(),
            Some(video::Pixels::Png(png)) => png.capacity(),
            None => 0,
        }
    }
}

impl App {
    /// The final drawn selection owns card playback; scrolling within it keeps its timer.
    pub fn card_video_drawn(&mut self, size: Option<ratatui::layout::Size>) {
        if self.post_view.is_some() {
            self.card_video = None;
            return;
        }
        self.video_area = size.map(|size| Rect::new(0, 0, size.width, size.height));
        let post = self.video_control_post();
        let target = post.as_ref().and_then(|post| {
            post.media
                .video_url()
                .and_then(|url| self.playback_url(url))
                .map(|url| (post.id.clone(), url))
        });
        match target {
            Some((id, url)) => {
                let shown = size.is_some() && !self.popup_open();
                match &mut self.card_video {
                    Some(card) if card.id == id && card.url == url => card.shown = shown,
                    _ => {
                        if self.video_requested.as_deref() != Some(&id) {
                            self.video_requested = None;
                        }
                        self.card_video = Some(CardPlayback {
                            id,
                            url,
                            shown,
                            since: Instant::now(),
                        })
                    }
                }
            }
            None => {
                self.card_video = None;
                self.video_requested = None;
            }
        }
    }

    pub fn video(&self) -> Option<&Player> {
        self.video.as_ref().map(|active| &active.player)
    }

    #[cfg(test)]
    pub fn video_mut(&mut self) -> Option<&mut Player> {
        self.video.as_mut().map(|active| &mut active.player)
    }

    /// Plays `player` for the selected post (if any), as the playback sync would start it.
    #[cfg(test)]
    pub fn set_video(&mut self, player: Option<Player>) {
        let post = self.video_control_post().map(|post| post.id);
        let card = self.post_view.is_none();
        self.video = player.map(|player| ActiveVideo {
            player,
            post: post.unwrap_or_default(),
            card,
        });
    }

    /// The live Player belongs to the selected post.
    fn video_owned_by_selection(&self) -> bool {
        self.video.as_ref().is_some_and(|active| {
            self.video_control_post()
                .is_some_and(|post| post.id == active.post)
        })
    }

    fn selected_active(&self) -> Option<&ActiveVideo> {
        self.video
            .as_ref()
            .filter(|_| self.video_owned_by_selection())
    }

    pub(super) fn selected_player_mut(&mut self) -> Option<&mut Player> {
        if !self.video_owned_by_selection() {
            return None;
        }
        self.video.as_mut().map(|active| &mut active.player)
    }

    pub fn selected_video(&self) -> Option<&Player> {
        if let Some(active) = self.selected_active() {
            return Some(&active.player);
        }
        let post = self.video_control_post()?;
        let url = self.playback_url(post.media.video_url()?)?;
        self.video_previews
            .iter()
            .rev()
            .find(|preview| {
                preview.post == post.id
                    && preview.url == url
                    && preview.card == self.post_view.is_none()
            })
            .and_then(|preview| preview.player.as_ref())
    }

    pub fn video_loading(&self) -> Option<&'static str> {
        let post = self.video_control_post()?;
        post.media.video_url()?;
        if self
            .post_view
            .as_ref()
            .is_some_and(|view| view.page.is_none() && view.error.is_some())
        {
            return None;
        }
        if self.post_view.is_none() && !self.card_video.as_ref().is_some_and(|card| card.shown) {
            return None;
        }
        match self.selected_video() {
            Some(player) if player.loading() => Some(if player.waiting_for_audio() {
                "starting audio…"
            } else {
                "loading video…"
            }),
            Some(_) => None,
            None => (self.config.media.video_autoplay
                || self.video_requested.as_deref() == Some(&post.id))
            .then_some("loading video…"),
        }
    }

    #[cfg(test)]
    pub fn card_player(&self) -> Option<&Player> {
        let card = self.card_video.as_ref()?;
        self.video
            .as_ref()
            .filter(|active| active.card && active.post == card.id)
            .map(|active| &active.player)
    }

    /// Selected pixels stay drawable while playback settles or a decoder restarts.
    pub fn video_preview(&self, card: bool) -> Option<(&video::Frame, bool)> {
        if card && self.post_view.is_some() {
            return None;
        }
        let post = self.video_control_post()?;
        let url = self.playback_url(post.media.video_url()?)?;
        let player = self.selected_video();
        if let Some(player) = player.filter(|player| player.url == url)
            && let Some(frame) = player.frame()
        {
            return Some((frame, !player.paused() && player.error().is_none()));
        }
        let preview = self
            .video_previews
            .iter()
            .rev()
            .find(|preview| preview.post == post.id && preview.url == url)?;
        let playing = !preview.resume.paused
            && (self.config.media.video_autoplay
                || self.video_requested.as_deref() == Some(&post.id));
        preview.frame.as_ref().map(|frame| (frame, playing))
    }

    pub fn inactive_card_frame(&self, post: &str, source: &str) -> Option<&video::Frame> {
        if !self.inline_images_on() {
            return None;
        }
        let url = self.playback_url(source)?;
        self.video
            .as_ref()
            .filter(|active| active.plays(post, true, &url))
            .and_then(|active| active.player.frame())
            .or_else(|| {
                self.video_previews
                    .iter()
                    .rev()
                    .find(|preview| preview.card && preview.post == post && preview.url == url)
                    .and_then(|preview| preview.frame.as_ref())
            })
    }

    pub fn video_failed_without_frame(&self, post: &Post) -> bool {
        let Some(url) = post
            .media
            .video_url()
            .and_then(|url| self.playback_url(url))
        else {
            return false;
        };
        if let Some(active) = self
            .video
            .as_ref()
            .filter(|active| active.player.url == url && active.post == post.id)
        {
            let player = &active.player;
            return player.error().is_some() && player.frame().is_none();
        }
        self.video_previews
            .iter()
            .rev()
            .find(|preview| preview.post == post.id && preview.url == url)
            .is_some_and(|preview| {
                preview.frame.is_none()
                    && preview
                        .player
                        .as_ref()
                        .is_some_and(|player| player.error().is_some())
            })
    }

    pub fn playback_url(&self, url: &str) -> Option<String> {
        if let Some(id) = crate::model::youtube_id(url) {
            Some(format!("https://www.youtube.com/watch?v={id}"))
        } else {
            self.reddit.media_url(url).ok()
        }
    }

    pub(super) fn retire_video(&mut self, now: Instant) -> bool {
        let Some(ActiveVideo {
            mut player,
            post,
            card,
        }) = self.video.take()
        else {
            return false;
        };
        if self.inline_images_on() {
            // Manual play and autoplay-off retirement also need the selected
            // preview to survive pruning, before a replacement Player exists.
            let protected = self
                .video_control_post()
                .and_then(|target| {
                    let url = self.playback_url(target.media.video_url()?)?;
                    self.take_video_preview(&target.id, &url)
                })
                .filter(|preview| preview.post != post || preview.url != player.url);
            self.video_previews
                .retain(|preview| preview.post != post || preview.url != player.url);
            player.set_hidden(true, now);
            let mut preview = VideoPreview {
                post,
                url: player.url.clone(),
                resume: player.resume(now),
                frame: player.frame().cloned(),
                player: None,
                card,
            };
            if !player.resolving() && (player.frame().is_some() || player.error().is_some()) {
                preview.player = Some(player);
            }
            self.video_previews.push_back(preview);
            if let Some(protected) = protected {
                self.video_previews.push_back(protected);
            }
            while self
                .video_previews
                .iter()
                .filter(|preview| preview.player.is_some())
                .count()
                > VIDEO_PLAYER_MAX
            {
                if let Some(oldest) = self
                    .video_previews
                    .iter_mut()
                    .find(|preview| preview.player.is_some())
                {
                    oldest.player = None;
                }
            }
            while self.video_previews.len() > VIDEO_PREVIEW_MAX
                || self
                    .video_previews
                    .iter()
                    .map(VideoPreview::bytes)
                    .sum::<usize>()
                    > VIDEO_PREVIEW_BYTES
            {
                self.video_previews.pop_front();
            }
        }
        true
    }

    pub(super) fn take_video_preview(&mut self, post: &str, url: &str) -> Option<VideoPreview> {
        self.video_previews
            .iter()
            .position(|preview| preview.post == post && preview.url == url)
            .and_then(|index| self.video_previews.remove(index))
    }

    /// One stream: loaded detail takes precedence; cards wait for selection to settle.
    pub(super) fn wanted_video(&self, now: Instant) -> Option<String> {
        let post = self.video_control_post()?;
        let requested = self.video_requested.as_deref() == Some(&post.id);
        if !self.config.media.video_autoplay && !requested {
            return None;
        }
        if let Some(view) = &self.post_view {
            self.playback_url(view.page.as_ref()?.post.media.video_url()?)
        } else {
            let card = self.card_video.as_ref()?;
            let playing = self
                .video
                .as_ref()
                .is_some_and(|active| active.post == card.id && active.player.url == card.url);
            if card.id != post.id
                || (!card.shown && self.video.is_none())
                || (!requested
                    && (now.saturating_duration_since(card.since) < NAV_SETTLE
                        || (!playing && self.picture_settle.holding(now))))
            {
                return None;
            }
            Some(card.url.clone())
        }
    }

    /// Starts, stops, pauses and advances the inline video to match the post view. It
    /// starts once a draw placed its area (restarting in place on a size change) and
    /// pauses while off screen or under an overlay. True when something visible changed.
    pub(super) fn sync_video(&mut self, now: Instant) -> bool {
        let font = self.image_picker.font_size();
        let reddit = self.reddit.clone();
        let upstream = self
            .video_control_post()
            .and_then(|post| post.media.video_url().map(str::to_owned));
        self.sync_video_with(now, move |url, area, resume| {
            Player::start(url, area, font, resume, now, move |cancelled| {
                reddit
                    .video_fallback(upstream.as_deref().ok_or("No video source")?, move || {
                        cancelled()
                    })
                    .map_err(|_| "Video alternative unavailable".into())
            })
        })
    }

    pub fn video_uses_playback(&self, url: &str) -> bool {
        if matches!(
            self.card_images.get(url),
            Some(CardImageSlot::Ready { .. } | CardImageSlot::Cached { .. })
        ) || matches!(self.images.get(url), Some(ImageSlot::Ready { .. }))
        {
            return false;
        }
        self.video_control_post().is_some_and(|post| {
            post.media.video_url() == Some(url)
                && (self.config.media.video_autoplay
                    || self.video_requested.as_deref() == Some(post.id.as_str())
                    || self
                        .selected_video()
                        .is_some_and(|player| player.frame().is_some()))
        })
    }

    pub(super) fn video_source(&self, url: &str) -> bool {
        self.tabs
            .iter()
            .flat_map(|tab| &tab.feed.posts)
            .chain(self.post_view.iter().map(|view| view.shown_post()))
            .chain(&self.search.preview.posts)
            .chain(self.state.bookmarks().iter().map(|bookmark| &bookmark.post))
            .chain(self.all_subs.groups.iter().flat_map(|group| &group.posts))
            .any(|post| post.media.video_url() == Some(url) && !video::plain_gif(url))
    }
}
