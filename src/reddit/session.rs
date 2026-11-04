//! Browser sessions for https://www.reddit.com; api.reddit.com and old.reddit.com get only
//! the `.reddit.com` cookies.

use std::io::Read;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use wreq::header::HeaderValue;

const INPUT_LIMIT: u64 = 1024 * 1024;
pub(super) const SIGN_IN: &str =
    "Reddit sign-in expired or missing; connect through Accounts or redtab --signin";
const CONNECTION: &str = "# redtab-connection ";

#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct Connection {
    pub account: super::Account,
    pub source: Option<crate::auth::Source>,
}

pub(super) struct Session {
    cookies: Vec<Cookie>,
    pub fingerprint: String,
    pub connection: Option<Connection>,
}

struct Cookie {
    domain: String,
    name: String,
    value: String,
    path: String,
    expires: u64,
}

impl Session {
    pub fn load() -> Result<Self> {
        let text = crate::auth::load_export()?;
        Self::stored(&text, now())
    }

    pub(super) fn stored(text: &str, now: u64) -> Result<Self> {
        Self::parse(text, now).map(|session| session.with_connection(text))
    }

    pub(super) fn archived(text: &str) -> Result<Self> {
        Self::parse_at(text, now(), true).map(|session| session.with_connection(text))
    }

    fn with_connection(mut self, text: &str) -> Self {
        self.connection = text.lines().find_map(|line| {
            let connection: Connection =
                serde_json::from_str(line.strip_prefix(CONNECTION)?).ok()?;
            connection.account.valid().then_some(connection)
        });
        self
    }

    pub(super) fn saved_id(&self) -> String {
        self.connection.as_ref().map_or_else(
            || format!("session:{}", self.fingerprint),
            |connection| connection.account.id.clone(),
        )
    }

    pub fn disconnected() -> Self {
        Self {
            cookies: Vec::new(),
            fingerprint: "disconnected".into(),
            connection: None,
        }
    }

    pub fn valid(&self) -> bool {
        self.cookies
            .iter()
            .any(|c| c.name == "reddit_session" && c.path == "/" && c.valid(now()))
    }

    pub(super) fn csrf_token(&self) -> Result<&str> {
        self.cookies
            .iter()
            .find(|cookie| {
                cookie.name == "csrf_token" && cookie.matches("/svc/shreddit/graphql", now())
            })
            .map(|cookie| cookie.value.as_str())
            .context("Reconnect through Accounts to refresh the browser's settings token")
    }

    pub fn export(&self, connection: Connection) -> Result<String> {
        let mut text = format!("{CONNECTION}{}\n", serde_json::to_string(&connection)?);
        for cookie in &self.cookies {
            let subdomains = cookie.domain == "reddit.com";
            text.push_str(&format!(
                "{}{}\t{}\t{}\tTRUE\t{}\t{}\t{}\n",
                if subdomains { "." } else { "" },
                cookie.domain,
                if subdomains { "TRUE" } else { "FALSE" },
                cookie.path,
                cookie.expires,
                cookie.name,
                cookie.value
            ));
        }
        Ok(text)
    }

    pub fn parse(text: &str, now: u64) -> Result<Self> {
        Self::parse_at(text, now, false)
    }

    fn parse_at(text: &str, now: u64, allow_expired: bool) -> Result<Self> {
        let mut cookies: Vec<Cookie> = Vec::new();
        for (line, raw) in text.trim_start_matches('\u{feff}').lines().enumerate() {
            let raw = raw.strip_prefix("#HttpOnly_").unwrap_or(raw);
            if raw.trim().is_empty() || raw.starts_with('#') {
                continue;
            }
            let cookie = parse_cookie(raw)
                .with_context(|| format!("invalid Netscape cookie at line {}", line + 1))?;
            if let Some(cookie) = cookie {
                if let Some(old) = cookies.iter_mut().find(|c| {
                    c.domain == cookie.domain && c.path == cookie.path && c.name == cookie.name
                }) {
                    *old = cookie;
                } else {
                    cookies.push(cookie);
                }
            }
        }
        // Browsers send the most specific paths first; preserve file order on ties.
        cookies.sort_by_key(|c| std::cmp::Reverse(c.path.len()));
        let login = cookies
            .iter()
            .find(|c| c.name == "reddit_session" && c.path == "/" && c.valid(now))
            .or_else(|| {
                if !allow_expired {
                    return None;
                }
                cookies
                    .iter()
                    .filter(|c| c.name == "reddit_session" && c.path == "/" && !c.value.is_empty())
                    .max_by_key(|c| c.expires)
            })
            .context(SIGN_IN)?;
        let fingerprint = format!("{:x}", Sha256::digest(login.value.as_bytes()));
        if cookies
            .iter()
            .any(|c| c.name == "reddit_session" && c.valid(now) && c.value != login.value)
        {
            bail!(
                "cookie export contains conflicting Reddit sessions; export cookies from one signed-in account"
            );
        }
        Ok(Self {
            cookies,
            fingerprint,
            connection: None,
        })
    }

    /// `www` is false for another reddit.com host, which gets only `.reddit.com` cookies.
    pub fn header(&self, path: &str, www: bool, now: u64) -> Result<HeaderValue> {
        let cookies: Vec<_> = self
            .cookies
            .iter()
            .filter(|c| (www || c.domain == "reddit.com") && c.matches(path, now))
            .collect();
        if !cookies
            .iter()
            .any(|c| c.name == "reddit_session" && c.path == "/" && c.valid(now))
        {
            bail!(SIGN_IN);
        }
        let value = cookies
            .iter()
            .map(|c| format!("{}={}", c.name, c.value))
            .collect::<Vec<_>>()
            .join("; ");
        let mut header = HeaderValue::from_str(&value)
            .map_err(|_| anyhow::anyhow!("invalid Reddit cookie header"))?;
        header.set_sensitive(true);
        Ok(header)
    }
}

impl Cookie {
    fn valid(&self, now: u64) -> bool {
        (self.expires == 0 || self.expires > now) && !self.value.is_empty()
    }

    fn matches(&self, path: &str, now: u64) -> bool {
        (self.expires == 0 || self.expires > now)
            && path.strip_prefix(&self.path).is_some_and(|rest| {
                rest.is_empty() || self.path.ends_with('/') || rest.starts_with('/')
            })
    }
}

// Seven tab-separated fields: https://curl.se/docs/http-cookies.html.
fn parse_cookie(raw: &str) -> Result<Option<Cookie>> {
    let fields: Vec<_> = raw.split('\t').collect();
    let [domain, subdomains, path, secure, expires, name, value] = fields.as_slice() else {
        bail!("expected seven tab-separated fields");
    };
    let domain = domain.strip_prefix('.').unwrap_or(domain);
    let matches = domain.eq_ignore_ascii_case("www.reddit.com")
        || (domain.eq_ignore_ascii_case("reddit.com") && *subdomains == "TRUE");
    if !matches {
        return Ok(None);
    }
    if !matches!(*subdomains, "TRUE" | "FALSE") || !matches!(*secure, "TRUE" | "FALSE") {
        bail!("invalid cookie flags");
    }
    let expires = expires
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid cookie expiry"))?;
    if !path.starts_with('/') || path.bytes().any(|c| c.is_ascii_control()) {
        bail!("invalid cookie path");
    }
    if name.is_empty()
        || !name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&c))
        || !value
            .bytes()
            .all(|c| matches!(c, 0x20..=0x3a | 0x3c..=0x7e))
    {
        bail!("invalid cookie name or value");
    }
    Ok(Some(Cookie {
        domain: domain.to_ascii_lowercase(),
        name: (*name).into(),
        value: (*value).into(),
        path: (*path).into(),
        expires,
    }))
}

pub(super) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(super) fn read_import(input: impl Read) -> Result<String> {
    let mut text = String::new();
    input
        .take(INPUT_LIMIT + 1)
        .read_to_string(&mut text)
        .map_err(|_| anyhow::anyhow!("cannot read sign-in input as UTF-8"))?;
    if text.len() as u64 > INPUT_LIMIT {
        bail!("sign-in input exceeds 1 MiB");
    }
    Session::parse(&text, now())?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOGIN: &str = ".reddit.com\tTRUE\t/\tTRUE\t0\treddit_session\tfixture-session\n";

    #[test]
    fn when_an_expired_saved_login_is_unnamed_it_should_keep_the_most_recent_login_key() {
        let text = ".reddit.com\tTRUE\t/\tTRUE\t2\treddit_session\trecent-token\nwww.reddit.com\tFALSE\t/\tTRUE\t1\treddit_session\tolder-token\n";

        let actual = Session::archived(text).unwrap();

        assert_eq!(
            actual.saved_id(),
            "session:162521a709853f9cb74410826b01c54eb840e412eafaeeaa289ee5ddc3fc0966"
        );
        assert!(!actual.valid());
    }

    #[test]
    fn when_saving_a_verified_session_untrusted_comments_and_foreign_cookies_should_be_removed() {
        let text = format!(
            "# redtab-connection forged\n{LOGIN}.example.com\tTRUE\t/\tTRUE\t0\tprivate\tunrelated\n"
        );
        let session = Session::parse(&text, 1000).unwrap();
        let actual = session
            .export(Connection {
                account: super::super::Account {
                    id: "fixture".into(),
                    name: "reader".into(),
                },
                source: None,
            })
            .unwrap();
        assert!(!actual.contains("forged"));
        assert!(!actual.contains("unrelated"));
        assert!(actual.contains("fixture-session"));
    }

    #[test]
    fn when_an_export_has_http_only_comments_and_crlf_it_should_load() {
        let session = Session::parse("\u{feff}# Netscape HTTP Cookie File\r\n\r\n#HttpOnly_.reddit.com\tTRUE\t/\tTRUE\t2000\treddit_session\tfixture-session\r\n", 1000).unwrap();
        let actual = session.header("/comments/abc.json", true, 1000).unwrap();
        assert_eq!(actual, "reddit_session=fixture-session");
        assert!(actual.is_sensitive());
        assert!(!format!("{actual:?}").contains("fixture-session"));
    }

    #[test]
    fn when_domains_do_not_cover_www_reddit_their_cookies_should_be_ignored() {
        let text = format!(
            "{LOGIN}www.reddit.com\tFALSE\t/\tFALSE\t0\tprefs\twww\n.reddit.com.evil.test\tTRUE\t/\tTRUE\t0\tevil\tsecret\nreddit.com\tFALSE\t/\tTRUE\t0\thost_only\tsecret\nold.reddit.com\tFALSE\t/\tTRUE\t0\told\tsecret\n"
        );
        let session = Session::parse(&text, 1000).unwrap();
        assert_eq!(
            session.header("/", true, 1000).unwrap(),
            "reddit_session=fixture-session; prefs=www"
        );
        assert_eq!(
            session.header("/", false, 1000).unwrap(),
            "reddit_session=fixture-session"
        );
    }

    #[test]
    fn when_paths_and_expiry_differ_only_matching_live_cookies_should_be_sent() {
        let text = format!(
            "{LOGIN}.reddit.com\tTRUE\t/r\tTRUE\t0\tprefs\tsubreddit\n.reddit.com\tTRUE\t/\tTRUE\t1000\told\texpired\n.reddit.com\tTRUE\t/\tTRUE\t0\tprefs\troot\n"
        );
        let session = Session::parse(&text, 1000).unwrap();
        assert_eq!(
            session.header("/r/rust/hot.json", true, 1000).unwrap(),
            "prefs=subreddit; reddit_session=fixture-session; prefs=root"
        );
        assert_eq!(
            session.header("/random", true, 1000).unwrap(),
            "reddit_session=fixture-session; prefs=root"
        );
    }

    #[test]
    fn when_a_cookie_is_repeated_the_last_exported_value_should_win() {
        let text = format!("{LOGIN}.reddit.com\tTRUE\t/\tTRUE\t0\treddit_session\treplacement\n");
        let actual = Session::parse(&text, 1000)
            .unwrap()
            .header("/", true, 1000)
            .unwrap();
        assert_eq!(actual, "reddit_session=replacement");
    }

    #[test]
    fn when_a_browser_cookie_contains_json_its_quotes_and_commas_should_be_preserved() {
        let text = format!("{LOGIN}.reddit.com\tTRUE\t/\tTRUE\t0\tprefs\t{{\"a\":1,\"b\":2}}\n");
        let actual = Session::parse(&text, 1000)
            .unwrap()
            .header("/", true, 1000)
            .unwrap();
        assert_eq!(
            actual,
            "reddit_session=fixture-session; prefs={\"a\":1,\"b\":2}"
        );
    }

    #[test]
    fn when_a_login_is_missing_expired_or_invalid_the_export_should_fail_without_values() {
        for text in [
            "",
            "# only comments\n",
            ".reddit.com\tTRUE\t/\tTRUE\t1000\treddit_session\texpired-secret\n",
            ".reddit.com\tTRUE\t/\tTRUE\t0\treddit_session\t\n",
            ".reddit.com\tTRUE\t/login\tTRUE\t0\treddit_session\tnarrow-secret\n",
            ".reddit.com\tTRUE\t/\tTRUE\t0\treddit_session\tinjected;secret\n",
            ".reddit.com\tTRUE\t/\tTRUE\tbad-expiry\treddit_session\tbad-secret\n",
            ".reddit.com\tTRUE\t/\tinvalid\t0\treddit_session\tbad-secret\n",
            ".reddit.com\tTRUE\t/\tTRUE\t0\treddit_session\tbad-secret\textra\n",
        ] {
            let actual = format!(
                "{:#}",
                Session::parse(text, 1000).err().expect("invalid export")
            );
            assert!(!actual.contains("secret"), "{actual}");
        }
    }

    #[test]
    fn when_a_session_expires_after_loading_it_should_stop_sending_requests() {
        let session = Session::parse(
            ".reddit.com\tTRUE\t/\tTRUE\t1001\treddit_session\tfixture-session\n",
            1000,
        )
        .unwrap();
        assert!(session.header("/", true, 1000).is_ok());
        assert_eq!(
            session.header("/", true, 1001).unwrap_err().to_string(),
            SIGN_IN
        );
    }

    #[test]
    fn when_a_session_is_removed_and_readded_only_the_current_export_should_load() {
        let text = format!("{LOGIN}.reddit.com\tTRUE\t/\tTRUE\t1\treddit_session\tdeleted\n");
        assert!(Session::parse(&text, 1000).is_err());
        assert!(Session::parse(&format!("{text}{LOGIN}"), 1000).is_ok());
    }

    #[test]
    fn when_valid_sessions_disagree_the_export_should_be_rejected() {
        let text =
            format!("{LOGIN}www.reddit.com\tFALSE\t/\tTRUE\t0\treddit_session\tother-session\n");
        let actual = Session::parse(&text, 1000).err().unwrap().to_string();
        assert!(actual.contains("conflicting Reddit sessions"));
        assert!(!actual.contains("other-session"));
    }
}
