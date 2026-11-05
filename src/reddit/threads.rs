use super::*;

impl Reddit {
    pub fn post(&self, permalink: &str) -> Result<PostPage> {
        self.sorted_post(permalink, "")
    }

    pub fn sorted_post(&self, permalink: &str, sort: &str) -> Result<PostPage> {
        self.focused_post(permalink, sort, None)
    }

    pub fn focused_post(
        &self,
        permalink: &str,
        sort: &str,
        comment: Option<&str>,
    ) -> Result<PostPage> {
        let mut page = parse::parse_post_page_value(self.thread_json(permalink, sort, comment)?)?;
        parse::set_reply_sort(&mut page.comments, sort)?;
        if page.post.nsfw && !self.1 {
            bail!("This is NSFW; press F2 and enable Show NSFW to see it");
        }
        Ok(page)
    }

    pub(super) fn thread_json(
        &self,
        permalink: &str,
        sort: &str,
        comment: Option<&str>,
    ) -> Result<Value> {
        if !matches!(
            sort,
            "" | "confidence" | "top" | "new" | "old" | "controversial" | "qa"
        ) {
            bail!("Invalid comment sort");
        }
        // Use only the post id; no slug or external URL can change the authenticated origin.
        let id = parse::post_id(permalink).context("invalid post permalink")?;
        let mut url = self.0.api.join(&format!("comments/{id}.json"))?;
        url.query_pairs_mut()
            .append_pair("raw_json", "1")
            .append_pair("limit", "100")
            .append_pair("depth", "10");
        if !sort.is_empty() {
            url.query_pairs_mut().append_pair("sort", sort);
        }
        if let Some(comment) = comment {
            if comment.is_empty() || !comment.bytes().all(|c| c.is_ascii_alphanumeric()) {
                bail!("Invalid comment identity");
            }
            url.query_pairs_mut()
                .append_pair("comment", comment)
                .append_pair("context", "3");
        }
        self.get(url)
    }

    pub fn more_replies(&self, cursor: &str) -> Result<ReplyPage> {
        let more: parse::MoreRequest =
            serde_json::from_str(cursor).context("invalid reply cursor")?;
        more.validate()?;
        let deadline = std::cell::Cell::new(Instant::now() + Duration::from_secs(20));
        let fetch = |comment: Option<&str>, context: &str| -> Result<Value> {
            let mut url = self.0.api.join(&format!("comments/{}.json", more.post))?;
            url.query_pairs_mut()
                .append_pair("context", context)
                .append_pair("depth", "10")
                .append_pair("limit", "100")
                .append_pair("raw_json", "1");
            if let Some(comment) = comment {
                url.query_pairs_mut().append_pair("comment", comment);
            }
            if !more.sort.is_empty() {
                url.query_pairs_mut().append_pair("sort", &more.sort);
            }
            let timeout = deadline.get().saturating_duration_since(Instant::now());
            if timeout.is_zero() {
                bail!("Comment expansion timed out; retry to load more replies");
            }
            let (response, waited) =
                self.send_with_timeout(url, "application/json", timeout, |client, url| {
                    client.get(url)
                })?;
            deadline.set(deadline.get() + waited);
            transport::json(response, Some(self.read_cancellation()))
        };
        let mut page = if more.children.is_empty() {
            parse::focused_replies(fetch(more.parent.strip_prefix("t1_"), "0")?, &more)?
        } else {
            parse::expanded_replies(&more, |comment| fetch(Some(comment), "8"))?
        };
        parse::set_reply_sort(&mut page.comments, &more.sort)?;
        parse::set_more_sort(&mut page.more, &more.sort)?;
        Ok(page)
    }
}
