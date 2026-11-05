use super::*;

impl Reddit {
    /// A person's avatar and profile page summary, for the top of their feed.
    /// Hiding NSFW keeps an NSFW profile to its karma, as People results leave it out.
    pub fn user_profile(&self, user: &str) -> Result<crate::model::UserProfile> {
        let mut url = self
            .0
            .api
            .join(&format!("user/{}/about.json", name(user)?))?;
        url.query_pairs_mut().append_pair("raw_json", "1");
        let value = self.get(url)?;
        let data = &value["data"];
        if value["kind"] != "t2" || data["name"].as_str().is_none() {
            bail!("Invalid Reddit profile");
        }
        let text = profile_text(data);
        if !self.1 && nsfw(data) {
            let karma = text.lines().next().unwrap_or_default();
            return Ok(crate::model::UserProfile::new(None, karma));
        }
        let icon = match parse::user_icon(data) {
            crate::model::SubscriptionIcon::Url(url) => Some(url),
            _ => None,
        };
        Ok(crate::model::UserProfile::new(icon, &text))
    }

    pub fn experience(&self, request: &Request) -> Result<Response> {
        match request {
            Request::Read {
                query: Query::Thread(link),
                ..
            } => {
                let (path, comment) = thread_location(link)?;
                self.focused_post(&path, "", comment.as_deref())
                    .map(|p| Response::Thread(Box::new(p)))
            }
            Request::Read { query, after } => self.experience_read(query, after.as_deref()),
            Request::Write(action) => self.experience_write(action),
        }
    }

    pub(in crate::reddit) fn read_url(&self, query: &Query, after: Option<&str>) -> Result<Url> {
        let (path, params) = query.route()?;
        let base = if matches!(query, Query::Search { .. }) {
            &self.0.classic
        } else {
            &self.0.api
        };
        let mut url = base.join(&path)?;
        url.query_pairs_mut()
            .extend_pairs(params)
            .append_pair("raw_json", "1")
            .append_pair("limit", &PAGE_LIMIT.to_string());
        if let Some(after) = after {
            url.query_pairs_mut().append_pair("after", after);
        }
        Ok(url)
    }

    pub(super) fn experience_read(&self, query: &Query, after: Option<&str>) -> Result<Response> {
        if let Query::Inspect(target) = query {
            let value = self.thread_json(&target.inspection_permalink()?, "", target.comment())?;
            return inspect(value, target).map(Response::Details);
        }
        let value = match self.get(self.read_url(query, after)?) {
            Ok(value) => value,
            Err(error)
                if error.is::<super::super::NotFound>() && matches!(query, Query::Wiki { .. }) =>
            {
                let Query::Wiki { subreddit, page } = query else {
                    unreachable!()
                };
                return Ok(Response::Page(Page {
                    title: format!("r/{subreddit} wiki/{page}"),
                    text: "This wiki page is unavailable. Reddit did not find a public page at this address.".into(),
                    entries: Vec::new(), after:None, url:query.url()?,
                }));
            }
            Err(error) => return Err(error),
        };
        read_page(query, value, self.1).map(Response::Page)
    }

    pub(super) fn csrf(&self) -> Result<HeaderValue> {
        let me = self.get(self.0.api.join("api/me.json")?)?;
        let account = me["data"]["name"]
            .as_str()
            .filter(|s| !s.is_empty())
            .context("Reddit did not confirm a signed-in account")?;
        if self
            .account()
            .is_some_and(|expected| !expected.name.eq_ignore_ascii_case(account))
        {
            bail!("Reddit account changed; reconnect before writing");
        }
        let token = me["data"]["modhash"]
            .as_str()
            .filter(|s| !s.is_empty())
            .context("Reddit did not supply a CSRF token; reconnect through Accounts")?;
        let mut header = HeaderValue::from_str(token)
            .map_err(|_| anyhow::anyhow!("Invalid Reddit CSRF token"))?;
        header.set_sensitive(true);
        Ok(header)
    }

    pub(super) fn experience_write(&self, action: &Action) -> Result<Response> {
        // Build and validate before querying the account or sending any mutation.
        let (path, fields, method) =
            action.form(self.account().as_ref().map(|a| a.name.as_str()))?;
        let token = self.csrf()?;
        if let Action::Edit { target, .. } | Action::Delete { target } = action {
            let details = match self.experience_read(&Query::Inspect(target.clone()), None)? {
                Response::Details(details) => details,
                _ => unreachable!(),
            };
            let me = self
                .account()
                .context("Reconnect to identify your account before editing")?;
            if !details.target.author.eq_ignore_ascii_case(&me.name) {
                bail!("Only your own content can be edited or deleted");
            }
            if matches!(action, Action::Edit { .. }) && !details.editable {
                bail!("Only comments and text-post bodies can be edited");
            }
        }
        let mut form = url::form_urlencoded::Serializer::new(String::new());
        form.append_pair("api_type", "json").extend_pairs(fields);
        let body = form.finish();
        let response = self.send(
            self.0.api.join(&path)?,
            "application/json",
            |client, url| {
                let request = match method {
                    "PUT" => client.put(url),
                    "DELETE" => client.delete(url),
                    _ => client.post(url),
                };
                request
                    .header("X-Modhash", token)
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(body)
            },
        )?;
        let value = if method == "DELETE" {
            let mut bytes = Vec::new();
            use std::io::Read;
            let mut reader = transport::Reader::new(response, 1024 * 1024, None);
            reader.by_ref().take(1024 * 1024).read_to_end(&mut bytes)?;
            if bytes.is_empty() {
                json!({})
            } else {
                serde_json::from_slice(&bytes)
                    .context("Reddit returned an invalid acknowledgment")?
            }
        } else {
            transport::json(response, None)?
        };
        acknowledge(action, &value)?;
        let url = value["json"]["data"]["url"]
            .as_str()
            .map(reddit_url)
            .transpose()?;
        Ok(Response::Written { url })
    }
}

pub(super) fn acknowledge(action: &Action, value: &Value) -> Result<()> {
    if let Some(errors) = value["json"]["errors"]
        .as_array()
        .filter(|errors| !errors.is_empty())
    {
        // Only fixed API codes are displayed: response messages may echo submitted text.
        let codes: Vec<&str> = errors
            .iter()
            .filter_map(|e| e[0].as_str())
            .filter(|s| {
                s.len() <= 64
                    && s.bytes()
                        .all(|c| c.is_ascii_uppercase() || b"_0123456789".contains(&c))
            })
            .collect();
        bail!(
            "Reddit rejected the action: {}. Check on Reddit before submitting again",
            if codes.is_empty() {
                "API error".into()
            } else {
                codes.join(", ")
            }
        );
    }
    if value.get("error").is_some() {
        bail!("Reddit rejected the action; check on Reddit before submitting again");
    }
    let empty = value.as_object().is_some_and(|o| o.is_empty());
    let acknowledged = value["json"]["errors"]
        .as_array()
        .is_some_and(Vec::is_empty);
    let confirmed = match action {
        Action::Reply { .. } | Action::Edit { .. } => {
            acknowledged
                && value["json"]["data"]["things"]
                    .as_array()
                    .is_some_and(|things| !things.is_empty())
        }
        Action::Submit { .. } => acknowledged && value["json"]["data"]["url"].as_str().is_some(),
        Action::CreateMulti { .. } => {
            value["kind"] == "LabeledMulti" && value["data"]["path"].as_str().is_some()
        }
        Action::MultiCommunity { add: true, .. } => value["name"].as_str().is_some() || empty,
        _ => empty || acknowledged,
    };
    if !confirmed {
        bail!("Reddit did not acknowledge the action; check on Reddit before submitting again");
    }
    Ok(())
}

pub(super) fn inspect(value: Value, fallback: &Target) -> Result<Details> {
    let listings = value
        .as_array()
        .filter(|v| v.len() == 2)
        .context("Invalid content details")?;
    let listing = &listings[usize::from(fallback.comment().is_some())];
    let things = listing["data"]["children"]
        .as_array()
        .context("Invalid content details")?;
    let thing = find_inspected_thing(things, &fallback.id)
        .context("Content was deleted or is unavailable")?;
    let data = &thing["data"];
    let mut target = target(data, fallback.id.clone());
    if target.permalink.is_empty() {
        target.permalink = fallback.permalink.clone();
    }
    if target.subreddit.is_empty() {
        target.subreddit = fallback.subreddit.clone();
    }
    let comment = target.id.starts_with("t1_");
    if !comment {
        parse::canonicalize_profile_post(data, &target.subreddit, &mut target.permalink)?;
    }
    Ok(Details {
        target,
        raw_text: data[if comment { "body" } else { "selftext" }]
            .as_str()
            .unwrap_or_default()
            .into(),
        saved: data["saved"].as_bool().unwrap_or(false),
        hidden: data["hidden"].as_bool().unwrap_or(false),
        vote: match data["likes"].as_bool() {
            Some(true) => 1,
            Some(false) => -1,
            None => 0,
        },
        editable: comment || data["is_self"].as_bool() == Some(true),
        crosspost: data["crosspost_parent_list"][0]["permalink"]
            .as_str()
            .map(str::to_string),
    })
}

pub(super) fn find_inspected_thing<'a>(things: &'a [Value], fullname: &str) -> Option<&'a Value> {
    let (kind, id) = fullname.split_once('_')?;
    things.iter().find_map(|thing| {
        let data = &thing["data"];
        let identity_matches = data["name"]
            .as_str()
            .map_or_else(|| data["id"].as_str() == Some(id), |name| name == fullname);
        if thing["kind"].as_str() == Some(kind) && identity_matches {
            return Some(thing);
        }
        data["replies"]["data"]["children"]
            .as_array()
            .and_then(|children| find_inspected_thing(children, fullname))
    })
}

pub(super) fn target(data: &Value, id: String) -> Target {
    let mut permalink = data["permalink"].as_str().unwrap_or_default().to_string();
    if permalink.is_empty()
        && let Some(context) = data["context"].as_str()
    {
        permalink = context.into();
    }
    if permalink.is_empty()
        && id.starts_with("t1_")
        && let Some(post) = data["link_id"]
            .as_str()
            .and_then(|id| id.strip_prefix("t3_"))
    {
        permalink = format!("/comments/{post}/_/{}/", id.trim_start_matches("t1_"));
    }
    Target {
        id,
        author: data["author"].as_str().unwrap_or("[deleted]").into(),
        subreddit: data["subreddit"].as_str().unwrap_or_default().into(),
        permalink,
    }
}
