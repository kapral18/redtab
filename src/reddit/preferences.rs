//! Browser account preferences; account rules remain in config.
use anyhow::{Context, Result, bail};
use scraper::{Html, Selector};
use serde_json::{Value, json};

use super::{Reddit, transport};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preferences {
    pub show_nsfw: bool,
    pub video_autoplay: bool,
    pub mature_content_editable: bool,
}

#[derive(Clone, Copy)]
pub enum Preference {
    MatureContent,
    Autoplay,
}

impl Preference {
    pub fn value(self, prefs: &Preferences) -> bool {
        match self {
            Self::MatureContent => prefs.show_nsfw,
            Self::Autoplay => prefs.video_autoplay,
        }
    }

    fn input(self, value: bool) -> Value {
        match self {
            Self::MatureContent => json!({"isNsfwEnabled":value}),
            Self::Autoplay => json!({"isVideoAutoplayEnabled":value}),
        }
    }
}

impl Reddit {
    pub fn preferences(&self) -> Result<Preferences> {
        let account = self
            .account()
            .context("Connect an identified Reddit account in Accounts")?;
        let response = self.request(self.0.api.join("settings/preferences")?, "text/html")?;
        let mut html = String::new();
        use std::io::Read;
        transport::Reader::new(response, 4 * 1024 * 1024, Some(self.read_cancellation()))
            .read_to_string(&mut html)
            .context("Could not read Reddit account preferences")?;
        parse(&html, &account.id)
    }

    pub fn set_preference(&self, key: Preference, value: bool) -> Result<Preferences> {
        let account = self
            .account()
            .context("Connect an identified Reddit account in Accounts")?;
        let me = self.get(self.0.api.join("api/me.json")?)?;
        if me["data"]["id"].as_str() != Some(&account.id) {
            bail!("Reddit account changed; reconnect before saving preferences");
        }
        let csrf = self.0.session.csrf_token()?;
        let body = json!({"operation":"UpdateAccountPreferences", "variables":{"input":key.input(value)}, "csrf_token":csrf}).to_string();
        let response = self.send(
            self.0.api.join("svc/shreddit/graphql")?,
            "application/json",
            |client, url| {
                client
                    .post(url)
                    .header("Content-Type", "application/json")
                    .header(
                        "Referer",
                        self.0.api.join("settings/preferences").unwrap().as_str(),
                    )
                    .body(body)
            },
        )?;
        let result = transport::json(response, None)?;
        if result
            .get("errors")
            .is_some_and(|v| !v.is_null() && !v.as_array().is_some_and(Vec::is_empty))
            || !result["data"].is_object()
        {
            bail!("Reddit did not confirm the preference save; r reloads the account values");
        }
        let actual = self
            .preferences()
            .context("Save may have succeeded; r reloads Reddit before another change")?;
        if key.value(&actual) != value {
            bail!("Reddit kept a different preference value; r reloads it before another change");
        }
        Ok(actual)
    }
}

fn parse(html: &str, expected: &str) -> Result<Preferences> {
    let document = Html::parse_document(html);
    let selector = Selector::parse("settings-preferences[initialstate]").expect("static selector");
    let mut nodes = document.select(&selector);
    let node = nodes
        .next()
        .context("Reddit did not provide account preferences; reconnect or open Reddit settings")?;
    if nodes.next().is_some() {
        bail!("Reddit returned ambiguous account preferences");
    }
    let initial: Value = serde_json::from_str(node.value().attr("initialstate").unwrap())
        .map_err(|_| anyhow::anyhow!("Reddit returned invalid account preferences"))?;
    if let Some(id) = initial["identity"]["id"].as_str()
        && id.strip_prefix("t2_").unwrap_or(id) != expected
    {
        bail!("Reddit preferences belong to another account; reconnect through Accounts");
    }
    let prefs = &initial["identity"]["preferences"];
    let age_gated = node.value().attr("is-age-gated-and-not-verified").is_some();
    Ok(Preferences {
        show_nsfw: prefs["isNsfwContentShown"]
            .as_bool()
            .context("Reddit did not provide the mature-content preference")?
            && !age_gated,
        video_autoplay: !prefs["isVideoAutoplayDisabled"]
            .as_bool()
            .context("Reddit did not provide the autoplay preference")?,
        mature_content_editable: !age_gated
            && initial["identity"]["preferencesState"]["isNsfwEnabled"]["state"] != "DISABLED",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reddit::tests::{Fixture, Reply, saved_export};
    use std::sync::{Arc, Mutex};

    fn markup(mature: bool, autoplay: bool) -> String {
        format!(
            "<settings-preferences initialstate='{}'></settings-preferences>",
            json!({"identity":{"id":"t2_readerid", "preferences":{"isNsfwContentShown":mature,"isVideoAutoplayDisabled":!autoplay}}})
        )
    }

    fn page(mature: bool, autoplay: bool) -> Reply {
        Reply {
            status: 200,
            body: markup(mature, autoplay),
            headers: vec![],
        }
    }

    fn export() -> String {
        format!(
            "{}.reddit.com\tTRUE\t/\tTRUE\t0\tcsrf_token\tfixture-csrf\n",
            saved_export("readerid", "reader")
        )
    }

    #[test]
    fn when_native_preferences_load_they_should_invert_disabled_autoplay_and_keep_mature_access_separate()
     {
        let fixture = Fixture::new(|request| {
            assert_eq!(request.url, "/settings/preferences");
            page(true, false)
        });
        let actual = fixture.saved_reddit(&export()).preferences().unwrap();
        assert!(actual.show_nsfw);
        assert!(!actual.video_autoplay);
        assert!(actual.mature_content_editable);
    }

    #[test]
    fn native_preferences_load_only_when_complete_and_for_this_account() {
        let incomplete = "<settings-preferences initialstate='{\"identity\":{\"preferences\":{\"isNsfwContentShown\":true}}}'></settings-preferences>";
        for (html, account) in [
            (incomplete.to_string(), "readerid"),
            (markup(true, true), "someoneelse"),
            ("<html>Sign in</html>".to_string(), "readerid"),
        ] {
            assert!(parse(&html, account).is_err(), "{account}: {html}");
        }
    }

    #[test]
    fn when_a_gated_account_has_raw_mature_access_it_should_still_hide_mature_content() {
        let html = markup(true, true).replace(
            "initialstate=",
            "is-age-gated-and-not-verified initialstate=",
        );
        let actual = parse(&html, "readerid").unwrap();
        assert!(!actual.show_nsfw);
        assert!(!actual.mature_content_editable);
    }

    #[test]
    fn when_autoplay_changes_it_should_send_only_that_native_input_and_confirm_the_account_value() {
        let stored = Arc::new(Mutex::new(true));
        let actual = stored.clone();
        let fixture = Fixture::new(move |request| match request.url.as_str() {
            "/api/me.json" => Reply::json(json!({"data":{"id":"readerid","name":"reader"}})),
            "/svc/shreddit/graphql" => {
                assert_eq!(request.method, "POST");
                assert_eq!(request.headers["content-type"], "application/json");
                assert_eq!(
                    serde_json::from_str::<Value>(&request.body).unwrap(),
                    json!({"operation":"UpdateAccountPreferences","variables":{"input":{"isVideoAutoplayEnabled":false}},"csrf_token":"fixture-csrf"})
                );
                *stored.lock().unwrap() = false;
                Reply::json(json!({"data":{"updateAccountPreferences":{"ok":true}}}))
            }
            "/settings/preferences" => page(true, *stored.lock().unwrap()),
            _ => panic!("Unexpected preference request"),
        });
        let prefs = fixture
            .saved_reddit(&export())
            .set_preference(Preference::Autoplay, false)
            .unwrap();
        assert!(!prefs.video_autoplay);
        assert!(prefs.show_nsfw);
        assert!(!*actual.lock().unwrap());
    }

    #[test]
    fn when_mature_content_changes_it_should_send_only_that_native_input() {
        let fixture = Fixture::new(|request| match request.url.as_str() {
            "/api/me.json" => Reply::json(json!({"data":{"id":"readerid","name":"reader"}})),
            "/svc/shreddit/graphql" => {
                let body: Value = serde_json::from_str(&request.body).unwrap();
                assert_eq!(body["variables"]["input"], json!({"isNsfwEnabled":true}));
                Reply::json(json!({"data":{"updateAccountPreferences":{"ok":true}}}))
            }
            "/settings/preferences" => page(true, false),
            _ => panic!("Unexpected preference request"),
        });
        assert!(
            fixture
                .saved_reddit(&export())
                .set_preference(Preference::MatureContent, true)
                .unwrap()
                .show_nsfw
        );
    }

    #[test]
    fn when_the_cookie_identity_changes_it_should_not_send_a_preference_write() {
        let fixture = Fixture::new(|request| {
            assert_eq!(request.url, "/api/me.json");
            Reply::json(json!({"data":{"id":"someoneelse","name":"reader"}}))
        });
        assert!(
            fixture
                .saved_reddit(&export())
                .set_preference(Preference::Autoplay, false)
                .unwrap_err()
                .to_string()
                .contains("account changed")
        );
    }

    #[test]
    fn when_a_preference_write_lacks_a_csrf_cookie_it_should_not_send_the_mutation() {
        let fixture = Fixture::new(|request| {
            assert_eq!(request.url, "/api/me.json");
            Reply::json(json!({"data":{"id":"readerid","name":"reader"}}))
        });
        assert!(
            fixture
                .saved_reddit(&saved_export("readerid", "reader"))
                .set_preference(Preference::Autoplay, false)
                .is_err()
        );
    }

    #[test]
    fn when_reddit_does_not_apply_a_preference_it_should_not_report_a_successful_save() {
        let fixture = Fixture::new(|request| match request.url.as_str() {
            "/api/me.json" => Reply::json(json!({"data":{"id":"readerid","name":"reader"}})),
            "/svc/shreddit/graphql" => {
                Reply::json(json!({"data":{"updateAccountPreferences":{"ok":true}}}))
            }
            "/settings/preferences" => page(true, true),
            _ => panic!("Unexpected preference request"),
        });
        assert!(
            fixture
                .saved_reddit(&export())
                .set_preference(Preference::Autoplay, false)
                .is_err()
        );
    }
}
