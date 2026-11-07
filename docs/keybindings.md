# Keyboard reference

[User guide](guide.md) · [Using redtab](usage.md)

## Notation

- `SPC` is Space. `SPC p s` means press Space, then `p`, then `s`.
- `C-f` is Ctrl+f. Uppercase letters need Shift.
- Pause for 500 ms after a prefix key to see what comes next. `Esc` cancels.
- `?` or `F1` opens help for the current view. Use `F1` while typing.
- In text fields, Space and letters type text.

## Navigation

| Keys | Action |
| --- | --- |
| `j` / `k` | Next / previous item |
| `h` / `l` | Back / open; move focus left / right |
| `gg` / `G` | First / last item |
| `C-f` / `C-b` | Page down / up |
| `C-d` / `C-u` | Half page down / up |
| `C-e` / `C-y` | One line down / up |
| `gt` / `gT` | Next / previous tab (also `Tab` / `Shift+Tab` outside Search and dialogs) |
| `C-o` / `C-i` | Back / forward in the jump list |
| `C-w h` / `C-w l` | Focus the pane to the left / right |
| `]s` / `[s` | Next / previous subreddit feed, All subs group, or Search result row |
| `]p` / `[p` | Next / previous post, staying in the post view |
| `]]` / `[[` | Next / previous top-level comment thread |
| `]i` / `[i` | Next / previous gallery image |
| `za` | Fold / unfold a comment, or load its replies |
| `/` | Search the community, find in comments, or type in Search |
| `r` | Reload or retry |
| `yy` | Copy the Reddit link |
| `q` | Close the dialog, post or right pane; quit when none is open |
| `Esc` / `Backspace` | Back; clear a feed search |
| `C-c` | Quit |

Arrows, Enter, Home/End and Page Up/Down also work.

`gt`, `gT`, `]s` and `[s` keep an open post open: you land on the destination's
selected post. From Search, `]s` and `[s` close the post and move to the next or
previous result row.

### Jump list

Works like Vim's. Each entry is a tab, plus the open post when the post pane has focus.
Moving inside a list does not add entries. It keeps the last 100 locations.

`C-i` differs from `Tab` only in terminals with the kitty keyboard protocol. In tmux
it also needs:

```tmux
set -g extended-keys on
set -g extended-keys-format csi-u
```

## Direct keys

These act on the selected post or comment.

| Keys | Action |
| --- | --- |
| `u` / `d` | Upvote / downvote; press again to remove the vote |
| `s` | Save / unsave on Reddit |
| `c` | Reply |
| `b` | Toggle a local bookmark |
| `o` / `O` | Open on Reddit / open media externally |
| `gx` / `yl` | Pick a link to open / copy |
| `p` / `m` | Play, pause or retry / mute |
| `H` / `L` | Seek back / forward five seconds |
| `zs` | Reveal / hide spoilers |
| `zi` | Image and video previews on / off |
| `zc` | Card / Compact view (not in All subs) |
| `zb` | Sidebar on / off |
| `gh` / `gb` / `ga` | Go to Home / Bookmarks / All subs |
| `gp` / `gr` | Go to Popular / All Reddit |
| `gc` / `gu` | Go to the current community or profile / a username |

In a custom-feed list, `d` removes a community instead of downvoting.

## Space groups

| Sequence | Action |
| --- | --- |
| `SPC a` | All actions for the selected post or comment |
| `SPC m` | Join / leave, or follow / unfollow |
| `SPC c` | Community or profile tools |
| `SPC r` | Reddit collections, creation and native tools |
| `SPC b b` | Choose a tab or open a subreddit / `u/name` |
| `SPC b f` / `SPC b l` | First / last tab |
| `SPC b d` | Close the current opened tab |
| `SPC f s` | Feed sort, search sort, or comment tools in a post |
| `SPC f t` | Time range |
| `SPC s p` / `SPC s c` / `SPC s C` / `SPC s m` / `SPC s u` | Search posts / communities / comments / media posts / people |
| `SPC s e` | Search or Explore (also `F3`) |
| `SPC o u` | Open a Reddit URL in redtab |
| `SPC o c` | Open the community or profile in the browser |
| `SPC p s` | Settings (also `F2`) |
| `SPC p a` | Accounts |

In a post, `gc`, `SPC c` and `SPC m` use the post's own community or profile.
On the Search tab, `gc` on a post or comment result opens that post in its community or
profile tab, at the comment for a comment result.

## Menus

Menus show each row's shortcut. Type it, or select with `j`/`k` and press `Enter`.
`h`, `q` or `Esc` goes back. Deleting always asks for `Enter` to confirm.

| Menu | Keys |
| --- | --- |
| Reddit: account | `ap` profile, `as` saved, `ah` hidden, `au` upvoted, `ad` downvoted, `af` following/favorites ↗ |
| Reddit: notifications | `nu` unread, `nc` comment replies, `np` post replies, `nm` mentions, `ni` inbox, `nn` notifications ↗ |
| Reddit: search and go | `sp`/`sc`/`sC`/`sm`/`su` posts/communities/comments/media/people, `gu` username, `gf` custom feeds, `gc` discovery, `gp` Popular, `ga` All |
| Reddit: create | `ct` text post, `cl` link post, `cf` private custom feed, `cm` upload/poll/AMA ↗ |
| Reddit: native tools ↗ | `mc` chat, `pp` privacy/blocks/profile, `pn` notification settings, `mq` mod queue, `mm` mod mail |
| Content: write | `u`/`d` vote, `s` save, `c` reply, `R` report, `e` edit, `D` delete |
| Content: go to | `a` author, `i` community/profile info, `x` original crosspost |
| Content: post | `H` hide/unhide, `f`/`F` follow/unfollow notifications, `nr`/`nu` mark read/unread |
| Content: native tools ↗ | `op` content/poll/awards, `oc` crosspost/notifications, `oa` author follow/block/chat, `os` source controls, `om` moderation |
| Message | `r` mark read, `u` mark unread, `o` notifications ↗ |
| Community tools | `f` feed, `o` browser ↗, `r` mark all read, `a` about, `R` rules, `wi`/`wp` wiki index/page, `mi` moderators, `mm` message mods ↗, `mq` mod queue ↗, `mM` mod mail ↗ |
| Profile tools | `m` follow/unfollow, `f` posts, `o` browser, `i` profile info |
| Comment tools | `/` find, `b`/`t`/`n`/`o`/`c`/`Q` Best/Top/New/Old/Controversial/Q&A |
| Feed sort | `b`/`H`/`n`/`t`/`r`/`c` Best/Hot/New/Top/Rising/Controversial, where available |
| Custom feed | `a` add community, `d` remove, `D` delete feed |

↗ opens reddit.com in your browser; on a menu name, every row does. In Reddit dialogs and forms, `C-o` opens the
matching reddit.com page.

## Text, forms and dialogs

| Context | Keys |
| --- | --- |
| Any text field | `Backspace`, `C-u` clear, `C-w` or Alt+Backspace delete word, `Esc` cancel |
| Search query, forms, Settings editor | Left/Right, Home/End, `C-a`/`C-e` move the caret; `Delete` removes the next character |
| Search query | Up/Down, `C-p`/`C-n` or `C-k`/`C-j` choose a result; `Enter` goes to results; `Esc` leaves typing |
| Forms | `Tab` / `Shift+Tab` next / previous field; `C-s` or Alt+Enter submits |
| Single-line field | `Enter` advances or submits |
| Multiline field | `Enter` adds a line |
| Tab picker | Up/Down or `Tab` choose; `Enter` opens |
| Accounts | `a` add, `r` reconnect, `d` remove, `Enter` switch |
| Settings | `Enter` toggles or edits, `r` refreshes or retries, `u` reverts unsaved values; `Esc`, `q` or `F2` closes when not editing |
| Settings editor | `Enter` or `C-s` applies, `Esc` cancels |
| chezmoi save prompt | `Enter` or `y` saves, `Esc` or `n` keeps the change for this session |
| Join / follow prompt | `Enter` confirms, `o` opens the feed, `Esc` cancels |
| Loading | `Esc` or `q` cancels, `r` retries a failure; in Reddit dialogs `C-o` opens reddit.com |
