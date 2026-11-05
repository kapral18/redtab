use super::*;

pub fn name(value: &str) -> Result<&str> {
    if !crate::model::is_username(value) {
        bail!("Enter a Reddit name using letters, numbers, underscores or hyphens");
    }
    Ok(value)
}

pub(super) fn fullname(value: &str, comment: bool) -> Result<&str> {
    let id = value
        .strip_prefix("t3_")
        .or_else(|| comment.then(|| value.strip_prefix("t1_")).flatten());
    if !id.is_some_and(|id| {
        !id.is_empty() && id.len() <= 32 && id.bytes().all(|c| c.is_ascii_alphanumeric())
    }) {
        bail!("Invalid Reddit content identity");
    }
    Ok(value)
}

pub fn reddit_url(path: &str) -> Result<String> {
    let base = Url::parse("https://www.reddit.com/").unwrap();
    let url = base.join(path).context("Invalid Reddit link")?;
    if url.origin() != base.origin() || !url.username().is_empty() || url.password().is_some() {
        bail!("Not a Reddit link");
    }
    Ok(url.into())
}

pub fn thread_location(link: &str) -> Result<(String, Option<String>)> {
    let url = Url::parse(&reddit_url(link)?)?;
    let parts: Vec<_> = url.path().trim_matches('/').split('/').collect();
    let comment = match parts.as_slice() {
        ["r" | "u" | "user", _, "comments", _, _, comment, ..]
        | ["comments", _, _, comment, ..] => Some(comment.to_string()),
        _ => None,
    };
    Ok((url.path().into(), comment))
}

pub(super) fn read_page(query: &Query, value: Value, show_nsfw: bool) -> Result<Page> {
    let mut page = Page {
        title: String::new(),
        text: String::new(),
        entries: Vec::new(),
        after: None,
        url: query.url()?,
    };
    match query {
        Query::Profile(user) => {
            let data = &value["data"];
            if value["kind"] != "t2" || data["name"].as_str().is_none() {
                bail!("Invalid Reddit profile");
            }
            page.title = format!("u/{user}");
            page.text = profile_text(data);
            page.entries = [UserFeed::Overview, UserFeed::Posts, UserFeed::Comments]
                .into_iter()
                .map(|feed| {
                    navigation(
                        match feed {
                            UserFeed::Overview => "Overview",
                            UserFeed::Posts => "Posts",
                            _ => "Comments",
                        },
                        Query::User {
                            name: user.clone(),
                            feed,
                        },
                    )
                })
                .collect();
        }
        Query::Community(sub) => {
            let data = &value["data"];
            if value["kind"] != "t5" {
                bail!("Invalid Reddit community");
            }
            page.title = format!("r/{sub}");
            page.text = format!(
                "{}\n{} members\n\n{}\n\n{}",
                text::without_markers(data["title"].as_str().unwrap_or_default()),
                data["subscribers"].as_u64().unwrap_or(0),
                text::body("", data["public_description"].as_str().unwrap_or_default()),
                text::body(
                    data["description_html"].as_str().unwrap_or_default(),
                    data["description"].as_str().unwrap_or_default()
                )
            );
            page.entries = vec![
                navigation("Rules", Query::Rules(sub.clone())),
                navigation(
                    "Wiki index",
                    Query::Wiki {
                        subreddit: sub.clone(),
                        page: "index".into(),
                    },
                ),
            ];
        }
        Query::Rules(sub) => {
            let rules = value["rules"]
                .as_array()
                .context("Invalid community rules")?;
            page.title = format!("r/{sub} rules");
            page.text = rules
                .iter()
                .map(|r| {
                    format!(
                        "{}\n{}",
                        text::without_markers(r["short_name"].as_str().unwrap_or_default()),
                        text::body(
                            r["description_html"].as_str().unwrap_or_default(),
                            r["description"].as_str().unwrap_or_default()
                        )
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n");
        }
        Query::Moderators(sub) => {
            if value["kind"] != "UserList" {
                bail!("Invalid moderator list");
            }
            let moderators = value["data"]["children"]
                .as_array()
                .context("Moderator list has no children")?;
            page.title = format!("r/{sub} moderators");
            for moderator in moderators {
                let user = moderator["name"]
                    .as_str()
                    .context("Moderator has no username")?;
                name(user)?;
                page.entries.push(navigation(
                    &format!("u/{user}"),
                    Query::Profile(user.into()),
                ));
            }
        }
        Query::Wiki {
            subreddit,
            page: wiki,
        } => {
            let data = &value["data"];
            let markdown = data["content_md"].as_str().context("Invalid wiki page")?;
            page.title = format!("r/{subreddit} wiki/{wiki}");
            page.text = text::body(data["content_html"].as_str().unwrap_or_default(), markdown);
        }
        Query::Multis => {
            let multis = value.as_array().context("Invalid custom feed list")?;
            page.title = "Custom feeds".into();
            for multi in multis {
                let data = &multi["data"];
                let path = data["path"].as_str().context("Custom feed has no path")?;
                let segments: Vec<_> = path.trim_matches('/').split('/').collect();
                if segments.len() != 4 || segments[0] != "user" || segments[2] != "m" {
                    bail!("Invalid custom feed path");
                }
                name(segments[1])?;
                name(segments[3])?;
                page.entries.push(Entry {
                    icon: Default::default(),
                    title: data["display_name"].as_str().unwrap_or(segments[3]).into(),
                    body: text::body("", data["description_md"].as_str().unwrap_or_default()),
                    target: None,
                    post: None,
                    query: Some(Query::Multi {
                        owner: segments[1].into(),
                        name: segments[3].into(),
                    }),
                });
            }
        }
        _ => {
            if value["kind"] != "Listing" {
                bail!("Invalid Reddit listing");
            }
            let children = value["data"]["children"]
                .as_array()
                .context("Listing has no children")?;
            page.title = match query {
                Query::Search { text, kind, time } => format!(
                    "{}: {text} · {}",
                    match kind {
                        ListSearch::Communities => "Communities",
                        ListSearch::People => "People",
                    },
                    time.as_str()
                ),
                Query::User { name, feed } => format!("u/{name} · {}", feed.label()),
                Query::Inbox(kind) => kind.label().into(),
                Query::Multi { owner, name } => format!("u/{owner}/m/{name}"),
                _ => "Discover communities".into(),
            };
            if !value["data"]["after"].is_null() && !value["data"]["after"].is_string() {
                bail!("Invalid listing cursor");
            }
            page.after = value["data"]["after"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            for thing in children {
                if !show_nsfw && nsfw(&thing["data"]) {
                    continue;
                }
                if let Some(entry) = entry(thing)? {
                    page.entries.push(entry);
                }
            }
        }
    }
    Ok(page)
}

/// A t2 account carries its NSFW flag on its profile subreddit.
pub(super) fn nsfw(data: &Value) -> bool {
    data["over_18"] == true || data["over18"] == true || data["subreddit"]["over_18"] == true
}

/// The profile page's summary; a People search result is the same account object.
pub(super) fn profile_text(data: &Value) -> String {
    if data["is_suspended"] == true {
        return "Suspended account".into();
    }
    let karma = [("Post", "link_karma"), ("Comment", "comment_karma")]
        .into_iter()
        .filter_map(|(label, key)| Some(format!("{label} karma: {}", data[key].as_i64()?)))
        .collect::<Vec<_>>()
        .join("    ");
    format!(
        "{karma}\n{}",
        text::body(
            "",
            data["subreddit"]["public_description"]
                .as_str()
                .unwrap_or_default()
        )
    )
}

pub(super) fn navigation(label: &str, query: Query) -> Entry {
    Entry {
        icon: Default::default(),
        title: label.into(),
        body: String::new(),
        target: None,
        post: None,
        query: Some(query),
    }
}

pub(super) fn entry(thing: &Value) -> Result<Option<Entry>> {
    let data = &thing["data"];
    let mut entry = Entry {
        icon: Default::default(),
        title: String::new(),
        body: String::new(),
        target: None,
        query: None,
        post: None,
    };
    match thing["kind"].as_str() {
        Some("t3") => {
            let (post, _) = parse::post(data.clone())?;
            entry.title = format!(
                "{} · {} · {} points",
                post.title,
                post.source_label(),
                post.score.map_or("?".into(), |n| n.to_string())
            );
            entry.target = Some(Target::from_post(&post));
            entry.post = Some(post);
        }
        Some("t1" | "t4") => {
            let kind = thing["kind"].as_str().unwrap();
            let mut id = data["name"]
                .as_str()
                .map(str::to_string)
                .or_else(|| data["id"].as_str().map(|id| format!("{kind}_{id}")))
                .context("Comment or message has no identity")?;
            if data["was_comment"] == true
                && let Some(suffix) = id.strip_prefix("t4_")
            {
                id = format!("t1_{suffix}");
            }
            if id.starts_with("t1_") {
                fullname(&id, true)?;
            }
            entry.title = format!(
                "u/{} · {}",
                data["author"].as_str().unwrap_or("[deleted]"),
                data["link_title"]
                    .as_str()
                    .or_else(|| data["subject"].as_str())
                    .unwrap_or("comment")
            );
            entry.body = text::body(
                data["body_html"].as_str().unwrap_or_default(),
                data["body"].as_str().unwrap_or_default(),
            );
            entry.target = Some(target(data, id));
        }
        Some("t5") => {
            let sub = data["display_name"]
                .as_str()
                .context("Community has no name")?;
            name(sub)?;
            entry.title = format!("r/{sub}");
            entry.body = text::body("", data["public_description"].as_str().unwrap_or_default());
            entry.query = Some(Query::Community(sub.into()));
            entry.icon = parse::community_icon(data);
        }
        Some("t2") => {
            let user = data["name"].as_str().context("User has no name")?;
            name(user)?;
            entry.title = format!("u/{user}");
            entry.body = profile_text(data);
            entry.query = Some(Query::Profile(user.into()));
            entry.icon = parse::user_icon(data);
        }
        _ => return Ok(None),
    }
    Ok(Some(entry))
}
