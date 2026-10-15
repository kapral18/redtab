# redtab

A terminal Reddit reader with tabs. It reads Reddit through a
[Redlib](https://github.com/redlib-org/redlib) instance, so it needs no Reddit
account or API key.

- **Tab sidebar**: a vertical list on the left with Home, Bookmarks, your subscriptions and
  subreddits you opened, numbered for `1`–`9` and badged with new-post counts. `\` hides it.
- **Home wall**: one padded card per subscribed subreddit with its top posts, so subreddits
  never mix. Enter opens a post; Space opens that subreddit's tab.
- **Fuzzy jump** (`p`): switch to any tab or open any subreddit by name.
- **Sorting**: hot, new, top, rising, controversial; time range for top and controversial.
- **Search** inside a subreddit (`/`).
- **Paging**: the next page loads as you reach the end of a list.
- **Post view**: selftext and a threaded, foldable comment tree; deep threads load on demand.
- **New-post badges**: tabs and Home cards count posts that appeared since your last visit;
  posts you opened are dimmed.
- **Local bookmarks** (`b`), stored on your machine and shared safely between several
  running redtab instances.
- **Spoilers**: spoiler-tagged posts list with a chip and no thumbnail, and open with their
  body, image and comments hidden; `>!spoiler!<` text in posts and comments shows as `▒`.
  `v` reveals.
- **Thumbnails and icons**: with a graphics protocol (kitty, iTerm2, sixel), post rows show a
  thumbnail on the left and subreddits show their icon in the sidebar and on Home cards.
  Half-block terminals keep text-only lists. `i` turns images on and off. redtab detects the
  graphics protocol only when `image_preview = true` at start, so with it off thumbnails and
  icons stay off.
- **Inline images** (kitty, iTerm2 and sixel protocols, half-blocks elsewhere). Inside tmux,
  redtab turns on `allow-passthrough` for its pane. When the terminal does not answer the
  protocol query there, or only tmux's own sixel support answers, and redtab runs directly in
  the pane, kitty graphics are used if every client attached to a session holding redtab's
  window has a terminal name containing `ghostty` or `kitty` (for example `xterm-ghostty`).
- **Inline video**: a Reddit-hosted video plays muted under the post's header when you open
  the post, in the same image protocol (kitty graphics in Ghostty and kitty), decoded at the
  area's pixel size up to 1280×720 (sixel: 640×360). `P` pauses and plays, `a` turns the
  sound on and off (it stays on for later videos until you turn it off), `,` and `.` seek 5
  seconds back and forward. It pauses while scrolled away or under a popup, loops at the end
  and stops when you leave the post; `m` pauses it (`P` plays it again). Needs `ffmpeg`;
  sound plays through `ffplay`, which comes with it.
- **Media handoff** (`m`): images and galleries are saved to a private folder under `$TMPDIR`
  (left in place for the viewer; the OS clears it) and opened with your image viewer (Preview on
  macOS), videos go to mpv in a window (Reddit-hosted video via HLS), links to the browser.
- **Filters and highlights**: hide posts by keyword, flair or author; highlight keywords.
- **Redlib health** in the sidebar, with an optional command to start Redlib (`S`).

redtab uses a Catppuccin Mocha palette with Reddit orange and honors `NO_COLOR`: colors go,
bold stays, and images need a graphics protocol (half-block images are only colors).

## Requirements

- A Redlib instance. Public instances usually show bot checks, so run one locally, for example:

  ```sh
  docker run -d --name redlib --restart unless-stopped -p 127.0.0.1:8080:8080 quay.io/redlib/redlib:latest
  ```

  If the official image fails with `Failed to create OAuth client`
  ([redlib#485](https://github.com/redlib-org/redlib/issues/485)), community forks such as
  `ghcr.io/silvenga/redlib` have kept working.
- Optional: `ffmpeg` (with `ffplay`) for inline video, `mpv` for video in a window (`m`), a
  terminal with an image protocol for inline images, thumbnails and icons.

## Install

Building requires Rust 1.90 or newer.

```sh
cargo install --git https://github.com/kapral18/redtab
```

## Configure

`$XDG_CONFIG_HOME/redtab/config.toml` (default `~/.config/redtab/config.toml`). Every key is optional.

```toml
redlib_url = "http://127.0.0.1:8080"

# Subscriptions: inline, and/or one per line in a file (`#` comments allowed).
subscriptions = ["neovim", "rust"]
subscriptions_file = "subscriptions"   # default; relative paths are next to config.toml, and a set path must exist

home_posts_per_group = 3
show_nsfw = false
image_preview = true
redlib_start_command = ["docker", "start", "redlib"]

[filters]
keywords = ["giveaway"]   # hide titles containing these (case-insensitive)
flairs = ["Meme"]         # hide these flairs
authors = []

[highlight]
keywords = ["lsp", "treesitter"]

[commands]                # defaults: `open` on macOS, `xdg-open` elsewhere; mpv for video
browser = ["open"]
video = ["mpv", "--force-window=immediate"]
image = ["open"]
```

`S` waits up to 20 seconds for the start command. A server that is still running
then stays running after redtab quits. Startup errors include at most the first
64 KiB of stderr; further output is discarded.

Reading state (seen and opened posts, bookmarks) lives in
`$XDG_STATE_HOME/redtab/state.json` (default `~/.local/state/redtab/state.json`).

`redtab --check` prints the config file, the Redlib URL and whether it answers, how many
subscriptions it read, and the state file. It exits with status 1 when Redlib does not answer.

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
| `Enter` `l` | open the post |
| `J` `K`, `Space` | Home: next / previous card, open the card's subreddit |
| `s`, `t`, `/` | subreddit: sort, time range, search (`Esc` clears) |
| `L` | load the next page now |
| `l` `c` `Enter` | post: select the comment you are reading (after `h`, the one you left); again on a selected comment: fold a thread or load more replies |
| `n` `N` | post: next / previous top-level comment |
| `v` | post: show / hide spoilers |
| `P`, `a`, `,` `.` | post video: pause / play, sound on / off, seek back / forward 5 s |
| `Space`, `C-d` `C-u`, `C-e` `C-y` | post: scroll by page, half page, line |
| `h`; `Esc` `q` | post: back one level (selected comment, then the post); back to the list |
| `o`, `y`, `m`, `b` | open on reddit.com, copy link, media, bookmark |
| `i`, `r`, `S` | toggle images and videos, reload, start Redlib |
| `x` | close a tab opened with `p` |
| `q`, `C-c` | quit |

## How it works

Redlib renders Reddit as server-side HTML with stable class names. redtab fetches those pages,
parses them with CSS selectors (`scraper`), and draws them with `ratatui`. Requests run on a small
thread pool; the UI never blocks on the network. Redlib settings travel as cookies, which is how
redtab gets HLS video links without changing the Redlib instance.

## Development

```sh
cargo test     # parser tests run against saved Redlib pages in tests/fixtures
cargo clippy --all-targets
```
