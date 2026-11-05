use super::*;

pub(super) const USAGE: &str = "\
redtab — terminal Reddit reader with tabs and browser-session sign-in

Usage: redtab [--config PATH] [--subreddit NAME]

Options:
  -c, --config PATH      config file (default: $XDG_CONFIG_HOME/redtab/config.toml)
  -s, --subreddit NAME   open this subreddit on start
      --signin           connect a browser session (or import Netscape cookies from stdin)
      --browser NAME     choose a browser with --signin (for example chrome or firefox)
      --profile NAME     choose its profile with --signin --browser
      --check            print config, Home status, account subscription count and state file;
                         exit 1 if Home or subscriptions cannot load
  -h, --help             show this help
  -V, --version          show the version

Sign-in is encrypted in macOS Keychain or Linux Secret Service.
Press ? inside redtab for key bindings.";

pub(super) struct Args {
    pub(super) config: Option<PathBuf>,
    pub(super) subreddit: Option<String>,
    pub(super) check: bool,
    pub(super) signin: bool,
    pub(super) browser: Option<String>,
    pub(super) profile: Option<String>,
}

pub(super) fn parse_args() -> Result<Option<Args>> {
    parse_args_from(std::env::args_os().skip(1))
}

pub(super) fn parse_args_from(
    mut it: impl Iterator<Item = std::ffi::OsString>,
) -> Result<Option<Args>> {
    let mut args = Args {
        config: None,
        subreddit: None,
        check: false,
        signin: false,
        browser: None,
        profile: None,
    };
    // OS strings: a non-UTF-8 argument errors instead of panicking.
    while let Some(arg) = it.next() {
        match arg.to_str() {
            Some(PROBE_ARG) => {
                probe_image_protocol();
                return Ok(None);
            }
            Some("-h" | "--help") => {
                println!("{USAGE}");
                return Ok(None);
            }
            Some("-V" | "--version") => {
                println!("redtab {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            Some("-c" | "--config") => {
                args.config = Some(it.next().context("--config needs a path")?.into())
            }
            Some("-s" | "--subreddit") => {
                let name = it.next().context("--subreddit needs a name")?;
                args.subreddit = Some(
                    name.into_string()
                        .map_err(|n| anyhow::anyhow!("{n:?} is not a subreddit name"))?,
                )
            }
            Some("--check") => args.check = true,
            Some("--signin") => args.signin = true,
            Some("--browser") => {
                args.browser = Some(
                    it.next()
                        .context("--browser needs a name")?
                        .into_string()
                        .map_err(|_| anyhow::anyhow!("invalid browser name"))?,
                )
            }
            Some("--profile") => {
                args.profile = Some(
                    it.next()
                        .context("--profile needs a name")?
                        .into_string()
                        .map_err(|_| anyhow::anyhow!("invalid profile name"))?,
                )
            }
            _ => bail!("unknown argument {arg:?}\n\n{USAGE}"),
        }
    }
    if (args.browser.is_some() || args.profile.is_some()) && !args.signin {
        bail!("--browser and --profile require --signin");
    }
    if args.profile.is_some() && args.browser.is_none() {
        bail!("--profile requires --browser");
    }
    if args.signin && (args.check || args.config.is_some() || args.subreddit.is_some()) {
        bail!("--signin cannot be combined with --check, --config or --subreddit");
    }
    Ok(Some(args))
}

#[cfg(test)]
mod tests;
