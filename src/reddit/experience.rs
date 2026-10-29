//! Reddit collections and explicit user actions, through the existing session transport.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use url::Url;
use wreq::header::HeaderValue;

use super::{Reddit, parse, text, transport};
use crate::model::{Post, PostPage, TimeRange};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Query {
    Search {
        text: String,
        kind: SearchKind,
        sort: SearchSort,
        time: TimeRange,
    },
    User {
        name: String,
        feed: UserFeed,
    },
    Profile(String),
    Inbox(Inbox),
    Communities,
    Community(String),
    Rules(String),
    Moderators(String),
    Wiki {
        subreddit: String,
        page: String,
    },
    Multis,
    Multi {
        owner: String,
        name: String,
    },
    Inspect(Target),
    Thread(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchKind {
    Posts,
    Communities,
    People,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchSort {
    Relevance,
    Hot,
    Top,
    New,
    Comments,
}

impl SearchSort {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Relevance => "relevance",
            Self::Hot => "hot",
            Self::Top => "top",
            Self::New => "new",
            Self::Comments => "comments",
        }
    }
    pub fn next(self) -> Self {
        match self {
            Self::Relevance => Self::Hot,
            Self::Hot => Self::Top,
            Self::Top => Self::New,
            Self::New => Self::Comments,
            Self::Comments => Self::Relevance,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserFeed {
    Overview,
    Posts,
    Comments,
    Saved,
    Upvoted,
    Downvoted,
    Hidden,
}

impl UserFeed {
    fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Posts => "Posts",
            Self::Comments => "Comments",
            Self::Saved => "Saved on Reddit",
            Self::Upvoted => "Upvoted posts",
            Self::Downvoted => "Downvoted posts",
            Self::Hidden => "Hidden posts",
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Posts => "submitted",
            Self::Comments => "comments",
            Self::Saved => "saved",
            Self::Upvoted => "upvoted",
            Self::Downvoted => "downvoted",
            Self::Hidden => "hidden",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inbox {
    Unread,
    Replies,
    PostReplies,
    Mentions,
    All,
}

impl Inbox {
    fn label(self) -> &'static str {
        match self {
            Self::Unread => "Unread notifications",
            Self::Replies => "Comment replies",
            Self::PostReplies => "Post replies",
            Self::Mentions => "Mentions",
            Self::All => "Inbox",
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Self::Unread => "unread",
            Self::Replies => "comments",
            Self::PostReplies => "selfreply",
            Self::Mentions => "mentions",
            Self::All => "inbox",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub id: String,
    pub author: String,
    pub subreddit: String,
    pub permalink: String,
}

impl Target {
    pub fn from_post(post: &Post) -> Self {
        Self {
            id: format!("t3_{}", post.id),
            author: post.author.clone(),
            subreddit: post.subreddit.clone(),
            permalink: post.permalink.clone(),
        }
    }
    pub fn comment(&self) -> Option<&str> {
        self.id.strip_prefix("t1_")
    }
    pub fn url(&self) -> Result<String> {
        reddit_url(&self.permalink)
    }

    fn inspection_permalink(&self) -> Result<String> {
        fullname(&self.id, true)?;
        if let Some(post) = self.id.strip_prefix("t3_") {
            return Ok(format!("/comments/{post}"));
        }
        let url = Url::parse(&self.url()?)?;
        let post = parse::post_id(url.path()).context("Comment has no post location")?;
        Ok(format!("/comments/{post}"))
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub title: String,
    pub body: String,
    pub target: Option<Target>,
    pub query: Option<Query>,
    pub post: Option<Post>,
}

#[derive(Debug, Clone)]
pub struct Page {
    pub title: String,
    pub text: String,
    pub entries: Vec<Entry>,
    pub after: Option<String>,
    pub url: String,
}

#[derive(Debug, Clone)]
pub struct Details {
    pub target: Target,
    pub raw_text: String,
    pub saved: bool,
    pub hidden: bool,
    pub vote: i8,
    pub editable: bool,
    pub crosspost: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Action {
    Vote {
        id: String,
        direction: i8,
    },
    Save {
        id: String,
        save: bool,
    },
    Hide {
        id: String,
        hide: bool,
    },
    Reply {
        parent: String,
        text: String,
    },
    Submit {
        subreddit: String,
        title: String,
        text: String,
        link: bool,
    },
    Edit {
        target: Target,
        text: String,
    },
    Delete {
        target: Target,
    },
    Report {
        id: String,
        reason: String,
    },
    Read {
        id: String,
        read: bool,
    },
    FollowPost {
        id: String,
        follow: bool,
    },
    CreateMulti {
        name: String,
        description: String,
    },
    MultiCommunity {
        owner: String,
        name: String,
        subreddit: String,
        add: bool,
    },
    DeleteMulti {
        owner: String,
        name: String,
    },
}

#[derive(Debug, Clone)]
pub enum Request {
    Read { query: Query, after: Option<String> },
    Write(Action),
}

#[derive(Debug)]
pub enum Response {
    Page(Page),
    Details(Details),
    Thread(Box<PostPage>),
    Written { url: Option<String> },
}

pub fn name(value: &str) -> Result<&str> {
    if !crate::model::is_username(value) {
        bail!("Enter a Reddit name using letters, numbers, underscores or hyphens");
    }
    Ok(value)
}

fn fullname(value: &str, comment: bool) -> Result<&str> {
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

impl Query {
    fn route(&self) -> Result<(String, Vec<(&'static str, String)>)> {
        let mut params = Vec::new();
        let path = match self {
            Self::Search {
                text,
                kind,
                sort,
                time,
            } => {
                if text.trim().is_empty() {
                    bail!("Enter a search query");
                }
                params.push(("q", text.clone()));
                params.push(("sort", sort.as_str().into()));
                params.push(("t", time.as_str().into()));
                match kind {
                    SearchKind::Posts => "search.json".into(),
                    SearchKind::Communities => "subreddits/search.json".into(),
                    SearchKind::People => "users/search.json".into(),
                }
            }
            Self::User { name: user, feed } => {
                params.push(("sort", "new".into()));
                format!("user/{}/{}.json", name(user)?, feed.as_str())
            }
            Self::Profile(user) => format!("user/{}/about.json", name(user)?),
            Self::Inbox(kind) => {
                params.push(("mark", "false".into()));
                format!("message/{}.json", kind.as_str())
            }
            Self::Communities => "subreddits/popular.json".into(),
            Self::Community(sub) => format!("r/{}/about.json", name(sub)?),
            Self::Rules(sub) => format!("r/{}/about/rules.json", name(sub)?),
            Self::Moderators(sub) => format!("r/{}/about/moderators.json", name(sub)?),
            Self::Wiki { subreddit, page } => {
                if page.split('/').any(|part| name(part).is_err()) {
                    bail!("Invalid wiki page name");
                }
                format!("r/{}/wiki/{page}.json", name(subreddit)?)
            }
            Self::Multis => "api/multi/mine.json".into(),
            Self::Multi { owner, name: multi } => {
                format!("user/{}/m/{}/hot.json", name(owner)?, name(multi)?)
            }
            Self::Inspect(_) | Self::Thread(_) => bail!("Threads use the comment reader"),
        };
        Ok((path, params))
    }

    pub fn url(&self) -> Result<String> {
        match self {
            Self::Inspect(target) => return target.url(),
            Self::Thread(link) => return reddit_url(link),
            Self::Profile(user) => return reddit_url(&format!("user/{}", name(user)?)),
            Self::Community(sub) => return reddit_url(&format!("r/{}", name(sub)?)),
            Self::Multis => return reddit_url("/"),
            _ => {}
        }
        let (path, params) = self.route()?;
        let path = path.strip_suffix(".json").unwrap_or(&path);
        let mut url = Url::parse(&reddit_url(path)?).unwrap();
        if !params.is_empty() {
            url.query_pairs_mut().extend_pairs(params);
        }
        Ok(url.into())
    }
}

impl Reddit {
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

    fn experience_read(&self, query: &Query, after: Option<&str>) -> Result<Response> {
        if let Query::Inspect(target) = query {
            let value = self.thread_json(&target.inspection_permalink()?, "", target.comment())?;
            return inspect(value, target).map(Response::Details);
        }
        let (path, params) = query.route()?;
        let mut url = self.0.api.join(&path)?;
        url.query_pairs_mut()
            .extend_pairs(params)
            .append_pair("raw_json", "1")
            .append_pair("limit", "50");
        if let Some(after) = after {
            url.query_pairs_mut().append_pair("after", after);
        }
        let value = match self.get(url) {
            Ok(value) => value,
            Err(error) if error.is::<super::NotFound>() && matches!(query, Query::Wiki { .. }) => {
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

    fn csrf(&self) -> Result<HeaderValue> {
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
            .context("Reddit did not supply a CSRF token; reconnect with A")?;
        let mut header = HeaderValue::from_str(token)
            .map_err(|_| anyhow::anyhow!("Invalid Reddit CSRF token"))?;
        header.set_sensitive(true);
        Ok(header)
    }

    fn experience_write(&self, action: &Action) -> Result<Response> {
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

type Form = (String, Vec<(&'static str, String)>, &'static str);

impl Action {
    fn form(&self, account: Option<&str>) -> Result<Form> {
        let mut fields = Vec::new();
        let mut method = "POST";
        let path = match self {
            Self::Vote { id, direction } => {
                if !(-1..=1).contains(direction) {
                    bail!("Invalid vote direction");
                }
                fields.extend([
                    ("id", fullname(id, true)?.into()),
                    ("dir", direction.to_string()),
                ]);
                "api/vote".into()
            }
            Self::Save { id, save } => {
                fields.push(("id", fullname(id, true)?.into()));
                if *save { "api/save" } else { "api/unsave" }.into()
            }
            Self::Hide { id, hide } => {
                fields.push(("id", fullname(id, false)?.into()));
                if *hide { "api/hide" } else { "api/unhide" }.into()
            }
            Self::Reply { parent, text } => {
                fields.extend([
                    ("thing_id", fullname(parent, true)?.into()),
                    ("text", checked_text(text, 10000)?.into()),
                ]);
                "api/comment".into()
            }
            Self::Submit {
                subreddit,
                title,
                text,
                link,
            } => {
                name(subreddit)?;
                if !super::is_community(subreddit) {
                    bail!("Choose an individual community for a post");
                }
                fields.extend([
                    ("sr", subreddit.clone()),
                    ("title", checked_text(title, 300)?.into()),
                    ("kind", if *link { "link" } else { "self" }.into()),
                    ("sendreplies", "true".into()),
                ]);
                if *link {
                    let url = Url::parse(text.trim()).context("Enter an http or https link")?;
                    if !matches!(url.scheme(), "http" | "https")
                        || !url.username().is_empty()
                        || url.password().is_some()
                    {
                        bail!("Enter an http or https link without credentials");
                    }
                    fields.push(("url", url.into()));
                } else {
                    fields.push(("text", checked_text(text, 40000)?.into()));
                }
                "api/submit".into()
            }
            Self::Edit { target, text } => {
                target.inspection_permalink()?;
                let id = &target.id;
                fields.extend([
                    ("thing_id", fullname(id, true)?.into()),
                    (
                        "text",
                        checked_text(text, if id.starts_with("t1_") { 10000 } else { 40000 })?
                            .into(),
                    ),
                ]);
                "api/editusertext".into()
            }
            Self::Delete { target } => {
                target.inspection_permalink()?;
                fields.push(("id", fullname(&target.id, true)?.into()));
                "api/del".into()
            }
            Self::Report { id, reason } => {
                fields.extend([
                    ("thing_id", fullname(id, true)?.into()),
                    ("reason", checked_text(reason, 100)?.into()),
                ]);
                "api/report".into()
            }
            Self::Read { id, read } => {
                let tail = id
                    .strip_prefix("t4_")
                    .or_else(|| id.strip_prefix("t1_"))
                    .context("Invalid inbox identity")?;
                if tail.is_empty() || !tail.bytes().all(|c| c.is_ascii_alphanumeric()) {
                    bail!("Invalid inbox identity");
                }
                fields.push(("id", id.clone()));
                if *read {
                    "api/read_message"
                } else {
                    "api/unread_message"
                }
                .into()
            }
            Self::FollowPost { id, follow } => {
                fields.extend([
                    ("fullname", fullname(id, false)?.into()),
                    ("follow", follow.to_string()),
                ]);
                "api/follow_post".into()
            }
            Self::CreateMulti {
                name: multi,
                description,
            } => {
                let owner = name(account.context("Reconnect to identify your account")?)?;
                name(multi)?;
                fields.push(("model", json!({"display_name":multi,"description_md":description,"visibility":"private","subreddits":[]}).to_string()));
                format!("api/multi/user/{owner}/m/{multi}")
            }
            Self::MultiCommunity {
                owner,
                name: multi,
                subreddit,
                add,
            } => {
                own_multi(owner, account)?;
                name(multi)?;
                name(subreddit)?;
                method = if *add { "PUT" } else { "DELETE" };
                fields.push(("model", json!({"name":subreddit}).to_string()));
                format!("api/multi/user/{owner}/m/{multi}/r/{subreddit}")
            }
            Self::DeleteMulti { owner, name: multi } => {
                own_multi(owner, account)?;
                name(multi)?;
                method = "DELETE";
                format!("api/multi/user/{owner}/m/{multi}")
            }
        };
        Ok((path, fields, method))
    }
}

fn own_multi(owner: &str, account: Option<&str>) -> Result<()> {
    name(owner)?;
    if !account.is_some_and(|account| owner.eq_ignore_ascii_case(account)) {
        bail!("Only your own custom feeds can be changed");
    }
    Ok(())
}

fn checked_text(text: &str, limit: usize) -> Result<&str> {
    if text.trim().is_empty() {
        bail!("Text cannot be empty");
    }
    if text.chars().count() > limit {
        bail!("Text exceeds Reddit's {limit}-character limit");
    }
    if text
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
    {
        bail!("Text contains unsupported control characters");
    }
    Ok(text)
}

fn acknowledge(action: &Action, value: &Value) -> Result<()> {
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

fn inspect(value: Value, fallback: &Target) -> Result<Details> {
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

fn find_inspected_thing<'a>(things: &'a [Value], fullname: &str) -> Option<&'a Value> {
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

fn target(data: &Value, id: String) -> Target {
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

fn read_page(query: &Query, value: Value, show_nsfw: bool) -> Result<Page> {
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
            page.text = format!(
                "Post karma: {}    Comment karma: {}\n{}",
                data["link_karma"].as_i64().unwrap_or(0),
                data["comment_karma"].as_i64().unwrap_or(0),
                text::body(
                    "",
                    data["subreddit"]["public_description"]
                        .as_str()
                        .unwrap_or_default()
                )
            );
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
                Query::Search {
                    text,
                    kind,
                    sort,
                    time,
                } => format!(
                    "{}: {text} · {} · {}",
                    match kind {
                        SearchKind::Posts => "Posts",
                        SearchKind::Communities => "Communities",
                        SearchKind::People => "People",
                    },
                    sort.as_str(),
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
                if !show_nsfw
                    && (thing["data"]["over_18"] == true || thing["data"]["over18"] == true)
                {
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

fn navigation(label: &str, query: Query) -> Entry {
    Entry {
        title: label.into(),
        body: String::new(),
        target: None,
        post: None,
        query: Some(query),
    }
}

fn entry(thing: &Value) -> Result<Option<Entry>> {
    let data = &thing["data"];
    let mut entry = Entry {
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
        }
        Some("t2") => {
            let user = data["name"].as_str().context("User has no name")?;
            name(user)?;
            entry.title = format!("u/{user}");
            entry.query = Some(Query::Profile(user.into()));
        }
        _ => return Ok(None),
    }
    Ok(Some(entry))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reddit::tests::{Fixture, Reply, saved_export};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    fn me() -> Reply {
        Reply::json(json!({"data":{"name":"reader","modhash":"fixture-csrf"}}))
    }
    fn read(reddit: &Reddit, query: Query) -> Response {
        reddit
            .experience(&Request::Read { query, after: None })
            .unwrap()
    }

    fn comment_target() -> Target {
        Target {
            id: "t1_comment".into(),
            author: "reader".into(),
            subreddit: "rust".into(),
            permalink: "/r/rust/comments/post/title/comment/".into(),
        }
    }

    fn action_thread() -> Value {
        json!([
            {"kind":"Listing","data":{"children":[{"kind":"t3","data":{
                "name":"t3_post","id":"post","author":"reader","subreddit":"rust",
                "permalink":"/r/rust/comments/post/title/","selftext":"post source",
                "saved":false,"likes":null,"hidden":true,"is_self":true
            }}]}},
            {"kind":"Listing","data":{"children":[{"kind":"t1","data":{
                "name":"t1_ancestor","id":"ancestor","author":"other","body":"ancestor",
                "replies":{"kind":"Listing","data":{"children":[{"kind":"t1","data":{
                    "name":"t1_comment","id":"comment","author":"reader","subreddit":"rust",
                    "permalink":"/r/rust/comments/post/title/comment/","body":">!spoiler!<\n**bold**",
                    "saved":true,"likes":true
                }}]}}
            }}]}}
        ])
    }

    #[test]
    fn when_info_is_forbidden_comment_actions_should_load_through_the_focused_thread() {
        let fixture = Fixture::new(|request| {
            if request.url.starts_with("/api/info.json") {
                return Reply::status(403);
            }
            let url = Url::parse(&format!("http://fixture{}", request.url)).unwrap();
            let query: HashMap<_, _> = url.query_pairs().into_owned().collect();
            assert_eq!(request.method, "GET");
            assert_eq!(url.path(), "/comments/post.json");
            assert_eq!(query["comment"], "comment");
            assert_eq!(query["context"], "3");
            assert_eq!(query["raw_json"], "1");
            assert_eq!(query["limit"], "100");
            assert_eq!(query["depth"], "10");
            Reply::json(action_thread())
        });

        let Response::Details(actual) = read(&fixture.reddit(), Query::Inspect(comment_target()))
        else {
            panic!("Expected comment actions");
        };

        assert_eq!(actual.target.id, "t1_comment");
        assert_eq!(
            actual.target.permalink,
            "/r/rust/comments/post/title/comment/"
        );
        assert_eq!(actual.raw_text, ">!spoiler!<\n**bold**");
        assert!(actual.saved);
        assert_eq!(actual.vote, 1);
        assert!(actual.editable);
    }

    #[test]
    fn post_actions_should_use_the_post_fullname_and_preserve_its_metadata() {
        let fixture = Fixture::new(|request| {
            assert!(request.url.starts_with("/comments/post.json?"));
            assert!(!request.url.contains("comment="));
            let mut thread = action_thread();
            thread[0]["data"]["children"][0]["data"]["crosspost_parent_list"] =
                json!([{"permalink":"/comments/original/title/"}]);
            Reply::json(thread)
        });
        let target = Target {
            id: "t3_post".into(),
            permalink: String::new(),
            ..comment_target()
        };

        let Response::Details(actual) = read(&fixture.reddit(), Query::Inspect(target)) else {
            panic!("Expected post actions");
        };

        assert_eq!(actual.target.id, "t3_post");
        assert_eq!(actual.raw_text, "post source");
        assert!(actual.hidden);
        assert!(!actual.saved);
        assert_eq!(actual.vote, 0);
        assert!(actual.editable);
        assert_eq!(
            actual.crosspost.as_deref(),
            Some("/comments/original/title/")
        );
    }

    #[test]
    fn a_missing_comment_should_not_borrow_the_ancestors_action_state() {
        let fixture = Fixture::new(|_| Reply::json(action_thread()));
        let target = Target {
            id: "t1_missing".into(),
            ..comment_target()
        };

        let actual = fixture.reddit().experience(&Request::Read {
            query: Query::Inspect(target),
            after: None,
        });

        assert!(
            actual
                .unwrap_err()
                .to_string()
                .contains("deleted or is unavailable")
        );
    }

    #[test]
    fn a_conflicting_fullname_should_not_match_only_the_bare_comment_id() {
        let fixture = Fixture::new(|_| {
            let mut thread = action_thread();
            thread[1]["data"]["children"][0]["data"]["replies"]["data"]["children"][0]["data"]["name"] =
                json!("t1_other");
            Reply::json(thread)
        });

        let actual = fixture.reddit().experience(&Request::Read {
            query: Query::Inspect(comment_target()),
            after: None,
        });

        assert!(
            actual
                .unwrap_err()
                .to_string()
                .contains("deleted or is unavailable")
        );
    }

    #[test]
    fn malformed_thread_metadata_should_not_create_an_actions_menu() {
        let fixture =
            Fixture::new(|_| Reply::json(json!({"kind":"Listing","data":{"children":[]}})));

        let actual = fixture.reddit().experience(&Request::Read {
            query: Query::Inspect(comment_target()),
            after: None,
        });

        assert!(
            actual
                .unwrap_err()
                .to_string()
                .contains("Invalid content details")
        );
    }

    #[test]
    fn invalid_comment_locations_should_fail_before_contacting_reddit() {
        let fixture = Fixture::new(|_| panic!("Invalid location reached HTTP"));
        for permalink in [
            "",
            "https://other.example/comments/post/title/comment/",
            "/comments/../title/comment/",
        ] {
            let target = Target {
                permalink: permalink.into(),
                ..comment_target()
            };
            assert!(
                fixture
                    .reddit()
                    .experience(&Request::Read {
                        query: Query::Inspect(target),
                        after: None
                    })
                    .is_err()
            );
        }
    }

    #[test]
    fn editing_then_deleting_a_comment_should_recheck_the_captured_thread_target() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let actual = writes.clone();
        let fixture = Fixture::new(move |request| {
            if request.url == "/api/me.json" {
                return me();
            }
            if request.method == "GET" {
                assert!(request.url.starts_with("/comments/post.json?"));
                assert!(request.url.contains("comment=comment"));
                return Reply::json(action_thread());
            }
            let form: HashMap<_, _> = url::form_urlencoded::parse(request.body.as_bytes())
                .into_owned()
                .collect();
            match request.url.as_str() {
                "/api/editusertext" => {
                    assert_eq!(form["thing_id"], "t1_comment");
                    assert_eq!(form["text"], "changed text");
                    actual.lock().unwrap().push("edit");
                    Reply::json(
                        json!({"json":{"errors":[],"data":{"things":[{"kind":"t1","data":{"id":"comment"}}]}}}),
                    )
                }
                "/api/del" => {
                    assert_eq!(form["id"], "t1_comment");
                    actual.lock().unwrap().push("delete");
                    Reply::json(json!({}))
                }
                _ => panic!("Unexpected mutation route"),
            }
        });
        let reddit = fixture.saved_reddit(&saved_export("reader", "reader"));

        reddit
            .experience(&Request::Write(Action::Edit {
                target: comment_target(),
                text: "changed text".into(),
            }))
            .unwrap();
        reddit
            .experience(&Request::Write(Action::Delete {
                target: comment_target(),
            }))
            .unwrap();

        assert_eq!(*writes.lock().unwrap(), ["edit", "delete"]);
    }

    #[test]
    fn editing_a_link_post_should_be_rejected_after_fresh_metadata_is_read() {
        let fixture = Fixture::new(|request| {
            if request.url == "/api/me.json" {
                return me();
            }
            assert_eq!(request.method, "GET");
            let mut thread = action_thread();
            thread[0]["data"]["children"][0]["data"]["is_self"] = json!(false);
            Reply::json(thread)
        });
        let reddit = fixture.saved_reddit(&saved_export("reader", "reader"));
        let target = Target {
            id: "t3_post".into(),
            permalink: "/comments/post".into(),
            ..comment_target()
        };

        let actual = reddit.experience(&Request::Write(Action::Edit {
            target,
            text: "changed".into(),
        }));

        assert!(
            actual
                .unwrap_err()
                .to_string()
                .contains("Only comments and text-post bodies")
        );
    }

    #[test]
    fn when_a_subreddit_is_named_comments_the_context_link_should_still_target_the_comment() {
        assert_eq!(
            thread_location(
                "https://www.reddit.com/r/comments/comments/post/title/reply/?context=3"
            )
            .unwrap(),
            (
                "/r/comments/comments/post/title/reply/".into(),
                Some("reply".into())
            )
        );
    }

    #[test]
    fn when_moderators_use_the_userlist_schema_their_profiles_should_be_navigable() {
        let fixture = Fixture::new(|request| {
            assert!(request.url.starts_with("/r/rust/about/moderators.json?"));
            Reply::json(
                json!({"kind":"UserList","data":{"children":[{"id":"t2_mod","name":"mod"}]}}),
            )
        });
        let Response::Page(actual) = read(&fixture.reddit(), Query::Moderators("rust".into()))
        else {
            panic!("Expected moderators");
        };
        assert!(matches!(&actual.entries[0].query, Some(Query::Profile(user)) if user == "mod"));
    }

    #[test]
    fn when_a_wiki_page_is_missing_it_should_show_an_unavailable_page_without_a_request_error() {
        let fixture = Fixture::new(|_| Reply::status(404));
        let actual = read(
            &fixture.reddit(),
            Query::Wiki {
                subreddit: "rust".into(),
                page: "index".into(),
            },
        );
        assert!(
            matches!(actual, Response::Page(page) if page.entries.is_empty()
            && page.text == "This wiki page is unavailable. Reddit did not find a public page at this address."
            && page.url == "https://www.reddit.com/r/rust/wiki/index")
        );
    }

    #[test]
    fn when_a_wiki_request_is_denied_it_should_preserve_the_permission_error() {
        let fixture = Fixture::new(|_| Reply::status(403));
        let actual = fixture
            .reddit()
            .experience(&Request::Read {
                query: Query::Wiki {
                    subreddit: "rust".into(),
                    page: "index".into(),
                },
                after: None,
            })
            .unwrap_err();
        assert!(actual.to_string().contains("HTTP 403"));
    }

    #[test]
    fn when_a_notification_is_a_comment_its_context_should_open_a_replyable_comment() {
        let fixture = Fixture::new(|_| {
            Reply::json(
                json!({"kind":"Listing","data":{"children":[{"kind":"t4","data":{"id":"reply","name":"t4_reply","was_comment":true,"author":"author","body":"reply","context":"/r/rust/comments/post/title/reply/?context=3"}}],"after":null}}),
            )
        });
        let Response::Page(actual) = read(&fixture.reddit(), Query::Inbox(Inbox::Unread)) else {
            panic!("Expected inbox");
        };
        assert_eq!(actual.entries[0].target.as_ref().unwrap().id, "t1_reply");
        assert_eq!(
            actual.entries[0].target.as_ref().unwrap().permalink,
            "/r/rust/comments/post/title/reply/?context=3"
        );
    }

    #[test]
    fn when_votes_are_changed_and_removed_the_server_should_receive_each_explicit_direction() {
        let actual = Arc::new(Mutex::new(Vec::new()));
        let votes = actual.clone();
        let fixture = Fixture::new(move |request| {
            if request.url == "/api/me.json" {
                return me();
            }
            assert_eq!(request.url, "/api/vote");
            assert_eq!(request.method, "POST");
            assert_eq!(request.headers["x-modhash"], "fixture-csrf");
            let form: HashMap<_, _> = url::form_urlencoded::parse(request.body.as_bytes())
                .into_owned()
                .collect();
            assert_eq!(form["id"], "t1_comment");
            votes.lock().unwrap().push(form["dir"].clone());
            Reply::json(json!({}))
        });
        let reddit = fixture.reddit();
        for direction in [1, 1, -1, 0, 1] {
            reddit
                .experience(&Request::Write(Action::Vote {
                    id: "t1_comment".into(),
                    direction,
                }))
                .unwrap();
        }
        assert_eq!(*actual.lock().unwrap(), ["1", "1", "-1", "0", "1"]);
    }

    #[test]
    fn when_saving_hiding_and_following_posts_the_forms_should_match_the_documented_contract() {
        let actual = Arc::new(Mutex::new(Vec::new()));
        let forms = actual.clone();
        let fixture = Fixture::new(move |request| {
            if request.url == "/api/me.json" {
                return me();
            }
            let form: HashMap<_, _> = url::form_urlencoded::parse(request.body.as_bytes())
                .into_owned()
                .collect();
            forms.lock().unwrap().push((request.url, form));
            Reply::json(json!({}))
        });
        let reddit = fixture.reddit();
        for action in [
            Action::Save {
                id: "t1_comment".into(),
                save: true,
            },
            Action::Save {
                id: "t1_comment".into(),
                save: false,
            },
            Action::Save {
                id: "t1_comment".into(),
                save: true,
            },
            Action::Hide {
                id: "t3_post".into(),
                hide: true,
            },
            Action::Hide {
                id: "t3_post".into(),
                hide: false,
            },
            Action::FollowPost {
                id: "t3_post".into(),
                follow: true,
            },
            Action::FollowPost {
                id: "t3_post".into(),
                follow: false,
            },
        ] {
            reddit.experience(&Request::Write(action)).unwrap();
        }
        let actual = actual.lock().unwrap();
        assert_eq!(
            actual
                .iter()
                .map(|(path, _)| path.as_str())
                .collect::<Vec<_>>(),
            [
                "/api/save",
                "/api/unsave",
                "/api/save",
                "/api/hide",
                "/api/unhide",
                "/api/follow_post",
                "/api/follow_post"
            ]
        );
        assert_eq!(actual[0].1["id"], "t1_comment");
        assert_eq!(actual[5].1["fullname"], "t3_post");
        assert_eq!(actual[5].1["follow"], "true");
        assert_eq!(actual[6].1["follow"], "false");
    }

    #[test]
    fn when_replying_with_unicode_and_markdown_the_body_should_round_trip_without_command_interpretation()
     {
        let fixture = Fixture::new(|request| {
            if request.url == "/api/me.json" {
                return me();
            }
            assert_eq!(request.url, "/api/comment");
            let form: HashMap<_, _> = url::form_urlencoded::parse(request.body.as_bytes())
                .into_owned()
                .collect();
            assert_eq!(form["thing_id"], "t1_parent");
            assert_eq!(form["text"], "**Հայերեն**\n$(not-a-command) & x");
            Reply::json(
                json!({"json":{"errors":[],"data":{"things":[{"kind":"t1","data":{"id":"new","parent_id":"t1_parent"}}]}}}),
            )
        });
        assert!(matches!(
            fixture
                .reddit()
                .experience(&Request::Write(Action::Reply {
                    parent: "t1_parent".into(),
                    text: "**Հայերեն**\n$(not-a-command) & x".into()
                }))
                .unwrap(),
            Response::Written { .. }
        ));
    }

    #[test]
    fn when_submitting_a_link_the_url_should_be_separate_from_the_title_and_return_a_reddit_link() {
        let fixture = Fixture::new(|request| {
            if request.url == "/api/me.json" {
                return me();
            }
            assert_eq!(request.url, "/api/submit");
            let form: HashMap<_, _> = url::form_urlencoded::parse(request.body.as_bytes())
                .into_owned()
                .collect();
            assert_eq!(form["kind"], "link");
            assert_eq!(form["sr"], "rust");
            assert_eq!(form["title"], "A useful link");
            assert_eq!(form["url"], "https://example.org/?x=1&y=2");
            Reply::json(
                json!({"json":{"errors":[],"data":{"url":"https://www.reddit.com/r/rust/comments/post/a/","name":"t3_post"}}}),
            )
        });
        let actual = fixture
            .reddit()
            .experience(&Request::Write(Action::Submit {
                subreddit: "rust".into(),
                title: "A useful link".into(),
                text: "https://example.org/?x=1&y=2".into(),
                link: true,
            }))
            .unwrap();
        assert!(
            matches!(actual, Response::Written { url: Some(url) } if url == "https://www.reddit.com/r/rust/comments/post/a/")
        );
    }

    #[test]
    fn when_submitting_a_text_post_its_markdown_should_remain_in_the_text_field() {
        let fixture = Fixture::new(|request| {
            if request.url == "/api/me.json" {
                return me();
            }
            assert_eq!(request.method, "POST");
            assert_eq!(request.url, "/api/submit");
            let form: HashMap<_, _> = url::form_urlencoded::parse(request.body.as_bytes())
                .into_owned()
                .collect();
            assert_eq!(form["kind"], "self");
            assert_eq!(form["sr"], "rust");
            assert_eq!(form["title"], "A text post");
            assert_eq!(form["text"], "**Հայերեն**\nA & B");
            assert!(!form.contains_key("url"));
            Reply::json(json!({"json":{"errors":[],"data":{"url":"/r/rust/comments/post/text/"}}}))
        });

        let actual = fixture
            .reddit()
            .experience(&Request::Write(Action::Submit {
                subreddit: "rust".into(),
                title: "A text post".into(),
                text: "**Հայերեն**\nA & B".into(),
                link: false,
            }))
            .unwrap();

        assert!(matches!(actual, Response::Written { url: Some(url) }
            if url == "https://www.reddit.com/r/rust/comments/post/text/"));
    }

    #[test]
    fn when_reporting_a_comment_the_reason_should_be_sent_with_its_fullname() {
        let fixture = Fixture::new(|request| {
            if request.url == "/api/me.json" {
                return me();
            }
            assert_eq!(request.method, "POST");
            assert_eq!(request.url, "/api/report");
            assert_eq!(request.headers["x-modhash"], "fixture-csrf");
            let form: HashMap<_, _> = url::form_urlencoded::parse(request.body.as_bytes())
                .into_owned()
                .collect();
            assert_eq!(form["thing_id"], "t1_comment");
            assert_eq!(form["reason"], "Spam & unwanted promotion");
            Reply::json(json!({}))
        });

        let actual = fixture
            .reddit()
            .experience(&Request::Write(Action::Report {
                id: "t1_comment".into(),
                reason: "Spam & unwanted promotion".into(),
            }))
            .unwrap();

        assert!(matches!(actual, Response::Written { url: None }));
    }

    #[test]
    fn when_inbox_items_are_marked_read_unread_and_read_the_explicit_state_should_be_sent() {
        let actual = Arc::new(Mutex::new(Vec::new()));
        let requests = actual.clone();
        let fixture = Fixture::new(move |request| {
            if request.url == "/api/me.json" {
                return me();
            }
            assert_eq!(request.method, "POST");
            let form: HashMap<_, _> = url::form_urlencoded::parse(request.body.as_bytes())
                .into_owned()
                .collect();
            requests
                .lock()
                .unwrap()
                .push((request.url, form["id"].clone()));
            Reply::json(json!({}))
        });
        let reddit = fixture.reddit();

        for (id, read) in [
            ("t4_message", true),
            ("t4_message", false),
            ("t4_message", true),
            ("t1_comment", true),
            ("t1_comment", false),
        ] {
            reddit
                .experience(&Request::Write(Action::Read {
                    id: id.into(),
                    read,
                }))
                .unwrap();
        }

        assert_eq!(
            *actual.lock().unwrap(),
            [
                ("/api/read_message".into(), "t4_message".into()),
                ("/api/unread_message".into(), "t4_message".into()),
                ("/api/read_message".into(), "t4_message".into()),
                ("/api/read_message".into(), "t1_comment".into()),
                ("/api/unread_message".into(), "t1_comment".into()),
            ]
        );
    }

    #[test]
    fn when_the_api_reports_an_error_the_write_should_not_claim_success_or_echo_submitted_text() {
        let fixture = Fixture::new(|request| {
            if request.url == "/api/me.json" {
                return me();
            }
            Reply::json(json!({"json":{"errors":[["RATELIMIT","private submitted text","text"]]}}))
        });
        let error = fixture
            .reddit()
            .experience(&Request::Write(Action::Reply {
                parent: "t3_post".into(),
                text: "private submitted text".into(),
            }))
            .unwrap_err()
            .to_string();
        assert!(error.contains("RATELIMIT"));
        assert!(!error.contains("private submitted text"));
    }

    #[test]
    fn when_a_write_is_unacknowledged_it_should_not_treat_http_success_as_api_success() {
        let fixture = Fixture::new(|request| {
            if request.url == "/api/me.json" {
                me()
            } else {
                Reply::json(json!({"unexpected":true}))
            }
        });
        assert!(
            fixture
                .reddit()
                .experience(&Request::Write(Action::Save {
                    id: "t3_post".into(),
                    save: true
                }))
                .unwrap_err()
                .to_string()
                .contains("acknowledge")
        );
    }

    #[test]
    fn when_an_edit_targets_another_author_it_should_not_send_the_mutation() {
        let fixture = Fixture::new(|request| {
            if request.url == "/api/me.json" {
                return me();
            }
            assert_eq!(request.method, "GET");
            let mut thread = action_thread();
            thread[1]["data"]["children"][0]["data"]["replies"]["data"]["children"][0]["data"]["author"] =
                json!("other");
            Reply::json(thread)
        });
        let reddit = fixture.saved_reddit(&saved_export("reader", "reader"));
        assert!(
            reddit
                .experience(&Request::Write(Action::Edit {
                    target: comment_target(),
                    text: "changed".into()
                }))
                .unwrap_err()
                .to_string()
                .contains("Only your own")
        );
    }

    #[test]
    fn when_the_verified_account_changes_it_should_not_write_through_the_old_account() {
        let fixture = Fixture::new(|request| {
            assert_eq!(request.method, "GET");
            Reply::json(json!({"data":{"name":"someoneelse","modhash":"fixture-csrf"}}))
        });
        let reddit = fixture.saved_reddit(&saved_export("reader", "reader"));
        assert!(
            reddit
                .experience(&Request::Write(Action::Save {
                    id: "t3_post".into(),
                    save: true
                }))
                .unwrap_err()
                .to_string()
                .contains("account changed")
        );
    }

    #[test]
    fn when_inputs_are_invalid_the_client_should_reject_them_before_any_request() {
        let fixture = Fixture::new(|_| panic!("Invalid input reached HTTP"));
        for action in [
            Action::Vote {
                id: "t3_post".into(),
                direction: 2,
            },
            Action::Save {
                id: "../bad".into(),
                save: true,
            },
            Action::Hide {
                id: "t1_comment".into(),
                hide: true,
            },
            Action::Reply {
                parent: "t3_post".into(),
                text: " ".into(),
            },
            Action::Submit {
                subreddit: "all".into(),
                title: "title".into(),
                text: "body".into(),
                link: false,
            },
            Action::Submit {
                subreddit: "rust".into(),
                title: "title".into(),
                text: "file:///private/file".into(),
                link: true,
            },
        ] {
            assert!(
                fixture
                    .reddit()
                    .experience(&Request::Write(action))
                    .is_err()
            );
        }
    }

    #[test]
    fn when_the_connection_is_retired_no_collection_or_write_should_contact_reddit() {
        let fixture = Fixture::new(|_| panic!("Retired session reached HTTP"));
        let reddit = fixture.reddit();
        reddit.retire();
        assert!(
            reddit
                .experience(&Request::Write(Action::Save {
                    id: "t3_post".into(),
                    save: true
                }))
                .is_err()
        );
        assert!(
            reddit
                .experience(&Request::Read {
                    query: Query::Multis,
                    after: None
                })
                .is_err()
        );
    }

    #[test]
    fn collection_post_titles_should_distinguish_profile_posts_from_communities() {
        for (subreddit, permalink, label) in [
            (
                "u_blackvault",
                "/user/blackvault/comments/post/title/",
                "u/blackvault",
            ),
            (
                "u_ordinary",
                "/r/u_ordinary/comments/post/title/",
                "r/u_ordinary",
            ),
        ] {
            let actual = entry(&json!({"kind":"t3", "data":{
                "id":"post", "title":"Submission", "subreddit":subreddit,
                "author":"reader", "permalink":permalink
            }}))
            .unwrap()
            .unwrap();
            assert!(
                actual.title.contains(&format!(" · {label} · ")),
                "{}",
                actual.title
            );
        }
    }

    #[test]
    fn when_a_saved_listing_contains_posts_and_comments_both_should_be_returned() {
        let fixture = Fixture::new(|request| {
            assert!(request.url.starts_with("/user/reader/saved.json?"));
            Reply::json(json!({"kind":"Listing","data":{"children":[
                {"kind":"t3","data":{"id":"post","title":"Saved post","subreddit":"rust","author":"a","permalink":"/r/rust/comments/post/a/"}},
                {"kind":"t1","data":{"id":"comment","author":"b","body":"saved reply","permalink":"/r/rust/comments/post/a/comment/","link_title":"Saved post","subreddit":"rust"}}
            ],"after":"t1_next"}}))
        });
        let Response::Page(actual) = read(
            &fixture.reddit(),
            Query::User {
                name: "reader".into(),
                feed: UserFeed::Saved,
            },
        ) else {
            panic!("Expected saved items");
        };
        assert_eq!(actual.entries.len(), 2);
        assert_eq!(actual.after.as_deref(), Some("t1_next"));
        assert_eq!(actual.entries[1].target.as_ref().unwrap().id, "t1_comment");
        assert_eq!(actual.entries[1].body, "saved reply");
    }

    #[test]
    fn when_searching_globally_the_query_sort_time_and_cursor_should_be_encoded() {
        let fixture = Fixture::new(|request| {
            let url = Url::parse(&format!("http://fixture{}", request.url)).unwrap();
            assert_eq!(url.path(), "/search.json");
            let params: HashMap<_, _> = url.query_pairs().into_owned().collect();
            assert_eq!(params["q"], "Հայերեն & rust");
            assert_eq!(params["sort"], "comments");
            assert_eq!(params["t"], "week");
            assert_eq!(params["after"], "cursor&next");
            assert!(!params.contains_key("restrict_sr"));
            Reply::json(json!({"kind":"Listing","data":{"children":[],"after":null}}))
        });
        assert!(matches!(
            fixture
                .reddit()
                .experience(&Request::Read {
                    query: Query::Search {
                        text: "Հայերեն & rust".into(),
                        kind: SearchKind::Posts,
                        sort: SearchSort::Comments,
                        time: TimeRange::Week
                    },
                    after: Some("cursor&next".into())
                })
                .unwrap(),
            Response::Page(_)
        ));
    }

    #[test]
    fn when_reading_notifications_it_should_not_implicitly_mark_them_read() {
        let fixture = Fixture::new(|request| {
            let url = Url::parse(&format!("http://fixture{}", request.url)).unwrap();
            assert_eq!(url.path(), "/message/comments.json");
            assert!(url.query_pairs().any(|(k, v)| k == "mark" && v == "false"));
            Reply::json(json!({"kind":"Listing","data":{"children":[],"after":null}}))
        });
        assert!(matches!(
            read(&fixture.reddit(), Query::Inbox(Inbox::Replies)),
            Response::Page(_)
        ));
    }

    #[test]
    fn when_a_custom_feed_is_created_added_removed_and_deleted_the_methods_should_match_the_multi_contract()
     {
        let actual = Arc::new(Mutex::new(Vec::new()));
        let methods = actual.clone();
        let fixture = Fixture::new(move |request| {
            if request.url == "/api/me.json" {
                return me();
            }
            methods
                .lock()
                .unwrap()
                .push((request.method.clone(), request.url.clone()));
            if request.method == "DELETE" {
                return Reply {
                    status: 204,
                    body: String::new(),
                    headers: vec![],
                };
            }
            if request.url.ends_with("/r/rust") {
                Reply::json(json!({"name":"rust"}))
            } else {
                Reply::json(json!({"kind":"LabeledMulti","data":{"path":"/user/reader/m/tools"}}))
            }
        });
        let reddit = fixture.saved_reddit(&saved_export("reader", "reader"));
        for action in [
            Action::CreateMulti {
                name: "tools".into(),
                description: "Tools".into(),
            },
            Action::MultiCommunity {
                owner: "reader".into(),
                name: "tools".into(),
                subreddit: "rust".into(),
                add: true,
            },
            Action::MultiCommunity {
                owner: "reader".into(),
                name: "tools".into(),
                subreddit: "rust".into(),
                add: false,
            },
            Action::DeleteMulti {
                owner: "reader".into(),
                name: "tools".into(),
            },
        ] {
            reddit.experience(&Request::Write(action)).unwrap();
        }
        assert_eq!(
            *actual.lock().unwrap(),
            [
                ("POST".into(), "/api/multi/user/reader/m/tools".into()),
                ("PUT".into(), "/api/multi/user/reader/m/tools/r/rust".into()),
                (
                    "DELETE".into(),
                    "/api/multi/user/reader/m/tools/r/rust".into()
                ),
                ("DELETE".into(), "/api/multi/user/reader/m/tools".into())
            ]
        );
    }

    #[test]
    fn when_an_external_link_is_presented_as_a_native_handoff_it_should_be_rejected() {
        assert!(reddit_url("https://evil.invalid/content").is_err());
        assert!(reddit_url("//evil.invalid/content").is_err());
        assert!(reddit_url("https://user:password@www.reddit.com/").is_err());
        assert_eq!(
            reddit_url("/r/rust").unwrap(),
            "https://www.reddit.com/r/rust"
        );
    }
}
