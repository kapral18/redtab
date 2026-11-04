# Development

[User guide](guide.md)

See [installation](guide.md#install) for build dependencies.

```sh
cargo fmt --check
cargo test
cargo clippy --all-targets
cargo build
```

CI runs these checks on Linux and macOS and checks the locked dependencies with
Rust 1.90.

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
pane tty, compatible clients in a session containing its window, and matching
measured client cell sizes. Confirmed Ghostty/Kitty clients use Kitty rather than
an unsupported protocol hint. Window pixel defaults cannot establish support.
Native Kitty temporarily uses pane-local `allow-passthrough all`: tmux's `on`
can discard uploads during redraws or while the window is hidden, without replay.
Exit restores the previous pane override or inheritance, unless the user changed
it meanwhile. Commands target the captured socket and pane and have deadlines.
Uploads still require an attached, unsuspended client whose session contains the
window; an unrelated attached client cannot establish delivery.

## Reddit text

The HTML body parser preserves inline styles using internal noncharacter markers,
alongside the existing spoiler and picture markers. Incoming text cannot inject
these markers. Markdown-only content goes through pulldown-cmark with Reddit
spoiler and superscript handling. Spoilers remain linear text markers across
Markdown formatting boundaries. One styled-line wrapper serves post documents
and collection dialogs; measurements use those same wrapped lines. Folded
previews and comment search strip style markers and link-target metadata. Link
destinations stay in the body separately from visible labels; the local `gx` picker
omits links in hidden spoilers. Raw edit source stays separate. Post and reply
cache keys are versioned to discard older flattened or URL-dump content.

## Request scheduling

Selected threads, active feeds and main images have separate worker capacity from
account work, community listings and other media. Cache readers run independently of HTTP.

Search retires pending row and preview reads when its tab is left. Interrupted
row pages resume on return with their rows, cursor and selection retained; edits
made before leaving take precedence over the interrupted query. Late
cancelled replies cannot replace them. Typed queries retire unused Explore
discovery. Pool resets leave hidden Search idle until return. Discovery and page
failures show the actual error beside a separate retry row, naming Esc then r
while typing.

Search community pages use selected-read HTTP priority ahead of passive post
previews. The viewport starts the next page one API page before the loaded
end; only one continuation runs, and cursor/error guards remain in force. Row
geometry wraps only the current window rather than every loaded community.
Search drill-down keeps the real community/user feed active and retains its
Search origin for layout and back navigation. Wide content pairs advance from
communities/posts to posts/detail. The first Left focuses the post list while
retaining visible detail. The next closes detail and restores communities/posts,
releasing detail media ownership while preserving the underlying list state.
Retained parent rows are inactive and resume their interrupted reads on return.

Scroll-triggered listings, pagination, detail previews and viewport media wait
200 ms after the last movement, including the first movement after a pause.
This covers islands, feeds, cards, sidebar navigation, comments and collection
dialogs. Feed pagination uses the last visible post and the existing five-item
buffer; redraws and arriving pages do not create further navigation demand.
Cached visible content and incoming updates stay immediate; direct entry,
explicit opens and refreshes keep their normal request paths.

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
Community listings and Search use suffix-free JSON routes on `api.reddit.com`;
`r/all` retains its `old.reddit.com` JSON route. These hosts receive only
domain-scoped Reddit cookies; a refusal there does not sign out the saved session.
At authenticated admission, selected threads precede regular feed listings, ordinary
foreground reads, island browsing and background reads. Each class yields after two starts when a lower
class is ready, so sustained browsing cannot starve feeds or account actions. All
classes obey server cooldowns. Requests with known quota share a 100 ms minimum
between starts. Background requests additionally stay 500 ms apart; their spacing
deadline participates in eligibility. Unknown budgets retain one-second pacing and
conservative shared spacing. Native HTML/GraphQL and classic JSON track separate
quota budgets. A new budget starts with credit from the reported remaining quota;
responses preserve spent credit, and credit stays capped by remaining quota and
in-flight reservations. Adaptive refill remains no faster than once per second.
Background reads require credit from a 16-request burst, refilled at the reported
quota rate. Island browsing also spends that credit, preventing an additional
background burst during navigation, but listings can continue using actual quota.
Responses preserve spent burst credit. Both leave the final three known credits
for interactive reads, including post opens; fairness never borrows that reserve.
After two ready higher-priority starts, background work can use available burst
credit without waiting for its normal quota interval.
Visible, refreshed and prefetched islands use browsing priority. Regular feeds
retain listing priority. Startup subscription snapshots use background admission;
forced membership refreshes retain foreground priority.
Bulk island cache warming has a separate reader from viewport demand. Once every
visible island has usable post rows or a completed empty/error state, one following
grid row may fetch cold listings through the same admission rules. Lookahead uses
cached listings, including stale ones, without
revalidation; explicit refreshes and established islands wait until drawn. It does
not request offscreen icon metadata or media, and leaving the demand window cancels it.
New icon metadata requests also wait for visible rows. Enqueueing listings first
alone cannot guarantee admission order among concurrent workers.

Queued and admission-waiting obsolete reads cancel immediately. Active reads get
a 500 ms reuse window, checked every 50 ms; returning to the same unfinished post
joins that read. Shared image URLs remain wanted while any card or detail consumer
needs them. Visible loads poll results at most 16 ms apart. Transmitted writes finish
their acknowledgments; buffered decoding and external player streams do not use
this cancellation.

Feed demand follows the active tab and current request generations. Retired reads
resume their cursors and forced-refresh intent on a settled return. Fresh quota
hints replace older pacing estimates; replies do not replenish burst credit.
Ancestor moves from a post follow the destination's selected post. Cold destinations
show their list while loading; a pending follow belongs to the active tab and group
and is cancelled by unrelated input or a context change. Empty and failed results
remain in the list.
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
cache. Refreshes retain an existing page until its replacement completes. Cold All subs
islands also publish complete posts before the listing tail finishes; established
islands retain their current order until the complete refresh is accepted. Best Home
streams complete cards from the browser's ranked HTML, including media and flags,
without separate post lookups. An initial partial of up to three posts fetches one
continuation. Cards from that continuation publish after its cursor is validated.
Failed continuations keep existing content. Cold loads retain the starter and reloads retain the shown page; `r` retries failed loading. Failed combined loads are not cached.

Collection pages retain the 32 most recently used query/cursor responses in memory
for five minutes per connection. Fresh pages reopen without HTTP; stale first pages
remain readable during refresh, preserving selection and preview scroll. Stale
continuations wait for a complete answer. Explicit refreshes bypass cached pages;
writes, reconnects and content-policy changes clear them. Full thread links reuse
the ordinary post disk cache; focused comment trees never replace full-post snapshots.

Collection downward navigation prefetches near the last entry. Each gesture starts
at most one continuation; empty/filtered pages advance on further navigation.
Cursor history and continuation errors belong to their Browse screen. Failures
wait for `r`; repeated cursors stop. Appends retain selection/description position
and reject duplicate entries and abandoned generations.

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
First-use FFmpeg capability checks run off the UI thread alongside stream
resolution, keeping the same frame-rate filter and HTTP options. Loading feedback
lasts until the new decoder supplies a frame and requested audio is ready; a
retained frame alone does not mark playback ready.
The loading row stays outside image protocol cells, preserving their upload
commands and terminal diff flags even in a one-row visible slice.

The selected video owns playback. Up to three recent loaded players remain hidden
in the existing post/source preview cache, so returning at the same decoder size
and card/detail ownership resumes without extraction or playlist reloads. Hidden
players stop their clocks and audio; decoder output blocks on its bounded frame
channel. Unresolved players are cancelled on departure. The preview cache retains
at most eight frames/32 MiB; connection resets and disabling images clear it.
Failed players remain failed until explicit retry.

When a native HLS master returns403, inline playback tries that video's sibling
DASH manifest once. The bounded reader accepts static, whole-file MP4 renditions
and selects video at the existing display size and audio at the highest bitrate.
Every selected file uses the shared media transport and loopback relay; raw XML
never reaches FFmpeg. Cancellation retires the alternate request. Unsupported or
failed alternatives preserve the original HLS error and wait for explicit retry.

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
Each native hold closes through passthrough even when tmux does not render the
hidden pane's grid. Card encoding failures paint an error in the same frame.
