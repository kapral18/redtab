use super::*;

impl Reddit {
    #[cfg(test)]
    pub fn listing(
        &self,
        subreddit: &str,
        sort: Sort,
        time: TimeRange,
        after: Option<&str>,
    ) -> Result<Listing> {
        self.listing_progress(subreddit, sort, time, after, &mut |_| {})
    }

    pub(crate) fn listing_progress(
        &self,
        subreddit: &str,
        sort: Sort,
        time: TimeRange,
        after: Option<&str>,
        progress: &mut dyn FnMut(ListingEvent<'_>),
    ) -> Result<Listing> {
        if sort == Sort::Best {
            bail!("Best is available on Home only");
        }
        let url = self.listing_url(subreddit, sort)?;
        self.fetch_listing(url, sort, time, after, progress)
    }

    pub(super) fn listing_url(&self, subreddit: &str, sort: Sort) -> Result<Url> {
        if subreddit.eq_ignore_ascii_case("all") {
            // old.reddit refuses `r/all/hot.json`, `new.json` and `rising.json` (HTTP 403) but
            // serves Hot at `r/all.json`; no host serves New or Rising for r/all.
            return match sort {
                Sort::Hot => Ok(self.0.old.join("r/all.json")?),
                Sort::New => {
                    bail!("Reddit refuses All Reddit sorted by New; use Hot, Top or Controversial")
                }
                Sort::Rising => {
                    bail!(
                        "Reddit refuses All Reddit sorted by Rising; use Hot, Top or Controversial"
                    )
                }
                _ => self.subreddit_at(&self.0.old, subreddit, &format!("{}.json", sort.as_str())),
            };
        }
        self.subreddit_at(&self.0.classic, subreddit, sort.as_str())
    }

    pub fn home(&self, sort: Sort, time: TimeRange, after: Option<&str>) -> Result<Listing> {
        self.home_progress(sort, time, after, &mut |_| {})
    }

    pub(crate) fn home_progress(
        &self,
        sort: Sort,
        time: TimeRange,
        after: Option<&str>,
        progress: &mut dyn FnMut(ListingEvent<'_>),
    ) -> Result<Listing> {
        if sort == Sort::Best {
            return self.browser_home(after, progress);
        }
        let url = self.0.api.join(&format!("{}.json", sort.as_str()))?;
        self.fetch_listing(url, sort, time, after, progress)
    }

    pub(super) fn home_partial(
        &self,
        cursor: Option<&str>,
        progress: &mut dyn FnMut(&Post),
    ) -> Result<home::Page> {
        let url = home::url(
            &self.0.api,
            cursor.unwrap_or("/svc/shreddit/feeds/home-feed?sort=BEST"),
        )?;
        let cancelled = self.read_cancellation();
        home::stream(
            transport::Reader::new(
                self.request(url, home::ACCEPT)?,
                16 * 1024 * 1024,
                Some(cancelled.clone()),
            ),
            &self.0.api,
            progress,
        )
        .map_err(|error| {
            if cancelled() {
                anyhow::anyhow!(REQUEST_CANCELLED)
            } else {
                error
            }
        })
    }

    pub(super) fn browser_home(
        &self,
        after: Option<&str>,
        progress: &mut dyn FnMut(ListingEvent<'_>),
    ) -> Result<Listing> {
        let mut seen = HashSet::new();
        let mut emit = |post: &Post| {
            if seen.insert(post.id.clone()) && (self.1 || !post.nsfw) {
                progress(ListingEvent::Post(post));
            }
        };
        let mut page = self.home_partial(after, &mut emit)?;
        let mut failure = None;
        if after.is_none()
            && page.posts.len() <= 3
            && let Some(next) = page.after.clone()
        {
            let mut streamed = Vec::new();
            match self
                .home_partial(Some(&next), &mut |post| {
                    streamed.push(post.clone());
                    emit(post);
                })
                .and_then(|continuation| {
                    if continuation.after.as_deref() == Some(&next) {
                        bail!("Reddit repeated a Home cursor");
                    }
                    Ok(continuation)
                }) {
                Ok(continuation) => page.after = continuation.after,
                Err(error) if error.to_string() == REQUEST_CANCELLED => return Err(error),
                Err(error) => failure = Some((next, error.to_string())),
            }
            // Shown as they streamed, so a failed continuation keeps them too.
            let mut ids: HashSet<_> = page.posts.iter().map(|p| p.id.clone()).collect();
            page.posts
                .extend(streamed.into_iter().filter(|p| ids.insert(p.id.clone())));
        }
        page.posts.retain(|post| self.1 || !post.nsfw);
        if let Some((cursor, error)) = &failure {
            progress(ListingEvent::ContinuationFailed { cursor, error });
        }
        Ok(Listing {
            posts: page.posts,
            after: page.after,
        })
    }

    pub(super) fn fetch_listing(
        &self,
        mut url: Url,
        sort: Sort,
        time: TimeRange,
        after: Option<&str>,
        progress: &mut dyn FnMut(ListingEvent<'_>),
    ) -> Result<Listing> {
        self.listing_params(&mut url, after);
        if sort.takes_time() {
            url.query_pairs_mut().append_pair("t", time.as_str());
        }
        self.listing_response(url, progress)
    }

    pub(super) fn listing_response(
        &self,
        url: Url,
        progress: &mut dyn FnMut(ListingEvent<'_>),
    ) -> Result<Listing> {
        let mut listing = transport::listing(
            self.request(url, "application/json")?,
            self.read_cancellation(),
            &mut |post| {
                if self.1 || !post.nsfw {
                    progress(ListingEvent::Post(post));
                }
            },
        )?;
        if !self.1 {
            listing.posts.retain(|post| !post.nsfw);
        }
        Ok(listing)
    }

    /// Follow every page before replacing the account's last complete subscription list.
    pub fn subscriptions(&self) -> Result<Vec<crate::model::Subscription>> {
        let mut names = Vec::new();
        let mut seen_names = HashSet::new();
        let mut cursors = HashSet::new();
        let mut after: Option<String> = None;
        loop {
            let mut url = self.0.api.join("subreddits/mine/subscriber.json")?;
            url.query_pairs_mut()
                .append_pair("limit", "100")
                .append_pair("raw_json", "1");
            if let Some(cursor) = &after {
                url.query_pairs_mut().append_pair("after", cursor);
            }
            let (page, next) = parse::parse_subscriptions(self.get(url)?)?;
            names.extend(page.into_iter().filter(|entry| {
                seen_names.insert((
                    entry.community().is_none(),
                    entry.name().to_ascii_lowercase(),
                ))
            }));
            let Some(next) = next else { break };
            if !cursors.insert(next.clone()) {
                bail!("Reddit repeated a subscription cursor");
            }
            after = Some(next);
        }
        names.sort_by_key(|entry| entry.name().to_ascii_lowercase());
        Ok(names)
    }

    pub(crate) fn user_posts_progress(
        &self,
        user: &str,
        sort: Sort,
        time: TimeRange,
        after: Option<&str>,
        progress: &mut dyn FnMut(ListingEvent<'_>),
    ) -> Result<Listing> {
        let user = experience::name(user)?;
        if !matches!(
            sort,
            Sort::Hot | Sort::New | Sort::Top | Sort::Controversial
        ) {
            bail!("Unsupported user-post sort");
        }
        let mut url = self.0.api.join(&format!("user/{user}/submitted.json"))?;
        url.query_pairs_mut().append_pair("sort", sort.as_str());
        self.fetch_listing(url, sort, time, after, progress)
    }

    #[cfg(test)]
    pub fn search(&self, subreddit: &str, query: &str, after: Option<&str>) -> Result<Listing> {
        self.search_progress(
            subreddit,
            query,
            experience::SearchSort::Relevance,
            TimeRange::All,
            after,
            &mut |_| {},
        )
    }

    pub(crate) fn search_progress(
        &self,
        subreddit: &str,
        query: &str,
        sort: experience::SearchSort,
        time: TimeRange,
        after: Option<&str>,
        progress: &mut dyn FnMut(ListingEvent<'_>),
    ) -> Result<Listing> {
        let url = self.search_url(subreddit, query, sort, time, after)?;
        self.listing_response(url, progress)
    }

    pub(super) fn search_url(
        &self,
        subreddit: &str,
        query: &str,
        sort: experience::SearchSort,
        time: TimeRange,
        after: Option<&str>,
    ) -> Result<Url> {
        let mut url = self.subreddit_at(&self.0.classic, subreddit, "search")?;
        self.listing_params(&mut url, after);
        url.query_pairs_mut()
            .append_pair("q", query)
            .append_pair("restrict_sr", "on")
            .append_pair("sort", sort.as_str())
            .append_pair("t", time.as_str());
        Ok(url)
    }

    pub(super) fn subreddit_url(&self, subreddit: &str, suffix: &str) -> Result<Url> {
        self.subreddit_at(&self.0.api, subreddit, suffix)
    }

    pub(super) fn subreddit_at(&self, base: &Url, subreddit: &str, suffix: &str) -> Result<Url> {
        if subreddit.is_empty()
            || !subreddit
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'+'))
        {
            bail!("invalid subreddit name");
        }
        Ok(base.join(&format!("r/{subreddit}/{suffix}"))?)
    }

    pub(super) fn listing_params(&self, url: &mut Url, after: Option<&str>) {
        url.query_pairs_mut()
            .append_pair("limit", "25")
            .append_pair("raw_json", "1")
            .append_pair("include_over_18", if self.1 { "on" } else { "off" });
        if let Some(after) = after {
            url.query_pairs_mut().append_pair("after", after);
        }
    }

    /// The subreddit's icon URL, if it has one.
    pub fn icon(&self, subreddit: &str) -> Result<Option<String>> {
        let mut url = self.subreddit_url(subreddit, "about.json")?;
        url.query_pairs_mut().append_pair("raw_json", "1");
        let about = match self.get(url) {
            // The subreddit's answer, kept like an icon: asking again gets the same 404.
            Err(e) if e.is::<NotFound>() => return Ok(None),
            result => result?,
        };
        Ok(["community_icon", "icon_img"].into_iter().find_map(|key| {
            about["data"][key]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        }))
    }
}
