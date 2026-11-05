use super::*;

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

impl Action {
    pub fn changes_reader_content(&self) -> bool {
        matches!(
            self,
            Self::Vote { .. }
                | Self::Save { .. }
                | Self::Hide { .. }
                | Self::Reply { .. }
                | Self::Submit { .. }
                | Self::Edit { .. }
                | Self::Delete { .. }
                | Self::Report { .. }
        )
    }
}

pub(super) type Form = (String, Vec<(&'static str, String)>, &'static str);

impl Action {
    pub(super) fn form(&self, account: Option<&str>) -> Result<Form> {
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
                if !super::super::is_community(subreddit) {
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

pub(super) fn own_multi(owner: &str, account: Option<&str>) -> Result<()> {
    name(owner)?;
    if !account.is_some_and(|account| owner.eq_ignore_ascii_case(account)) {
        bail!("Only your own custom feeds can be changed");
    }
    Ok(())
}

pub(super) fn checked_text(text: &str, limit: usize) -> Result<&str> {
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
