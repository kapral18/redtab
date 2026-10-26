//! Account subscription writes; cookies and CSRF tokens stay in memory.

use crate::model::MembershipTarget;
use anyhow::{Context, Result, bail};
use wreq::header::HeaderValue;

use super::{Reddit, transport};

pub(crate) fn is_community(name: &str) -> bool {
    crate::config::is_subreddit_name(name)
        && !matches!(name.to_ascii_lowercase().as_str(), "all" | "popular")
}

impl Reddit {
    pub fn set_membership(&self, target: &MembershipTarget, join: bool) -> Result<()> {
        if !target.is_valid() {
            bail!("Choose a valid community or user to follow");
        }
        let me = self.get(self.0.api.join("api/me.json")?)?;
        let account = me["data"]["name"]
            .as_str()
            .filter(|name| !name.is_empty())
            .context("Reddit did not confirm the signed-in account")?;
        if self
            .account()
            .is_some_and(|expected| !expected.name.eq_ignore_ascii_case(account))
        {
            bail!("Reddit account changed; reconnect before writing");
        }
        let token = me["data"]["modhash"]
            .as_str()
            .filter(|value| !value.is_empty())
            .context("Reddit did not supply a CSRF token; sign in again")?;
        let mut token = HeaderValue::from_str(token)
            .map_err(|_| anyhow::anyhow!("Reddit supplied an invalid CSRF token"))?;
        token.set_sensitive(true);
        let mut form = url::form_urlencoded::Serializer::new(String::new());
        form.append_pair("action", if join { "sub" } else { "unsub" })
            .append_pair("api_type", "json");
        match target {
            MembershipTarget::Community(name) => {
                form.append_pair("sr_name", name);
            }
            MembershipTarget::User(name) => {
                form.append_pair("sr", &self.profile_membership_id(name)?);
            }
        }
        if join {
            form.append_pair("skip_initial_defaults", "true");
        }
        let body = form.finish();
        let response = self.send(
            self.0.api.join("api/subscribe")?,
            "application/json",
            |client, url| {
                client
                    .post(url)
                    .header("X-Modhash", token)
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(body)
            },
        )?;
        let value = transport::json(response, None)?;
        if value.as_object().is_some_and(|object| object.is_empty())
            || (value["json"]["errors"]
                .as_array()
                .is_some_and(|errors| errors.is_empty())
                && value.get("error").is_none())
        {
            return Ok(());
        }
        bail!(
            "Reddit did not acknowledge the membership change; refresh subscriptions before retrying"
        )
    }

    fn profile_membership_id(&self, name: &str) -> Result<String> {
        let profile = self.get(self.0.api.join(&format!("user/{name}/about.json"))?)?;
        let data = &profile["data"];
        let subreddit = &data["subreddit"];
        let valid = data["name"]
            .as_str()
            .is_some_and(|actual| actual.eq_ignore_ascii_case(name))
            && subreddit["subreddit_type"] == "user"
            && subreddit["url"]
                .as_str()
                .and_then(crate::model::profile_user)
                .is_some_and(|actual| actual.eq_ignore_ascii_case(name));
        if !valid {
            bail!(
                "Reddit did not confirm this user's profile; open the profile in your browser to check access"
            );
        }
        let id = subreddit["name"]
            .as_str()
            .filter(|id| {
                id.strip_prefix("t5_").is_some_and(|suffix| {
                    !suffix.is_empty()
                        && suffix.len() <= 32
                        && suffix.bytes().all(|c| c.is_ascii_alphanumeric())
                })
            })
            .context("Reddit did not identify this user's profile subscription")?;
        Ok(id.into())
    }
}

impl MembershipTarget {
    pub fn is_valid(&self) -> bool {
        match self {
            Self::Community(name) => is_community(name),
            Self::User(name) => crate::model::is_username(name),
        }
    }
}
