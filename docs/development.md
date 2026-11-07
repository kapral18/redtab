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

CI runs these on Ubuntu 24.04 and macOS 26: stable for formatting, tests and clippy,
Rust 1.90.0 for the minimum-version check, and `cargo deny` on Ubuntu with
[deny.toml](../deny.toml). CI runs tests with `--test-threads=1` because many tests
measure real time and miss their margins on busy runners. Build a debug binary
with `cargo build --locked`.

## Releases

Releases are automated with [release-plz](https://release-plz.dev) and
[release.yml](../.github/workflows/release.yml); nobody pushes tags by hand.

1. Every push to `main` that changes the packaged crate updates a pull request
   named `chore: release v<version>` that bumps `version` in `Cargo.toml` and adds
   a `CHANGELOG.md` entry. The next version comes from the
   [Conventional Commits](https://www.conventionalcommits.org) since the last
   release, following Cargo's SemVer rules: while the version is `0.x`, a breaking
   change (`feat!:` or a `BREAKING CHANGE:` footer) bumps the minor and anything
   else bumps the patch; from `1.0`, `feat:` bumps the minor and a breaking change
   bumps the major.
2. Whenever the `version` in `Cargo.toml` on `main` has no tag, the workflow
   releases that commit. Merging the release pull request is how that normally
   happens. The workflow tests and builds the commit for the four targets, and
   only if that passes tags it `v<version>` and publishes the GitHub release with
   the archives, `SHA256SUMS` and build attestations. After that, two jobs run side
   by side: one publishes the crate to crates.io through trusted publishing from
   the `release` environment, the other rewrites `Formula/redtab.rb` in
   [kapral18/homebrew-tap](https://github.com/kapral18/homebrew-tap) with the new
   URLs and checksums.

Only one run of the workflow is active at a time, and GitHub keeps only the newest
waiting run, so a push during a release updates the release pull request only
after that release is done. Merge the release pull request when it is up to date
with `main`: commits that land between its last update and the start of its run
ship in that release without a changelog line. A version that is not higher than
the latest one on crates.io stops the workflow.

If a release job fails for a temporary reason (a network error), re-run the failed
jobs or all jobs; a run finishes the release at the commit it tagged and skips what
is already done. If GitHub drops a waiting run (for example because a re-run took
its place), start the Release workflow on `main` from the Actions tab; it releases
or updates the release pull request from the current state. If the build fails
before tagging, push the fix to `main`; the version is still untagged, so that
push releases it. A tag never moves: if a step after tagging needs a code fix,
commit the fix together with a higher `version` in `Cargo.toml` and its
`CHANGELOG.md` entry. While a tagged version is missing from crates.io, the release
pull request job fails on every push, so re-run the failed release first;
publishing by hand is not the fix, because the crate accepts only trusted
publishing. Prerelease versions (with a `-`) stop the workflow, and the Homebrew
formula is never moved back to an older version.

Commit messages on `main` must follow Conventional Commits. Squash-merge
contributor pull requests and give them a conventional title, because the title
becomes the commit message. The release pull request is opened by GitHub Actions,
so its CI runs start only after a maintainer approves them on the pull request.

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
