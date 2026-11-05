use super::*;

#[derive(Default)]
pub(super) struct Encoder {
    pub(super) downscaled: Option<(Weak<DynamicImage>, u32, Arc<DynamicImage>)>,
    pub(super) residency: Arc<Mutex<NativeResidency>>,
}

impl Encoder {
    pub(super) fn encode(&mut self, request: &Request) -> Result<Tile, String> {
        let width = u32::from(request.key.size.width) * u32::from(request.key.font.0.max(1));
        let sample = if width < request.image.width() {
            if self
                .downscaled
                .as_ref()
                .is_none_or(|(source, old_width, _)| {
                    !Weak::ptr_eq(source, &Arc::downgrade(&request.image)) || *old_width != width
                })
            {
                self.downscaled = Some((
                    Arc::downgrade(&request.image),
                    width,
                    Arc::new(request.image.resize(
                        width,
                        request.image.height(),
                        ratatui_image::FilterType::Triangle,
                    )),
                ));
            }
            self.downscaled
                .as_ref()
                .map(|(_, _, pixels)| pixels.as_ref())
        } else {
            None
        };
        let kitty_video = request.key.video
            && request.picker.protocol_type() == ratatui_image::picker::ProtocolType::Kitty;
        let scale = if kitty_video {
            let (fit, _, _) = picture_fit(&request.image, &request.key);
            let height = f64::from(request.key.size.height) * f64::from(request.key.font.1.max(1));
            let badge_scale = if request.key.overlay.is_some() {
                BADGE_MIN_PIXELS / f64::from(width)
            } else {
                0.0
            };
            fit.recip()
                .max(badge_scale)
                .min(1.0)
                .min((crate::video::MAX_VIDEO_PIXELS / (f64::from(width) * height)).sqrt())
        } else {
            1.0
        };
        let pixels = tile_pixels(&request.image, sample, &request.key, scale);
        let cols = request.key.cols;
        let size = Size::new(cols, request.key.rows);
        if !request.key.video
            && request.picker.protocol_type() == ratatui_image::picker::ProtocolType::Kitty
        {
            let id = 0x8000_0000 | (NEXT_IMAGE_ID.fetch_add(1, Ordering::Relaxed) & 0x7fff_ffff);
            let frame = png_frame(DynamicImage::ImageRgba8(pixels))?;
            let upload =
                crate::video::kitty_transmit(id, &frame, size.width, size.height, request.key.tmux);
            return Ok(Tile::KittyImage {
                upload,
                size,
                id,
                decoded_bytes: frame.width as usize * frame.height as usize * 4,
                residency: Arc::clone(&self.residency),
            });
        }
        if kitty_video {
            let mut hash = std::collections::hash_map::DefaultHasher::new();
            request.key.hash(&mut hash);
            // Upload into the hidden buffer; synchronized placeholders reveal it later.
            request.buffer.hash(&mut hash);
            std::process::id().hash(&mut hash);
            let id = 0x0080_0000 | (hash.finish() as u32 & 0x007f_ffff);
            let frame = png_frame(DynamicImage::ImageRgb8(
                DynamicImage::ImageRgba8(pixels).to_rgb8(),
            ))?;
            let upload =
                crate::video::kitty_transmit(id, &frame, size.width, size.height, request.key.tmux);
            return Ok(Tile::Kitty {
                upload,
                frame,
                size,
                id,
                buffer: request.buffer,
                sent: AtomicBool::new(false),
                residency: Arc::clone(&self.residency),
            });
        }
        request
            .picker
            .new_protocol(
                DynamicImage::ImageRgba8(pixels),
                size,
                Resize::Scale(Some(ratatui_image::FilterType::Triangle)),
            )
            .map(Tile::Image)
            .map_err(|error| error.to_string())
    }
}

pub(super) fn png_frame(image: DynamicImage) -> Result<crate::video::Frame, String> {
    let mut png = Vec::new();
    image::ImageEncoder::write_image(
        image::codecs::png::PngEncoder::new_with_quality(
            &mut png,
            image::codecs::png::CompressionType::Fast,
            image::codecs::png::FilterType::Paeth,
        ),
        image.as_bytes(),
        image.width(),
        image.height(),
        image.color().into(),
    )
    .map_err(|error| error.to_string())?;
    Ok(crate::video::Frame {
        width: image.width(),
        height: image.height(),
        pixels: crate::video::Pixels::Png(png),
        same_as_first: false,
    })
}

pub(super) fn tile_pixels(
    image: &DynamicImage,
    sample: Option<&DynamicImage>,
    key: &Key,
    scale: f64,
) -> image::RgbaImage {
    let cols = key.cols;
    let full_width = f64::from(key.size.width) * f64::from(key.font.0.max(1));
    let full_height = f64::from(key.size.height) * f64::from(key.font.1.max(1));
    let top = f64::from(key.start) * f64::from(key.font.1.max(1));
    let left = f64::from(key.left) * f64::from(key.font.0.max(1));
    let width = (f64::from(cols) * f64::from(key.font.0.max(1)) * scale)
        .ceil()
        .max(1.0) as u32;
    let height = (f64::from(key.rows) * f64::from(key.font.1.max(1)) * scale)
        .ceil()
        .max(1.0) as u32;
    let sample = sample.unwrap_or(image);
    let rgb = key.video.then(|| sample.as_rgb8()).flatten();
    let (_, content_width, content_height) = picture_fit(image, key);
    // Retain at least one encoded pixel for extreme aspect ratios.
    let content_width = (content_width * scale).ceil().max(1.0) / scale;
    let content_height = (content_height * scale).ceil().max(1.0) / scale;
    let pad_left = ((full_width - content_width) * scale / 2.0).floor() / scale;
    let pad_top = ((full_height - content_height) * scale / 2.0).floor() / scale;
    let mut pixels = image::RgbaImage::from_fn(width, height, |x, y| {
        let x = left + (f64::from(x) + 0.5) / scale;
        let y = top + (f64::from(y) + 0.5) / scale;
        let u = (x - pad_left) / content_width;
        let v = (y - pad_top) / content_height;
        if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
            return if key.video {
                image::Rgba([17, 17, 27, 255])
            } else {
                image::Rgba([0, 0, 0, 0])
            };
        }
        match rgb {
            Some(rgb) => image::imageops::sample_bilinear(rgb, u as f32, v as f32)
                .map(|pixel| image::Rgba([pixel[0], pixel[1], pixel[2], 255])),
            None => image::imageops::sample_bilinear(sample, u as f32, v as f32),
        }
        .unwrap_or(image::Rgba([0, 0, 0, 0]))
    });
    if let Some(overlay) = &key.overlay {
        overlay.paint(
            &mut pixels,
            ((left * scale) as u32, (top * scale) as u32),
            OverlayGeometry {
                width: (full_width * scale).ceil() as u32,
                top: (f64::from(overlay.top) * f64::from(key.font.1.max(1)) * scale) as u32,
                height: (f64::from(overlay.rows) * f64::from(key.font.1.max(1)) * scale).ceil()
                    as u32,
                font_height: (f64::from(key.font.1.max(1)) * scale).round().max(1.0) as u32,
            },
        );
    }
    pixels
}

/// Fit the source into its stable cell reservation, including rounded cell edges.
pub(super) fn picture_fit(image: &DynamicImage, key: &Key) -> (f64, f64, f64) {
    let width = f64::from(key.size.width) * f64::from(key.font.0.max(1));
    let height = f64::from(key.size.height) * f64::from(key.font.1.max(1));
    let pixels = if key.video {
        (image.width(), image.height())
    } else {
        key.pixels
    };
    let (w, h) = (f64::from(pixels.0.max(1)), f64::from(pixels.1.max(1)));
    let scale = (width / w).min(height / h);
    (scale, w * scale, h * scale)
}

#[cfg(test)]
pub(crate) fn encode_tile(
    picker: &Picker,
    image: &DynamicImage,
    sample: Option<&DynamicImage>,
    (size, pixels, overlay): (Size, (u32, u32), Option<&Overlay>),
    start: u16,
    rows: u16,
    column: u16,
) -> Result<Protocol, String> {
    let mut key = Key::new("test-image", picker, size, pixels, start, rows, column);
    key.overlay = overlay.cloned();
    let pixels = tile_pixels(image, sample, &key, 1.0);
    picker
        .new_protocol(
            DynamicImage::ImageRgba8(pixels),
            Size::new(TILE_COLS.min(size.width - column * TILE_COLS), rows),
            Resize::Scale(Some(ratatui_image::FilterType::Triangle)),
        )
        .map_err(|error| error.to_string())
}
