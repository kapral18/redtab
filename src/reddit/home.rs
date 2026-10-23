//! Browser Home partials supply ranking and pagination; JSON supplies post details.

use std::collections::HashSet;

use anyhow::{Context, Result, bail};
use scraper::{Html, Selector};
use url::Url;

pub(super) const PATH: &str = "/svc/shreddit/feeds/home-feed";
pub(super) const ACCEPT: &str = "text/vnd.reddit.partial+html";

pub(super) struct Page {
    pub ids: Vec<String>,
    pub after: Option<String>,
}

pub(super) fn url(api: &Url, cursor: &str) -> Result<Url> {
    let url = api
        .join(cursor)
        .map_err(|_| anyhow::anyhow!("invalid Home cursor"))?;
    if url.origin() != api.origin()
        || url.path() != PATH
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        bail!("invalid Home cursor origin or path");
    }
    Ok(url)
}

pub(super) fn parse(html: &str, api: &Url) -> Result<Page> {
    let document = Html::parse_fragment(html);
    let posts = Selector::parse("shreddit-post").expect("static selector");
    let next = Selector::parse("#feed-next-page-partial").expect("static selector");
    if document.select(&posts).next().is_none() && document.select(&next).next().is_none() {
        bail!("Reddit returned an unrecognized Home feed; try refreshing or signing in again");
    }
    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    for post in document.select(&posts) {
        if post.value().attr("is-ad").is_some() {
            continue;
        }
        let id = post
            .value()
            .attr("id")
            .context("Home post has no identity")?;
        let bare = id
            .strip_prefix("t3_")
            .context("invalid Home post identity")?;
        if bare.is_empty() || bare.len() > 32 || !bare.bytes().all(|c| c.is_ascii_alphanumeric()) {
            bail!("invalid Home post identity");
        }
        if seen.insert(id) {
            ids.push(id.to_string());
        }
    }
    let after = document
        .select(&next)
        .next()
        .and_then(|el| el.value().attr("src"))
        .map(|cursor| url(api, cursor).map(|url| url.to_string()))
        .transpose()?;
    Ok(Page { ids, after })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn when_home_markup_is_not_a_feed_it_should_fail_instead_of_returning_empty() {
        let api = Url::parse("https://www.reddit.com/").unwrap();
        assert!(parse("<html><body>Sign in</body></html>", &api).is_err());
        assert!(parse("", &api).is_err());
        assert!(parse("<shreddit-post id='bad/id'></shreddit-post>", &api).is_err());
    }

    #[test]
    fn when_a_cursor_leaves_home_it_should_fail_before_authentication() {
        let api = Url::parse("https://www.reddit.com/").unwrap();
        for cursor in [
            "https://other.example/svc/shreddit/feeds/home-feed",
            "/api/delete",
            "https://user:pass@www.reddit.com/svc/shreddit/feeds/home-feed",
            "/svc/shreddit/feeds/home-feed#fragment",
        ] {
            assert!(url(&api, cursor).is_err(), "{cursor}");
        }
    }

    #[test]
    fn when_a_feed_contains_ads_and_duplicates_it_should_keep_only_unique_organic_ids() {
        let page = parse("<shreddit-post id='t3_a'></shreddit-post><shreddit-post id='t3_a'></shreddit-post><shreddit-post id='t3_ad' is-ad></shreddit-post><shreddit-ad-post id='t3_ad2'></shreddit-ad-post>", &Url::parse("https://www.reddit.com/").unwrap()).unwrap();
        assert_eq!(page.ids, ["t3_a"]);
        assert!(page.after.is_none());
    }

    #[test]
    fn when_the_feed_end_marker_is_empty_it_should_complete_without_more_posts() {
        let page = parse(
            "<faceplate-partial id='feed-next-page-partial'></faceplate-partial>",
            &Url::parse("https://www.reddit.com/").unwrap(),
        )
        .unwrap();
        assert!(page.ids.is_empty() && page.after.is_none());
    }
}
