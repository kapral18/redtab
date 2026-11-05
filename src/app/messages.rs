use super::*;

impl App {
    pub fn on_msg(&mut self, msg: Msg) {
        self.behind_settings(|app| app.on_background_msg(msg));
    }

    pub(super) fn on_background_msg(&mut self, msg: Msg) {
        // New list data can put another post under the cursor: the side pane follows it.
        let list_data = match &msg {
            Msg::Snapshot { message, .. } => {
                matches!(
                    **message,
                    Msg::Feed { .. }
                        | Msg::FeedProgress { .. }
                        | Msg::GroupProgress { .. }
                        | Msg::FeedFallback { .. }
                        | Msg::Group { .. }
                        | Msg::Subscriptions { .. }
                )
            }
            message => matches!(
                message,
                Msg::Feed { .. }
                    | Msg::FeedProgress { .. }
                    | Msg::GroupProgress { .. }
                    | Msg::FeedFallback { .. }
                    | Msg::Group { .. }
                    | Msg::Subscriptions { .. }
            ),
        };
        self.on_result(msg, Source::default(), false);
        if list_data {
            self.prefetch_short_home();
            self.preview_selection();
            self.sync_feed_demand();
            self.follow_ancestor();
        }
    }

    pub(super) fn on_result(&mut self, msg: Msg, source: Source, refreshing: bool) {
        match msg {
            Msg::FeedFallback {
                tab,
                generation,
                listing,
                cursor,
                error,
            } => self.on_feed_fallback(
                tab,
                generation,
                listing,
                cursor,
                error,
                Freshness {
                    fetched_at: now(),
                    source,
                },
            ),
            Msg::FeedProgress {
                tab,
                generation,
                post,
                source,
            } => self.on_feed_progress(tab, generation, *post, source),
            Msg::GroupProgress {
                subreddit,
                generation,
                post,
                source,
            } => self.on_group_progress(&subreddit, generation, *post, source),
            Msg::SortedPost {
                generation,
                sort,
                result,
            } => {
                if self
                    .post_view
                    .as_ref()
                    .is_some_and(|view| view.generation == generation && view.comment_sort == sort)
                {
                    self.on_result(Msg::Post { generation, result }, source, refreshing);
                }
            }
            Msg::Search { generation, result } => self.on_search(generation, result),
            Msg::Experience { generation, result } => self.on_experience(
                generation,
                result,
                Freshness {
                    fetched_at: now(),
                    source,
                },
            ),
            Msg::Membership { generation, result } => self.on_membership(generation, result),
            Msg::Subscriptions { generation, result } => self.on_subscriptions(
                generation,
                result,
                Freshness {
                    fetched_at: now(),
                    source,
                },
                refreshing,
            ),
            Msg::Snapshot {
                message,
                fetched_at,
                refreshing,
                source,
            } => {
                self.on_snapshot(*message, Freshness { fetched_at, source }, refreshing);
            }
            Msg::CacheWarning(warning) => {
                if !self.cache_warnings.contains(&warning) {
                    self.error(warning);
                    if let Some(status) = &mut self.status {
                        status.cache_warning = true;
                    }
                }
            }
            Msg::Feed {
                tab,
                generation,
                append,
                result,
            } => self.on_feed(
                tab,
                generation,
                append,
                result,
                Freshness {
                    fetched_at: now(),
                    source,
                },
                refreshing,
            ),
            Msg::Group {
                subreddit,
                generation,
                result,
            } => self.on_group(
                &subreddit,
                generation,
                result,
                Freshness {
                    fetched_at: now(),
                    source,
                },
                refreshing,
            ),
            Msg::Post { generation, result } => {
                self.on_post(generation, result, source, refreshing)
            }
            Msg::MoreReplies {
                generation,
                comment_id,
                result,
            } => self.on_more_replies(generation, comment_id, result, source, refreshing),
            Msg::Image { url, result } => self.on_image(url, result, refreshing),
            // Skipped as unwanted; wanted again since (the slot waits), it goes back in.
            Msg::ImageSkipped { url } => {
                if let Some(background) = self.queued_images.remove(&url)
                    && (matches!(
                        self.images.get(&url),
                        Some(
                            ImageSlot::Loading
                                | ImageSlot::Ready {
                                    refresh: ImageRefresh::Loading,
                                    ..
                                }
                        )
                    ) || (!self.picture_settle.holding(Instant::now())
                        && matches!(
                            self.card_images.get(&url),
                            Some(
                                CardImageSlot::Loading
                                    | CardImageSlot::Cached {
                                        refreshing: true,
                                        ..
                                    }
                            )
                        )))
                {
                    self.queued_images.insert(url.clone(), background);
                    self.pool.submit(self.image_job(&url, background));
                }
            }
            Msg::Thumb { url, result } => {
                let slot = match result {
                    Ok(img) => {
                        let img = fill_slot(img, self.image_picker.font_size());
                        ImageSlot::Ready {
                            pixels: (img.width(), img.height()),
                            protocol: Box::new(self.image_picker.new_resize_protocol(img)),
                            refresh: if refreshing {
                                ImageRefresh::Loading
                            } else {
                                ImageRefresh::Idle
                            },
                        }
                    }
                    Err(e) => {
                        if let Some(ImageSlot::Ready { refresh, .. }) = self.thumbs.get_mut(&url) {
                            *refresh = if refreshing {
                                ImageRefresh::Loading
                            } else {
                                ImageRefresh::Failed
                            };
                            return;
                        }
                        ImageSlot::Failed(e)
                    }
                };
                self.thumbs.insert(url, slot);
            }
            Msg::Profile { user, result } => {
                let key = user.to_ascii_lowercase();
                self.profile_loading.remove(&key);
                match result {
                    Ok(profile) => self.observe_profile(&user, profile),
                    Err(error) if error == crate::reddit::REQUEST_CANCELLED => {}
                    Err(_) => {
                        self.profile_failed.insert(key);
                    }
                }
            }
            Msg::Icon { subreddit, result } => {
                let key = subreddit.to_ascii_lowercase();
                self.icon_lookups.remove(&key);
                if result
                    .as_ref()
                    .err()
                    .is_some_and(|error| error == crate::reddit::REQUEST_CANCELLED)
                {
                    self.icon_demands.remove(&key);
                }
                // A failure keeps a stale disk answer; with none, the next load asks again.
                if let Ok(icon) = result {
                    self.observe_icon(&key, icon, source);
                }
            }
            // Forget it so it reloads if it scrolls back; a skipped refresh of a shown
            // image keeps it.
            Msg::ThumbSkipped { url } => {
                if let Some(ImageSlot::Ready { refresh, .. }) = self.thumbs.get_mut(&url) {
                    *refresh = ImageRefresh::Needed;
                } else {
                    self.thumbs.remove(&url);
                }
            }
            Msg::MediaResolved { permalink, result } => {
                if !self.pending_media.remove(&permalink) {
                    return;
                }
                match result {
                    Ok(Media::Gallery { images, .. }) if images.is_empty() => {
                        self.error("Gallery has no readable images")
                    }
                    Ok(Media::Video { url: None, .. }) => {
                        self.error("This video has no playable source")
                    }
                    Ok(media) => self.hand_off_media(&permalink, media),
                    Err(e) => self.error(format!("media: {e}")),
                }
            }
            Msg::ImagesDownloaded { permalink, result } => {
                self.pending_media.remove(&permalink);
                match result {
                    Ok(paths) => {
                        let args: Vec<String> =
                            paths.iter().map(|p| p.display().to_string()).collect();
                        match media::spawn_detached(&self.config.commands.image, &args) {
                            Ok(()) => self.info("Opened media"),
                            Err(e) => self.error(format!("{e:#}")),
                        }
                    }
                    Err(e) => self.error(format!("image download: {e}")),
                }
            }
        }
    }
}
