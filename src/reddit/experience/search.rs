//! Community, Comment, Media and People search through the pages reddit.com renders; the
//! classic `subreddits/search` and `users/search` listings answer some sessions with no
//! results, and the classic API has no comment or media search.

use scraper::{ElementRef, Html, Selector};

use super::*;
use crate::model::{Listing, TimeRange};

pub(in crate::reddit) const PATH: &str = "/svc/shreddit/search/";

impl Reddit {
    pub(super) fn search_rows(
        &self,
        text: &str,
        kind: ListSearch,
        after: Option<&str>,
    ) -> Result<Page> {
        let html = self.search_html(search_url(&self.0.api, text, kind.param(), &[], after)?)?;
        search_page(&html, text, kind, &self.0.api, self.1)
    }

    /// Media results are posts: the search page names them in order and each post's own
    /// thread JSON reads it; Reddit refuses `by_id` for browser sessions. A post that fails
    /// to load is left out; the page fails only when none loads.
    pub(crate) fn media_search_progress(
        &self,
        text: &str,
        sort: SearchSort,
        time: TimeRange,
        after: Option<&str>,
        progress: &mut dyn FnMut(super::super::ListingEvent<'_>),
    ) -> Result<Listing> {
        let params = [("sort", sort.as_str()), ("t", time.as_str())];
        let mut url = search_url(&self.0.api, text, MEDIA, &params, after)?;
        // Reddit's next-page link can leave out the chosen sort and time range.
        for (name, value) in params {
            if !url.query_pairs().any(|(key, _)| key == name) {
                url.query_pairs_mut().append_pair(name, value);
            }
        }
        let html = self.search_html(url)?;
        let (ids, after) = media_page(&html, text, &self.0.api)?;
        let mut posts = Vec::new();
        let mut failure = None;
        for id in &ids {
            match self.media_post(&id[3..]) {
                Ok(post) if self.1 || !post.nsfw => {
                    progress(super::super::ListingEvent::Post(&post));
                    posts.push(post);
                }
                Ok(_) => {}
                Err(error) => {
                    failure.get_or_insert(error);
                }
            }
        }
        if let Some(error) = failure.filter(|_| posts.is_empty()) {
            return Err(error);
        }
        Ok(Listing { posts, after })
    }

    fn media_post(&self, id: &str) -> Result<crate::model::Post> {
        let mut url = self.0.api.join(&format!("comments/{id}.json"))?;
        url.query_pairs_mut()
            .append_pair("raw_json", "1")
            .append_pair("limit", "1")
            .append_pair("depth", "1");
        Ok(parse::parse_post_page_value(self.get(url)?)?.post)
    }

    fn search_html(&self, url: Url) -> Result<String> {
        let response = self.request(url, "text/html")?;
        let mut html = String::new();
        use std::io::Read;
        transport::Reader::new(response, 8 * 1024 * 1024, Some(self.read_cancellation()))
            .read_to_string(&mut html)
            .context("Could not read Reddit search results")?;
        Ok(html)
    }
}

const MEDIA: &str = "media";

impl ListSearch {
    /// The `type` reddit.com's search page takes.
    pub(super) fn param(self) -> &'static str {
        match self {
            ListSearch::Communities => "communities",
            ListSearch::Comments => "comments",
            ListSearch::People => "people",
        }
    }
}

/// The first page, or a next-page cursor from an earlier page of the same search.
pub(super) fn search_url(
    api: &Url,
    text: &str,
    kind: &str,
    params: &[(&str, &str)],
    after: Option<&str>,
) -> Result<Url> {
    let text = text.trim();
    if text.is_empty() {
        bail!("Enter a search query");
    }
    let Some(after) = after else {
        let mut url = api.join(PATH.trim_start_matches('/'))?;
        url.query_pairs_mut()
            .append_pair("q", text)
            .append_pair("type", kind)
            .extend_pairs(params);
        return Ok(url);
    };
    let url = super::super::home::cursor_url(api, after, PATH)
        .ok_or_else(|| anyhow::anyhow!("invalid search cursor"))?;
    let pairs: Vec<_> = url.query_pairs().into_owned().collect();
    let param = |name: &str| {
        pairs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, v)| v.as_str())
    };
    if param("q").map(str::trim) != Some(text) || param("type") != Some(kind) {
        bail!("invalid search cursor");
    }
    Ok(url)
}

fn selector(css: &str) -> Selector {
    Selector::parse(css).expect("static selector")
}

/// The loader Reddit renders below a page for the same search's next page.
fn next_cursor(document: &Html, api: &Url, text: &str, kind: &str) -> Option<String> {
    document
        .select(&selector("faceplate-partial[src]"))
        .filter_map(|partial| partial.value().attr("src"))
        .find(|src| search_url(api, text, kind, &[], Some(src)).is_ok() && src.contains("cursor="))
        .map(str::to_string)
}

/// Rows this reader cannot read, or no rows without Reddit's own no-results message,
/// mean Reddit changed the page: say so instead of showing "No matches".
fn check_readable(document: &Html, found: bool, rows_seen: usize, unreadable: usize) -> Result<()> {
    let no_results = document
        .select(&selector(r#"[data-testid="search-error-message"]"#))
        .next()
        .is_some();
    if !found && (unreadable > 0 || (rows_seen == 0 && !no_results)) {
        bail!("Reddit returned an unrecognized search page; update redtab");
    }
    Ok(())
}

/// The tracking context Reddit attaches to a result: its ids, names and flags.
fn context(row: ElementRef) -> Option<Value> {
    row.select(&selector(
        "search-telemetry-tracker[data-faceplate-tracking-context]",
    ))
    .next()
    .and_then(|tracker| tracker.value().attr("data-faceplate-tracking-context"))
    .and_then(|context| serde_json::from_str(context).ok())
}

pub(super) fn search_page(
    html: &str,
    text: &str,
    kind: ListSearch,
    api: &Url,
    show_nsfw: bool,
) -> Result<Page> {
    let document = Html::parse_document(html);
    let rows = selector(match kind {
        ListSearch::Communities => r#"[data-testid="search-community"]"#,
        ListSearch::Comments => r#"[data-testid="search-sdui-comment-unit"]"#,
        ListSearch::People => r#"[data-testid="search-author"]"#,
    });
    let mut page = Page {
        title: format!(
            "{}: {text}",
            match kind {
                ListSearch::Communities => "Communities",
                ListSearch::Comments => "Comments",
                ListSearch::People => "People",
            }
        ),
        text: String::new(),
        entries: Vec::new(),
        after: None,
        url: Query::Search {
            text: text.into(),
            kind,
        }
        .url()?,
    };
    let (mut rows_seen, mut unreadable) = (0, 0);
    for row in document.select(&rows) {
        rows_seen += 1;
        let Some(context) = context(row) else {
            unreadable += 1;
            continue;
        };
        let named = |key: &str| context[key]["name"].as_str().map(str::to_string);
        let entry = match kind {
            ListSearch::Communities => named("subreddit")
                .map(|name| community(row, &context["subreddit"], &name, show_nsfw)),
            ListSearch::People => named("profile").map(|name| person(row, &name, show_nsfw)),
            ListSearch::Comments => comment(row, &context, show_nsfw).ok().map(Ok),
        };
        match entry {
            Some(entry) => page.entries.extend(entry?),
            None => unreadable += 1,
        }
    }
    page.after = next_cursor(&document, api, text, kind.param());
    check_readable(&document, !page.entries.is_empty(), rows_seen, unreadable)?;
    Ok(page)
}

/// The post fullnames of a media search page, in Reddit's order, and its next-page cursor.
pub(super) fn media_page(
    html: &str,
    text: &str,
    api: &Url,
) -> Result<(Vec<String>, Option<String>)> {
    let document = Html::parse_document(html);
    let (mut ids, mut rows_seen, mut unreadable) = (Vec::<String>::new(), 0, 0);
    for row in document.select(&selector(r#"[data-id="search-media-post-unit"]"#)) {
        rows_seen += 1;
        match context(row)
            .and_then(|context| context["post"]["id"].as_str().map(str::to_string))
            .filter(|id| fullname(id, false).is_ok())
        {
            Some(id) if !ids.contains(&id) => ids.push(id),
            Some(_) => {}
            None => unreadable += 1,
        }
    }
    check_readable(&document, !ids.is_empty(), rows_seen, unreadable)?;
    Ok((ids, next_cursor(&document, api, text, MEDIA)))
}

fn description(row: ElementRef) -> String {
    row.select(&selector(r#"[data-testid="search-subreddit-desc-text"]"#))
        .next()
        .map(|p| p.text().collect::<String>().trim().to_string())
        .unwrap_or_default()
}

/// The first picture `css` selects is the community icon or the person's avatar; a row
/// without one leaves the icon unknown, so the community's `about.json` can still supply it.
fn icon(row: ElementRef, css: &str) -> crate::model::SubscriptionIcon {
    match row
        .select(&selector(css))
        .next()
        .and_then(|img| img.value().attr("src"))
    {
        Some(src) if Url::parse(src).is_ok_and(|url| transport::media_allowed(&url)) => {
            crate::model::SubscriptionIcon::Url(src.into())
        }
        Some(_) => crate::model::SubscriptionIcon::Missing,
        None => crate::model::SubscriptionIcon::Unknown,
    }
}

fn community(row: ElementRef, data: &Value, sub: &str, show_nsfw: bool) -> Result<Option<Entry>> {
    name(sub)?;
    if !show_nsfw && data["nsfw"] == true {
        return Ok(None);
    }
    Ok(Some(Entry {
        icon: icon(row, "img[src]"),
        title: format!("r/{sub}"),
        body: text::body("", &description(row)),
        target: None,
        query: Some(Query::Community(sub.into())),
        post: None,
    }))
}

fn person(row: ElementRef, user: &str, show_nsfw: bool) -> Result<Option<Entry>> {
    name(user)?;
    let nsfw = row
        .select(&selector(
            r#"[data-testid="search-warnings"] .text-category-nsfw"#,
        ))
        .next()
        .is_some();
    if !show_nsfw && nsfw {
        return Ok(None);
    }
    let karma = row
        .select(&selector("faceplate-number[number]"))
        .next()
        .and_then(|number| number.value().attr("number"))
        .map(|number| format!("Karma: {number}"))
        .unwrap_or_default();
    Ok(Some(Entry {
        icon: icon(row, "img[src]"),
        title: format!("u/{user}"),
        body: format!("{karma}\n{}", text::body("", &description(row))),
        target: None,
        query: Some(Query::Profile(user.into())),
        post: None,
    }))
}

/// A matched comment under its post's title: the body's first line says where and who,
/// the rest is the comment. It opens its thread at the comment.
fn comment(row: ElementRef, context: &Value, show_nsfw: bool) -> Result<Option<Entry>> {
    let id = context["comment"]["id"].as_str().unwrap_or_default();
    let post = context["post"]["id"].as_str().unwrap_or_default();
    let sub = context["subreddit"]["name"].as_str().unwrap_or_default();
    let comment_id = id
        .strip_prefix("t1_")
        .filter(|_| fullname(id, true).is_ok())
        .context("Invalid Reddit content identity")?;
    let post_id = fullname(post, false)?.trim_start_matches("t3_");
    name(sub)?;
    if !show_nsfw && (context["post"]["nsfw"] == true || context["subreddit"]["nsfw"] == true) {
        return Ok(None);
    }
    let author = row
        .select(&selector(r#"faceplate-partial[src*="/user-hover-card/"]"#))
        .next()
        .and_then(|partial| partial.value().attr("src"))
        .and_then(|src| src.split("/user-hover-card/").nth(1))
        .and_then(|rest| rest.split(['?', '/']).next())
        .filter(|user| name(user).is_ok())
        .unwrap_or("[deleted]")
        .to_string();
    let votes = row
        .select(&selector(
            r#"[data-testid="search-comment-content"] faceplate-number[number]"#,
        ))
        .next()
        .and_then(|number| number.value().attr("number"))
        .and_then(|number| number.parse::<i64>().ok());
    let body = row
        .select(&selector(r#"[id$="-post-rtjson-content"]"#))
        .next()
        .map(|content| text::body(&content.inner_html(), ""))
        .unwrap_or_default();
    let meta = [
        Some(format!("r/{sub}")),
        Some(format!("u/{author}")),
        votes.map(|votes| format!("{votes} {}", if votes == 1 { "vote" } else { "votes" })),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    let permalink = format!("/r/{sub}/comments/{post_id}/_/{comment_id}/");
    Ok(Some(Entry {
        icon: icon(row, r#"[data-id="community-hover-card"] img[src]"#),
        title: context["post"]["title"]
            .as_str()
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .unwrap_or("Untitled post")
            .to_string(),
        body: format!("{meta}\n{}", body.trim()),
        target: Some(Target {
            id: id.into(),
            author,
            subreddit: sub.into(),
            permalink: permalink.clone(),
        }),
        query: Some(Query::Thread(permalink)),
        post: None,
    }))
}
