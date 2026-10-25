# Accounts

[User guide](guide.md)

## Connect Reddit

1. Sign in to Reddit in your browser.
2. Start redtab and select that browser profile in **Connect Reddit**.
3. Allow OS credential access if prompted.
4. Check the verified username, then press `Enter` to save. `Esc` cancels.

redtab imports the session without a browser extension. Your browser can stay
closed afterward. Sessions are not renewed or reimported in the background.

Supported browsers on macOS/Linux: Chrome, Chromium, Firefox, Brave, Edge, Arc,
Zen and LibreWolf. Dia is also supported on macOS. The browser's session store
must be accessible; cookies held only in memory may be unavailable. A profile
with conflicting Reddit sessions is rejected.

On macOS, allow Keychain access. On Linux, install `secret-tool` and unlock
Secret Service/KWallet before importing.

If import fails, choose **Open a separate Reddit login window**. This needs a
Chrome/Chromium-compatible browser. Complete sign-in and confirm the verified
account in redtab. The temporary profile is closed and removed on completion or
cancellation. Browser challenges can still prevent sign-in.

## Manage saved accounts

Press `A` to open **Accounts**. Use `j`/`k` or the arrows to select a row.

| Action | Control | Result |
| --- | --- | --- |
| Switch | `Enter` on an account | Verify and restore that account's bookmarks and history |
| Add | `a` or **Add account** | Import a browser session and confirm its username |
| Reconnect | `r` on an account | Reimport an expired session; the current account keeps its reading position |
| Remove | `d`, then `Enter` | Delete the saved login; removing the active one also disconnects redtab |

Failed verification, saving or removal keeps the current connection. Switching
waits for pending Reddit writes. Removing a login leaves bookmarks, history,
caches and the browser session intact.

Startup restores the last saved account. **Existing connection** verifies an
older login whose account identity has not been saved.

Credentials are stored in macOS Keychain or Linux Secret Service. The vault holds
up to 32 accounts and 4 MiB; Linux's `secret-tool` also limits the encoded entry
to 8 KiB. An import that exceeds a limit leaves saved logins intact.

## Sign in from the command line

```sh
redtab --signin
redtab --signin --browser firefox --profile Work
redtab --signin --browser dia --profile Default  # macOS
```

`--profile` requires `--browser`; both require `--signin`. Sign-in cannot be
combined with `--check`, `--config` or `--subreddit`.

To import an existing Netscape cookie export, include HttpOnly cookies and a valid
`reddit_session`. Keep cookie values out of shell arguments, terminal output and logs.

```sh
redtab --signin < /path/to/your/reddit-cookies.txt
```

Piped imports verify the account before saving it. Browser/profile selection
requires an interactive terminal.

## Data and privacy

| Data | Location |
| --- | --- |
| Bookmarks and reading history | `$XDG_STATE_HOME/redtab/accounts/<account-id>/state.json` |
| Unidentified legacy connection state | `$XDG_STATE_HOME/redtab/state.json` |
| Cached posts and previews | `$XDG_CACHE_HOME/redtab` |

Defaults are `~/.local/state` for state and `~/.cache` for cache. Clearing the
cache keeps bookmarks and history. Reading state and content caches are separated
by account; unidentified sessions use a session fingerprint for their cache.
Reconnecting preserves the current reading state when redtab recognizes the same
account, or the same cookie for an unidentified account.

Saved sessions are encrypted by the OS credential store. Loaded cookies stay in
memory and are sent only to `https://www.reddit.com`. They are never logged,
cached or passed to media players. Browser imports may create private temporary
database snapshots; redtab removes its helper directory on completion or cancellation.
Cached content can include private posts your account can access.

redtab connects directly, ignoring `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY` and
`NO_PROXY`. External video players receive a loopback stream, so keep redtab open
while they play. Authenticated requests start at least one second apart and slow
further when Reddit reports a smaller remaining budget. Media uses a separate path.

On a rate limit, safe background reads stay pending until Reddit's wait expires
(60 seconds when no interval is supplied). Foreground work takes priority. Writes
are never automatically replayed. The footer shows the countdown and triggering
operation, HTTP status and quota when available; diagnostics omit queries and cookies.

Community membership loads at startup. Offscreen listings and icons use cached
data; live community listings load when shown. Reloading All subs marks communities
for refresh as they enter view, avoiding a request for every subscription at once.
