# redtab

A terminal Reddit reader with tabs. It reads Reddit JSON using an anonymous guest session;
no Reddit account or API key is needed.

## Features

- **Tabs**: a sidebar with Home, Bookmarks, your subscriptions and subreddits you opened,
  numbered `1`–`9` and badged with new-post counts. `\` hides it.
- **Side by side**: at 158+ columns (128 with the sidebar hidden) the list and the open post
  share the screen and the post follows the list selection; narrower, the post replaces
  the list.
- **Home**: one card per subscription with its top posts.
- **Fuzzy jump** (`p`) to any tab or subreddit; sort, time range and search per subreddit.
- **Post view**: selftext and a foldable comment tree; deep threads load on demand.
- **Images**: pictures in posts and comments, sized to their shape (as half-block colors in
  terminals without graphics). Thumbnails and subreddit icons in lists need kitty, iTerm2 or
  sixel graphics; Ghostty and kitty also work inside tmux. `i` turns images on and off.
- **Video and GIFs** play inline, muted, with `ffmpeg`; `a` turns sound on.
- **Media** (`m`): opens images in your viewer, video in mpv, links in the browser.
- **Bookmarks** (`b`), spoiler masking (`v` reveals), keyword/flair/author filters and
  keyword highlights.
- **Offline-friendly cache**: feeds, posts and thumbnails show instantly from the last visit
  and refresh in the background.
- **Connection status** in the sidebar: up, down, or paused on a rate limit.

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

```sh
redtab                    # Home: your subscriptions
redtab -s neovim          # open r/neovim on start
redtab -c ~/redtab.toml   # another config file
redtab --check            # print config, status and files; exit 1 unless Reddit is up
```

## Configure

`$XDG_CONFIG_HOME/redtab/config.toml` (default `~/.config/redtab/config.toml`). Every key is
optional.

```toml
# Subscriptions: inline, and/or one per line in a file (`#` comments allowed).
subscriptions = ["neovim", "rust"]
# subscriptions_file = "my-subreddits"  # override; relative to config.toml; must exist

home_posts_per_group = 3
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

Keep public subscriptions inline in `config.toml`, or a private list in
`$XDG_STATE_HOME/redtab/subscriptions` (default `~/.local/state/redtab/subscriptions`).
The private file is optional. Inline names come first, followed by file entries, with
case-insensitive duplicates removed. `-c` does not change the private file's default location;
`subscriptions_file` overrides it, with relative paths resolved beside the config file.

Reading state (seen posts, bookmarks) lives in `$XDG_STATE_HOME/redtab/state.json`; the
disposable cache in `$XDG_CACHE_HOME/redtab`. Deleting the cache loses nothing you keep.

redtab connects to Reddit directly and ignores `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY` and
`NO_PROXY`. Guest tokens are never written to the cache or passed to media players. Video
players receive a loopback stream, so keep redtab open while they play.

On a rate limit, redtab pauses requests for the time Reddit gives (60 s if none) and keeps
its guest session.

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
| `J` `K`, `Space` | Home: next / previous card, open the card's subreddit |
| `s`, `t`, `/` | subreddit: sort, time range, search (`Esc` clears) |
| `L` | subreddit: load the next page now |
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
