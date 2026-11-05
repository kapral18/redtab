# Keyboard reference

[User guide](guide.md) · [Using redtab](usage.md)

`SPC` means Space. Press sequence keys in order: `SPC p s` opens Settings.
`C-f` means Ctrl+f; uppercase letters require Shift. Buffers are redtab's feed tabs.

Press `Space` and pause for 500 ms to see available action groups. Each following
key narrows the menu. Other sequence hints use the same pause. There is no
sequence timeout. `Esc` cancels a pending sequence without leaving the view. An
invalid sequence cancels without running its last key as another command.
Changing tabs, accounts or dialogs clears a pending sequence. Paste is always
handled separately from commands. In text fields, Space and letters enter text.

`?` or `F1` opens help for the current context. Use `F1` while typing. Help covers
the current view and preserves drafts, selections and confirmations. Help and
sequence hints support paging in short terminals; dismissing help consumes the key.

## Navigation

| Keys | Behavior |
| --- | --- |
| `j` / `k` | Next / previous item, comment or sidebar tab |
| `h` / `l` | Back / open; focus left / right in the main reading panes |
| `gg` / `G` | First / last item; top / bottom of a document |
| `C-f` / `C-b` | Page down / up |
| `C-d` / `C-u` | Half page down / up |
| `C-e` / `C-y` | Scroll down / up one line |
| `gt` / `gT` | Next / previous feed tab |
| `C-o` / `C-i` | Older / newer location in the jump list |
| `C-w h` / `C-w l` | Focus the pane to the left / right |
| `]s` / `[s` | Next / previous Search source, subreddit feed or All subs community group |
| `]p` / `[p` | Next / previous post while keeping details open |
| `]]` / `[[` | Next / previous root comment thread, including from deep replies |
| `]i` / `[i` | Next / previous gallery image |
| `za` | Select, fold or unfold the current comment; load replies when selected |
| `/` | Search the community feed; find loaded comments in a post; type in the Search tab elsewhere |
| `r` | Reload; retry failed loading, including a failed continuation |
| `yy` | Copy the selected Reddit permalink |
| `q` | Close the current dialog or post, or a feed opened beside Search; quit from the main feed |
| `Esc` / `Backspace` | Back; clear a feed search when applicable. In the Search tab's query, `Esc` leaves it and `Backspace` deletes |
| `C-c` | Quit |

Arrows, Enter, Home/End and Page Up/Down remain terminal equivalents. Tab and
Shift+Tab switch feed tabs outside dialogs. Inside Search (its query, its results, a
post opened from them, or a feed opened beside them) they switch Posts / Communities /
People instead. The sidebar and pending post-follow navigation keep buffer switching. Enter also activates a selected comment.
In narrow post view, `h` leaves the selected comment before closing the post.
In wide Search, `l`/Enter keeps community rows beside the active post list.
Opening a post advances the pair to post list and post detail. The first `h`/Left
focuses the post list while keeping detail visible. The second closes detail and
shifts the pair back to communities and posts, keeping focus on posts. The third
focuses Search rows, preserving their selection and the feed position. Narrow
panes show one content layer at a time. Explicit tab switching or a connection
reset ends the Search return path; `C-w h` moves focus without shifting the pair.
`C-w h` / `C-w l` follow the visible panes, including community rows and their
post preview. Focusing that preview activates its feed. `Esc`/`Backspace` first
handle the current detail or feed filter, then return a nested feed to Search.
On source rows, `gc` and `SPC c f` open the same feed as `l`/Enter and retain that
return path. `o`, `SPC o c` and `yy` open or copy the selected source's Reddit URL;
Popular and All support these navigation keys but have no membership action.
List paging follows Card geometry. Compact, All subs and tiny Card panes move
ten posts for a full page, or five for a half page.
Search source rows, sidebar and menu paging move ten rows, or five for half pages.
`C-e`/`C-y` move one item in Search source rows, Compact lists and All subs. Collection dialogs
scroll descriptions by ten, five or one line. Downward navigation loads more results
near the end, including when a filtered page has no visible entries. Failed loads
wait for `r`; continued scrolling does not retry errors. Selection and descriptions
remain readable while more results load. Post paging follows the visible height.

The jump list works like Vim's. A location is a feed tab together with the post
focused in it, if any. Changing either records the previous location; moving
within a list, browsing the sidebar and dialogs do not. A tab change made through
the sidebar records one jump when focus returns to the main view. `C-o` from the
newest location first records the current one so `C-i` can return to it. Revisiting
a location moves it to the newest end, the list keeps 100 locations, and closed
tabs drop out. A post reopened by a jump starts at its top. Telling `C-i` apart
from Tab requires a terminal that implements the kitty keyboard protocol. Inside
tmux it also requires `extended-keys on` and `extended-keys-format csi-u` in the
tmux configuration, with a terminal that sends extended keys to tmux. Everywhere
else `C-i` sends Tab and does what Tab does. In dialogs and while typing, `C-i`
always does what Tab does.

Structural motions navigate the containing view without changing `j/k/h/l`.
In details, `]p`/`[p` follows the displayed post's position in the loaded feed,
bookmarks or All subs, skips filtered posts and empty groups, and keeps detail
focus in both narrow and split layouts. It stops at the ends; a directly opened
post absent from the loaded parent results stays open and has no post-motion keys.
In Explore, Communities and People, `]s`/`[s` follows the shown source rows. From
a nested feed or any depth in its post, it closes detail, returns to Search and
focuses the next/previous source row. People follows profiles. At either end it
still returns to the source rows, keeping the endpoint selected. Query fields and
dialogs keep their own keys. Elsewhere, `]s`/`[s` follows sidebar order through
joined and opened subreddit feeds, skipping user feeds and fixed tabs. In All subs
it moves between community groups. Outside Search source navigation, those keys
and `gt`/`gT` keep details focused on the selected post in the destination. A feed
or group still loading shows its list until a post is available; empty or failed
destinations stay in the list. Another reading action cancels the pending follow.
`[[` from a deep reply selects its root; another `[[` reaches the preceding root.
`]]` skips the remaining replies to the next root thread.
Pane-focus hints appear only when the requested destination exists; moving left
from the main list can reveal a hidden sidebar.
Direct `gh/gb/ga/gp/gr/gc` commands close detail and focus their destination list,
including when that feed is already active. Buffer motions keep their detail-follow rule.

## Direct verbs

Direct verbs act on the focused post or comment, so one key means the same action
everywhere. Only applicable keys work: media keys need an available video, gallery
and spoiler keys need a post with those elements.

| Keys | Behavior |
| --- | --- |
| `u` / `d` | Upvote / downvote; pressing again removes the vote |
| `s` | Save / unsave on Reddit |
| `c` | Reply to the focused post or comment |
| `b` | Toggle a local bookmark |
| `o` / `O` | Open on Reddit in the browser / open media externally (image, video player, YouTube fallback) |
| `gx` / `yl` | Pick a body link to open / yank (`o` opens, `y` copies) |
| `p` / `m` | Play, pause or retry / mute the video |
| `H` / `L` | Seek backward / forward five seconds |
| `zs` / `zi` / `zc` / `zb` | Spoilers / image and video previews / Card or Compact / sidebar |
| `gh` / `gb` / `ga` | Go Home / local Bookmarks / All subs |
| `gp` / `gr` | Go to Popular / All Reddit |
| `gc` / `gu` | Go to the current source feed / a username's feed, led by their profile |

Vote, save and reply load the target's current state first, then act; a vote
toggles from Reddit's state, not the card's.
Messages use `SPC a` for read/unread actions instead of post voting/saving verbs.
In a custom-feed collection, `d` removes a community; `u`, `s` and `c` still act
on the selected post. Opening that post restores the ordinary downvote key.
Video mute appears once a player exists, and seek appears when it is ready;
`p` remains available to start or retry playback.
After choosing a body link, `gx` preselects Open and `yl` preselects Copy URL.
Press Enter to run that selected operation, or use `o`/`y` explicitly.

## Space groups

The leader holds global and rare actions.

| Group | Sequence | Action |
| --- | --- | --- |
| **a**ctions | `SPC a` | Every Reddit content action, including report, hide, edit and delete |
| **m**embership | `SPC m` | Join/leave a community or follow/unfollow a person |
| **c**ommunity | `SPC c` | Tools for the current community or profile |
| **r**eddit | `SPC r` | Reddit collections, creation and native tools |
| **b**uffers | `SPC b b` | Choose a tab or open a subreddit / `u/name` |
| | `SPC b f` / `SPC b l` | First / last tab |
| | `SPC b d` | Close the current opened tab |
| **f**eed | `SPC f s` | Feed sort; cycles search sort in a search feed; comment tools in a post |
| | `SPC f t` | Cycle the feed/search time range |
| **s**earch | `SPC s p` / `SPC s c` / `SPC s u` | Search tab: type posts / communities / people |
| | `SPC s e` | Search tab: type a query, or browse Explore rows (`F3` also works) |
| **o**pen | `SPC o u` | Paste a Reddit URL to open inside redtab |
| | `SPC o c` | Current community/profile in browser |
| **p**references | `SPC p s` | Settings (`F2` also works) |
| | `SPC p a` | Accounts |

`gc`, `SPC c`, membership and source-browser actions capture the focused
community or profile. In post details they use the post's actual source. Local
bookmarks and Reddit saves are separate actions.

## Menu mnemonics

Menu rows display their shortcuts. Type a displayed shortcut, or select with
`j/k`, `gg/G`, paging and `l`/Enter. Multi-key shortcuts open the same sequence
hints as Space groups. `h`, `q` or Esc goes back. A mnemonic opens a deletion
confirmation; Enter is still required to perform the deletion.

| Menu | Keys and actions |
| --- | --- |
| Reddit: account | `ap` my profile; `as` saved, `ah` hidden, `au` upvoted, `ad` downvoted; `af` native following/favorites |
| Reddit: notifications | `nu` unread, `nc` comment replies, `np` post replies, `nm` mentions, `ni` inbox; `nn` native notifications |
| Reddit: search/go | `sp/sc/su` posts/communities/people; `sC` native comment search, `sm` native media search; `gu` enter username; `gf` custom feeds, `gc` discovery, `gp` Popular, `ga` All |
| Reddit: create | `ct` text post, `cl` link post, `cf` private custom feed, `cm` native upload/poll/AMA |
| Reddit: native tools | `mc` chat; `pp` privacy/blocks/profile, `pn` notification settings; `mq` moderation queue, `mm` mod mail |
| Content: write | `u/d` upvote/downvote or remove current vote; `s` save/unsave; `c` reply; `R` report; `e` edit own content; `D` delete own content |
| Content: navigation | `a` author, `i` community/profile info, `x` original crosspost |
| Content: post/notification | `H` hide/unhide; `f/F` follow/unfollow notifications; `nr/nu` mark notification read/unread |
| Content: native tools | `op` content/poll/awards, `oc` crosspost/notifications, `oa` author follow/block/chat, `os` source controls, `om` moderation |
| Message actions | `r` mark read, `u` mark unread, `o` native notifications |
| Community tools | `f` feed, `o` browser, `r` mark all fetched posts read; `a` about/sidebar, `R` rules; `wi/wp` wiki index/page; `mi` moderators, `mm` message moderators, `mq` mod queue, `mM` mod mail |
| Profile tools | `m` follow/unfollow, `f` submitted posts, `o` browser, `i` profile information |
| Comment tools | `/` find loaded comments; `b/t/n/o/c/Q` Best/Top/New/Old/Controversial/Q&A |
| Body links | Select a label with `j/k` and `l`/Enter; then `o` opens it, `y` copies its URL |
| Feed sort | `b/H/n/t/r/c` Best/Hot/New/Top/Rising/Controversial, where supported |
| Custom feed collection | `a` add a community, `d` remove one, `D` delete the feed after confirmation |

Account-owned actions appear when connected. Edit/delete appear only for owned
content. Native choices use the browser's signed-in account and name that handoff.
In Reddit reading dialogs and forms, `C-o` opens the corresponding native page.
In reading dialogs, `o` and `yy` use the selected item when it has a query or
permalink; otherwise they use the dialog page. Local comment-find `r` reruns the
loaded-comment search and preserves the selected match, without reading a remote thread.

## Text, accounts and settings

| Context | Keys |
| --- | --- |
| All text input | Literal letters/spaces and paste; Backspace, `C-u` clear, `C-w` or Alt+Backspace delete the preceding word; Esc cancel/back |
| Search query/forms/settings editor | Left/Right, Home/End or `C-a/e` move the caret; Delete removes the next character |
| Search query | Up/`C-p`/`C-k` selects the previous result; Down/`C-n`/`C-j` selects the next; Enter runs a pending query and leaves typing for the results, where Enter opens; Esc leaves typing |
| Multiple fields | Tab / Shift+Tab next / previous field |
| Single-line form | Enter advances or submits; `C-s` or Alt+Enter submits the form |
| Multiline form | Enter adds a newline; `C-s` or Alt+Enter submits |
| Tab picker | Up/Down, `C-p/n`, `C-k/j` or Tab selects results; Enter or Alt+Enter opens |
| Accounts | `a` add, `r` reconnect/rescan, `d` remove saved account; `l`/Enter selects; removal and imported-account saving require Enter confirmation |
| Settings list | `l`/Enter toggles a boolean or edits a value; `r` refreshes Reddit preferences/retries a file save; `u` reverts unsaved file values |
| Settings editor | Enter, Alt+Enter or `C-s` applies the value; Esc cancels the draft |
| Managed-config confirmation | Enter or `y` approves the displayed chezmoi save; Esc/`n`/`q`/`F2` keeps session changes; navigation only scrolls |
| Membership confirmation | Enter confirms; `o` opens the captured feed instead; Esc/`q` cancels |
| Pending read | Esc/`q` cancels/returns; continuation reads allow navigation, with activation held until completion; `r` retries a failed read; `C-o` opens the native page |
| Pending write | Draft/target stays protected; duplicate submission and cancellation are blocked until acknowledgment |

Unrelated Ctrl/Alt combinations do not become plain commands or confirmations.
Help never forwards dismissal, paste or submission keys to the covered view.

## Audit coverage

The action inventory covers feed/sidebar/buffer navigation, retained post/comment
reading, galleries/spoilers/video, search/explore, body links, every Reddit menu,
custom feeds, membership, account phases, settings editing and managed saves,
confirmations, pending reads/writes, help and text/paste input. Every interactive
action has a direct key or a displayed menu sequence. Startup flags and external
browser/OS controls keep their separate interfaces.

The dispatch and action hints share [binding definitions](../src/keys/map.rs).
[Context dispatch](../src/keys/dispatch.rs) preserves text editing and confirmation guards;
[menu definitions](../src/app/experience/menus.rs) supply the displayed mnemonics.
