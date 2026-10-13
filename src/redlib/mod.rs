//! HTTP client for a Redlib instance.

pub mod parse;

use std::time::Duration;

use anyhow::{Context, Result, bail};
use ureq::Agent;

use crate::model::{Comment, Listing, PostPage, Sort, TimeRange};

/// Redlib settings are cookies. HLS makes Reddit-hosted videos playable;
/// the notification cookie hides Redlib's "Enable HLS" banner.
const BASE_COOKIES: &str = "use_hls=on; hide_hls_notification=on";
const PAGE_LIMIT: u64 = 16 * 1024 * 1024;
const IMAGE_LIMIT: u64 = 64 * 1024 * 1024;

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

    /// Absolute URL for a Redlib-relative path; absolute URLs pass through.
    pub fn absolute(&self, path: &str) -> String {
        if path.starts_with("http://") || path.starts_with("https://") {
            path.to_string()
        } else if path.starts_with('/') {
            format!("{}{path}", self.base)
        } else {
            format!("{}/{path}", self.base)
        }
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
        let url = self.absolute(path);
        let mut resp = self
            .agent
            .get(&url)
            .header("Cookie", &self.cookies)
            .call()
            .with_context(|| format!("GET {url}"))?;
        if !resp.status().is_success() {
            bail!("GET {url}: HTTP {}", resp.status());
        }
        Ok(resp
            .body_mut()
            .with_config()
            .limit(IMAGE_LIMIT)
            .read_to_vec()?)
    }

    /// True when Redlib answers its settings page.
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
        if !status.is_success() && !body.contains("<title>Error") {
            bail!("GET {url}: HTTP {status}");
        }
        Ok(body)
    }
}
