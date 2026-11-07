# Accounts

[User guide](guide.md)

## Connect Reddit

1. Sign in to Reddit in your browser.
2. Start redtab and pick that browser profile in **Connect Reddit**.
3. Allow OS credential access if asked.
4. Check the username and press `Enter` to save. `Esc` cancels.

redtab copies the session once; the browser can stay closed afterward.
It does not renew or reimport the session in the background.

Supported on macOS and Linux: Chrome, Chromium, Firefox, Brave, Edge, Zen and
LibreWolf. Arc and Dia are supported only on macOS.

- macOS: allow Keychain access.
- Linux: saved logins require `secret-tool` and an unlocked Secret Service provider.
  Chromium import may separately need Secret Service or KWallet to decrypt cookies.

### If import fails

Choose **Open a separate Reddit login window** (requires Chrome, Chromium, Brave
or Edge). Sign in there, then confirm the account in redtab. The temporary profile
is deleted afterward.

Import can also fail when:

- the browser keeps cookies only in memory, or
- the profile holds more than one Reddit session.

## Manage accounts

Press `SPC p a`.

| Key | Action |
| --- | --- |
| `Enter` | Switch to the selected account |
| `a` | Add an account |
| `r` | Reconnect an expired session |
| `d`, then `Enter` | Remove the saved login |

- Switching restores that account's bookmarks and history.
- A failed switch, save or removal keeps the current connection.
- Removing a login keeps bookmarks, history, cache and the browser session.
  Removing the active account disconnects redtab.
- Startup restores the last account.
- Up to 32 accounts can be saved. On Linux, the encoded data for all saved logins
  must also fit `secret-tool`'s 8 KiB input limit.

## Sign in from the command line

```sh
redtab --signin
redtab --signin --browser firefox --profile Work
redtab --signin --browser dia --profile Default  # macOS
```

`--profile` needs `--browser`. `--signin` cannot be combined with `--check`,
`--config` or `--subreddit`.

To import a Netscape cookie export, pipe it in. Include `reddit_session` and enable
HttpOnly cookies in the export:

```sh
redtab --signin < /path/to/your/reddit-cookies.txt
```

Keep cookie values out of shell arguments and logs.

## Connection recovery

- If Reddit requires sign-in, connect with `SPC p a`.
- For HTTP 403, check sign-in and content access in your browser. Reconnect with
  `SPC p a` if needed; reloading a feed does not replace the saved session.
- For HTTP 429 (rate limit), wait before retrying; follow the countdown when shown.
- If Reddit is not answering, cached content may remain readable; `r` retries
  the failed view.
- Credential-store errors: check [OS access](#connect-reddit). redtab never falls
  back to plaintext cookie storage.

`redtab --check` runs live Home and subscription diagnostics; it does not renew
cookies or reconnect accounts.

## Data and privacy

| Data | Location |
| --- | --- |
| Bookmarks and reading history | `~/.local/state/redtab/accounts/<account-id>/state.json` |
| Cached posts and previews | `~/.cache/redtab` |
| Images opened externally | Private `redtab-media-<pid>-…` folders in the OS temporary directory (`$TMPDIR` when set) |
| Browser-import copies and login profiles | Private `redtab-auth-*` folders in the OS temporary directory |
| Saved logins | macOS Keychain or Linux Secret Service |

Absolute `$XDG_STATE_HOME` and `$XDG_CACHE_HOME` override these defaults.
Clearing the cache keeps bookmarks and history. Under the state root, signed-out
sessions use `redtab/signed-out/state.json`; unidentified legacy connections use
`redtab/state.json`.

Read marks and feed observations expire after 90 days; bookmarks do not. State
saves run every minute and on exit, including after a crash; bookmark changes also save immediately.
Saves merge other windows' saved changes.
Invalid JSON moves to `state.json.bad`, with a unique backup name if needed.
Unreadable files stay intact; state saves stop for that session.

The disk cache has a 256 MiB / 4,096-entry budget. Listings and threads stay fresh
for five minutes; images for seven days. Stale content may appear during refresh.
Entries become unreadable 30 days after fetching. Later writes prune old files
and enforce the budget; the cache is not a permanent offline archive.

External-image downloads are outside the cache budget. Before each download,
redtab removes files at least 24 hours old from its process's temporary folder.
Startup removes folders from exited redtab processes once unchanged for 24 hours.
Nothing is removed at exit or on a timer.

Bookmarks, history, reader filters, highlights and cached entries are account-scoped;
accounts share the cache directory, not its entries. Device settings apply to all accounts.
Reddit preferences stay on Reddit.

Cookies:

- are sent only to `www.reddit.com`, `api.reddit.com` and `old.reddit.com`;
- are excluded from redtab's logs and content cache, and never passed to media players.

Browser import can make temporary copies of cookie databases. Cleanup runs when
the sign-in helper finishes or is cancelled. The content cache can contain private
posts your account can see.

redtab ignores `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY` and `NO_PROXY`.
`O` on a Reddit video gives `commands.video` a `http://127.0.0.1:<port>/…` URL.
redtab fetches the video from Reddit and serves it at that URL, so the player never
receives your cookies. Quitting redtab closes that URL and ends the player's stream.
