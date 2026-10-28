# Development

[User guide](guide.md)

See [installation](guide.md#install) for build dependencies.

```sh
cargo fmt --check
cargo test
cargo clippy --all-targets
cargo build
```

Tests use synthetic Reddit data, local HTTP fixtures and Ratatui's TestBackend;
they do not contact Reddit. UI tests cover representative wide, narrow and short
terminals, display-column layout and color contrast. They do not cover every
terminal protocol or Reddit payload. OS prompts, browser handoffs and visible
video/audio playback need separate manual checks.

The [UI reference](ui.md) lists layout rules, screens, fields, menus and states.
Configuration defaults live in [Config](../src/config.rs); labels, descriptions
and generated comments use the [settings schema](../src/config/schema.rs).

Startup graphics negotiation collects replies for at most one second, even when the
status reply arrives before graphics or cell measurements. A separate child owns
stdin and is killed/reaped after two seconds. The tmux fallback requires the actual
pane tty, compatible clients and matching measured client cell sizes. If no client
shows that window, all attached server clients must agree. Window pixel defaults
cannot establish native graphics support.

## Request scheduling

Selected threads, active feeds and main images have separate worker capacity from
account work, community listings and other media. Cache readers run independently of HTTP.

Account discovery preserves community and followed-user identities. Only communities
seed All subs; user tabs fetch submitted posts with distinct feed cache keys.
Community icon URLs and known absence travel with membership snapshots. Ordered
metadata seeds sidebar icons; only visible pixels load from the CDN. Unknown
communities alone need a separate metadata lookup. The subscription cache key is
versioned separately from other cached content.
Community and user membership share the protected write/acknowledgment and fresh
subscription reconciliation path. User writes resolve a validated profile fullname
from `user/<name>/about.json` and use `sr` in `api/subscribe`; community writes keep
`sr_name`. Neither writes nor ambiguous failures retry automatically. Ordered cache
tombstones prevent earlier reads from restoring pre-write membership.
At authenticated admission, selected threads precede visible listings, ordinary
foreground reads and background reads. Each class yields after two starts when a lower
class waits, so sustained browsing cannot starve feeds or account actions. All
classes share a 500 ms minimum between starts and obey server cooldowns. Native
HTML/GraphQL and classic JSON track separate quota budgets. Each budget has up to 32
credits, which refill no faster than once per second and stay capped by remaining quota.
Background normally spends only idle overflow, retaining up to three credits for
interaction; after two ready higher-priority starts it can borrow one for fairness.
That turn uses available credit without waiting for the normal background interval.
Visible island feeds use listing priority. Startup subscription snapshots use background admission;
forced membership refreshes retain foreground priority.

Queued and admission-waiting obsolete reads cancel immediately. Active reads get
a 500 ms reuse window, checked every 50 ms; returning to the same unfinished post
joins that read. Shared image URLs remain wanted while any card or detail consumer
needs them. Visible loads poll results at most 16 ms apart. Transmitted writes finish
their acknowledgments; buffered decoding and external player streams do not use
this cancellation.

Feed demand follows the active tab and current request generations. Retired reads
resume their cursors and forced-refresh intent on a settled return. Fresh quota
hints replace older pacing estimates; replies do not replenish burst credit.
Each admission captures the older requests still outstanding. Its quota hint retains
those reservations and deducts every newer start, regardless of completion order or
missing headers. Failed/cancelled requests release their outstanding entry; existing
admissions retain their reservation against delayed hints.
Home partials and JSON retain separate budgets while sharing
base request spacing and explicit cooldowns. Spacing runs from request starts,
so a slow response does not add another full interval. Long quota waits appear
in the loading state with a countdown. Background pacing alone does not mark the
connection paused; server cooldowns remain global.

Feed responses publish complete post records as the body arrives. Provisional pages
keep their request pending and cursor unchanged; only complete responses enter the
cache. Refreshes retain an existing page until its replacement completes. Best Home
streams complete cards from the browser's ranked HTML, including media and flags,
without separate post lookups. An initial partial of up to three posts fetches one
continuation. Cards from that continuation publish after its cursor is validated.
Failed continuations keep existing content. Cold loads retain the starter with `L`
retry; reloads retain the shown page with `r` retry. Failed combined loads are not cached.

## Media rendering

YouTube recognition is derived from stored link URLs, so cached posts and bookmarks
use the same video/poster behavior without a storage migration. Their canonical page
URL identifies playback; signed stream URLs stay in a 32-entry memory cache for at
most five minutes, shortened to expire before the signed URL. Failed playback clears
its cached source.

When Reddit supplies no YouTube thumbnail, cards and detail use the public video
thumbnail from `i.ytimg.com`. It loads through the image cache without waiting for
`yt-dlp`; Reddit cookies never reach that host.

Sound uses `ffplay`'s reported audio clock. Video waits for audio readiness and catches
up after rendering delays without restarting sound. ffmpeg decodes sound from the
exact start position and pipes it to `ffplay`; `ffplay`'s own seek can land seconds
early in YouTube audio. With FFmpeg 8.1 or newer, googlevideo files load through ranged
requests, which avoid googlevideo's near-real-time pacing; older builds wait seconds for the
sound after a seek. Muted playback uses the local clock.
Decoding continues while tiles encode; each picture finishes before newer pixels
replace it in the same viewport. Slow rendering drops pictures instead of building
a playback backlog. Viewport or source changes still cancel obsolete encodings.
The decoder discards pictures behind the audio target even after terminal writes
block the UI. Skipped pictures still count toward EOF.
Direct YouTube files use a 32 KiB probe limit to avoid unnecessary frame-rate
analysis before the first picture. Other media keeps its existing probing.

One background extraction runs at a time. Player disposal cancels it; the worker
kills and reaps its process group on cancellation, a 20-second deadline or oversized
output. `yt-dlp` runs without user config, plugins, cookie import or filesystem cache.
It prints only selected streams and access-validation fields; full metadata such
as captions and unselected formats cannot consume the inline output budget.
Only public, non-live metadata and HTTPS Google video streams are accepted. Only
User-Agent, YouTube Referer and Origin headers reach ffmpeg. This path never
uses the authenticated Reddit transport.

Extraction skips HLS manifests, which this playback path cannot use. DASH remains
available because its metadata can also expose direct HTTPS files.

Main media keeps its aspect ratio and fits the usable pane. Recent cards reuse
decoded pixels and prepared graphics; rapid navigation delays uncached preparation.
Kitty/Ghostty reuse uploaded strips across scrolling and popup clipping.

Article cards limit previews to 30% of the pane height and 1.5 times the source
resolution. Small thumbnails sit beside the headline; links without images show
a domain monogram. A corner arrow identifies link previews. YouTube keeps video
card behavior.

The media relay preserves upstream HTTP errors without forwarding response bodies.
Unavailable videos with no poster or retained frame leave room for the comments;
the footer keeps the error and retry action. A deleted post's video may remain in
Reddit metadata even after the CDN stops serving it.
Crossposts label an explicitly deleted original separately from media availability.
A deleted author alone does not establish post deletion. Loaded pages also update
the matching list records and saved bookmark.
Account-local deletion observations survive cached feed refreshes for 90 days;
bookmarks retain their learned flag.

Card navigation pauses the last loaded player so returning can reuse its decoder.
When another video takes over, up to eight recent frames (32 MiB) and playback
positions stay in memory, keyed by post and source. These cover selection settling
and decoder startup. Account and content-policy changes clear them; images off
releases them. Unfinished extraction still cancels when its selection changes.
Returning to a detail video uses the current sound preference.

Images above 16 million pixels use validated Reddit resolutions, preferring the
smallest with a long edge of at least 1600 pixels, or the largest available below
it. Original URLs stay separate for media handoff and downloads. Still decoding uses a 128 MiB
allocation budget; oversized originals without a usable preview offer `o`.

Native video retains a 921,600-pixel budget, allocated proportionally for portrait
frames. Fast lossless PNG carries the RGB pixels. Prepared strips publish together;
alternating hidden Kitty buffers precede synchronized placeholder updates. The
hardware cursor stays hidden during playback; focused text fields keep their caret.
The [terminal writer](../src/terminal_output.rs) coordinates uploads and tmux holds.
