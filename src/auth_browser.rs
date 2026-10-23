//! Browser access runs only in the private, cancellable sign-in helper.

use std::io::{IsTerminal, Read};
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::auth::Source;

pub const HELPER: &str = "--browser-auth-helper";
pub const LIMIT: u64 = 1024 * 1024;
pub const PACKET_LIMIT: u64 = 8 * LIMIT;

#[derive(Serialize, Deserialize)]
pub enum Request {
    Discover,
    Import(Source),
    Load,
    Accounts,
    LoadAccount(String),
    Store(String),
    Activate {
        id: String,
        expected: String,
        text: String,
    },
    Delete(String),
}

#[derive(Serialize, Deserialize)]
pub enum Reply {
    Sources(Vec<Source>),
    Cookies(String),
    Accounts(Vec<crate::reddit::SavedAccount>),
    Error(String),
    Stored,
    Deleted,
}

pub fn helper() -> Result<()> {
    if std::io::stdin().is_terminal() || std::io::stdout().is_terminal() {
        bail!("The browser helper requires private input/output pipes");
    }
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(PACKET_LIMIT + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > PACKET_LIMIT {
        bail!("Invalid browser helper request");
    }
    let request: Request =
        serde_json::from_slice(&bytes).context("Invalid browser helper request")?;
    let reply = match request {
        Request::Discover => discover().map(Reply::Sources),
        Request::Import(Source::Browser {
            browser, profile, ..
        }) => import(&browser, &profile).map(Reply::Cookies),
        Request::Import(Source::Window { executable }) => login(&executable).map(Reply::Cookies),
        Request::Load => crate::reddit::read_store().map(Reply::Cookies),
        Request::Accounts => crate::reddit::saved_accounts().map(Reply::Accounts),
        Request::LoadAccount(id) => crate::reddit::read_account(&id).map(Reply::Cookies),
        Request::Store(text) => crate::reddit::write_store(&text).map(|()| Reply::Stored),
        Request::Activate { id, expected, text } => {
            crate::reddit::activate_account(&id, &expected, &text).map(|()| Reply::Stored)
        }
        Request::Delete(id) => crate::reddit::remove_store(&id).map(|()| Reply::Deleted),
    }
    .unwrap_or_else(|error| Reply::Error(error.to_string()));
    // The parent captures this pipe; helper errors/panics never inherit the terminal.
    serde_json::to_writer(std::io::stdout().lock(), &reply)?;
    Ok(())
}

fn discover() -> Result<Vec<Source>> {
    let mut sources = Vec::new();
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use rookie_cookies::{ExecutionControl, profiles_with, supported_browsers};
        for browser in supported_browsers()
            .map_err(|_| anyhow::anyhow!("Could not list supported browsers"))?
        {
            let id = browser.id.to_string();
            if !matches!(
                id.as_str(),
                "chrome" | "chromium" | "firefox" | "brave" | "edge" | "arc" | "zen" | "librewolf"
            ) {
                continue;
            }
            let control = ExecutionControl::default().timeout(Duration::from_secs(3));
            if let Ok(profiles) = profiles_with(&id, control) {
                for profile in profiles {
                    sources.push(Source::Browser {
                        browser: id.clone(),
                        profile: profile.profile.profile_id.to_string(),
                        label: clean_label(&format!(
                            "{} · {}",
                            browser.display_name, profile.profile.display_name
                        )),
                    });
                }
            }
        }
    }
    #[cfg(target_os = "macos")]
    if let Some(root) = dia_profile_root() {
        sources.extend(dia_profiles(&root));
    }
    if let Some(executable) = browser_executable() {
        sources.push(Source::Window { executable });
    }
    Ok(sources)
}

#[cfg(target_os = "macos")]
fn dia_profile_root() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| home.is_absolute())
        .map(|home| home.join("Library/Application Support/Dia/User Data"))
}

#[cfg(any(target_os = "macos", test))]
fn dia_profiles(root: &Path) -> Vec<Source> {
    let names: Value = std::fs::File::open(root.join("Local State"))
        .ok()
        .and_then(|file| serde_json::from_reader(file.take(LIMIT)).ok())
        .unwrap_or(Value::Null);
    let mut profiles: Vec<_> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let profile = entry.ok()?.file_name().into_string().ok()?;
            dia_cookie_path(root, &profile)?;
            let name = names["profile"]["info_cache"][profile.as_str()]["name"]
                .as_str()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or(&profile);
            let label = clean_label(&format!("Dia · {name}"));
            Some((profile, label))
        })
        .collect();
    profiles.sort();
    profiles
        .into_iter()
        .map(|(profile, label)| Source::Browser {
            browser: "dia".into(),
            profile,
            label,
        })
        .collect()
}

#[cfg(any(target_os = "macos", test))]
fn dia_cookie_path(root: &Path, profile: &str) -> Option<PathBuf> {
    if !(profile == "Default" || profile.starts_with("Profile "))
        || Path::new(profile).components().count() != 1
    {
        return None;
    }
    let root = root.canonicalize().ok()?;
    let directory = root.join(profile).canonicalize().ok()?;
    if !directory.starts_with(&root) || !directory.is_dir() {
        return None;
    }
    ["Network/Cookies", "Cookies"].into_iter().find_map(|name| {
        let path = directory.join(name).canonicalize().ok()?;
        (path.starts_with(&directory) && path.is_file()).then_some(path)
    })
}

#[cfg(target_os = "macos")]
#[allow(deprecated)]
fn dia_cookies(path: PathBuf) -> Result<Vec<rookie_cookies::enums::Cookie>> {
    // The registry lacks Dia; this public API accepts its Keychain identity.
    let config = rookie_cookies::config::Browser {
        paths: Vec::new(),
        channels: None,
        unix_crypt_name: None,
        osx_key_service: Some("Dia Safe Storage".into()),
        osx_key_user: Some("Dia".into()),
    };
    rookie_cookies::chromium_based(&config, path, Some(vec!["reddit.com".into()]), false)
}

pub fn clean_label(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).take(100).collect()
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn import(browser: &str, profile: &str) -> Result<String> {
    use rookie_cookies::{AppBoundPolicy, ExtractRequest, extract};
    let cookies: Result<Vec<_>> = match browser {
        #[cfg(target_os = "macos")]
        "dia" => {
            let path = dia_profile_root()
                .and_then(|root| dia_cookie_path(&root, profile))
                .context("Dia profile is no longer available; rescan browser profiles")?;
            dia_cookies(path)
        }
        _ => {
            let request = ExtractRequest::browser(browser)
                .profile(profile)
                .domains(Some(vec!["reddit.com".into()]))
                .include_session()
                .app_bound(AppBoundPolicy::Disabled)
                .timeout(Duration::from_secs(30));
            extract(request).map_err(Into::into)
        }
    };
    let cookies = cookies.map_err(|_| anyhow::anyhow!(
        "Could not read this browser session. Unlock the desktop keyring or allow Keychain access, then retry. You can also use the separate login window."))?;
    cookie_export(cookies.into_iter().map(|c| BrowserCookie {
        domain: c.domain,
        path: c.path,
        name: c.name,
        value: c.value,
        secure: c.secure,
        expires: c.expires.unwrap_or(0),
    }))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn import(_: &str, _: &str) -> Result<String> {
    bail!("Browser import supports macOS and Linux")
}

struct BrowserCookie {
    domain: String,
    path: String,
    name: String,
    value: String,
    secure: bool,
    expires: u64,
}

fn cookie_export(cookies: impl IntoIterator<Item = BrowserCookie>) -> Result<String> {
    let mut text = String::new();
    let mut login: Option<String> = None;
    for cookie in cookies {
        let domain = cookie
            .domain
            .strip_prefix('.')
            .unwrap_or(&cookie.domain)
            .to_ascii_lowercase();
        if !matches!(domain.as_str(), "reddit.com" | "www.reddit.com") {
            continue;
        }
        if cookie.name == "reddit_session"
            && cookie.path == "/"
            && !cookie.value.is_empty()
            && (cookie.expires == 0
                || cookie.expires
                    > std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs())
            && (cookie.domain.starts_with('.') || domain == "www.reddit.com")
        {
            if login.as_ref().is_some_and(|old| old != &cookie.value) {
                bail!(
                    "The profile contains multiple Reddit accounts; use one signed-in profile or the separate login window"
                );
            }
            login = Some(cookie.value.clone());
        }
        if [&cookie.domain, &cookie.path, &cookie.name, &cookie.value]
            .iter()
            .any(|s| s.bytes().any(|b| matches!(b, b'\t' | b'\r' | b'\n')))
        {
            bail!("The browser returned malformed Reddit cookies");
        }
        text.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            cookie.domain,
            if cookie.domain.starts_with('.') {
                "TRUE"
            } else {
                "FALSE"
            },
            cookie.path,
            if cookie.secure { "TRUE" } else { "FALSE" },
            cookie.expires,
            cookie.name,
            cookie.value
        ));
        if text.len() as u64 > LIMIT {
            bail!("Reddit session exceeds the import limit");
        }
    }
    if text.is_empty() {
        bail!("No Reddit session found. Sign in at reddit.com in this profile, then retry.");
    }
    Ok(text)
}

fn browser_executable() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    for name in [
        "Google Chrome.app/Contents/MacOS/Google Chrome",
        "Chromium.app/Contents/MacOS/Chromium",
        "Brave Browser.app/Contents/MacOS/Brave Browser",
        "Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    ] {
        let path = Path::new("/Applications").join(name);
        if path.is_file() {
            return Some(path);
        }
    }
    for directory in std::env::split_paths(&std::env::var_os("PATH")?) {
        for name in [
            "google-chrome",
            "chromium",
            "chromium-browser",
            "brave-browser",
            "microsoft-edge",
        ] {
            let path = directory.join(name);
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

/// The parent owns this helper's private working directory and process group.
fn login(executable: &Path) -> Result<String> {
    let profile = std::env::temp_dir().join("login-profile");
    let mut browser = Command::new(executable)
        .arg(format!("--user-data-dir={}", profile.display()))
        .args([
            "--remote-debugging-port=0",
            "--remote-debugging-address=127.0.0.1",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-background-mode",
            "https://www.reddit.com/login/",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("Could not open the separate browser login window")?;
    let result = login_cookies(&mut browser, &profile);
    let _ = browser.kill();
    let _ = browser.wait();
    result
}

fn login_cookies(browser: &mut std::process::Child, profile: &Path) -> Result<String> {
    let started = Instant::now();
    let (port, endpoint) = loop {
        if browser.try_wait()?.is_some() {
            bail!("The login window was closed; choose a profile or try again");
        }
        if started.elapsed() > Duration::from_secs(30) {
            bail!("The login window did not start in time");
        }
        if let Ok(text) = std::fs::read_to_string(profile.join("DevToolsActivePort"))
            && let Some(address) = debug_address(&text)
        {
            break address;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let stream = TcpStream::connect_timeout(
        &SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).into(),
        Duration::from_secs(2),
    )?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let config = tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(PACKET_LIMIT as usize))
        .max_frame_size(Some(PACKET_LIMIT as usize));
    let (mut socket, _) = tungstenite::client::client_with_config(
        format!("ws://127.0.0.1:{port}{endpoint}"),
        stream,
        Some(config),
    )
    .map_err(|_| anyhow::anyhow!("Could not connect to the login window"))?;
    for id in 1..600_u64 {
        if browser.try_wait()?.is_some() {
            bail!("The login window was closed");
        }
        socket
            .send(tungstenite::Message::Text(
                json!({"id":id,"method":"Storage.getCookies"})
                    .to_string()
                    .into(),
            ))
            .map_err(|_| anyhow::anyhow!("The login window disconnected"))?;
        let cookies = read_cookies(&mut socket, id)?;
        if let Ok(export) = cookie_export(cookies) {
            if crate::reddit::Reddit::from_export(&export, false)
                .and_then(|candidate| candidate.verify_account())
                .is_ok()
            {
                return Ok(export);
            }
            std::thread::sleep(Duration::from_secs(3));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    bail!("Sign-in timed out; open the login window again when ready")
}

fn debug_address(text: &str) -> Option<(u16, String)> {
    let mut lines = text.lines();
    let port = lines.next()?.parse::<u16>().ok().filter(|p| *p != 0)?;
    let endpoint = lines.next()?;
    let token = endpoint.strip_prefix("/devtools/browser/")?;
    if token.is_empty()
        || !token
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
    {
        return None;
    }
    Some((port, endpoint.into()))
}

fn read_cookies(
    socket: &mut tungstenite::WebSocket<TcpStream>,
    id: u64,
) -> Result<Vec<BrowserCookie>> {
    for _ in 0..32 {
        let message = socket
            .read()
            .map_err(|_| anyhow::anyhow!("The login window stopped responding"))?;
        let bytes = match message {
            tungstenite::Message::Text(text) => text.as_bytes().to_vec(),
            tungstenite::Message::Binary(bytes) => bytes.to_vec(),
            tungstenite::Message::Close(_) => bail!("The login window disconnected"),
            _ => continue,
        };
        if bytes.len() as u64 > PACKET_LIMIT {
            bail!("The login window returned too much session data");
        }
        let value: Value =
            serde_json::from_slice(&bytes).context("Invalid login window response")?;
        if value["id"].as_u64() != Some(id) {
            continue;
        }
        let cookies = value["result"]["cookies"]
            .as_array()
            .context("Could not read the login window session")?;
        return cookies
            .iter()
            .map(|c| {
                Ok(BrowserCookie {
                    domain: c["domain"]
                        .as_str()
                        .context("Invalid cookie domain")?
                        .into(),
                    path: c["path"].as_str().context("Invalid cookie path")?.into(),
                    name: c["name"].as_str().context("Invalid cookie name")?.into(),
                    value: c["value"].as_str().context("Invalid cookie value")?.into(),
                    secure: c["secure"].as_bool().unwrap_or(true),
                    expires: c["expires"].as_f64().unwrap_or(0.0).max(0.0) as u64,
                })
            })
            .collect();
    }
    bail!("The login window did not answer the session request")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dia_cookie_fixture(root: &Path, relative: &str) -> PathBuf {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"").unwrap();
        path
    }

    #[test]
    fn when_dia_profiles_exist_discovery_should_keep_their_ids_and_clean_display_names() {
        let directory = tempfile::tempdir().unwrap();
        dia_cookie_fixture(directory.path(), "Default/Cookies");
        dia_cookie_fixture(directory.path(), "Profile 二/Network/Cookies");
        dia_cookie_fixture(directory.path(), "System Profile/Cookies");
        std::fs::write(
            directory.path().join("Local State"),
            r#"{"profile":{"info_cache":{"Default":{"name":"Personal"},"Profile 二":{"name":"仕事\n"}}}}"#,
        )
        .unwrap();

        let actual = dia_profiles(directory.path());

        assert_eq!(
            actual,
            vec![
                Source::Browser {
                    browser: "dia".into(),
                    profile: "Default".into(),
                    label: "Dia · Personal".into(),
                },
                Source::Browser {
                    browser: "dia".into(),
                    profile: "Profile 二".into(),
                    label: "Dia · 仕事".into(),
                },
            ]
        );
    }

    #[test]
    fn when_dia_metadata_is_invalid_discovery_should_use_the_profile_directory_name() {
        let directory = tempfile::tempdir().unwrap();
        dia_cookie_fixture(directory.path(), "Default/Cookies");
        std::fs::write(directory.path().join("Local State"), "invalid JSON").unwrap();

        let actual = dia_profiles(directory.path());

        assert_eq!(actual[0].label(), "Dia · Default");
    }

    #[test]
    fn when_dia_metadata_is_missing_discovery_should_use_the_profile_directory_name() {
        let directory = tempfile::tempdir().unwrap();
        dia_cookie_fixture(directory.path(), "Default/Cookies");

        let actual = dia_profiles(directory.path());

        assert_eq!(actual[0].label(), "Dia · Default");
    }

    #[test]
    fn when_dia_metadata_has_a_blank_name_discovery_should_use_the_profile_directory_name() {
        let directory = tempfile::tempdir().unwrap();
        dia_cookie_fixture(directory.path(), "Default/Cookies");
        std::fs::write(
            directory.path().join("Local State"),
            r#"{"profile":{"info_cache":{"Default":{"name":"  "}}}}"#,
        )
        .unwrap();

        let actual = dia_profiles(directory.path());

        assert_eq!(actual[0].label(), "Dia · Default");
    }

    #[test]
    fn when_dia_is_not_installed_discovery_should_find_no_dia_profiles() {
        let directory = tempfile::tempdir().unwrap();

        let actual = dia_profiles(&directory.path().join("missing"));

        assert!(actual.is_empty());
    }

    #[test]
    fn when_dia_has_both_cookie_locations_import_should_prefer_network_cookies() {
        let directory = tempfile::tempdir().unwrap();
        dia_cookie_fixture(directory.path(), "Default/Cookies");
        let expected = dia_cookie_fixture(directory.path(), "Default/Network/Cookies")
            .canonicalize()
            .unwrap();

        let actual = dia_cookie_path(directory.path(), "Default");

        assert_eq!(actual, Some(expected));
    }

    #[test]
    fn when_the_selected_dia_cookie_file_disappears_it_should_not_resolve() {
        let directory = tempfile::tempdir().unwrap();
        let path = dia_cookie_fixture(directory.path(), "Default/Cookies");
        std::fs::remove_file(path).unwrap();

        let actual = dia_cookie_path(directory.path(), "Default");

        assert_eq!(actual, None);
    }

    #[test]
    fn when_the_dia_profile_selector_contains_traversal_it_should_not_resolve() {
        let directory = tempfile::tempdir().unwrap();
        dia_cookie_fixture(directory.path(), "Default/Cookies");

        let actual = dia_cookie_path(directory.path(), "Profile 2/../Default");

        assert_eq!(actual, None);
    }

    #[test]
    fn when_the_dia_profile_selector_is_empty_it_should_not_resolve() {
        let directory = tempfile::tempdir().unwrap();
        dia_cookie_fixture(directory.path(), "Default/Cookies");

        let actual = dia_cookie_path(directory.path(), "");

        assert_eq!(actual, None);
    }

    #[cfg(unix)]
    #[test]
    fn when_a_dia_profile_symlink_escapes_the_browser_root_it_should_not_be_discovered() {
        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        dia_cookie_fixture(outside.path(), "Cookies");
        std::os::unix::fs::symlink(outside.path(), directory.path().join("Default")).unwrap();

        let actual = dia_profiles(directory.path());

        assert!(actual.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn when_dia_cookies_are_symlinked_outside_the_profile_they_should_not_resolve() {
        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let path = dia_cookie_fixture(outside.path(), "Cookies");
        std::fs::create_dir(directory.path().join("Default")).unwrap();
        std::os::unix::fs::symlink(path, directory.path().join("Default/Cookies")).unwrap();

        let actual = dia_cookie_path(directory.path(), "Default");

        assert_eq!(actual, None);
    }

    fn cookie(domain: &str, name: &str, value: &str) -> BrowserCookie {
        BrowserCookie {
            domain: domain.into(),
            path: "/".into(),
            name: name.into(),
            value: value.into(),
            secure: true,
            expires: 0,
        }
    }

    #[test]
    fn when_containers_supply_different_sessions_the_import_should_not_choose_the_last_account() {
        let actual = cookie_export([
            cookie(".reddit.com", "reddit_session", "fixture-session"),
            cookie(".reddit.com", "reddit_session", "other-session"),
        ])
        .unwrap_err()
        .to_string();
        assert!(actual.contains("multiple Reddit accounts"));
        assert!(!actual.contains("other-session"));
    }

    #[test]
    fn when_other_sites_share_a_profile_only_reddit_cookies_should_be_imported() {
        let actual = cookie_export([
            cookie(".reddit.com", "reddit_session", "fixture-session"),
            cookie(".example.com", "private", "unrelated"),
        ])
        .unwrap();
        assert_eq!(
            actual,
            ".reddit.com\tTRUE\t/\tTRUE\t0\treddit_session\tfixture-session\n"
        );
    }

    #[test]
    fn when_a_cookie_contains_a_row_separator_the_import_should_fail_without_its_value() {
        let actual = cookie_export([cookie(".reddit.com", "reddit_session", "private\nvalue")])
            .unwrap_err()
            .to_string();
        assert!(!actual.contains("private"));
    }

    #[test]
    fn when_debug_address_leaves_the_browser_endpoint_it_should_be_rejected() {
        assert_eq!(debug_address("9222\n//evil.test\n"), None);
        assert_eq!(
            debug_address("9222\n/devtools/browser/fixture-id\n"),
            Some((9222, "/devtools/browser/fixture-id".into()))
        );
    }

    #[test]
    fn when_the_login_socket_sends_a_ping_it_should_still_read_the_cookie_response() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (done, finished) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut socket = tungstenite::accept(stream).unwrap();
            socket
                .send(tungstenite::Message::Ping(Vec::new().into()))
                .unwrap();
            socket.send(tungstenite::Message::Text(json!({"id":1,"result":{"cookies":[{"domain":".reddit.com","path":"/","secure":true,"expires":-1,"name":"reddit_session","value":"fixture-session"}]}}).to_string().into())).unwrap();
            let _ = finished.recv_timeout(Duration::from_secs(3));
        });
        let stream = TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let (mut socket, _) = tungstenite::client(format!("ws://{address}/"), stream).unwrap();
        let actual = cookie_export(read_cookies(&mut socket, 1).unwrap()).unwrap();
        done.send(()).unwrap();
        server.join().unwrap();
        assert_eq!(
            actual,
            ".reddit.com\tTRUE\t/\tTRUE\t0\treddit_session\tfixture-session\n"
        );
    }
}
