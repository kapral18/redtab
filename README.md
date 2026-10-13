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
- **Local bookmarks** (`b`), stored on your machine.
- **Inline images** (kitty, iTerm2 and sixel protocols, half-blocks elsewhere). Inside tmux the
  graphics protocols need `set -g allow-passthrough on`.
- **Media handoff** (`m`): images and galleries are saved to a private folder under `$TMPDIR`
  (left in place for the viewer; the OS clears it) and opened with your image viewer (Preview on
  macOS), videos go to mpv (Reddit-hosted video via HLS), links to the browser.
- **Filters and highlights**: hide posts by keyword, flair or author; highlight keywords.
- **Redlib health** in the sidebar, with an optional command to start Redlib (`S`).

redtab uses a Catppuccin Mocha palette with Reddit orange and honors `NO_COLOR`.

## Requirements

- A Redlib instance. Public instances usually show bot checks, so run one locally, for example:

  ```sh
  docker run -d --name redlib --restart unless-stopped -p 127.0.0.1:8080:8080 quay.io/redlib/redlib:latest
  ```

  If the official image fails with `Failed to create OAuth client`
  ([redlib#485](https://github.com/redlib-org/redlib/issues/485)), community forks such as
  `ghcr.io/silvenga/redlib` have kept working.
- Optional: `mpv` for video, a terminal with an image protocol for inline images.

## Install

```sh
cargo install --git https://github.com/kapral18/redtab
```

## Configure

`$XDG_CONFIG_HOME/redtab/config.toml` (default `~/.config/redtab/config.toml`). Every key is optional.

```toml
redlib_url = "http://127.0.0.1:8080"

# Subscriptions: inline, and/or one per line in a file (`#` comments allowed).
subscriptions = ["neovim", "rust"]
subscriptions_file = "~/.config/redtab/subscriptions"   # this path is also the default

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

Reading state (seen and opened posts, bookmarks) lives in
`$XDG_STATE_HOME/redtab/state.json` (default `~/.local/state/redtab/state.json`).

`redtab --check` prints the Redlib URL, whether it answers, and how many subscriptions it read.

## Keys

Press `?` in redtab for the full list.

| Keys | Action |
| --- | --- |
| `Tab` `]` / `S-Tab` `[` | next / previous tab |
| `1`–`9`, `0` | jump to a tab, last tab |
| `p` | fuzzy jump to a tab or open a subreddit |
| `\` | hide / show the sidebar |
| `j` `k`, `C-d` `C-u`, `g` `G` | move, page, first / last |
| `Enter` `l` | open the post |
| `J` `K`, `Space` | Home: next / previous card, open the card's subreddit |
| `s`, `t`, `/` | subreddit: sort, time range, search (`Esc` clears) |
| `L` | load the next page now |
| `c`, `n` `N` | post: fold a thread or load more replies, next / previous top-level comment |
| `Space`, `C-d` `C-u`, `C-e` `C-y` | post: scroll by page, half page, line |
| `h` `Esc` `q` | back from a post |
| `o`, `y`, `m`, `b` | open on reddit.com, copy link, media, bookmark |
| `i`, `r`, `S` | toggle images, reload, start Redlib |
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
