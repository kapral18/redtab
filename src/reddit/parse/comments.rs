use super::*;

#[cfg(test)]
pub fn parse_post_page(json: &str) -> Result<PostPage> {
    parse_post_page_value(serde_json::from_str(json)?)
}

pub(in crate::reddit) fn parse_post_page_value(value: Value) -> Result<PostPage> {
    let Value::Array(parts) = value else {
        bail!("expected Reddit thread array");
    };
    let mut parts = parts.into_iter();
    let posts = listing(parts.next().context("missing thread post")?)?;
    let thing = posts
        .children
        .into_iter()
        .find(|t| t.kind == "t3")
        .context("thread has no post")?;
    let (post, body) = post(thing.data)?;
    let comments = listing(parts.next().context("missing comments listing")?)?;
    let parent = format!("t3_{}", post.id);
    let (comments, more) = comments_from(comments.children, &post.id, &parent, 0)?;
    // All top-level comments share a stable logical parent, even when a refresh
    // moves an expanded comment into the initial page or removes the More row.
    let comments = vec![container(format!("{THREAD_ROOT}{parent}"), comments, more)];
    Ok(PostPage {
        post,
        body,
        comments,
    })
}

#[derive(Deserialize)]
pub(super) struct CommentData {
    pub(super) id: String,
    #[serde(default)]
    pub(super) author: Option<String>,
    #[serde(default)]
    pub(super) author_flair_text: Option<String>,
    #[serde(default)]
    pub(super) is_submitter: bool,
    #[serde(default)]
    pub(super) distinguished: Option<String>,
    #[serde(default)]
    pub(super) stickied: bool,
    #[serde(default)]
    pub(super) score: Option<i64>,
    #[serde(default)]
    pub(super) score_hidden: bool,
    #[serde(default)]
    pub(super) created_utc: Option<f64>,
    #[serde(default)]
    pub(super) body: String,
    #[serde(default)]
    pub(super) body_html: Option<String>,
    #[serde(default)]
    pub(super) replies: Value,
}

pub(super) fn comment(value: Value, post: &str, depth: usize) -> Result<Comment> {
    let data: CommentData = serde_json::from_value(value).context("invalid Reddit comment")?;
    if !id_valid(&data.id) {
        bail!("invalid comment id");
    }
    let (replies, more) = if data.replies.is_object() {
        comments_from(
            listing(data.replies)?.children,
            post,
            &format!("t1_{}", data.id),
            depth + 1,
        )?
    } else {
        (Vec::new(), None)
    };
    Ok(Comment {
        id: data.id,
        author: data
            .author
            .map_or_else(|| "[deleted]".into(), |s| clean(&s)),
        flair: optional(data.author_flair_text),
        is_op: data.is_submitter,
        is_mod: data.distinguished.as_deref() == Some("moderator"),
        stickied: data.stickied,
        score: if data.score_hidden { None } else { data.score },
        created: created(data.created_utc),
        body: text::body(data.body_html.as_deref().unwrap_or_default(), &data.body),
        replies,
        more,
        sources: None,
    })
}

#[derive(Serialize, Deserialize, Clone)]
pub(in crate::reddit) struct MoreRequest {
    pub post: String,
    pub parent: String,
    pub children: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sort: String,
}

pub(in crate::reddit) fn set_more_sort(more: &mut Option<MoreReplies>, sort: &str) -> Result<()> {
    if let Some(more) = more {
        let mut request: MoreRequest = serde_json::from_str(&more.cursor)?;
        request.sort = sort.into();
        more.cursor = serde_json::to_string(&request)?;
    }
    Ok(())
}

pub(in crate::reddit) fn set_reply_sort(comments: &mut [Comment], sort: &str) -> Result<()> {
    for comment in comments {
        set_more_sort(&mut comment.more, sort)?;
        set_reply_sort(&mut comment.replies, sort)?;
    }
    Ok(())
}

/// Ids one reply cursor keeps; replies past them are not offered.
pub(super) const MORE_LIMIT: usize = 10_000;

pub(super) const EXPANSION_BATCH: usize = 4;

impl MoreRequest {
    pub fn validate(&self) -> Result<()> {
        if !matches!(
            self.sort.as_str(),
            "" | "confidence" | "top" | "new" | "old" | "controversial" | "qa"
        ) || !id_valid(&self.post)
            || !self
                .parent
                .strip_prefix("t1_")
                .or_else(|| self.parent.strip_prefix("t3_"))
                .is_some_and(id_valid)
            || self.children.len() > MORE_LIMIT
            || self.children.iter().any(|id| !id_valid(id))
            || (self.parent.starts_with("t3_") && self.parent != format!("t3_{}", self.post))
        {
            bail!("invalid reply cursor");
        }
        Ok(())
    }

    pub(super) fn cursor(&self, count: Option<u64>) -> MoreReplies {
        MoreReplies {
            cursor: serde_json::to_string(self).expect("reply cursor"),
            count,
        }
    }
}

pub(super) fn comments_from(
    things: Vec<Thing>,
    post: &str,
    parent: &str,
    depth: usize,
) -> Result<(Vec<Comment>, Option<MoreReplies>)> {
    if depth > 64 {
        bail!("comment nesting exceeds limit");
    }
    let mut comments = Vec::new();
    let mut children = Vec::new();
    let mut count = 0u64;
    let mut has_more = false;
    let mut ids = HashSet::new();
    let mut listed = HashSet::new();
    for thing in things {
        match thing.kind.as_str() {
            "t1" => {
                let c = comment(thing.data, post, depth)?;
                if ids.insert(c.id.clone()) {
                    comments.push(c);
                }
            }
            "more" => {
                has_more = true;
                count = count.saturating_add(thing.data["count"].as_u64().unwrap_or(0));
                if let Some(list) = thing.data["children"].as_array() {
                    for id in list.iter().filter_map(Value::as_str) {
                        if !id_valid(id) {
                            bail!("invalid more comment id");
                        }
                        if children.len() < MORE_LIMIT && listed.insert(id.to_owned()) {
                            children.push(id.to_string());
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let more = has_more.then(|| {
        // A truncated list offers only what it can load.
        let count = if children.len() == MORE_LIMIT {
            count.min(MORE_LIMIT as u64)
        } else {
            count
        };
        MoreRequest {
            post: post.to_owned(),
            parent: parent.to_owned(),
            children,
            sort: String::new(),
        }
        .cursor((count > 0).then_some(count))
    });
    Ok((comments, more))
}

pub(super) fn container(id: String, replies: Vec<Comment>, more: Option<MoreReplies>) -> Comment {
    Comment {
        id,
        author: String::new(),
        flair: None,
        is_op: false,
        is_mod: false,
        stickied: false,
        score: None,
        created: None,
        body: String::new(),
        replies,
        more,
        sources: None,
    }
}
