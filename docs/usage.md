# Using redtab

[User guide](guide.md) · [Keyboard reference](keybindings.md)

Press `Space` and wait for the action menu. Press `?` or `F1` for help.

## Tabs

The sidebar lists:

- **Home**: your personalized Best feed, including recommendations.
- **Bookmarks**: posts you bookmarked with `b`. These are local and separate from Reddit saves.
- **All subs**: the first Hot posts from each joined community, one island per community.
- **Search**: Reddit search and Explore.
- **Subscriptions**: communities you joined.
- **Following**: people you follow.
- **Opened**: other tabs you opened.

| Keys | Action |
| --- | --- |
| `gt` / `gT` | Next / previous tab |
| `SPC b b` | Find a tab, or open a subreddit or `u/name` |
| `SPC o u` | Open a post, comment, community, profile or `redd.it` link inside redtab (`/s/` share links need the full post URL) |
| `SPC b d` | Close an opened tab |
| `zb` | Hide or show the sidebar |
| `zc` | Switch between Card and Compact view, outside All subs (remembered across restarts) |

At 158 columns or wider (128 with the sidebar hidden), an open post appears beside
the list. In narrower terminals it replaces the list.

## Feeds

- Feeds load more posts as you move down. If loading fails, `r` retries and keeps loaded posts.
- `SPC f s` changes the sort; `SPC f t` changes the time range. All Reddit offers
  only Hot, Top and Controversial. User feeds offer New, Hot, Top and Controversial.
- `/` searches within the current community.
- Inside All subs, `r` refreshes your subscriptions and their posts.

### Updates and `r`

Shown content updates as it arrives: scores, comment counts, text, media, order and
removals. Your selected post stays selected and in place.

New posts that appear above everything you have seen wait behind
**New posts · r refresh**. Press `r` to show them and jump to the top.
With nothing waiting, `r` fetches fresh content and jumps to the top only when it
finds new posts.

Every 60 seconds, redtab checks the shown feed and visible All subs islands,
fetching listings older than five minutes. Failed first-page loads wait for `r`.
Comment threads do not poll; reopen them or press `r` to fetch updates.
See [Update rules](ui.md#update-rules).

### Unread counts

Community tab badges count fetched posts you have not opened; opening a post clears it.
The Bookmarks badge counts bookmarks. Other tabs have no badge.

- `?` means redtab has no history for that community yet.
- A blank badge means no known unread posts.
- Counts follow your filters and NSFW setting and survive restarts.
- Reading history expires after 90 days.
- **Mark community read** in `SPC c` clears a community's count. It only changes local history.

## Search

Press `F3` or `SPC s e` to start typing. `/` does the same in a list outside a
community tab; in a community it searches that community, in a post it finds comments.
`SPC s p`, `SPC s c`, `SPC s C`, `SPC s m` and `SPC s u` start a search for posts,
communities, comments, media posts or people.

| Keys | Action |
| --- | --- |
| `↑` / `↓` or `C-p` / `C-n` | Choose a result while typing |
| `Enter` | Run the search now and move to the results; on a result, open it |
| `Esc` | Leave the query and use list keys; `/` returns to it |
| `Tab` / `Shift+Tab` | Switch Posts / Communities / Comments / Media / People |

With an empty query, Search shows Explore: Popular, All Reddit and suggested communities.

**Posts** and **Media** results are feed cards; Media holds only posts with pictures or
video. `l` or `Enter` opens a post beside the list.

**Comments** results show the comment's post title, community, author, votes and first
line; the full comment shows beside the list. `l` or `Enter` opens the thread at that
comment beside the results, with spoilers revealed. `h` returns to the results; moving
to another result closes the thread.

`gc` on a Posts, Media or Comments result, or in a thread opened from one, opens the
post in its community or profile tab. For a comment result, the thread opens at that
comment.

**Communities** and **People** results show the selected row's hot posts beside the
list when there is room. `l` or `Enter` opens the community or person as a feed.
`q` closes the right pane until you open a result again.

To go back, press `h` or `Left`:

1. Focus the post list (detail stays visible).
2. Close detail and show the result rows beside the posts.
3. Return to the Search rows with your query and selection kept.

`[s` / `]s` jumps to the previous / next result row from a nested feed or post.

## Communities and people

| Keys | Action |
| --- | --- |
| `gc` | Open the current community or profile feed |
| `SPC m` | Join / leave a community, or follow / unfollow a person |
| `SPC c` | Community or profile tools: info, rules, moderators, wiki |
| `SPC o c` | Open the community or profile in your browser |

Membership changes ask for confirmation. Leaving or unfollowing moves the tab to
**Opened** and keeps your reading position. Failed changes are not retried.

A person's feed shows their profile (avatar, karma, description) above their posts.

## Posts and comments

| Keys | Action |
| --- | --- |
| `Enter` / `l` | Open the post |
| `za` | Fold or unfold a comment, or load its replies |
| `[[` / `]]` | Previous / next top-level comment thread |
| `[p` / `]p` | Previous / next post without leaving the post view |
| `/` | Find text or authors in loaded comments |
| `SPC f s` | Comment sort |
| `zs` | Reveal / hide spoilers |
| `gx` | Pick a link in the selected comment or post body |
| `o` / `yy` | Open / copy the Reddit link |

## Images and video

- Images and videos fit the pane width and at most 60% of its height, without cropping.
- `[i` / `]i` browse gallery images. `zi` turns previews on or off.
- Without a graphics protocol, images use half-block colors. Thumbnails and icons
  need Kitty, iTerm2 or Sixel. Ghostty and Kitty also work in tmux.
- `NO_COLOR` is supported.

With `ffmpeg` installed, the selected video or GIF plays muted once you stop moving.
An open post takes over playback.

| Keys | Action |
| --- | --- |
| `p` | Play, pause or retry |
| `m` | Toggle sound (needs `ffplay`) |
| `H` / `L` | Seek back / forward five seconds |
| `O` | Open the image, video or link externally |

Turn off **Autoplay videos** in Settings (`F2`) to start videos only with `p`.

### YouTube

Public and unlisted YouTube videos and Shorts play inline with the same keys. They need `yt-dlp`
2026.08.19 or newer and Deno 2.3+ or Node.js 22+. Some installs also need yt-dlp's
[EJS components](https://github.com/yt-dlp/yt-dlp/wiki/EJS).

Live, scheduled and login-required videos open in your browser with `O`.
redtab does not use YouTube cookies.

## Reddit actions

| Keys | Action |
| --- | --- |
| `u` / `d` | Upvote / downvote; press again to remove the vote |
| `s` | Save / unsave on Reddit |
| `c` | Reply |
| `b` | Bookmark locally |
| `SPC a` | All actions: report, hide, notifications, edit or delete your own content |
| `SPC r` | Saved, hidden and voted posts, profiles, notifications, custom feeds, post creation |

In a custom feed, `a` adds a community, `d` removes one and `D` deletes the feed.

In forms, `Tab` switches fields, `Enter` adds a line in multiline fields and
`Ctrl+s` submits. A failed submission keeps your draft. redtab never retries a
write, so check Reddit before resubmitting an uncertain one.

Items marked **open Reddit ↗** open in your browser, signed in with the browser's
account: chat, uploads, poll voting, blocks, flair, awards and advanced moderation.
Draft text is not transferred.
