# redtab

A terminal Reddit reader with tabs. It reads Reddit using your signed-in browser session,
held in your OS credential store. No Reddit API registration is needed.

## Features

- **Tabs**: a sidebar with Home, Bookmarks, All subs, your subscriptions and subreddits you opened,
  numbered `1`–`9` and badged with new-post counts. `\` hides it.
- **Side by side**: at 158+ columns (128 with the sidebar hidden) the list and the open post
  share the screen and the post follows the list selection; narrower, the post replaces
  the list.
- **Home**: Reddit’s browser Best feed, including recommendations from communities you
  haven't joined. Short initial batches continue automatically.
- **List views**: cards by default, with wrapped titles and full-width media previews that
  preserve their aspect ratio. `Page Up/Down` scrolls tall cards; `Ctrl+e/y` scrolls by line.
  `V` switches between Card and Compact in Home, subreddit feeds, search and Bookmarks;
  your choice is saved across restarts. Previews reserve their space while loading,
  and retained previews stay visible while full-quality images return from the disk cache.
  All subs keeps its community islands.
- **All subs**: one island per joined community with its top non-stickied posts. Subscriptions
  come from your Reddit account automatically; `r` here refreshes membership and posts.
- **Community actions**: `Space` opens the selected post's subreddit from any list.
  `S` opens Join/Leave; `Enter` confirms a change to your Reddit account. Joined communities
  move into Subscriptions; leaving keeps their tabs under Opened with the reading position intact.
- **Fuzzy jump** (`p`) to any tab or subreddit; sort, time range and search per subreddit.
- **Post view**: selftext and a foldable comment tree; deep threads load on demand.
- **Images**: pictures in posts and comments, sized to their shape (as half-block colors in
  terminals without graphics). Card previews also support half-blocks. Compact thumbnails and subreddit icons need kitty, iTerm2 or
  sixel graphics; Ghostty and kitty also work inside tmux. `i` turns images on and off.
- **Video and GIFs** play inline, muted, with `ffmpeg`; `a` turns sound on.
- **Media** (`m`): opens images in your viewer, video in mpv, links in the browser.
- **Bookmarks** (`b`), spoiler masking (`v` reveals), keyword/flair/author filters and
  keyword highlights.
- **Offline-friendly cache**: feeds, posts and still previews reuse saved data and refresh
  in the background.
- **Connection status** in the sidebar: up, down, or paused on a rate limit.
- **Saved accounts** (`A`): switch between saved Reddit logins, add an account or reconnect
  an expired session. Each account keeps its own bookmarks, history and content cache.

Honors `NO_COLOR`.

## Requirements

- Optional: `ffmpeg` (with `ffplay`) for inline video and sound, `mpv` for `m` on videos.

## Install

Requires Rust 1.90 or newer, CMake, a C/C++ compiler and libclang for the HTTP client's
BoringSSL build. On macOS these come from the Command Line Tools plus CMake; on Debian/Ubuntu
the build packages are `build-essential cmake clang libclang-dev pkg-config`.

```sh
cargo install --git https://github.com/kapral18/redtab --locked
```

## Usage

Start `redtab`. On first launch, **Connect Reddit** lists detected browser profiles.
Sign in at reddit.com in your browser, select its profile in redtab, and allow OS access
if prompted. redtab imports Reddit cookies, verifies the account with Reddit, then shows
the username. Press `Enter` to save the verified connection and continue. `Esc` cancels
before saving. No browser extension is needed.

Press `A` for **Accounts**, choose a saved username with `j`/`k` or the arrow keys, then
press `Enter` to verify and switch without another browser import. If verification or saving
fails, redtab keeps its current connection. Each redtab instance uses one account; startup
restores the last saved choice.

Choose **Add account** or press `a` to import another account. Press `r` on a saved account
to reconnect from its browser profile when sign-in expires. Imports show the verified
username for confirmation before saving a new account or updating an existing session.
Reconnecting the current account keeps your reading position; switching restores that
account's bookmarks and history.

To **Remove from redtab**, press `A`, select an account, then `d` and `Enter`. This deletes
its saved session. Removing the active account also disconnects redtab and clears its
displayed content. Other saved accounts, bookmarks, history and cached content are kept.
Your browser session is unchanged.
`Esc` cancels; failed removal keeps the current connection and shows an error.
redtab does not renew sessions or import browser cookies in the background.

Existing logins survive upgrades. Select **Existing connection** to verify an older login
whose account identity has not been saved.

Chrome/Chromium, Firefox, Brave, Edge, Arc, Zen and LibreWolf profiles are supported on
macOS/Linux where their session stores are accessible. Dia profiles are also supported on
macOS. Allow Keychain access if prompted; on Linux, unlock Secret Service/KWallet first.
Session cookies held only in browser memory
may be unavailable. Profiles containing conflicting Reddit sessions are rejected rather
than choosing an account. The import library can make private temporary database snapshots;
redtab removes its helper directory on completion or cancellation.

If import fails, choose **Open a separate Reddit login window**. This requires a detected
Chrome/Chromium-compatible browser. Complete login there; redtab captures the Reddit session
and asks you to confirm the verified account. This temporary profile closes and is removed
after import or cancellation. Browser login/challenges may still refuse access.

Credentials are encrypted in macOS Keychain or Linux Secret Service (`secret-tool`, supplied
by `libsecret-tools` on Debian/Ubuntu). Keep the store unlocked. Saved logins share the
existing OS entry. The collection is bounded to 32 accounts and 4 MiB; Linux's `secret-tool`
also limits the encoded entry to 8 KiB. If a new login would exceed a limit, the existing
saved logins are kept. You can also start the account dialog directly:

```sh
redtab --signin
redtab --signin --browser firefox --profile Work
redtab --signin --browser dia --profile Default
```

For an existing Netscape export, include HttpOnly cookies and a valid `reddit_session`.
Piped imports also verify the account before adding or updating its saved login. Keep cookie
values out of terminal output, shell arguments and automation logs:

```sh
redtab --signin < /path/to/your/reddit-cookies.txt
```

Your browser can stay closed while you read. Reconnecting does not require restarting redtab.

```sh
redtab                    # Home: your personalized Reddit feed
redtab -s neovim          # open r/neovim on start
redtab -c ~/redtab.toml   # another config file
redtab --check            # check Home and account subscriptions; exit 1 if either fails
```

If the community list fails to refresh, redtab keeps the last known list. Join/Leave waits
for Reddit to confirm membership before updating tabs and community islands. Pending
changes appear in the footer and are not retried automatically. Press `r` in All subs to
refresh membership after a failure.

## Configure

`$XDG_CONFIG_HOME/redtab/config.toml` (default `~/.config/redtab/config.toml`). Every key is
optional.

```toml
all_subs_posts_per_group = 3
show_nsfw = false
image_preview = true                    # graphics are detected only when this is on at start

[filters]
keywords = ["giveaway"]   # hide titles containing these (case-insensitive)
flairs = ["Meme"]
authors = []

[highlight]
keywords = ["lsp", "treesitter"]

[commands]                # defaults: `open` on macOS, `xdg-open` elsewhere; mpv for video
browser = ["open"]
video = ["mpv", "--force-window=immediate"]
image = ["open"]
```

The old `subscriptions`, `subscriptions_file` and `home_posts_per_group` config keys are
removed. Remove them from existing config files; use `all_subs_posts_per_group` for island
size. Existing subscription files are no longer read and are left untouched.

## Data and privacy

Bookmarks and reading history live in
`$XDG_STATE_HOME/redtab/accounts/<account-id>/state.json`.
The older `$XDG_STATE_HOME/redtab/state.json` is left intact.
Reconnecting while redtab is open keeps your current reading state if redtab recognizes
the same account. If the previous account is unknown, this requires the same session cookie;
otherwise, redtab loads the selected account's saved history.

Cached posts and previews live in `$XDG_CACHE_HOME/redtab`. Clearing that directory keeps
your bookmarks and reading history.

redtab connects to Reddit directly and ignores `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY` and
`NO_PROXY`. The session is encrypted locally at rest; loaded cookies stay in memory and are sent only to `https://www.reddit.com`;
they are never logged, cached or passed to media players. Content caches are separated by
the verified account (older unidentified sessions use a session fingerprint). Cached content
can include private posts your account can access.
Video players receive a loopback stream, so keep redtab open while they play.

On a rate limit, redtab pauses requests for the time Reddit gives (60 s if none) and keeps
the same session.

## Keys

Press `?` in redtab for the full list.

| Keys | Action |
| --- | --- |
| `Tab` `]` / `S-Tab` `[` | next / previous tab |
| `1`–`9`, `0` | jump to a tab, last tab |
| `p` | fuzzy jump to a tab or open a subreddit |
| `\` | hide / show the sidebar |
| `h`, then `j` `k` `g` `G` | from a list: focus the sidebar and switch tabs in it (`l` `Enter` `Esc` return) |
| `j` `k`, `C-d` `C-u`, `g` `G` | move, page, first / last |
| `Enter` `l` | open the selected post, or resume its retained detail |
| `V` | switch Card / Compact list view (saved across restarts) |
| `A` | saved accounts: switch, add, reconnect or remove a login |
| `Page Down/Up`, `C-d` `C-u`, `C-e` `C-y` | cards: scroll by page, half page, line; at an edge, move to the next / previous post |
| `J` `K` | All subs: next / previous card |
| `Space` | list: open the selected community; post: page down |
| `S` | community: Join/Leave on Reddit (`Enter` confirms, `Esc` cancels) |
| `s`, `t` | Home/subreddit: sort, time range |
| `/` | subreddit: search (`Esc` clears) |
| `L` | Home/subreddit: load the next page now |
| `l` `c` `Enter` | post: select the comment you are reading; again on a selected comment: fold a thread or load more replies |
| `n` `N` | post: next / previous top-level comment |
| `v` | post: show / hide spoilers |
| `P`, `a`, `,` `.` | post video: pause / play, sound on / off, seek back / forward 5 s |
| `Space`, `C-d` `C-u`, `C-e` `C-y` | post: scroll by page, half page, line |
| `h` | split detail: focus the list without losing your place (moving the list then shows the selected post beside it); narrow detail: back one level |
| `Esc` `q` | detail: close the post and return to the list |
| `o`, `y`, `m`, `b` | open on reddit.com, copy link, media, bookmark |
| `i`, `r` | toggle images and videos, reload |
| `x` | close a tab opened with `p` |
| `q`, `C-c` | quit (`q` closes an open post first, even when the list has focus) |

## Development

```sh
cargo fmt --check
cargo test     # synthetic Reddit JSON and local HTTP fixtures; no Reddit traffic
cargo clippy --all-targets
cargo build
```

## License

[MIT](LICENSE)
