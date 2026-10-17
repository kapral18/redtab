//! HTTP client for a Redlib instance.

pub mod parse;

use std::io::Read;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use image::DynamicImage;
use ureq::Agent;

use crate::model::{Comment, Listing, PostPage, Sort, TimeRange};

/// Settings are cookies. HLS makes videos playable (banner hidden); card layout and
/// the sidebar are pinned because other defaults drop media blocks and icons.
const BASE_COOKIES: &str =
    "use_hls=on; hide_hls_notification=on; layout=card; hide_sidebar_and_summary=off";
const PAGE_LIMIT: u64 = 16 * 1024 * 1024;
const IMAGE_LIMIT: u64 = 64 * 1024 * 1024;
/// Largest decoded image, as the `image` crate's default allocation limit.
const DECODE_LIMIT: u64 = 512 * 1024 * 1024;

#[derive(Clone)]
pub struct Redlib {
    base: String,
    cookies: String,
    agent: Agent,
}

impl Redlib {
    pub fn new(base: &str, show_nsfw: bool) -> Self {
        let agent: Agent = Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(20)))
            .http_status_as_error(false)
            .user_agent(concat!("redtab/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        let nsfw = if show_nsfw { "on" } else { "off" };
        Self {
            base: base.trim_end_matches('/').to_string(),
            cookies: format!("{BASE_COOKIES}; show_nsfw={nsfw}"),
            agent,
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// Every response-affecting setting is part of disk-cache identity.
    pub fn cache_namespace(&self) -> String {
        format!("{}\n{}", self.base, self.cookies)
    }

    /// Absolute URL for a Redlib path. Non-ASCII slugs are percent-encoded: Redlib
    /// answers raw UTF-8 with 400.
    pub fn absolute(&self, path: &str) -> String {
        let joined = if path.starts_with("http://") || path.starts_with("https://") {
            path.to_string()
        } else if path.starts_with('/') {
            format!("{}{path}", self.base)
        } else {
            format!("{}/{path}", self.base)
        };
        url::Url::parse(&joined).map_or(joined, String::from)
    }

    pub fn listing(
        &self,
        subreddit: &str,
        sort: Sort,
        time: TimeRange,
        after: Option<&str>,
    ) -> Result<Listing> {
        let mut url = format!("{}/r/{subreddit}/{}", self.base, sort.as_str());
        let mut query = Vec::new();
        if sort.takes_time() {
            query.push(format!("t={}", time.as_str()));
        }
        if let Some(after) = after {
            query.push(format!("after={after}"));
        }
        if !query.is_empty() {
            url.push('?');
            url.push_str(&query.join("&"));
        }
        parse::parse_listing(&self.get_text(&url)?)
    }

    pub fn search(&self, subreddit: &str, query: &str, after: Option<&str>) -> Result<Listing> {
        let mut url = url::Url::parse(&format!("{}/r/{subreddit}/search", self.base))?;
        url.query_pairs_mut()
            .append_pair("q", query)
            .append_pair("restrict_sr", "on")
            .append_pair("sort", "relevance")
            .append_pair("t", "all");
        if let Some(after) = after {
            url.query_pairs_mut().append_pair("after", after);
        }
        parse::parse_listing(&self.get_text(url.as_str())?)
    }

    pub fn post(&self, permalink: &str) -> Result<PostPage> {
        parse::parse_post_page(&self.get_text(&self.absolute(permalink))?)
    }

    /// Replies behind a "More replies" link: the replies of the comment it points at.
    pub fn more_replies(&self, path: &str) -> Result<Vec<Comment>> {
        parse::parse_more_replies(&self.get_text(&self.absolute(path))?)
    }

    pub fn image(&self, path: &str) -> Result<Vec<u8>> {
        Ok(self
            .image_response(path)?
            .body_mut()
            .with_config()
            .limit(IMAGE_LIMIT)
            .read_to_vec()?)
    }

    /// An image to show. A GIF gives its first frame and reading stops there, so the rest
    /// of the file is never downloaded however large it is; other formats are read whole.
    /// The format comes from the file, not the URL: Reddit serves `.gif?format=png8`
    /// video posters as PNG.
    pub fn still_image(&self, path: &str) -> Result<DynamicImage> {
        let mut resp = self.image_response(path)?;
        decode_still(resp.body_mut().with_config().limit(IMAGE_LIMIT).reader())
    }

    fn image_response(&self, path: &str) -> Result<ureq::http::Response<ureq::Body>> {
        let url = self.absolute(path);
        let resp = self
            .agent
            .get(&url)
            .header("Cookie", &self.cookies)
            .call()
            .with_context(|| format!("GET {url}"))?;
        if !resp.status().is_success() {
            bail!("GET {url}: HTTP {}", resp.status());
        }
        Ok(resp)
    }

    pub fn healthy(&self) -> bool {
        let agent: Agent = Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(3)))
            .http_status_as_error(false)
            .build()
            .into();
        agent
            .get(&format!("{}/settings", self.base))
            .call()
            .is_ok_and(|r| r.status().is_success())
    }

    fn get_text(&self, url: &str) -> Result<String> {
        let mut resp = self
            .agent
            .get(url)
            .header("Cookie", &self.cookies)
            .call()
            .with_context(|| format!("cannot reach Redlib at {}", self.base))?;
        let status = resp.status();
        let body = resp
            .body_mut()
            .with_config()
            .limit(PAGE_LIMIT)
            .read_to_string()?;
        // Redlib's error and NSFW-gate pages have error statuses; the parser reports them.
        let redlib_page =
            body.contains("<div id=\"error\">") || body.contains("<title>NSFW content gated");
        if !status.is_success() && !redlib_page {
            bail!("GET {url}: HTTP {status}");
        }
        Ok(body)
    }
}

/// Decodes an image; a GIF only up to its first frame, reading nothing past it.
fn decode_still(mut r: impl Read) -> Result<DynamicImage> {
    let mut bytes = Vec::with_capacity(4);
    (&mut r).take(4).read_to_end(&mut bytes)?;
    if bytes.starts_with(b"GIF8") {
        return first_gif_frame(bytes.as_slice().chain(r));
    }
    r.read_to_end(&mut bytes)?;
    Ok(image::load_from_memory(&bytes)?)
}

/// A GIF's first frame on its canvas, as `image` decodes GIFs.
fn first_gif_frame(r: impl Read) -> Result<DynamicImage> {
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
mod tests {
    use super::*;

    #[test]
    fn non_ascii_post_paths_are_percent_encoded() {
        let redlib = Redlib::new("http://127.0.0.1:8085/", false);

        assert_eq!(
            redlib.absolute("/r/pokemon/comments/1x/every_pokémon/"),
            "http://127.0.0.1:8085/r/pokemon/comments/1x/every_pok%C3%A9mon/"
        );
        assert_eq!(
            redlib.absolute("/preview/pre/a.jpg?width=140&s=abc"),
            "http://127.0.0.1:8085/preview/pre/a.jpg?width=140&s=abc"
        );
    }

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
