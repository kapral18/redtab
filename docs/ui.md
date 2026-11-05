# UI reference

[Development](development.md) · [User guide](guide.md)

Layout rules and inventory for redtab's terminal UI. Browser pages, OS dialogs
and terminal-emulator controls are outside this reference.

## Layout rules

- Use one cell of horizontal inner padding. Dialogs leave two outer columns and
  one outer row where space permits.
- Separate form fields, compact posts and expanded cards by one blank row.
  Community islands have two columns between cards and one row between card rows.
- Show partial islands and post entries at the bottom of the content area.
  Keep the selected entry fully visible when space allows.
- Size dialogs to content; scroll long menus, collections and help. Keep the
  primary action and Back/Cancel visible. Short pickers use one input row.
- Keep dialog bounds and list, preview and control regions stable while changing
  selection or active fields. Reserve space across the current entries; refit
  when data, filtering, dialog stage or terminal size changes.
- Collection footers show `Ctrl+d/u` for scrolling descriptions and `Ctrl+e/y`
  for single lines alongside Open and Back. Narrow layouts prioritize line scrolling.
- Keep main headers three rows high. Fit metadata to the available columns;
  failures and recovery keys take priority over counts and freshness.
- Use a full-width background, bold text and marker for selection. Distinguish
  unfocused selection and focused borders without relying on color.
- Accent marks only the focused pane. With two or more panes on screen (sidebar,
  list, open post, or the feed opened beside Search), a thin accent outline
  surrounds the focused pane, and the footer names it (`SIDEBAR`, `LIST`, `POST`)
  in the activity slot while nothing loads. Every pane keeps a one-cell inset for
  the outline, so moving focus never moves content; a lone pane uses those cells
  for content and has no outline. Inside, only the focused pane's selection uses
  the accent `▌`; an unfocused list or comment selection keeps its background with
  a grey `│`. Outside the sidebar's focus its active tab is a location, not a
  cursor: bold name and `●`, no row background. No pane is tinted or dimmed.
- Give the dark palette an explicit background and readable muted text.
  `NO_COLOR` retains markers and modifiers; the terminal supplies colors.
- Align labels and values, mark optional fields, and measure the same wrapped
  rows that are drawn. Keep the active label visible while long drafts scroll.
  Validation names and focuses the missing field. Edit by Unicode grapheme.
- Put NSFW/spoiler labels before long names. Compress deep comment indentation
  with a continuation marker. Cap fixed titles; keep the full title scrollable.
- Fit main media within the usable width and 60% of the content viewport height.
  Preserve aspect ratio and center narrow previews. Keep loading feedback inside
  the reserved media slot.
- Preserve input, caret, selection and existing results on errors. Keep controls
  visible in short dialogs; mark clipped titles/feedback with an ellipsis.
  Failed continuations retain loaded entries and offer continuation retry.

## Loading

Loaders share a spinner, spacing and muted palette within each kind. Labels name
the work. Use compact variants when the surrounding layout has only one row.

| Kind | Presentation and placement |
| --- | --- |
| Load more | Accent spinner and shimmering skeleton after posts or collection entries; a one-row spinner and rail for replies. Footer fallback when the append position is hidden or too small |
| Initial content | Centered spinner and label in the empty content area; compact in dialogs and community islands |
| Background refresh | Inline spinner beside the retained content's status or header |
| Media preparation | Centered spinner within the reserved image/video slot; inline for embedded pictures and playback status |
| Account or write action | Warm spinner with the specific action and its existing controls; pending writes retain drafts and block duplicate submission |

### Update rules

Scrolling moves content immediately. New scroll-triggered requests wait until
movement stops for 200 ms, across lists, cards, islands, comments and collection
dialogs. Existing pixels and incoming updates remain visible during that wait.
Explicit opens and refreshes start immediately.
In Compact and Card feeds, navigation requests the next batch when the bottom of
the view enters a five-item buffer before the loaded end.

Every list and the post page follow the same rules when data arrives:

1. **Only new items announce themselves.** New items lead the list: when a background
   refresh of a feed's first page or an All subs island (including the refetch of a
   shown one whose posts are five minutes old) ranks an unseen item above
   every item already shown (pinned posts aside), every unseen item in that answer
   waits behind **New posts · r refresh**, so `r` always finds them at the top.
   While an open post has focus, `r` refreshes that post and the list shows only
   **New posts**.
   Everything else applies as it arrives, with no notice: scores, comment counts,
   text, media URLs, reordering, removals, later pages, and unseen items that enter
   lower in the ranking. While an answer waits, the posts it drops stay until `r`, or
   until a refetched next page shows they left the listing, so nothing shown
   disappears just because new posts arrived; an All subs island also keeps its
   order until `r`. If a later page, a filter or a removal leaves nothing new leading,
   the waiting answer applies like any other update and the notice clears.
   In All subs, acknowledging the notice applies the downloaded updates without
   forcing subscription or island reloads. With no notice, `r` fetches fresh content or retries.
   Switching into a feed or All subs applies posts already waiting before entry.
   Answers arriving afterward leave new leading posts behind the notice. A cached
   list stays in its displayed order during its initial live refresh; matching posts
   still receive updated scores and text. Following the same open post between
   these views keeps arrivals waiting.
2. **Arrivals never move what you are reading.** Only scrolling moves the selection.
   Updates keep the selected item and its screen position: a reorder, a picture
   loading, or a resize moves the view to keep it. Where the selection is when an
   answer arrives decides two cases:
   - An untouched top, the first row of a list or island that you have not
     scrolled into and whose post is not open from it, stays on the list's first
     post, whatever the answer puts there.
   - The post being read, the open post or any other selection, stays in its row
     when a background answer to the shown tab drops it, and leaves with the first
     update after you move off it. Hidden tabs and `r` show the list as Reddit
     returns it, so a selection their answer drops hands its row to the post now
     there.

   Reddit lists (`SPC r`) scroll with their selection, so they keep the selected
   entry and its row follows it.
3. **`r` shows new items from the top.** When `r` applies waiting items, or its fetch
   brings new ones, the view goes to the top of the list or page. When nothing new
   arrives, `r` keeps your position. Moving, scrolling or opening a post before the
   answer arrives cancels the jump: new list items then wait behind the notice, and
   new comments on the post page arrive in place.
4. **Older answers never replace newer ones.** Cached or late responses that are
   provably older than what is shown are ignored.

| Surface | New items | Other updates | `r` |
| --- | --- | --- | --- |
| Home, community, user and search feeds | First-page answers led by unseen posts hold every unseen post; a feed showing nothing takes its answer at once. Unseen posts entering lower, revalidated later pages and pagination appear at once. In community, user and search feeds, rows missing from a page's new answer may have moved onto the next page, so they stay just below that page until the next page answers. Any page keeps them while a next page is loaded or requested, which is then fetched again from the new end at once; otherwise the first page keeps them only while its answer waits behind the notice, and a later page keeps them until the next page loads as you scroll. Kept rows that the next page does not contain then leave. Home pages are separate personalized batches: a row Home drops leaves at once, or once the answer waiting behind the notice applies | Live; selection and screen row kept. An untouched top stays on the first post; the post being read stays in its row when a background answer drops it | Applies waiting posts, or fetches; jumps to the top only when new posts arrive. Switching into the feed applies posts already waiting; later answers leave new posts behind the notice. Retrying a failed next page keeps position |
| All subs islands | Answers led by unseen posts hold every unseen post, and the island keeps its posts in their order until `r`, updating their scores and text; an island showing nothing (empty, refused, emptied by a filter or newly denied access) takes its answer at once. Cold islands show complete post records before the response finishes; established islands keep their current records until completion | Live. Every island reserves room for all the posts it may show, so loading never resizes the grid, and the selected island keeps its row when islands load or join above it. An untouched island top stays on the island's first post; the post being read keeps its slot when a background answer reranks or drops it | Applies waiting posts, then jumps to the top. A refetch that brings new posts before you move also jumps to the top. Switching into All subs applies posts already waiting; later answers leave new posts behind the notice |
| Bookmarks | Local; saves merged from another instance appear at once | Selection and screen row follow the bookmark; an untouched top stays on the first bookmark. A bookmark removed elsewhere leaves at once and its row goes to the next bookmark | — |
| Reddit lists (`SPC r`: profile, saved, hidden, votes, inbox, custom feeds, communities) | Recent pages reopen from memory; stale first pages remain readable while refreshing | After an action, the list is read again and keeps the selected entry and its preview scroll; an entry beyond the first page, or one the action removed, keeps its place within the first page | Reads the list again, keeping the selected entry; a failed next page retries, keeping position |
| Post page | Comments arrive live when the page opens or after your own writes; there is no background polling | Live, anchored to the line or comment being read, including header rewraps as ages change and loads that land after a fold | Jumps to the top only when an unseen comment comes before every comment you have seen (seen pinned comments are skipped); unseen replies and lower comments arrive in place |
| Sidebar | Subscriptions sync in place; a spinner on the section header shows progress without shifting the listed tabs. Loading rows appear only before the first subscription list arrives | — | `r` in All subs resyncs |

Writes such as hide, report and delete remove the post from every feed and island
(local bookmarks keep it), keep it out of listings for the rest of the session unless
you unhide it, and mark it read, so it leaves unread counts; a hidden post also stays
hidden on Reddit until you unhide it. A refetch after a write follows rule 1: its new
posts wait behind the notice. Unread counts cover every post observed in a community
that you have not read and your filters allow, including pinned posts, posts beyond an
island's preview and posts waiting behind the notice.

All subs warms at most the following grid row of cold islands once the visible
islands have readable rows or completed empty/error states. New icon metadata
requests follow the same readiness rule. Lookahead reuses stale cached listings without refreshing them;
established islands and explicit refreshes wait until visible. Icons and media
follow actual visibility, and obsolete lookahead requests cancel on navigation.

Animation advances at most every 100 ms while the active view or sidebar is busy. Help and
static overlays pause it; busy dialogs animate their own progress. Completion or
failure stops it. Redraws reuse cached media.

Visible videos load posters or first-frame stills independently of selection and
autoplay. Selected playback supplies its own frame rather than starting a second
preview decode. Selected cards and detail reserve a text row inside the media slot
for animated loading until the new playback frame and requested audio are ready.
Posters and retained frames keep the remaining space; full media geometry stays fixed.
Large native video frames are paced by encoded size to limit
terminal output; playback can skip frames while retaining pixel detail and media time.
A background output owner commits one complete terminal frame at a time; input and
application state continue updating while it writes. Exit and panic restoration wait
for the outstanding frame before restoring terminal modes.

Ordinary video-frame preparation does not show image-loading activity. Footer
progress uses a reserved slot beside the key hints, so starting or finishing work
does not move them or change which hints fit. Narrow slots use compact labels or
just the spinner. Errors and connection/membership
feedback use the available message width; help stays visible.

## Main views

Rendering: [main UI](../src/ui.rs).

| Surface | Fields and layout |
| --- | --- |
| Frame | Sidebar, list, retained detail, focus outline, footer and overlays; split/fullscreen follows width |
| Sidebar identity | Name and version, separate from navigation |
| Sidebar navigation | Home, Bookmarks, All subs and Search stay fixed above scrolling Subscriptions, Following and Opened sections. Very short sidebars prioritize the selected tab when all fixed entries cannot fit |
| Search tab | One bar across the tab, above the list and the preview: query box (accent while typing; empty, it names the kind typing searches) with Posts/Communities/People tabs at its right (below it when the tab is under 66 columns wide). The searched kind is accent and underlined; in Explore, which shows no kind's results, the chosen kind is only bold. Under the bar one title line: "<Kind> matching “q”", or "Explore" with "popular feeds and suggested communities" when that fits whole. The footer leads with `/ type` and `Tab kind`; while typing it shows `↑↓ choose`, `Tab kind` and `⏎/Esc results`; Enter runs a pending query and leaves typing without opening a row, and the sidebar offers `Esc list keys` and `F1 help`. Posts then show a line of sort, time and freshness chips and the feed cards. Communities/People and Explore (Popular, All Reddit, suggested communities) show spaced entries with a one-cell gap after the community icon, the person's avatar or the glyph, beside a wrapped name and a dimmed summary of up to three lines: a community's description, or a person's karma. Summaries shorten with an ellipsis and fit the pane height. Explore fills the pane and prefetches another page when the loaded end is within one API page beyond the viewport. Pages load sequentially and failures wait for retry. Scrolling keeps its viewport until selection crosses an edge, including when moving upward. Loading or next-page status stays below the entries. Failed discovery/pages show the actual error and a separate retry row; typing requires Esc then r, the list uses r, and a focused detail requires C-w h then r to retry its visible feed. When needed, empty-feed search/sort/source and membership/access hints name the list-focus sequence. Sidebar commands keep their active feed; clearing its local filter uses Esc then Esc. Retained detail advertises retry, spoiler, gallery, media and reply activation keys only while it owns input (spoiler galleries advertise browsing only after reveal); its document cache updates those cues with focus. The selected row's hot posts show as feed cards in the Card/Compact view with pictures and no selection marker (hide filters applied): in a right column where a post would open beside the list (the same widths as a feed and its post), hidden when narrower. A person's preview starts with their profile summary: the avatar beside the post and comment karma line, then their description, at most a third of the pane, above their posts. The preview reuses a matching loaded Hot feed or cached posts, updates fields in place and holds changed membership or order until `r`. Only unseen leading posts show the refresh notice; metadata edits, reranking and lower-ranked additions do not. Status belongs to the selected source. Opening a matching feed applies waiting preview posts; returning reconciles its newer snapshot and pending posts, preserving other queries and newer failures. A failed refresh keeps the displayed posts with a failure marker and the retry keys for the current focus. Failures name Esc then r while typing and r from the list. With Search results focused, "No matches" names `Tab` when it changes kind, and an empty preview names Enter; other focuses direct the reader back to Search. `l`/`Enter` opens the selected community or person as an active feed. At split widths the sidebar stays in place while the content pair advances from community rows and posts to post list and post detail. `j`/`k` acts on the focused list; only that list has the active selection marker. Source rows use ten-item pages, five-item half pages and one-item `C-e`/`C-y` motions. `gc` and source-menu feed commands retain the same Search return path as Enter. `o`/`SPC o c`/`yy` target the selected source URL, including Popular and All; membership stays unavailable for those two feeds. `C-w h`/`C-w l` follow visible panes, activating the post preview when focused. Escape applies ordinary detail/filter rules before returning a nested feed to Search. Inactive parent rows direct retry/query changes back to Search instead of advertising keys that act on the child feed. The first `h`/Left focuses the post list while keeping detail visible. The second closes detail and shifts the pair back to community rows and posts, keeping focus on posts. The third focuses Search rows with query, kind and list position preserved. Narrow layouts show one content layer at a time. Explicit tab switching ends that return path; `F3` returns to typing in Search |
| Sidebar tab | Number, community icon or user avatar/person glyph, name, community unread count (`?` when unknown), selection |
| Sidebar status | Reddit up/unknown/paused/down/signed out; images on/off; signed out offers `SPC p a` (Esc first while typing in Search), when it fits |
| Home/community/user header | Title, Card/Compact, sort, eligible time range; a search shows its query, search sort and time range; stable title/state rows. A user feed then shows the person's profile summary above the posts in a fixed block of three rows (at most a third of the pane): the avatar beside the post and comment karma line, then up to two description lines. People results supply it; otherwise the tab asks Reddit once, and again after `r` on the tab or a Show NSFW change. The block reads "Loading profile…" until it arrives and "Profile unavailable" with the retry key after a failure, so the list never moves. With NSFW hidden, an NSFW profile shows only its karma. The sidebar uses Following's avatar, or the profile's when Following has none or it failed |
| Membership | User feeds show Following/Not following/unknown beside `SPC m unfollow`/`SPC m follow`/`SPC m check`; communities show Joined/Not joined/unknown and leave/join/check. Both use confirmation, progress and automatic sidebar reconciliation |
| Feed activity | Unread count, loading, refresh/continuation error and freshness; refresh errors retain posts |
| Compact post | Two rows: selection, thumbnail, score, unread/bookmark/pin, title; then comments, safety labels, community, author, age, flair and media/domain |
| Expanded card | Metadata, wrapped title, media, score and comment count; stable loading geometry and continuous scrolling |
| Media indicator | Centered play/link symbol, VIDEO/GIF badge, domain or gallery count; dark backing; play symbol hides during playback |
| List boundary | Shared load-more loader in Card/Compact, outside real-post geometry; failures offer `r` retry |
| Scrollbar | Visible thumb and track, separate from text |
| All subs header | Community/unread/unknown/loading counts, subscription sync, error and freshness |
| Community island | Icon, name, unread badge, freshness, refresh error and focus border; 1–3 columns; lower-border retry |
| Island post | Compact post fields; clipping keeps the selection visible |
| Bookmarks | Count, view mode, stored posts, selection and removal; label as local bookmarks |
| Detail header | Community, author, age, flair, bookmark, NSFW/spoiler, bounded title, score and comment count |
| Structural navigation | `[[`/`]]` jumps root threads; `[p`/`]p` changes posts; `[s`/`]s` changes subreddit feeds or All subs groups; `gt`/`gT` changes tabs. A post follows the destination's selected post when available |
| Detail media metadata | Gallery index/keys, URL or media/domain, open-media hint; freshness on border |
| Post document | Selftext, quotes/code, polls, crossposts, gallery captions, main image and comment divider |
| Comment identity | Selection, depth rails, author, OP/MOD, flair, score, known age and folded count; wrap metadata |
| Comment body | Full text, folded preview, spoilers and inline image; selection marker spans content |
| More replies | Optional count, singular/plural label, Enter and loading state; preserve expansion anchors |
| Footer | Errors/success, membership and connection state before activity; rate-limit countdown with sanitized operation/status/quota; whole applicable key/action pairs; help stays visible |

## Reddit text

Post bodies, comments, messages, search previews, community descriptions, rules,
wiki pages and custom-feed descriptions share formatted text rendering. Bold,
italic, strikethrough, headings, code, quotes, numbered/nested lists, tables and
links retain their meaning when wrapped. Links show styled labels; `gx` opens a
link picker for the selected comment, post body or dialog text. Select a label
with `j`/`k` and `Enter`, then open its destination in the browser or copy its URL.
Raw URLs remain visible. Superscript
uses a caret because terminal cells have no smaller baseline. Spoilers remain
masked until revealed. Titles and Markdown editing fields stay literal.

## Media states

Images distinguish loading, preparing, ready, failed, disabled and unusable video
poster. A failed thumbnail keeps its slot; a failed community icon uses a glyph.
Failures do not trigger a retry on every draw.

Popups cover only their rectangle. Card/detail strips stay visible outside it;
stateful thumbnails and embedded pictures wait only when they intersect it.
A modal pauses video and retains its background frame. Warm graphics are reused;
see [rendering](development.md#media-rendering) for transport details.

Video/GIF state includes playback, loading/error, pause, position/duration and
sound. One active player serves the settled selected card, or detail when open.
If Reddit denies a native HLS master, playback tries the same video's DASH
video and audio files before showing the error.
Up to three recent loaded players stay hidden for reuse, with clocks and audio
stopped; older entries keep only bounded preview frames and playback position.
Geometry stays stable while loading. A failed video without a usable poster or
frame collapses to one status row; `p` retries with its normal media area.
Posters and retained frames remain visible, with controls in the footer. Turning off **Autoplay videos** in F2 requires `p` to start.

Unread counts share local observation/read history across sidebar, Jump, feeds and
islands. Only an accepted, revealed post document drawn outside overlays records a
read; loading/error screens and sidebar focus do not. The sidebar updates in that
frame; other list labels receive one follow-up redraw. `SPC c` captures the relevant
community or user for its tools; local bulk-read applies to communities. `SPC o c` opens
the source directly. Empty successful listings
establish known zero; unfetched/failed listings remain unknown. See the
[reading-history limits](usage.md#feeds-and-tabs).

## Form fields

Form types: [app forms](../src/app/experience/types.rs). Per-form fields: [experience actions](../src/app/experience/). Validation: [form submit](../src/app/experience/input.rs).
Layout and editing: [form renderer](../src/ui/experience/form.rs).
Required fields below describe client validation.

| Form | Fields and defaults | Submission |
| --- | --- | --- |
| Open Reddit URL (`SPC o u`) | Reddit URL; required, empty, single line | Open post/comment, community or user feed; invalid links retain input |
| User profile | Username; required, empty; accepts `u/` | Open |
| Reply | Reply; required, empty Markdown body | Reply |
| Edit | Body; required Markdown, initialized from raw content | Save to captured target |
| Report | Reason; required, empty, single line, up to 100 characters | Report |
| Text post | Subreddit (current community or empty), Title (empty), Body (empty Markdown); all required | Post |
| Link post | Subreddit (current community or empty), Title, URL; all required; HTTP(S) URL without credentials | Post |
| Private custom feed | Name (required), Description (optional multiline); both empty | Create |
| Add community to feed | Subreddit; required, empty | Add to captured owner/feed |
| Remove community from feed | Subreddit; required, empty | Remove from captured owner/feed |
| Community wiki | Page; required, defaults to `index` | Open in captured community |
| Find loaded comments | Text or author; required, empty | Search loaded comments |
| Jump | Single-line tab/subreddit query; empty lists tabs | Open selected result; show no matches explicitly |
| Subreddit search | Single-line query for named community | Search; `Esc` cancels |

Arrows, Home/End, Backspace/Delete, `Ctrl+u`, `Ctrl+w`, `Alt+Backspace` and paste edit text.
`Tab`/`Shift+Tab` switch fields when there is more than one. Single-line `Enter`
advances or submits; multiline `Enter` adds a newline. `Ctrl+s`/`Alt+Enter` submit.
Pending writes keep the draft and prevent duplicate submission or cancellation.

## Menus

Definitions: [app menus](../src/app/experience/menus.rs).
Presentation: [menu renderer](../src/ui/experience/menu.rs).

| Menu/group | Choices |
| --- | --- |
| Reddit / Notifications | Unread, comment replies, post replies, mentions, inbox, native notifications |
| Reddit / Messages | Chat on Reddit |
| Reddit / Your account | Profile, saved, hidden, upvoted, downvoted; connected accounts only |
| Reddit / Search | Posts, communities, people, user profile |
| Reddit / Explore | Custom feeds, discovery, popular, all Reddit |
| Reddit / Create | Text post, link post, private custom feed |
| Reddit / On Reddit | Comment/media search, upload/poll/AMA, following/favorites, privacy/blocks/profile settings, notification settings, moderation queue, mod mail |
| Content / Reply and edit | Reply, edit own content when editable |
| Content / Vote and save | Upvote/downvote or remove vote, save/unsave; labels follow state |
| Content / Notifications | Mark read/unread for inbox items |
| Content / Post controls | Hide/unhide, follow/unfollow post notifications; posts only |
| Content / Related | Author, community, original crosspost when present |
| Content / Report | Report |
| Content / On Reddit | Content/polls/awards, crosspost/notifications, author follow/block/chat, community mute/favorite/flair, moderation |
| Content / Destructive | Delete own content; last, with confirmation |
| Message actions | Mark read/unread, native notifications |
| Community / Community | Open feed, open community in browser, mark all fetched posts read |
| Community / About | About/sidebar, rules |
| Community / Wiki | Wiki index, page |
| Community / Moderators | Moderator list |
| Community / On Reddit | Message moderators, mute/favorites/notifications/flair, moderation queue, mod mail |
| Comments / Find | Find loaded comments |
| Comments / Sort | Best, Top, New, Old, Controversial, Q&A |
| Feed sort | Home: Best, Hot, New, Top, Rising, Controversial; communities omit Best |

Pickers show the selected row, available letter shortcuts and Enter/Esc controls.
Long menus show selection index/count. Home explains recommendations. Native choices name the destination and leave the
workflow to Reddit. Combine equivalent author links; omit destinations for
invalid/deleted authors.

## Dialogs and collections

Models: [Reddit pages](../src/reddit/experience/types.rs).
Presentation: [dialogs](../src/ui/experience.rs).

| Surface | Contents and behavior |
| --- | --- |
| Jump/search | Query and caret, selected result with icon/badge, no-match state, Enter/Esc and movement |
| Join/Leave / Follow/Unfollow | Captured community or user, confirm/cancel, `o` to open its feed |
| Content/feed deletion | Named target, irreversible Reddit consequence, confirm/cancel |
| Loading read | Action/page, destination, pending state, cancel/back and native fallback; continuation reads retain reading navigation while activation waits |
| Failed first read | Error, `r` retry, `Esc` back, `Ctrl+o` native fallback |
| Failed continuation | Retain entries/selection and the failed continuation; `r` retries it |
| Pending/successful write | Sending state blocks duplicate actions; update only after acknowledgment |
| Profile | Username, karma/description, Overview/Posts/Comments |
| User feed | Following tabs show submitted posts in Card/Compact with community metadata, sorting and pagination. `SPC m` and the first profile tool follow/unfollow; unfollow moves the retained feed to Opened. Post details target the actual community/profile |
| Inbox | Category, entries, read/unread actions, preview and pagination |
| Community | Title, members, description/sidebar, Rules and Wiki |
| Rules/wiki | Scrollable text; missing wiki says unavailable; no entry/action/paging hints |
| Moderators | Usernames and profile links; no content-action hint without a target |
| Custom feeds | Owner/name, description, listing; add/remove/delete on the feed itself |
| Comment matches | Author/ID, body preview and navigation; search is explicitly loaded-only |
| Empty subscriptions/bookmarks | Distinguish disconnected, no joined communities and no bookmarks; offer relevant connect/reload/jump guidance |
| Post | Loading/error, failed refresh of existing content, spoilers, empty/folded/loaded/loading comments |

Empty feeds distinguish sign-out, loading, read failure, private access, filtering
and no results/posts. Only an explicit private reason restricts a community.
Generic HTTP 403 remains a request failure: cached posts stay readable, and `r`
retries; it does not establish a community restriction or a Reddit outage. Private
access offers `o` to Reddit and `r` to recheck. Private cached posts stay unselectable.
Tabs and All subs share access observations; only a sufficiently newer success
clears a denial. Successful empty lists with NSFW off suggest `F2` → **Show NSFW**
without claiming it caused the empty result. Keep this guidance compact in islands
and include it when sizing content dialogs.

## Accounts

Presentation: [auth UI](../src/ui/auth_dialog.rs). Lifecycle: [controller](../src/auth/controller.rs).

| Phase | Visible data and controls |
| --- | --- |
| Idle | Source guidance, available choices, rescan/back |
| LoadingAccounts | Saved-account loading, cancel |
| Accounts | Names, active/expired state, selection, Add; switch, reconnect/rescan, remove saved row, close |
| Switching | Verification, cancel; failed switch keeps connection |
| Discovering | Profile discovery, cancel, applicable saved-login removal |
| Choose | Browser/profile or separate-window choice, move/connect/rescan/cancel, OS-access disclosure; no connect hint without a source |
| Importing | Session read or separate-window login, OS-access guidance, cancel, applicable removal |
| Confirm | Verified username, save or keep connection; confirmation controls |
| Saving | Progress; cancellation unavailable |
| DisconnectConfirm | Named account, active/inactive consequence, saved-data/browser scope, confirm/cancel |
| Disconnecting | Removal progress; cancellation unavailable |

Reserve space for errors, selection and primary controls. Short dialogs prioritize
the account/target and a combined Enter/Esc row. Move crowded status badges below
the name. Hide credentials; selecting a source precedes importing it.

## Settings and help

[Settings](../src/ui/settings.rs) groups Reddit account preferences, local reader
rules for the named account, and device settings shared by all accounts;
the selection shows its local file key, or "Reddit account", and its description. Account values load
in the background and display unknown/error states until confirmed. Booleans toggle; other
values use a TOML editor. Keep editing, validation and save/conflict feedback
visible. See [Settings](configuration.md#save-status-and-recovery) for status and recovery.

Read-only managed configs offer a scrolling source/target/key-change confirmation.
Enter saves through chezmoi; Esc keeps session changes. Discovery/apply use the
Action loader and hold editing/reloads; help remains available. Saved requires
the applied values and protected permissions to match.

External changes apply live, deferred during Reddit writes. NSFW changes replace
content workers/cache namespace while preserving connection, reading state and
unsent composer fields/carets; prior read screens clear. Close comment search if
its backing post becomes hidden.

[Help](../src/ui/help.rs) puts the active view/action first. It follows account
phase, available collection entries/actions/pagination and form field count.
Descriptions wrap; scrolling help does not act on the covered view or draft.

The [CLI](../src/cli.rs) prints usage/version and argument errors; [startup](../src/event_loop.rs)
prints sign-in results and diagnostics. Both are plain terminal output.
