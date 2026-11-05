use super::*;

impl Reddit {
    pub fn image(&self, url: &str) -> Result<Vec<u8>> {
        if self.0.retired.load(Ordering::Acquire) {
            bail!("This Reddit connection was replaced");
        }
        let mut bytes = Vec::new();
        let cancelled = self.read_cancellation();
        Reader::new(
            self.0.transport.media(url, None, Some(&cancelled))?,
            IMAGE_LIMIT,
            Some(cancelled),
        )
        .read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    pub fn still_image(&self, url: &str) -> Result<DynamicImage> {
        if self.0.retired.load(Ordering::Acquire) {
            bail!("This Reddit connection was replaced");
        }
        let cancelled = self.read_cancellation();
        decode_still(Reader::new(
            self.0.transport.media(url, None, Some(&cancelled))?,
            IMAGE_LIMIT,
            Some(cancelled),
        ))
    }

    pub(crate) fn video_fallback(
        &self,
        url: &str,
        cancelled: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Result<VideoManifest> {
        let mut manifest = Url::parse(url).context("invalid video URL")?;
        let folder = manifest
            .path()
            .strip_suffix("HLSPlaylist.m3u8")
            .filter(|folder| folder.ends_with('/'))
            .context("video has no native DASH alternative")?;
        let path = format!("{folder}DASHPlaylist.mpd");
        manifest.set_path(&path);
        let connection = self.read_cancellation();
        let cancelled: transport::Cancellation = Arc::new(move || connection() || cancelled());
        let response = self
            .0
            .transport
            .media(manifest.as_str(), None, Some(&cancelled))?;
        let final_url = Url::parse(&response.uri().to_string()).context("invalid DASH location")?;
        let mut text = String::new();
        Reader::new(response, 1024 * 1024, Some(cancelled)).read_to_string(&mut text)?;
        dash::parse(&text, &final_url, |url| self.media_url(url))
    }

    pub fn media_url(&self, url: &str) -> Result<String> {
        if self.0.retired.load(Ordering::Acquire) {
            bail!("This Reddit connection was replaced");
        }
        let parsed = Url::parse(url).context("invalid media URL")?;
        if !self.0.transport.media_allowed(&parsed) {
            bail!("unsupported media host or protocol");
        }
        if self.0.relay.get().is_none() {
            let relay = relay::Relay::start(self.0.transport.clone())?;
            let _ = self.0.relay.set(relay);
        }
        Ok(self
            .0
            .relay
            .get()
            .expect("relay initialized")
            .url(parsed.as_str()))
    }
}

/// Decodes an image; a GIF only up to its first frame, reading nothing past it.
pub(super) fn decode_still(mut r: impl Read) -> Result<DynamicImage> {
    let mut bytes = Vec::with_capacity(4);
    (&mut r).take(4).read_to_end(&mut bytes)?;
    if bytes.starts_with(b"GIF8") {
        return first_gif_frame(bytes.as_slice().chain(r));
    }
    r.read_to_end(&mut bytes)?;
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(STILL_DECODE_LIMIT);
    reader.limits(limits);
    reader.decode().map_err(|error| match error {
        image::ImageError::Limits(_) => {
            anyhow::anyhow!("Image too large for inline preview; open the original on Reddit")
        }
        error => error.into(),
    })
}

/// A GIF's first frame on its canvas, as `image` decodes GIFs.
pub(super) fn first_gif_frame(r: impl Read) -> Result<DynamicImage> {
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);
    options.set_memory_limit(gif::MemoryLimit::Bytes(
        DECODE_LIMIT.try_into().expect("non-zero limit"),
    ));
    let mut decoder = options.read_info(r).context("not a GIF")?;
    let (width, height) = (u32::from(decoder.width()), u32::from(decoder.height()));
    // The frame is bounded by the decoder; the canvas, a header field, is not.
    if u64::from(width) * u64::from(height) * 4 > DECODE_LIMIT {
        bail!("the GIF is too large: {width}×{height}");
    }
    let frame = decoder.read_next_frame()?.context("the GIF has no frame")?;
    let part = image::ImageBuffer::<image::Rgba<u8>, &[u8]>::from_raw(
        frame.width.into(),
        frame.height.into(),
        &frame.buffer[..],
    )
    .context("a cut GIF frame")?;
    let mut canvas = image::RgbaImage::new(width, height);
    image::imageops::replace(&mut canvas, &part, frame.left.into(), frame.top.into());
    Ok(DynamicImage::ImageRgba8(canvas))
}

#[cfg(test)]
mod image_tests {
    use super::*;

    /// A GIF of `frames` frames, each `(left, top, width, height, rgba)`, on a canvas of
    /// `screen`, without the trailer that ends the file.
    fn gif_without_trailer(
        screen: (u16, u16),
        frames: &[(u16, u16, u16, u16, Vec<u8>)],
    ) -> Vec<u8> {
        let mut encoder = gif::Encoder::new(Vec::new(), screen.0, screen.1, &[]).unwrap();
        for (left, top, width, height, rgba) in frames {
            let mut frame = gif::Frame::from_rgba(*width, *height, &mut rgba.clone());
            frame.left = *left;
            frame.top = *top;
            encoder.write_frame(&frame).unwrap();
        }
        let mut bytes = encoder.into_inner().unwrap();
        assert_eq!(bytes.pop(), Some(0x3B), "the trailer");
        bytes
    }

    /// Reading past what it was given fails.
    struct Nothing;

    impl Read for Nothing {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("read past the first frame"))
        }
    }

    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];

    #[test]
    fn when_an_original_exceeds_inline_memory_limits_it_should_offer_the_reddit_handoff() {
        // RGB8000×6000 header (144MB), with a deliberately short pixel stream: reject
        // the declared allocation before attempting to decode those pixels.
        let png = [
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 31, 64, 0, 0, 23,
            112, 8, 2, 0, 0, 0, 190, 6, 115, 105, 0, 0, 0, 12, 73, 68, 65, 84, 120, 156, 99, 96,
            96, 96, 0, 0, 0, 4, 0, 1, 246, 23, 56, 85, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96,
            130,
        ];

        let error = decode_still(png.as_slice()).unwrap_err().to_string();

        assert_eq!(
            error,
            "Image too large for inline preview; open the original on Reddit"
        );
    }

    #[test]
    fn when_a_gif_streams_its_first_frame_should_decode_without_reading_further() {
        let first = gif_without_trailer((2, 2), &[(0, 0, 2, 2, [RED, BLUE, BLUE, RED].concat())]);

        let actual = decode_still(first.as_slice().chain(Nothing))
            .unwrap()
            .to_rgba8();

        let pixels: Vec<[u8; 4]> = actual.pixels().map(|p| p.0).collect();
        assert_eq!(pixels, [RED, BLUE, BLUE, RED]);
    }

    #[test]
    fn when_the_first_frame_is_smaller_than_the_canvas_it_should_sit_at_its_offset() {
        let first = gif_without_trailer((2, 2), &[(1, 0, 1, 2, [RED, BLUE].concat())]);

        let actual = decode_still(first.as_slice().chain(Nothing))
            .unwrap()
            .to_rgba8();

        let clear = [0, 0, 0, 0];
        let pixels: Vec<[u8; 4]> = actual.pixels().map(|p| p.0).collect();
        assert_eq!(pixels, [clear, RED, clear, BLUE]);
    }

    #[test]
    fn when_a_gif_url_serves_a_png_it_should_still_decode() {
        // Reddit's `.gif?format=png8` video posters are PNG files.
        let mut png = Vec::new();
        image::RgbaImage::from_raw(1, 1, RED.to_vec())
            .unwrap()
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();

        let actual = decode_still(png.as_slice()).unwrap().to_rgba8();

        assert_eq!(actual.get_pixel(0, 0).0, RED);
    }

    #[test]
    fn when_a_gif_canvas_is_too_large_it_should_fail_without_allocating_it() {
        // 65535×65535 RGBA is 17 GB; its first frame is one pixel.
        let huge = gif_without_trailer((65535, 65535), &[(0, 0, 1, 1, RED.to_vec())]);

        let actual = decode_still(huge.as_slice().chain(Nothing));

        let error = actual.unwrap_err().to_string();
        assert!(error.contains("too large"), "{error}");
    }
}
