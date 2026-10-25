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

## Request scheduling

Selected threads, active feeds and main images have separate worker capacity from
account work, community listings and other media. Cache readers run independently of HTTP.
At authenticated admission, selected threads precede visible listings, ordinary
foreground reads and background reads. Each class yields after two starts when a lower
class waits, so sustained browsing cannot starve feeds or account actions. All
classes obey the same base spacing and server cooldowns. Visible reads and actions
can use up to three quota credits, refilled at the reported quota rate and capped
by the remaining request count. Background work waits for its paced slot and cannot
spend those credits early. Startup subscription snapshots use background admission;
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
cache. Refreshes retain an existing page until its replacement completes. Home combines
an initial partial of up to three IDs with one continuation before JSON hydration,
saving the starter's separate details request. Hydration retains browser order.
Failed continuations keep existing content. Cold loads retain the starter with `L`
retry; reloads retain the shown page with `r` retry. Failed combined loads are not cached.

## Media rendering

Main media keeps its aspect ratio and fits the usable pane. Recent cards reuse
decoded pixels and prepared graphics; rapid navigation delays uncached preparation.
Kitty/Ghostty reuse uploaded strips across scrolling and popup clipping.

Images above 16 million pixels use validated Reddit resolutions, preferring the
smallest with a long edge of at least 1600 pixels, or the largest available below
it. Original URLs stay separate for media handoff and downloads. Still decoding uses a 128 MiB
allocation budget; oversized originals without a usable preview offer `o`.

Native video retains a 921,600-pixel budget, allocated proportionally for portrait
frames. Fast lossless PNG carries the RGB pixels. Prepared strips publish together;
alternating hidden Kitty buffers precede synchronized placeholder updates. The
hardware cursor stays hidden during playback; focused text fields keep their caret.
The [terminal writer](../src/terminal_output.rs) coordinates uploads and tmux holds.
