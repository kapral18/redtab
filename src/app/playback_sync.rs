use super::*;

impl App {
    pub(super) fn video_control_post(&self) -> Option<Post> {
        if !self.inline_images_on() {
            return None;
        }
        if let Some(view) = &self.post_view {
            return (!view.spoiler_hidden()).then(|| view.shown_post().clone());
        }
        if self.state.list_view() != ListView::Card
            || matches!(self.active_tab().kind, TabKind::AllSubs)
        {
            return None;
        }
        self.current_post().filter(|post| !post.spoiler)
    }

    pub fn video_controls_available(&self) -> bool {
        self.focus != Focus::Sidebar
            && self
                .video_control_post()
                .is_some_and(|post| post.media.video_url().is_some())
    }

    pub(super) fn sync_video_with(
        &mut self,
        now: Instant,
        start_player: impl FnOnce(String, Size, video::Resume) -> Player,
    ) -> bool {
        let Some(url) = self.wanted_video(now) else {
            // Detail waits for its own draw; a settling card already supplied its box.
            if self.post_view.is_some() || self.card_video.is_none() {
                self.video_area = None;
            }
            self.video_resize = None;
            // A loaded card can pause through other cards and resume without a
            // second network/decoder startup. Unfinished extraction still retires.
            if self.inline_images_on()
                && self.post_view.is_none()
                && self.state.list_view() == ListView::Card
                && !matches!(self.active_tab().kind, TabKind::AllSubs)
                && self.config.media.video_autoplay
                && self.video_control_post().is_some()
                && let Some(player) = self
                    .video
                    .as_mut()
                    .map(|active| &mut active.player)
                    .filter(|p| p.frame().is_some() && !p.resolving())
            {
                player.set_hidden(true, now);
                return false;
            }
            if !self.inline_images_on() {
                self.video_previews.clear();
            }
            return self.retire_video(now);
        };
        let card = self.post_view.is_none();
        let id = self.video_control_post().map(|post| post.id).unwrap();
        let mut changed = false;
        if let Some(area) = self.video_area.map(Rect::as_size) {
            let start = match &self.video {
                Some(active) if active.plays(&id, card, &url) && active.player.area == area => {
                    self.video_resize = None;
                    false
                }
                // A new size: wait until it holds still (a window drag passes many).
                Some(active) if active.plays(&id, card, &url) => match self.video_resize {
                    Some((size, since)) if size == area => {
                        now.saturating_duration_since(since) >= VIDEO_RESIZE_SETTLE
                    }
                    _ => {
                        self.video_resize = Some((area, now));
                        false
                    }
                },
                _ => true,
            };
            if start {
                // Same video, new size: go on from where it was.
                let continuing = self
                    .video
                    .as_ref()
                    .is_some_and(|active| active.post == id && active.player.url == url);
                // Protect the destination before retirement can evict an old entry.
                let previous = self.take_video_preview(&id, &url);
                self.retire_video(now);
                let mut previous = self.take_video_preview(&id, &url).or(previous);
                let parked = previous
                    .as_mut()
                    .filter(|preview| preview.card == card)
                    .and_then(|preview| preview.player.take())
                    .filter(|player| player.area == area);
                let mut resume =
                    previous
                        .as_ref()
                        .map(|preview| preview.resume)
                        .unwrap_or(video::Resume {
                            sound: !card && self.video_sound_on,
                            ..video::Resume::default()
                        });
                if !card && !continuing {
                    resume.sound = self.video_sound_on;
                }
                let mut player = parked.unwrap_or_else(|| start_player(url, area, resume));
                if !card && !continuing && player.sound() != self.video_sound_on {
                    player.toggle_sound(now);
                }
                player.retain_frame(previous.and_then(|preview| preview.frame));
                self.video = Some(ActiveVideo {
                    player,
                    post: id,
                    card,
                });
                if card {
                    self.video_sound_on = resume.sound;
                }
                self.video_resize = None;
                changed = true;
            }
        } else {
            self.video_resize = None;
        }
        let shown = !self.popup_open()
            && if card {
                self.card_video.as_ref().is_some_and(|card| card.shown)
            } else {
                self.post_view.as_ref().is_some_and(|v| v.image_shown)
            };
        let rendering = self.card_renderer.pending();
        if let Some(ActiveVideo { player, .. }) = &mut self.video {
            player.set_hidden(!shown, now);
            // Keep audio-clock catch-up independent of tile preparation.
            changed |= player.tick(now) && !rendering;
            self.card_renderer.set_video_rate(player.rate());
            if let Some((text, error)) = player.take_notice() {
                if error {
                    // No ffplay: no later video can play sound either.
                    self.video_sound_on = false;
                    self.error(text);
                } else {
                    self.info(text);
                }
                changed = true;
            }
        }
        changed
    }

    pub(super) fn restore_selected_video(&mut self, now: Instant) {
        let Some(post) = self.video_control_post() else {
            return;
        };
        let Some(url) = post
            .media
            .video_url()
            .and_then(|url| self.playback_url(url))
        else {
            return;
        };
        let card = self.post_view.is_none();
        if self
            .video
            .as_ref()
            .is_some_and(|active| active.plays(&post.id, card, &url))
        {
            return;
        }
        let Some(index) = self.video_previews.iter().position(|preview| {
            preview.post == post.id
                && preview.url == url
                && preview.card == card
                && preview.player.is_some()
        }) else {
            return;
        };
        let mut previous = self.video_previews.remove(index).unwrap();
        self.retire_video(now);
        let mut player = previous.player.take().unwrap();
        player.set_hidden(true, now);
        if card {
            self.video_sound_on = player.sound();
        } else if player.sound() != self.video_sound_on {
            player.toggle_sound(now);
        }
        self.video = Some(ActiveVideo {
            player,
            post: post.id,
            card,
        });
        self.video_resize = None;
    }

    pub fn toggle_video_pause(&mut self) {
        self.restore_selected_video(Instant::now());
        let post = self
            .video_control_post()
            .filter(|p| p.media.video_url().is_some());
        if let Some(post) = &post {
            self.video_requested = Some(post.id.clone());
        }
        if self
            .video
            .as_ref()
            .is_some_and(|active| post.as_ref().is_none_or(|post| active.post != post.id))
        {
            self.retire_video(Instant::now());
        }
        if let Some(active) = self.video.take_if(|active| active.player.error().is_some()) {
            self.video_previews
                .retain(|preview| preview.post != active.post);
        }
        match &mut self.video {
            Some(active) => active.player.toggle_pause(Instant::now()),
            None => {
                if let Some(post) = self
                    .video_control_post()
                    .filter(|p| p.media.video_url().is_some())
                {
                    let url = post
                        .media
                        .video_url()
                        .and_then(|url| self.playback_url(url));
                    for preview in &mut self.video_previews {
                        if preview.post == post.id && Some(&preview.url) == url.as_ref() {
                            preview.resume.paused = false;
                            if let Some(player) = &mut preview.player {
                                if player.error().is_some() {
                                    preview.player = None;
                                } else if player.paused() {
                                    player.toggle_pause(Instant::now());
                                }
                            }
                        }
                    }
                    self.video_requested = Some(post.id);
                } else {
                    self.info("No video plays here");
                }
            }
        }
    }

    /// The mute action toggles this video's sound; fresh cards start muted.
    pub fn toggle_video_sound(&mut self) {
        self.restore_selected_video(Instant::now());
        let wanted = !self.video_sound_on;
        let Some(player) = self.selected_player_mut() else {
            self.info("No video plays here");
            return;
        };
        if player.sound() != wanted {
            player.toggle_sound(Instant::now());
        }
        let notice = player.take_notice();
        self.video_sound_on = wanted;
        match notice {
            Some((text, true)) => {
                self.video_sound_on = false;
                self.error(text);
            }
            Some((text, false)) => self.info(text),
            None => self.info(if wanted { "Sound on" } else { "Sound off" }),
        }
    }

    /// Whether videos start with sound.
    pub fn video_sound_wanted(&self) -> bool {
        self.video_sound_on
    }

    pub fn seek_video(&mut self, forward: bool) {
        self.restore_selected_video(Instant::now());
        let by = if forward {
            video::SEEK_STEP
        } else {
            -video::SEEK_STEP
        };
        let notice = match self.selected_player_mut() {
            None => "No video plays here",
            Some(player) if player.error().is_some() => "The video is unavailable",
            Some(player) if player.ready() => {
                player.seek(by, Instant::now());
                return;
            }
            Some(_) => "The video is still loading",
        };
        self.info(notice);
    }
}
