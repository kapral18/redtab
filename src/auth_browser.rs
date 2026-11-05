//! Browser access runs only in the private, cancellable sign-in helper.

use std::io::{BufRead, BufReader, IsTerminal, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::{net::UnixStream, process::CommandExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::auth::Source;

pub const HELPER: &str = "--browser-auth-helper";
pub const LIMIT: u64 = 1024 * 1024;
pub const PACKET_LIMIT: u64 = 8 * LIMIT;
pub const WINDOW_TIMED_OUT: &str =
    "The login window timed out before sign-in finished; Enter opens it again.";

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
    let (mut browser, mut connection) = spawn_login(executable, &profile)?;
    let result = login_cookies(&mut browser, &mut connection);
    let _ = browser.kill();
    let _ = browser.wait();
    result
}

fn spawn_login(executable: &Path, profile: &Path) -> Result<(std::process::Child, UnixStream)> {
    let (connection, child_connection) = UnixStream::pair()?;
    connection.set_read_timeout(Some(Duration::from_secs(30)))?;
    connection.set_write_timeout(Some(Duration::from_secs(2)))?;
    let raw = unsafe { libc::fcntl(child_connection.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 5) };
    if raw < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let descriptor = unsafe { OwnedFd::from_raw_fd(raw) };
    let mut command = Command::new(executable);
    command
        .arg(format!("--user-data-dir={}", profile.display()))
        .args([
            "--remote-debugging-pipe",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-background-mode",
            "https://www.reddit.com/login/",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        command.pre_exec(move || {
            for target in [3, 4] {
                if libc::dup2(raw, target) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
    let browser = command
        .spawn()
        .context("Could not open the separate browser login window")?;
    drop(descriptor);
    drop(child_connection);
    Ok((browser, connection))
}

fn login_cookies(browser: &mut std::process::Child, connection: &mut UnixStream) -> Result<String> {
    let mut reader = BufReader::new(connection.try_clone()?);
    for id in 1..600_u64 {
        if browser.try_wait()?.is_some() {
            bail!("The login window was closed");
        }
        connection
            .write_all(format!("{}\0", json!({"id":id,"method":"Storage.getCookies"})).as_bytes())
            .map_err(|_| anyhow::anyhow!("The login window disconnected"))?;
        let cookies = read_cookies(&mut reader, id)?;
        connection.set_read_timeout(Some(Duration::from_secs(2)))?;
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
    bail!("{WINDOW_TIMED_OUT}")
}

fn read_cookies(reader: &mut impl BufRead, id: u64) -> Result<Vec<BrowserCookie>> {
    for _ in 0..32 {
        let mut bytes = Vec::new();
        reader
            .take(PACKET_LIMIT + 1)
            .read_until(0, &mut bytes)
            .map_err(|_| anyhow::anyhow!("The login window stopped responding"))?;
        if bytes.len() as u64 > PACKET_LIMIT || bytes.last() != Some(&0) {
            bail!("The login window returned too much session data");
        }
        bytes.pop();
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("Invalid login window response"))?;
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
mod tests;
