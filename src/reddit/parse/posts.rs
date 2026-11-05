use super::*;

#[derive(Deserialize)]
pub(super) struct PostData {
    pub(super) id: String,
    pub(super) title: String,
    pub(super) subreddit: String,
    pub(super) permalink: String,
    #[serde(default)]
    pub(super) author: Option<String>,
    #[serde(default)]
    pub(super) link_flair_text: Option<String>,
    #[serde(default)]
    pub(super) score: Option<i64>,
    #[serde(default)]
    pub(super) hide_score: bool,
    #[serde(default)]
    pub(super) num_comments: u64,
    #[serde(default)]
    pub(super) created_utc: Option<f64>,
    #[serde(default)]
    pub(super) stickied: bool,
    #[serde(default)]
    pub(super) over_18: bool,
    #[serde(default)]
    pub(super) spoiler: bool,
    #[serde(default)]
    pub(super) selftext: String,
    #[serde(default)]
    pub(super) selftext_html: Option<String>,
}

pub(super) fn clean(s: &str) -> String {
    text::without_markers(s).into_owned()
}

pub(super) fn created(seconds: Option<f64>) -> Option<DateTime<Utc>> {
    seconds
        .filter(|n| n.is_finite())
        .and_then(|n| DateTime::from_timestamp(n as i64, 0))
}

pub(super) fn optional(s: Option<String>) -> Option<String> {
    s.map(|s| clean(&s)).filter(|s| !s.is_empty())
}

pub(in crate::reddit) fn canonicalize_profile_post(
    value: &Value,
    subreddit: &str,
    permalink: &mut String,
) -> Result<()> {
    if value["subreddit_type"] == "user" && crate::model::profile_user(permalink).is_none() {
        let owner = value["subreddit_name_prefixed"]
            .as_str()
            .and_then(|name| name.strip_prefix("u/"))
            .or_else(|| subreddit.strip_prefix("u_"))
            .filter(|name| crate::model::is_username(name))
            .context("invalid profile post owner")?;
        let prefix = format!("/r/{subreddit}/");
        let tail = permalink
            .strip_prefix(&prefix)
            .context("invalid profile post location")?;
        *permalink = format!("/user/{owner}/{tail}");
    }
    Ok(())
}

pub(in crate::reddit) fn post(value: Value) -> Result<(Post, String)> {
    let mut data = PostData::deserialize(&value).context("invalid Reddit post")?;
    canonicalize_profile_post(&value, &data.subreddit, &mut data.permalink)?;
    if !id_valid(&data.id) || post_id(&data.permalink) != Some(data.id.as_str()) {
        bail!("invalid Reddit post identity");
    }
    let mut body = text::body(
        data.selftext_html.as_deref().unwrap_or_default(),
        &data.selftext,
    );
    if let Some(poll) = value.get("poll_data") {
        if !body.is_empty() {
            body.push_str("\n\n");
        }
        body.push_str(&format!(
            "Poll: {} votes",
            poll["total_vote_count"].as_u64().unwrap_or(0)
        ));
        if let Some(options) = poll["options"].as_array() {
            for option in options {
                if let Some(label) = option["text"].as_str() {
                    body.push_str(&format!("\n• {}", clean(label)));
                    if let Some(votes) = option["vote_count"].as_u64() {
                        body.push_str(&format!(" — {votes} votes"));
                    }
                }
            }
        }
    }
    if let Some(original) = value["crosspost_parent_list"]
        .as_array()
        .and_then(|items| items.first())
    {
        let subreddit = original["subreddit"].as_str().unwrap_or_default();
        let mut permalink = original["permalink"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        // Invalid optional attribution must not hide the crosspost itself.
        if canonicalize_profile_post(original, subreddit, &mut permalink).is_ok() {
            if original["removed_by_category"].as_str() == Some("deleted") {
                body.push_str("\n\nOriginal post deleted");
            }
            body.push_str(&format!(
                "\n\nCrossposted from {} by u/{}: {}\n{}",
                crate::model::profile_user(&permalink).map_or_else(
                    || format!("r/{}", clean(subreddit)),
                    |name| format!("u/{name}")
                ),
                clean(original["author"].as_str().unwrap_or_default()),
                clean(original["title"].as_str().unwrap_or_default()),
                clean(&permalink)
            ));
        }
    }
    if let Some(items) = media_source(&value)["gallery_data"]["items"].as_array() {
        for (index, item) in items.iter().enumerate() {
            let caption = item["caption"].as_str().unwrap_or_default();
            let link = item["outbound_url"].as_str().unwrap_or_default();
            if !caption.is_empty() || !link.is_empty() {
                body.push_str(&format!(
                    "\n\nImage {}: {}\n{}",
                    index + 1,
                    clean(caption),
                    clean(link)
                ));
            }
        }
    }
    let media = media(&value);
    Ok((
        Post {
            id: data.id,
            subreddit: clean(&data.subreddit),
            title: clean(&data.title),
            author: data
                .author
                .map_or_else(|| "[deleted]".into(), |s| clean(&s)),
            flair: optional(data.link_flair_text),
            score: if data.hide_score { None } else { data.score },
            comments: data.num_comments,
            created: created(data.created_utc),
            permalink: data.permalink,
            preview_size: preview_size(&value, &media),
            media,
            stickied: data.stickied,
            nsfw: data.over_18,
            spoiler: data.spoiler,
            original_deleted: value["crosspost_parent_list"]
                .as_array()
                .and_then(|parents| parents.first())
                .is_some_and(|parent| parent["removed_by_category"].as_str() == Some("deleted")),
        },
        body,
    ))
}

/// A t5 listing's icon: a usable URL, known to have none, or not in the answer.
pub(crate) fn community_icon(data: &Value) -> crate::model::SubscriptionIcon {
    let icon = ["community_icon", "icon_img"]
        .into_iter()
        .find_map(|key| media_url(&data[key]));
    match icon {
        Some(url) => crate::model::SubscriptionIcon::Url(url),
        None if data.get("community_icon").is_some() || data.get("icon_img").is_some() => {
            crate::model::SubscriptionIcon::Missing
        }
        None => crate::model::SubscriptionIcon::Unknown,
    }
}

pub(crate) fn user_icon(data: &Value) -> crate::model::SubscriptionIcon {
    media_url(&data["icon_img"]).map_or_else(Default::default, crate::model::SubscriptionIcon::Url)
}
