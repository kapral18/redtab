# UI reference

[Development](development.md) · [User guide](guide.md)

Rules for redtab's terminal UI. Browser pages, OS dialogs and terminal settings are
out of scope.

## Layout

- Use one cell of inner padding. Dialogs keep two columns and one row of margin when
  space allows.
- Put one blank row between form fields, compact posts and cards. Islands have two
  columns between cards and one row between rows.
- Selection or focus changes never move dialog bounds, lists, previews or controls.
  Refit only when data, filters, dialog stage or terminal size change.
- Mark selection with a full-width background, bold text and a marker, never color
  alone. Keep the selected entry fully visible.
- With two or more panes, a thin accent outline marks the focused one and the footer
  names it (`SIDEBAR`, `LIST`, `POST`). Every pane keeps a one-cell inset for the
  outline, so focus never shifts content. A lone pane has no outline.
  - Focused selection: accent `▌`. Unfocused selection: background plus grey `│`.
  - The sidebar's active tab without focus: bold name and a thin `▏`, no background.
  - Focus never tints or dims a pane.
- Main headers are three rows. Errors and recovery keys win over counts and freshness.
- Dialogs size to content and scroll when long. The primary action and Back/Cancel
  stay visible.
- Images and video fit the pane width and 60% of its height, keep their aspect ratio
  and are centered when narrow. Link-card pictures take at most 30% of the height and
  sit beside the text from 48 columns. Loading feedback stays inside the reserved slot.
- NSFW/spoiler labels go before long names. Deep comment indents compress behind a
  continuation marker. Text edits by grapheme.
- Errors keep input, caret, selection and loaded results. Failed continuations keep
  entries and offer `r`.
- The dark palette sets an explicit background. `NO_COLOR` keeps markers and modifiers.

## Loading

| Kind | Look and place |
| --- | --- |
| Load more | Spinner and skeleton after the last entry; footer fallback when hidden |
| First load | Centered spinner and label; compact in dialogs and islands |
| Refresh | Inline spinner beside the header or status |
| Media | Spinner inside the reserved media slot |
| Account or write | Warm spinner naming the action; drafts stay, duplicates are blocked |

The footer reserves a slot for progress, so key hints never shift.
Animation runs at most every 100 ms and stops when work finishes.

## Update rules

New scroll-triggered requests wait until scrolling stops for 200 ms. Explicit opens
and refreshes start at once. Feeds request the next page five items before the end.

1. A refresh that ranks an unseen post above everything already shown (pinned posts
   aside) holds all unseen posts in that answer behind **New posts · r refresh**.
   Everything else applies live: scores, comment counts, text, media, order, removals,
   later pages and unseen posts lower down. While an answer waits, posts it drops stay
   put and an All subs island keeps its order.
2. Arrivals never move the selection. It keeps its item and screen row through
   reorders, image loads and resizes.
   - An untouched top (first row, not scrolled, no post open) stays on the first post.
   - The post being read stays when a background answer drops it, and leaves after you
     move off it.
3. When `r` applies waiting posts or fetches new ones, the view jumps to the top.
   Otherwise it keeps your place. Moving before the answer arrives cancels the jump.
4. A cached or late answer older than what is shown is ignored.

| Surface | Notes |
| --- | --- |
| Feeds | Rows missing from a page's new answer stay below it until the next page answers. Home drops them at once |
| All subs | Islands reserve room for every post they may show, so loading never resizes the grid |
| Bookmarks | Local; changes from another instance appear at once |
| Reddit lists (`SPC r`) | Reopen from memory; after an action the list rereads and keeps the selection |
| Post page | Comments load on open and after your writes; no background polling. `r` jumps to the top only for a new comment above all seen ones |
| Sidebar | Subscriptions sync in place with a spinner on the section header |

Hide, report and delete remove the post everywhere except bookmarks and mark it read.

## Main views

Rendering: [main UI](../src/ui.rs).

| Surface | Contents |
| --- | --- |
| Sidebar | Name and version; fixed Home, Bookmarks, All subs, Search; scrolling Subscriptions, Following, Opened |
| Sidebar tab | Number (first nine), icon or avatar, name, badge: unread count for communities (`?` when unknown, `99+` above 99), bookmark count for Bookmarks |
| Sidebar status | Reddit up / paused / down / signed out; images on/off |
| Feed header | Title, view, sort, time range; user feeds add a three-row profile block |
| Compact post | Row 1: thumbnail, score, flags, title. Row 2: comments, labels, community, author, age, flair, domain |
| Card | Metadata, title, media, score, comment count |
| Media badge | Play or link symbol, VIDEO/GIF badge, domain or gallery count |
| All subs | Header counts and sync state; islands with icon, name, unread badge, 1–3 columns |
| Post header | Community, author, age, flair, labels, title, score, comments |
| Comment | Depth rails, author, OP/MOD, flair, score, age, folded count, body |
| Footer | Errors and connection state first, then activity, then key hints; help always visible |

### Search tab

- One bar: query box, then Posts / Communities / Comments / Media / People (below the box
  under 84 columns; unpadded where the padded kinds do not fit).
- The active kind is accent and underlined; in Explore it is only bold.
- Title line: `<Kind> matching “q”`, or **Explore**.
- Posts and Media show sort/time chips and feed cards.
- Communities and People rows show an icon or avatar, name and up to three summary lines.
- Comments rows show the community icon, post title, then community, author, votes and
  the comment's first line.
- The selected row's hot posts preview on the right at split widths; hidden when narrower.
  A Comments row previews the whole comment instead.
- Errors show the actual message and a retry row.
- Footer while typing: `↑↓ choose`, `Tab kind`, `⏎/Esc results`.

## Text and media

- Bodies, comments, messages, rules, wiki pages and descriptions share one renderer:
  bold, italic, strikethrough, headings, code, quotes, lists, tables and links.
  Superscript uses `^`. Spoilers stay masked until revealed. Titles and editors stay literal.
- Image states are loading, preparing, ready, failed and disabled. A failed thumbnail
  keeps its slot; a failed icon becomes a glyph. Failures never retry on every draw.
- Popups cover only their rectangle. A modal pauses video.
- One player serves the selected card, or the open post.
- A failed video with no poster collapses to one status row; `p` retries.

## Forms

Types: [forms](../src/app/experience/types.rs). Required fields: [submit](../src/app/experience/input.rs).
Length and URL checks: [actions](../src/reddit/experience/actions.rs).
Rendering: [form](../src/ui/experience/form.rs).

| Form | Fields (all required unless noted) |
| --- | --- |
| Open Reddit URL | URL |
| User profile | Username (`u/` accepted) |
| Reply / Edit | Markdown body |
| Report | Reason, up to 100 characters |
| Text post | Subreddit, Title, Body |
| Link post | Subreddit, Title, HTTP(S) URL |
| Custom feed | Name; Description optional |
| Add / remove feed community | Subreddit |
| Wiki | Page, default `index` |
| Find comments | Text or author |

## Menus

Definitions: [menus](../src/app/experience/menus.rs). Rendering: [menu](../src/ui/experience/menu.rs).
Feed sorts: [tabs](../src/app/tabs.rs), drawn by [popups](../src/ui/popups.rs).

- Rows show their shortcut. Long menus show position and count.
- Native choices name reddit.com as the destination.
- Delete is last and always confirms.
- Feed sort: Home has Best, Hot, New, Top, Rising, Controversial. Communities omit
  Best. All Reddit has Hot, Top, Controversial. User feeds have New, Hot, Top,
  Controversial.

## Dialogs

Models: [Reddit pages](../src/reddit/experience/types.rs). Rendering: [dialogs](../src/ui/experience.rs).

- Loading dialogs show the action, destination, cancel and the `C-o` reddit.com fallback.
- A failed read shows the error, `r` retry and `Esc` back.
- Sending a write blocks duplicates; the UI updates only after Reddit acknowledges it.
- Confirmations name the target and the consequence.
- Empty feeds distinguish signed out, loading, failed, private, filtered and empty.
  A plain HTTP 403 is a failure, not a private community.

## Accounts

Rendering: [auth UI](../src/ui/auth_dialog.rs). Phases: [auth](../src/auth.rs). Flow: [controller](../src/auth/controller.rs).

| Phase | Shows |
| --- | --- |
| LoadingAccounts / Switching / Discovering | Progress |
| Accounts | Names, active/expired state, Add, switch, reconnect, remove |
| Choose | Browser profiles or separate window, OS-access note |
| Importing | Progress, cancel |
| Confirm | Verified username, save or keep current |
| Saving / Disconnecting | Progress; no cancel |
| DisconnectConfirm | Account and what removal deletes |

Credentials are never shown.

## Settings and help

- [Settings](../src/ui/settings.rs) shows the three groups, each value's file key or
  "Reddit account", and its description. Reddit values show Sign in, Reconnect, Loading… or Unavailable until loaded.
- [Help](../src/ui/help.rs) lists the active view's actions first. Scrolling help
  never acts on the view underneath.
- The [CLI](../src/cli.rs) and [startup](../src/event_loop.rs) print plain text.
