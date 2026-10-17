# redtab

A terminal Reddit reader with tabs. It reads Reddit through a
[Redlib](https://github.com/redlib-org/redlib) instance, so it needs no Reddit account or
API key.

## Features

- **Tabs**: a sidebar with Home, Bookmarks, your subscriptions and subreddits you opened,
  numbered `1`–`9` and badged with new-post counts. `\` hides it.
- **Side by side**: at 158+ columns (128 with the sidebar hidden) the list and the open post
  share the screen; narrower, the post replaces the list.
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
- **Redlib health** in the sidebar, with an optional command to start it (`S`).

Honors `NO_COLOR`.

## Requirements

- A Redlib instance. Public ones usually show bot checks, so run one locally:

  ```sh
  docker run -d --name redlib --restart unless-stopped -p 127.0.0.1:8080:8080 quay.io/redlib/redlib:latest
  ```

  If it fails with `Failed to create OAuth client`
  ([redlib#485](https://github.com/redlib-org/redlib/issues/485)), the fork
  `ghcr.io/silvenga/redlib` has kept working.
- Optional: `ffmpeg` (with `ffplay`) for inline video and sound, `mpv` for `m` on videos.

## Install

Requires Rust 1.90 or newer.

```sh
cargo install --git https://github.com/kapral18/redtab
```

## Usage

```sh
redtab                    # Home: your subscriptions
redtab -s neovim          # open r/neovim on start
redtab -c ~/redtab.toml   # another config file
redtab --check            # config, Redlib status and state file; exits 1 if Redlib is down
```

Press `?` inside redtab for the keys.

## Configure

`$XDG_CONFIG_HOME/redtab/config.toml` (default `~/.config/redtab/config.toml`). Every key is
optional.

```toml
redlib_url = "http://127.0.0.1:8080"

# Subscriptions: inline, and/or one per line in a file (`#` comments allowed).
subscriptions = ["neovim", "rust"]
subscriptions_file = "subscriptions"   # default; relative to config.toml; a set path must exist

home_posts_per_group = 3
show_nsfw = false
image_preview = true                    # graphics are detected only when this is on at start
redlib_start_command = ["docker", "start", "redlib"]   # run by `S`

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

Reading state (seen posts, bookmarks) lives in `$XDG_STATE_HOME/redtab/state.json`; the
disposable cache in `$XDG_CACHE_HOME/redtab`. Deleting the cache loses nothing you keep.

`redtab --check` prints the config file, Redlib's status, the subscription count and the
state file, and exits 1 when Redlib does not answer.

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
| `L` | load the next page now |
| `l` `c` `Enter` | post: select the comment you are reading; again on a selected comment: fold a thread or load more replies |
| `n` `N` | post: next / previous top-level comment |
| `v` | post: show / hide spoilers |
| `P`, `a`, `,` `.` | post video: pause / play, sound on / off, seek back / forward 5 s |
| `Space`, `C-d` `C-u`, `C-e` `C-y` | post: scroll by page, half page, line |
| `h` | split detail: focus the list without losing your place; narrow detail: back one level |
| `Esc` `q` | detail: close the post and return to the list |
| `o`, `y`, `m`, `b` | open on reddit.com, copy link, media, bookmark |
| `i`, `r`, `S` | toggle images and videos, reload, start Redlib |
| `x` | close a tab opened with `p` |
| `q`, `C-c` | quit (`q` closes an open post first, even when the list has focus) |

## Development

```sh
cargo fmt --check
cargo test     # parser tests run against saved Redlib pages in tests/fixtures
cargo clippy --all-targets
cargo build
```

## License

[MIT](LICENSE)
