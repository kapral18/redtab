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

CI runs these on Ubuntu 24.04 and macOS 26 as separate parallel jobs (tests,
formatting and clippy, and the Rust 1.90.0 minimum-version check), plus
`cargo deny` on Ubuntu with [deny.toml](../deny.toml). Jobs restore a compile
cache, and runs on `main` update it. Tests run on the default parallel test
threads, so a test that measures real time must leave room for a busy runner.
Build a debug binary with `cargo build --locked`.

## Releases

Every push to `main` runs [release.yml](../.github/workflows/release.yml). A push
that contains a `feat`, `fix` or `perf` commit, or a breaking change, since the
last `v*` tag releases; any other push only runs CI. Nobody bumps versions or
pushes tags by hand. Runs are serialized, one at a time.

| Commits since the last tag | Next version while `0.x` | From `1.0` |
| --- | --- | --- |
| only `chore:`, `docs:`, `ci:`, `test:`, `refactor:`, `style:`, `build:` or non-conventional | no release | no release |
| `fix:` or `perf:` | patch | patch |
| `feat:` | minor | minor |
| `<type>!:` or a `BREAKING CHANGE:` footer | minor | major |

1. **ci** runs [ci.yml](../.github/workflows/ci.yml) on the pushed commit.
2. **plan** asks [git-cliff](https://git-cliff.org) ([cliff.toml](../cliff.toml))
   for the next version. If it equals the last tag, the run ends.
3. **build** sets that version in `Cargo.toml` and builds the four targets. It
   also runs the tests on Intel macOS and ARM Linux, which CI does not cover.
   Nothing outside the run has changed yet.
4. **release** commits `chore(release): v<version>` (version in `Cargo.toml` and
   `Cargo.lock`, `CHANGELOG.md` regenerated) on top of the pushed commit, tags it,
   and pushes both to `main` in one atomic push. If `main` moved in the meantime,
   the push is rejected, nothing is released, and the run for the newer push
   releases everything. Then it creates the GitHub release with the archives,
   `SHA256SUMS`, build attestations and the changelog section as notes.
5. **crate** publishes to crates.io through trusted publishing from the `release`
   environment, and **homebrew** rewrites `Formula/redtab.rb` in
   [kapral18/homebrew-tap](https://github.com/kapral18/homebrew-tap). They run
   side by side from the tag.

The tag is the record of a release. Pull after a release: the release commit
lands on `main` after your push. If a job fails, re-run the failed jobs: a run
that finds its tag already pushed finishes what is missing (the GitHub release,
the crate, the formula) and skips what is done. A failure before the tag push
leaves no trace; fix it and push. Re-running an old run can make GitHub cancel
a newer waiting run; if one shows as cancelled, re-run it. A tag never moves, so a problem found after it
ships as the next version. The Homebrew formula never moves back to an older
version. Trusted publishing is registered for `release.yml` and the `release`
environment; renaming either breaks publishing until it is re-registered on
crates.io.

On `main`, CI runs only inside the Release run, so a push waits for the release
before it; when pushes pile up, only the newest is tested and released, and it
contains the others.

Squash-merge contributor pull requests and give them a conventional title,
because the title becomes the commit message.

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
