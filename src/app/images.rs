use super::*;

/// Refetch state of a loaded picture; its pixels stay drawn whatever this says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageRefresh {
    Idle,
    Loading,
    Failed,
    /// Skipped work or an explicit retry waits until the URL is drawn.
    Needed,
}

pub enum ImageSlot {
    Loading,
    Ready {
        protocol: Box<StatefulProtocol>,
        refresh: ImageRefresh,
        /// The picture's size in pixels, which decides its shape in the post view.
        pixels: (u32, u32),
    },
    Failed(String),
}

pub enum CardImageSlot {
    Loading,
    Ready {
        image: std::sync::Arc<image::DynamicImage>,
        protocol: Option<CardImageProtocol>,
    },
    /// Usable retained or stale pixels while a full preview is restored.
    Cached {
        image: std::sync::Arc<image::DynamicImage>,
        protocol: Option<CardImageProtocol>,
        refreshing: bool,
    },
    Failed(String),
}

pub struct CardImageProtocol {
    pub size: Size,
    pub pixels: (u32, u32),
    pub overlay: Option<crate::card_image::Overlay>,
    pub tiles: HashMap<(u16, u16, u16, u16), std::sync::Arc<crate::card_image::Tile>>,
}

impl App {
    pub fn card_preparation_allowed(&self) -> bool {
        !self.picture_settle.holding(Instant::now())
    }

    pub fn card_image_is_retained(&self, url: &str) -> bool {
        self.card_previews.get(url).is_some_and(|preview| {
            matches!(self.card_images.get(url), Some(CardImageSlot::Cached { image, .. })
                if std::sync::Arc::ptr_eq(image, preview))
        })
    }

    pub(super) fn request_image(&mut self, url: &str) {
        if self.video_uses_playback(url) {
            return;
        }
        if !self.images.contains_key(url)
            && let Some(CardImageSlot::Ready { image, .. }) = self.card_images.get(url)
        {
            self.pool.want_image(url);
            self.images.insert(
                url.to_string(),
                ImageSlot::Ready {
                    pixels: (image.width(), image.height()),
                    protocol: Box::new(self.image_picker.new_resize_protocol((**image).clone())),
                    refresh: if self.queued_images.contains_key(url) {
                        ImageRefresh::Loading
                    } else {
                        ImageRefresh::Idle
                    },
                },
            );
            return;
        }
        self.fetch_image(url, false);
    }

    /// A picture in post or comment text, fetched in the background once: a failed one
    /// waits for `r`. Held during a quick run of moves.
    pub fn request_text_picture(&mut self, url: &str) {
        if matches!(
            self.images.get(url),
            None | Some(ImageSlot::Ready {
                refresh: ImageRefresh::Needed,
                ..
            })
        ) && !self.picture_settle.holding(Instant::now())
        {
            self.fetch_image(url, true);
        }
    }

    /// A picture in post or comment text now far from view: let it go, loaded or still
    /// queued (the fetch is skipped), so the ones in view come first. Asked for again
    /// when near.
    pub fn drop_text_picture(&mut self, url: &str) {
        if !matches!(self.images.get(url), Some(ImageSlot::Failed(_))) {
            self.images.remove(url);
            if !self.card_images.contains_key(url) {
                self.pool.unwant_image(url);
            }
        }
    }

    pub fn card_pictures_drawn(&mut self, urls: &[&str]) {
        let keep_pixels = self.inline_images_on();
        if !keep_pixels {
            self.card_renderer.clear();
            self.card_previews.clear();
            self.card_preview_order.clear();
        }
        let offscreen: Vec<String> = self
            .card_images
            .keys()
            .filter(|url| !urls.contains(&url.as_str()))
            .cloned()
            .collect();
        for url in offscreen {
            match self.card_images.get_mut(&url) {
                Some(CardImageSlot::Ready { protocol, .. }) if keep_pixels => {
                    *protocol = None;
                }
                Some(CardImageSlot::Failed(_)) if keep_pixels => {}
                _ => {
                    self.card_images.remove(&url);
                }
            }
            if !self.images.contains_key(&url) {
                self.pool.unwant_image(&url);
            }
        }
        self.card_image_order
            .retain(|url| self.card_images.contains_key(url));
        for &url in urls {
            self.card_image_order.retain(|cached| cached != url);
            self.card_image_order.push_back(url.to_string());
            if self.card_previews.contains_key(url) {
                self.card_preview_order.retain(|cached| cached != url);
                self.card_preview_order.push_back(url.to_string());
            }
        }
        let cached: Vec<_> = self
            .card_image_order
            .iter()
            .filter(|url| !urls.contains(&url.as_str()))
            .filter_map(|url| match self.card_images.get(url) {
                Some(CardImageSlot::Ready { image, .. }) => {
                    Some((url.clone(), image.as_bytes().len()))
                }
                Some(CardImageSlot::Failed(_)) => Some((url.clone(), 0)),
                _ => None,
            })
            .collect();
        let mut count = cached.len();
        let mut bytes: usize = cached.iter().map(|(_, bytes)| bytes).sum();
        for (url, size) in cached {
            if count <= CARD_CACHE_MAX && bytes <= CARD_CACHE_BYTES {
                break;
            }
            self.card_images.remove(&url);
            self.card_image_order.retain(|cached| cached != &url);
            count -= 1;
            bytes -= size;
        }
        let settled = !self.picture_settle.holding(Instant::now());
        for &url in urls {
            if !self.card_images.contains_key(url) {
                let slot = match self.card_previews.get(url) {
                    Some(image) => CardImageSlot::Cached {
                        image: image.clone(),
                        protocol: None,
                        refreshing: true,
                    },
                    None if settled => CardImageSlot::Loading,
                    None => continue,
                };
                self.card_images.insert(url.to_string(), slot);
            }
            if settled
                && matches!(
                    self.card_images.get(url),
                    Some(
                        CardImageSlot::Loading
                            | CardImageSlot::Cached {
                                refreshing: true,
                                ..
                            }
                    )
                )
            {
                self.pool.want_image(url);
                if let std::collections::hash_map::Entry::Vacant(entry) =
                    self.queued_images.entry(url.to_string())
                {
                    entry.insert(true);
                    self.pool.submit(self.image_job(url, true));
                }
            }
        }
    }

    pub(super) fn image_job(&self, url: &str, background: bool) -> Job {
        if self.video_source(url) {
            Job::VideoFrame {
                url: url.into(),
                background,
            }
        } else {
            Job::Image {
                url: url.into(),
                background,
            }
        }
    }

    pub(super) fn fetch_image(&mut self, url: &str, background: bool) {
        let fetch = match self.images.get_mut(url) {
            None | Some(ImageSlot::Failed(_)) => {
                self.images.insert(url.to_string(), ImageSlot::Loading);
                true
            }
            Some(ImageSlot::Ready { refresh, .. }) if *refresh == ImageRefresh::Needed => {
                *refresh = ImageRefresh::Loading;
                true
            }
            _ => false,
        };
        let promote = !background
            && self.queued_images.get(url) == Some(&true)
            && matches!(
                self.images.get(url),
                Some(
                    ImageSlot::Loading
                        | ImageSlot::Ready {
                            refresh: ImageRefresh::Loading,
                            ..
                        }
                )
            );
        if fetch || promote {
            self.pool.want_image(url);
            // A fetch still queued for it runs now that it is wanted again.
            let queued = self.queued_images.insert(url.to_string(), background);
            if queued.is_none() || (!background && queued == Some(true)) {
                self.pool.submit(self.image_job(url, background));
            }
        }
    }

    /// Pictures encoded for the previous protocol are dropped and drawn again with this one.
    pub fn replace_image_picker(&mut self, picker: Picker) {
        self.image_picker = picker;
        self.images.clear();
        self.thumbs.clear();
        self.card_renderer.clear();
        for slot in self.card_images.values_mut() {
            if let CardImageSlot::Ready { protocol, .. } | CardImageSlot::Cached { protocol, .. } =
                slot
            {
                *protocol = None;
            }
        }
        self.reload_image_sources();
    }

    /// Thumbnails and icons need a graphics protocol: half-blocks at a few cells are noise.
    pub fn thumbnails_on(&self) -> bool {
        self.show_images && self.image_picker.protocol_type() != ProtocolType::Halfblocks
    }

    /// The post view's image: not under NO_COLOR with half-blocks, which are only colors.
    pub fn inline_images_on(&self) -> bool {
        self.show_images && !self.no_images_possible()
    }

    pub(super) fn no_images_possible(&self) -> bool {
        self.no_color && self.image_picker.protocol_type() == ProtocolType::Halfblocks
    }

    /// A thumbnail or icon URL to draw, unless it failed; its row slot is reserved regardless.
    pub fn usable_thumb<'a>(&self, url: Option<&'a str>) -> Option<&'a str> {
        url.filter(|u| !matches!(self.thumbs.get(*u), Some(ImageSlot::Failed(_))))
    }

    /// After a frame: load drawn thumbnails, let off-screen queued ones be skipped, and past
    /// `THUMB_CACHE_MAX` keep only what is on screen or in flight. Rows a quick run of moves
    /// only passes are not fetched; the redraw once it stops fetches what it shows.
    pub fn thumbs_drawn(&mut self, urls: &[&str]) {
        self.pool.set_on_screen(urls);
        if !self.picture_settle.holding(Instant::now()) {
            for url in urls {
                let submit = match self.thumbs.get_mut(*url) {
                    None => {
                        self.thumbs.insert(url.to_string(), ImageSlot::Loading);
                        true
                    }
                    Some(ImageSlot::Ready { refresh, .. }) if *refresh == ImageRefresh::Needed => {
                        *refresh = ImageRefresh::Loading;
                        true
                    }
                    _ => false,
                };
                if submit {
                    self.pool.submit(if self.video_source(url) {
                        Job::VideoThumb {
                            url: url.to_string(),
                        }
                    } else {
                        Job::Thumb {
                            url: url.to_string(),
                        }
                    });
                }
            }
        }
        if self.thumbs.len() > THUMB_CACHE_MAX {
            let drawn: HashSet<&str> = urls.iter().copied().collect();
            self.thumbs.retain(|url, slot| {
                matches!(
                    slot,
                    ImageSlot::Loading
                        | ImageSlot::Ready {
                            refresh: ImageRefresh::Loading,
                            ..
                        }
                ) || drawn.contains(url.as_str())
            });
        }
    }
}
