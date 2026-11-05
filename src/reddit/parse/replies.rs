use super::*;

pub(in crate::reddit) fn focused_replies(value: Value, more: &MoreRequest) -> Result<ReplyPage> {
    fn find(comments: Vec<Comment>, id: &str) -> Option<Comment> {
        for c in comments {
            if c.id == id {
                return Some(c);
            }
            if let Some(c) = find(c.replies, id) {
                return Some(c);
            }
        }
        None
    }
    let page = parse_post_page_value(value)?;
    if page.post.id != more.post {
        bail!("reply post identity does not match");
    }
    let parent = if more.parent.starts_with("t3_") {
        format!("{THREAD_ROOT}{}", more.parent)
    } else {
        more.parent.trim_start_matches("t1_").to_string()
    };
    let c = find(page.comments, &parent).context("reply parent missing")?;
    if c.replies.is_empty() && c.more.is_some() {
        bail!("Reddit returned no additional replies");
    }
    Ok(if c.more.is_none() {
        ReplyPage::complete(c.replies)
    } else {
        ReplyPage {
            order: c
                .replies
                .iter()
                .map(|c| c.id.clone())
                .chain(remaining_ids(c.more.as_ref()))
                .collect(),
            refreshed: Some(c.replies.iter().map(|c| c.id.clone()).collect()),
            comments: c.replies,
            more: c.more,
        }
    })
}

pub(super) fn remaining_ids(more: Option<&MoreReplies>) -> Vec<String> {
    more.map(|c| {
        serde_json::from_str::<MoreRequest>(&c.cursor)
            .expect("parser cursor")
            .children
    })
    .unwrap_or_default()
}

/// Preserve pending replies from partial trees, excluding replies already loaded.
pub(crate) fn merge_reply_continuations(
    previous: Option<MoreReplies>,
    incoming: Option<MoreReplies>,
    loaded: &[Comment],
) -> Option<MoreReplies> {
    fn request(more: &MoreReplies) -> Result<MoreRequest> {
        let request: MoreRequest = serde_json::from_str(&more.cursor)?;
        request.validate()?;
        Ok(request)
    }
    let mut cursors = previous.into_iter().chain(incoming);
    let first = cursors.next()?;
    let Ok(mut left) = request(&first) else {
        return Some(first);
    };
    let mut seen = HashSet::new();
    let mut stack: Vec<_> = loaded.iter().collect();
    while let Some(comment) = stack.pop() {
        seen.insert(comment.id.clone());
        stack.extend(&comment.replies);
    }
    let mut focused = left.children.is_empty();
    left.children.retain(|id| seen.insert(id.clone()));
    if let Some(next) = cursors.next() {
        let Ok(right) = request(&next) else {
            return Some(first);
        };
        if (left.post.as_str(), left.parent.as_str(), left.sort.as_str())
            != (
                right.post.as_str(),
                right.parent.as_str(),
                right.sort.as_str(),
            )
        {
            return Some(first);
        }
        focused |= right.children.is_empty();
        for id in right.children {
            if seen.insert(id.clone()) {
                if left.children.len() == MORE_LIMIT {
                    focused = true;
                    break;
                }
                left.children.push(id);
            }
        }
    }
    if focused {
        // A parent-focused read retains access when an ID union exceeds the bound.
        left.children.clear();
    } else if left.children.is_empty() {
        return None;
    }
    Some(left.cursor(None))
}

pub(in crate::reddit) fn expanded_replies(
    more: &MoreRequest,
    mut fetch: impl FnMut(&str) -> Result<Value>,
) -> Result<ReplyPage> {
    let mut things: Vec<Thing> = Vec::new();
    let attempted: Vec<_> = more
        .children
        .iter()
        .take(EXPANSION_BATCH)
        .cloned()
        .collect();
    for id in &attempted {
        if !has_thing(&things, id) {
            add_focused_things(&mut things, fetch(id)?, more)?;
        }
        while has_thing(&things, id) {
            let Some(parent) = missing_reply_ancestor(&things, id, more)? else {
                break;
            };
            add_focused_things(&mut things, fetch(&parent)?, more)?;
            if !has_thing(&things, &parent) {
                bail!("Reply ancestor is unavailable");
            }
        }
    }
    let loaded: HashSet<_> = things
        .iter()
        .filter(|t| t.kind == "t1")
        .filter_map(|t| t.data["id"].as_str().map(str::to_owned))
        .collect();
    let mut by_parent: HashMap<String, Vec<Thing>> = HashMap::new();
    for mut thing in things {
        if thing.kind == "more"
            && let Some(children) = thing.data["children"].as_array_mut()
        {
            let had_ids = !children.is_empty();
            let before = children.len();
            children.retain(|id| {
                id.as_str().is_some_and(|id| {
                    !loaded.contains(id) && !attempted.iter().any(|done| done == id)
                })
            });
            if had_ids && children.is_empty() {
                continue;
            }
            let removed = before - children.len();
            if let Some(count) = thing.data["count"].as_u64() {
                thing.data["count"] = Value::from(count.saturating_sub(removed as u64));
            }
        }
        let parent = thing.data["parent_id"]
            .as_str()
            .context("reply has no parent")?
            .to_string();
        by_parent.entry(parent).or_default().push(thing);
    }
    fn build(
        parent: &str,
        post: &str,
        map: &mut HashMap<String, Vec<Thing>>,
        depth: usize,
    ) -> Result<(Vec<Comment>, Option<MoreReplies>)> {
        let (mut comments, more) =
            comments_from(map.remove(parent).unwrap_or_default(), post, parent, depth)?;
        for c in &mut comments {
            let (nested, continuation) = build(&format!("t1_{}", c.id), post, map, depth + 1)?;
            let mut seen: HashSet<_> = c.replies.iter().map(|r| r.id.clone()).collect();
            c.replies
                .extend(nested.into_iter().filter(|r| seen.insert(r.id.clone())));
            c.more = merge_reply_continuations(c.more.take(), continuation, &c.replies);
        }
        Ok((comments, more))
    }
    let (comments, mut continuation) = build(&more.parent, &more.post, &mut by_parent, 0)?;
    set_more_sort(&mut continuation, &more.sort)?;
    fn returned_ids(comments: &[Comment], ids: &mut Vec<String>) {
        for comment in comments {
            ids.push(comment.id.clone());
            returned_ids(&comment.replies, ids);
        }
    }
    let mut returned = Vec::new();
    returned_ids(&comments, &mut returned);
    let remaining: Vec<_> = more
        .children
        .iter()
        .filter(|id| !returned.contains(id) && !attempted.contains(id))
        .cloned()
        .collect();
    if !remaining.is_empty() {
        let next = MoreRequest {
            children: remaining,
            ..more.clone()
        };
        continuation = merge_reply_continuations(
            continuation,
            Some(next.cursor(Some(next.children.len() as u64))),
            &comments,
        );
    }
    let pending = remaining_ids(continuation.as_ref());
    let mut refreshed = attempted;
    for id in returned {
        if !refreshed.contains(&id) {
            refreshed.push(id);
        }
    }
    Ok(ReplyPage {
        order: comments
            .iter()
            .map(|c| c.id.clone())
            .chain(pending.iter().cloned())
            .collect(),
        comments,
        more: continuation,
        refreshed: Some(refreshed),
    })
}

pub(super) fn has_thing(things: &[Thing], id: &str) -> bool {
    things.iter().any(|t| t.kind == "t1" && t.data["id"] == id)
}

pub(super) fn missing_reply_ancestor(
    things: &[Thing],
    id: &str,
    more: &MoreRequest,
) -> Result<Option<String>> {
    let mut parent = format!("t1_{id}");
    let mut seen = HashSet::new();
    while parent != more.parent {
        if !seen.insert(parent.clone()) || seen.len() > 64 {
            bail!("invalid reply ancestry");
        }
        let id = parent
            .strip_prefix("t1_")
            .context("Reply is outside the requested parent")?;
        let Some(thing) = things.iter().find(|t| t.kind == "t1" && t.data["id"] == id) else {
            return Ok(Some(id.to_string()));
        };
        parent = thing.data["parent_id"]
            .as_str()
            .context("reply has no parent")?
            .into();
    }
    Ok(None)
}

pub(super) fn add_focused_things(
    things: &mut Vec<Thing>,
    value: Value,
    more: &MoreRequest,
) -> Result<()> {
    let page = parse_post_page_value(value.clone())?;
    if page.post.id != more.post {
        bail!("reply post identity does not match");
    }
    let children = listing(value[1].clone())?.children;
    flatten_focused(children, None, &more.post, things)
}

pub(super) fn flatten_focused(
    children: Vec<Thing>,
    parent: Option<&str>,
    post: &str,
    things: &mut Vec<Thing>,
) -> Result<()> {
    for mut thing in children {
        if thing.kind == "more" {
            let parent = parent.context("focused reply contains an unresolved placeholder")?;
            if thing.data["parent_id"]
                .as_str()
                .is_some_and(|id| id != parent)
            {
                bail!("reply parent identity does not match");
            }
            thing.data["parent_id"] = Value::from(parent);
            things.push(thing);
            continue;
        }
        if thing.kind != "t1" {
            bail!("invalid focused reply type");
        }
        let id = thing.data["id"]
            .as_str()
            .context("reply has no identity")?
            .to_string();
        let fullname = format!("t1_{id}");
        let actual_parent = thing.data["parent_id"]
            .as_str()
            .context("reply has no parent")?;
        let valid_parent = actual_parent.strip_prefix("t1_").is_some_and(id_valid)
            || actual_parent == format!("t3_{post}");
        if !valid_parent
            || parent.is_some_and(|parent| parent != actual_parent)
            || thing.data["link_id"]
                .as_str()
                .is_some_and(|link| link != format!("t3_{post}"))
            || thing.data["name"]
                .as_str()
                .is_some_and(|name| name != fullname)
        {
            bail!("reply parent or content identity does not match");
        }
        let replies = if thing.data["replies"].is_object() {
            listing(thing.data["replies"].take())?.children
        } else {
            Vec::new()
        };
        thing.data["replies"] = Value::String(String::new());
        if let Some(existing) = things
            .iter_mut()
            .find(|t| t.kind == "t1" && t.data["id"] == id)
        {
            if existing.data["parent_id"] != thing.data["parent_id"] {
                bail!("reply parent changed");
            }
            *existing = thing;
        } else {
            things.push(thing);
        }
        flatten_focused(replies, Some(&fullname), post, things)?;
    }
    Ok(())
}
