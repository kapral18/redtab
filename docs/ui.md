# UI reference

[Development](development.md) · [User guide](guide.md)

Layout rules and inventory for redtab's terminal UI. Browser pages, OS dialogs
and terminal-emulator controls are outside this reference.

## Layout rules

- Use one cell of horizontal inner padding. Dialogs leave two outer columns and
  one outer row where space permits.
- Separate form fields, compact posts and expanded cards by one blank row.
  Community islands have two columns between cards and one row between card rows.
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

Subscriptions and Following each show a loading row while the account list loads,
including before any entries arrive. Refresh keeps existing rows visible.

Animation advances at most every 100 ms while the active view or sidebar is busy. Help and
static overlays pause it; busy dialogs animate their own progress. Completion or
failure stops it. Redraws reuse cached media.

Ordinary video-frame preparation does not show image-loading activity. Footer
progress uses a reserved slot beside the key hints, so starting or finishing work
does not move them or change which hints fit. Narrow slots use compact labels or
just the spinner. Errors and connection/membership
feedback use the available message width; help stays visible.

## Main views

Rendering: [main UI](../src/ui.rs).

| Surface | Fields and layout |
| --- | --- |
| Frame | Sidebar, list, retained detail, focus divider, footer and overlays; split/fullscreen follows width |
| Sidebar identity | Name and version, separate from navigation |
| Sidebar navigation | Home, Bookmarks, All subs, Subscriptions, Following, Opened; selected tab stays visible |
| Sidebar tab | Number, community icon or user avatar/person glyph, name, community unread count (`?` when unknown), selection |
| Sidebar status | Reddit up/unknown/paused/down/signed out; images on/off; signed out offers `A` |
| Home/community/user header | Title, Card/Compact, sort, eligible time range, search; stable title/state rows |
| Membership | User feeds show Following/Not following/unknown beside `S unfollow`/`S follow`/`S check`; communities show Joined/Not joined/unknown and leave/join/check. Both use confirmation, progress and automatic sidebar reconciliation |
| Feed activity | Unread count, loading, refresh/continuation error and freshness; refresh errors retain posts |
| Compact post | Two rows: selection, thumbnail, score, unread/bookmark/pin, title; then comments, safety labels, community, author, age, flair and media/domain |
| Expanded card | Metadata, wrapped title, media, score and comment count; stable loading geometry and continuous scrolling |
| Media indicator | Centered play/link symbol, VIDEO/GIF badge, domain or gallery count; dark backing; play symbol hides during playback |
| List boundary | Shared load-more loader in Card/Compact, outside real-post geometry; failures offer `L` retry |
| Scrollbar | Visible thumb and track, separate from text |
| All subs header | Community/unread/unknown/loading counts, subscription sync, error and freshness |
| Community island | Icon, name, unread badge, freshness, refresh error and focus border; 1–3 columns; lower-border retry |
| Island post | Compact post fields; clipping keeps the selection visible |
| Bookmarks | Count, view mode, stored posts, selection and removal; label as local bookmarks |
| Detail header | Community, author, age, flair, bookmark, NSFW/spoiler, bounded title, score and comment count |
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
links retain their meaning when wrapped. Links show styled labels; `w` opens a
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
sound. One player serves the settled selected card, or detail when open. Keep
geometry stable without a poster; retain pause within the same item. Show status
beside the picture and controls in the footer. Failures keep the poster and
retry/media guidance. Turning off **Autoplay videos** in F2 requires `P` to start.

Unread counts share local observation/read history across sidebar, Jump, feeds and
islands. Only an accepted, revealed post document drawn outside overlays records a
read; loading/error screens and sidebar focus do not. The sidebar updates in that
frame; other list labels receive one follow-up redraw. `B` captures the relevant
community or user for its tools; local bulk-read applies to communities. `O` opens
the source directly. Empty successful listings
establish known zero; unfetched/failed listings remain unknown. See the
[reading-history limits](usage.md#feeds-and-tabs).

## Form fields

Definitions and validation: [app forms](../src/app/experience.rs).
Layout and editing: [form renderer](../src/ui/experience.rs).
Required fields below describe client validation.

| Form | Fields and defaults | Submission |
| --- | --- | --- |
| Open Reddit URL (`Ctrl+L`) | Reddit URL; required, empty, single line | Open post/comment, community or user feed; invalid links retain input |
| Search posts | Query; required, empty, single line | Search |
| Search communities | Query; required, empty, single line | Search |
| Search people | Query; required, empty, single line | Search |
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

Arrows, Home/End, Backspace/Delete, `Ctrl+u`, `Alt+Backspace` and paste edit text.
`Tab`/`Shift+Tab` switch fields when there is more than one. Single-line `Enter`
advances or submits; multiline `Enter` adds a newline. `Ctrl+s`/`Alt+Enter` submit.
Pending writes keep the draft and prevent duplicate submission or cancellation.

## Menus

Definitions: [app menus](../src/app/experience.rs).
Presentation: [menu renderer](../src/ui/experience.rs).

| Menu/group | Choices |
| --- | --- |
| Search and explore (`F3`; `/` outside community tabs) | Search posts, communities, people; discover communities; Popular; All Reddit. Results open posts or feeds directly. |
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

Models: [Reddit pages](../src/reddit/experience.rs).
Presentation: [dialogs](../src/ui/experience.rs).

| Surface | Contents and behavior |
| --- | --- |
| Jump/search | Query and caret, selected result with icon/badge, no-match state, Enter/Esc and movement |
| Join/Leave / Follow/Unfollow | Captured community or user, confirm/cancel, `Space` to open its feed |
| Content/feed deletion | Named target, irreversible Reddit consequence, confirm/cancel |
| Loading read | Action/page, destination, pending state, cancel/back and native fallback |
| Failed first read | Error, `r` retry, `Esc` back, `Ctrl+o` native fallback |
| Failed continuation | Retain entries/selection; `L` retries next page |
| Pending/successful write | Sending state blocks duplicate actions; update only after acknowledgment |
| Profile | Username, karma/description, Overview/Posts/Comments |
| User feed | Following tabs show submitted posts in Card/Compact with community metadata, sorting and pagination. `S` and the first profile tool follow/unfollow; unfollow moves the retained feed to Opened. Post details target the actual community/profile |
| Inbox | Category, entries, read/unread actions, preview and pagination |
| Search | Posts/Communities/People, query, supported sort/time, results, preview and pagination |
| Community | Title, members, description/sidebar, Rules and Wiki |
| Rules/wiki | Scrollable text; missing wiki says unavailable; no entry/action/paging hints |
| Moderators | Usernames and profile links; no content-action hint without a target |
| Custom feeds | Owner/name, description, listing; add/remove/delete on the feed itself |
| Comment matches | Author/ID, body preview and navigation; search is explicitly loaded-only |
| Empty subscriptions/bookmarks | Distinguish disconnected, no joined communities and no bookmarks; offer relevant connect/reload/jump guidance |
| Post | Loading/error, failed refresh of existing content, spoilers, empty/folded/loaded/loading comments |

Empty feeds distinguish sign-out, loading, read failure, private access, unknown
restriction, filtering and no results/posts. Only an explicit private reason labels
a community private; generic HTTP 403 does not imply a Reddit outage. Private
access offers `o` to Reddit and `r` to recheck. Denied cached posts stay unselectable.
Tabs and All subs share access observations; only a sufficiently newer success
clears a denial. Successful empty lists with NSFW off suggest `F2` → **Show NSFW**
without claiming it caused the empty result. Keep this guidance compact in islands
and include it when sizing content dialogs.

## Accounts

Presentation: [auth UI](../src/ui.rs). Lifecycle: [controller](../src/auth.rs).

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
the selection shows its exact local file key and description. Account values load
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

The [CLI](../src/main.rs) prints usage/version, argument errors, sign-in results
and diagnostics as plain terminal output.
