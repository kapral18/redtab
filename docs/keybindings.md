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
| `C-w h` / `C-w l` | Focus the pane to the left / right |
| `]s` / `[s` | Next / previous subreddit feed; community group in All subs |
| `]p` / `[p` | Next / previous post while keeping details open |
| `]]` / `[[` | Next / previous root comment thread, including from deep replies |
| `]i` / `[i` | Next / previous gallery image |
| `za` | Select, fold or unfold the current comment; load replies when selected |
| `/` | Search the community feed; find loaded comments in a post; global post search elsewhere |
| `r` | Reload; retry failed loading, including a failed continuation |
| `yy` | Copy the selected Reddit permalink |
| `q` | Close the current dialog or post; quit from the main feed |
| `Esc` / `Backspace` | Back; clear a feed search when applicable |
| `C-c` | Quit |

Arrows, Enter, Home/End and Page Up/Down remain terminal equivalents. Tab and
Shift+Tab switch feed tabs outside dialogs. Enter also activates a selected comment.
In narrow post view, `h` leaves the selected comment before closing the post.
List paging follows Card geometry; Compact and All subs move by ten posts.
Sidebar and menu paging move ten rows, or five for half pages. Collection dialogs
scroll descriptions by ten, five or one line. Downward navigation loads more results
near the end, including when a filtered page has no visible entries. Failed loads
wait for `r`; continued scrolling does not retry errors. Selection and descriptions
remain readable while more results load. Post paging follows the visible height.

Structural motions navigate the containing view without changing `j/k/h/l`.
In details, `]p`/`[p` follows the displayed post's position in the loaded feed,
bookmarks or All subs, skips filtered posts and empty groups, and keeps detail
focus in both narrow and split layouts. It stops at the ends; a directly opened
post absent from the loaded parent results stays open. `]s`/`[s` follows sidebar
order through joined and opened subreddit feeds, skipping user feeds and fixed
tabs. In All subs it moves between community groups. From a post, those keys and
`gt`/`gT` keep details focused on the selected post in the destination. A feed
or group still loading shows its list until a post is available; empty or failed
destinations stay in the list. Another reading action cancels the pending follow.
`[[` from a deep reply selects its root; another `[[` reaches the preceding root.
`]]` skips the remaining replies to the next root thread.

## Space groups

Only applicable actions appear. For example, gallery keys require a revealed
gallery, media controls require an available video, and post actions need a target.

| Group | Sequence | Action |
| --- | --- | --- |
| **a**ctions | `SPC a a` | Reddit content actions |
| | `SPC a b` | Toggle a local post bookmark |
| | `SPC a m` | Join/leave a community or follow/unfollow a person |
| **c**ommunity | `SPC c` | Tools for the current community or profile |
| **r**eddit | `SPC r` | Reddit collections, creation and native tools |
| **b**uffers | `SPC b b` | Choose a tab or open a subreddit / `u/name` |
| | `SPC b f` / `SPC b l` | First / last tab |
| | `SPC b d` | Close the current opened tab |
| **g**o | `SPC g h` / `SPC g b` / `SPC g a` | Home / local Bookmarks / All subs |
| | `SPC g p` / `SPC g r` | Popular / All Reddit |
| | `SPC g c` | Current source's community or user feed |
| | `SPC g u` | Enter a username to open its profile |
| **f**eed | `SPC f s` | Feed/search sort; comment tools in a post |
| | `SPC f t` | Cycle the feed/search time range |
| **s**earch | `SPC s p` / `SPC s c` / `SPC s u` | Search posts / communities / people |
| | `SPC s e` | Search and explore menu (`F3` also works) |
| **o**pen | `SPC o u` | Paste a Reddit URL to open inside redtab |
| | `SPC o l` | Pick a link from the body to open or copy |
| | `SPC o r` | Current post, comment or reading page on Reddit in browser |
| | `SPC o c` | Current community/profile in browser |
| | `SPC o m` | Original image, external video player or YouTube/browser fallback |
| **m**edia | `SPC m p` | Play / pause / retry video |
| | `SPC m m` | Toggle mute |
| | `SPC m h` / `SPC m l` | Seek backward / forward five seconds |
| **v**iew | `SPC v i` | Toggle image/video previews |
| | `SPC v c` | Toggle Card / Compact view |
| | `SPC v b` | Toggle sidebar |
| | `SPC v s` | Reveal / hide spoilers |
| **p**references | `SPC p s` | Settings (`F2` also works) |
| | `SPC p a` | Accounts |

`SPC g c`, `SPC c`, membership and source-browser actions capture the focused
community or profile. In post details they use the post's actual source. Local
bookmarks and Reddit saves are separate actions.

## Menu mnemonics

Menu rows display their shortcuts. Type a displayed shortcut, or select with
`j/k`, `gg/G`, paging and `l`/Enter. Multi-key shortcuts open the same sequence
hints as Space groups. `h`, `q` or Esc goes back. A mnemonic opens a deletion
confirmation; Enter is still required to perform the deletion.

| Menu | Keys and actions |
| --- | --- |
| Search and explore | `sp` posts, `sc` communities, `su` people; `gc` discover communities, `gp` Popular, `ga` All Reddit |
| Reddit: account | `ap` my profile; `as` saved, `ah` hidden, `au` upvoted, `ad` downvoted; `af` native following/favorites |
| Reddit: notifications | `nu` unread, `nc` comment replies, `np` post replies, `nm` mentions, `ni` inbox; `nn` native notifications |
| Reddit: search/go | `sp/sc/su` posts/communities/people; `sC` native comment search, `sm` native media search; `gu` enter username; `gf` custom feeds, `gc` discovery, `gp` Popular, `ga` All |
| Reddit: create | `ct` text post, `cl` link post, `cf` private custom feed, `cm` native upload/poll/AMA |
| Reddit: native tools | `mc` chat; `pp` privacy/blocks/profile, `pn` notification settings; `mq` moderation queue, `mm` mod mail |
| Content: write | `u/d` upvote/downvote or remove current vote; `s` save/unsave; `r` reply; `R` report; `e` edit own content; `D` delete own content |
| Content: navigation | `a` author, `c` community/profile info, `x` original crosspost |
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

## Text, accounts and settings

| Context | Keys |
| --- | --- |
| All text input | Literal letters/spaces and paste; Backspace, `C-u` clear, `C-w` or Alt+Backspace delete the preceding word; Esc cancel/back |
| Forms/settings editor | Left/Right, Home/End or `C-a/e` move the caret; Delete removes the next character |
| Multiple fields | Tab / Shift+Tab next / previous field |
| Single-line form | Enter advances or submits; `C-s` or Alt+Enter submits the form |
| Multiline form | Enter adds a newline; `C-s` or Alt+Enter submits |
| Tab picker | Up/Down, `C-p/n`, `C-k/j` or Tab selects results; Enter opens |
| Accounts | `a` add, `r` reconnect/rescan, `d` remove saved account; `l`/Enter selects; removal and imported-account saving require Enter confirmation |
| Settings list | `l`/Enter toggles a boolean or edits a value; `r` refreshes Reddit preferences/retries a file save; `u` reverts unsaved file values |
| Settings editor | Enter or `C-s` applies the value; Esc cancels the draft |
| Managed-config confirmation | Enter or `y` approves the displayed chezmoi save; Esc/`n`/`q` keeps session changes; navigation only scrolls |
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
[Context dispatch](../src/keys.rs) preserves text editing and confirmation guards;
[menu definitions](../src/app/experience.rs) supply the displayed mnemonics.
