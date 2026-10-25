//! Account subscription writes; cookies and CSRF tokens stay in memory.

use anyhow::{Context, Result, bail};
use wreq::header::HeaderValue;

use super::{Reddit, transport};

pub(crate) fn is_community(name: &str) -> bool {
    crate::config::is_subreddit_name(name)
        && !matches!(name.to_ascii_lowercase().as_str(), "all" | "popular")
}

impl Reddit {
    pub fn set_subscription(&self, subreddit: &str, join: bool) -> Result<()> {
        if !is_community(subreddit) {
            bail!("Choose an individual subreddit to join or leave");
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
            .append_pair("sr_name", subreddit)
            .append_pair("api_type", "json");
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
}
