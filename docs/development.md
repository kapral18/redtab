# Development

[User guide](guide.md) · [UI reference](ui.md)

Build dependencies: see [installation](guide.md#install).

```sh
cargo +stable fmt --check
cargo +stable test --locked --all-targets
cargo +stable clippy --locked --all-targets -- -D warnings
cargo +1.90.0 check --locked --all-targets
cargo deny check advisories bans licenses sources
```

CI runs these on Ubuntu 24.04 and macOS 15: stable for formatting, tests and clippy,
Rust 1.90.0 for the minimum-version check, and `cargo deny` on Ubuntu with
[deny.toml](../deny.toml). CI runs tests with `--test-threads=1` because many tests
measure real time and miss their margins on busy runners. Pushing a `v*` tag runs
[release.yml](../.github/workflows/release.yml): tests, release builds, archives,
`SHA256SUMS`, build attestations and a GitHub release. Build a debug binary with
`cargo build --locked`.

## Tests

- Tests use synthetic data, in-memory HTTP fixtures and Ratatui's TestBackend. They
  never contact Reddit.
- UI tests cover wide, narrow and short terminals.
- Check OS prompts, browser handoffs and video/audio playback by hand.

## Where things live

| Topic | Source |
| --- | --- |
| Config defaults | [config](../src/config.rs) |
| Setting labels and comments | [settings schema](../src/config/schema.rs) |
| Key bindings and hints | [key map](../src/keys/map.rs), [dispatch](../src/keys/dispatch.rs) |
| Menus | [menus](../src/app/experience/menus.rs) |
| Terminal output | [terminal writer](../src/terminal_output.rs) |

## Reddit endpoints

| Data | Host and route |
| --- | --- |
| Home, Best sort | `www.reddit.com` browser pages |
| Home, other sorts; subscriptions; user feeds | `www.reddit.com` JSON |
| Community, Comments, Media and People search | `www.reddit.com/svc/shreddit/search/` |
| Community listings, Posts search | `api.reddit.com` JSON |
| r/all | `old.reddit.com`: `r/all.json`, `r/all/<sort>.json` |

- HTTP 401 on `www` signs the session out. A 401 on `api` or `old`, or a 403 on any host, does not.
- `api` and `old` receive only domain-wide `.reddit.com` cookies.
- The classic `subreddits/search` and `users/search` return empty lists for some
  sessions, and the classic API has no comment or media search, so search uses
  rendered pages. Media results then load each post from `www` `comments/<id>.json`.
- Reddit refuses r/all New and Rising on every host.

## Request rules

- Priority order: open thread, feeds, foreground reads, island browsing, background.
  Each class yields after two starts so lower classes never starve.
- Native pages and JSON have separate quota budgets. Consecutive starts are at least
  100 ms apart when the budget's quota is known and 500 ms apart when it is not; a
  budget with unknown quota also waits one second between its own starts. Background
  starts are at least 500 ms apart. Island browsing and background work cannot spend a
  budget's last three credits.
- A rate limit pauses every class until Reddit's wait expires (60 s if unspecified).
  Writes are never replayed.
- Scroll-triggered requests wait 200 ms after movement stops.
- Queued obsolete reads cancel at once. Active reads get 500 ms in case you return.
  Sent writes always finish.
- Feeds publish complete posts as the body arrives; only complete responses enter
  the cache.
- The 32 most recent collection pages stay in memory for five minutes.

## Media

- Images over 16 megapixels use a Reddit preview: the smallest with a long edge of at
  least 1600 px, else the largest under 16 megapixels.
  Decoding has a 128 MiB budget.
- Video frames have a 921,600-pixel budget. Slow terminals drop frames instead of
  building a backlog.
- The three most recent players stay hidden for reuse; eight frames (32 MiB) cover
  selection changes.
- `ffmpeg` decodes sound from the exact position and pipes it to `ffplay`.
  FFmpeg 8.1+ seeks YouTube faster.
- `yt-dlp` extractions run one at a time, with no user config, plugins or cookies.
  Each has a 20 s deadline that includes waiting for its turn. Only public or unlisted,
  non-live HTTPS streams are accepted. Stream URLs are cached
  for at most five minutes.
- A 403 on a Reddit HLS master retries once with the same video's DASH files.
- Native Kitty graphics in tmux set pane-local `allow-passthrough all` and restore
  the previous value on exit.

## Reddit text

HTML bodies keep inline styles through internal markers that incoming text cannot
inject. Markdown-only content goes through pulldown-cmark. One wrapper renders
posts and dialogs, and layout measures the same wrapped lines. Cache keys are
versioned so older flattened content is discarded.
