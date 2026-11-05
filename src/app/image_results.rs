use super::*;

impl App {
    pub(super) fn on_image(
        &mut self,
        url: String,
        result: Result<crate::worker::LoadedImage, String>,
        refreshing: bool,
    ) {
        if !refreshing {
            self.queued_images.remove(&url);
        }
        // A cached snapshot stays usable until its live refresh finishes.
        let updating = matches!(
            self.images.get(&url),
            Some(ImageSlot::Ready {
                refresh: ImageRefresh::Loading,
                ..
            })
        );
        if updating && result.is_err() {
            if let Some(ImageSlot::Ready { refresh, .. }) = self.images.get_mut(&url) {
                *refresh = ImageRefresh::Failed;
            }
        } else if matches!(self.images.get(&url), Some(ImageSlot::Loading)) || updating {
            let slot = match &result {
                Ok(img) => ImageSlot::Ready {
                    pixels: (img.image.width(), img.image.height()),
                    protocol: Box::new(self.image_picker.new_resize_protocol((*img.image).clone())),
                    refresh: if refreshing {
                        ImageRefresh::Loading
                    } else {
                        ImageRefresh::Idle
                    },
                },
                Err(e) => ImageSlot::Failed(e.clone()),
            };
            self.images.insert(url.clone(), slot);
        }
        if matches!(
            self.card_images.get(&url),
            Some(
                CardImageSlot::Loading
                    | CardImageSlot::Cached {
                        refreshing: true,
                        ..
                    }
            )
        ) {
            if result.is_err()
                && let Some(CardImageSlot::Cached { refreshing, .. }) =
                    self.card_images.get_mut(&url)
            {
                *refreshing = false;
                return;
            }
            let slot = match result {
                Ok(image) => {
                    self.card_image_metadata.insert(url.clone(), image.metadata);
                    self.card_previews.insert(url.clone(), image.preview);
                    let image = image.image;
                    self.card_preview_order.retain(|cached| cached != &url);
                    self.card_preview_order.push_back(url.clone());
                    while self.card_preview_order.len() > CARD_PREVIEW_MAX {
                        if let Some(old) = self.card_preview_order.pop_front() {
                            self.card_previews.remove(&old);
                        }
                    }
                    if refreshing {
                        CardImageSlot::Cached {
                            image,
                            protocol: None,
                            refreshing: true,
                        }
                    } else {
                        CardImageSlot::Ready {
                            image,
                            protocol: None,
                        }
                    }
                }
                Err(error) => CardImageSlot::Failed(error),
            };
            self.card_images.insert(url, slot);
        }
    }
}
