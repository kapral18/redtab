# Using redtab

[User guide](guide.md)

## Feeds and tabs

The sidebar contains Home, Bookmarks, All subs, Subscriptions, Following and opened
tabs. Subscriptions are communities; Following contains people you follow. Number
keys switch tabs; `p` finds a tab or opens a subreddit or `u/name`. `\` hides the sidebar.

`Ctrl+L` opens a Reddit URL inside redtab. Paste a post, comment, community or
profile link, then press `Enter`. Reddit host variants and `redd.it` post links
work; `/s/` share links need the full post URL. Invalid links keep the input for editing.
Post and comment links select their community or profile tab once loaded and keep
the linked comment selected.

In a post or reading dialog, `w` lists links in the selected comment, post body
or dialog text. Choose a label with `j`/`k` and `Enter`, then open the link in your
browser or copy its URL. Hidden spoiler links stay out of this list. `o` and `y`
still open or copy the selected post/comment's Reddit permalink.

`F3` opens Search and explore: search posts across Reddit, communities or people,
discover communities, or browse Popular and All. `Enter` opens a result's post or
feed; `B` shows community/profile tools. Post searches support `s` sort, `t` time
and `L` next page. `Esc` returns to the query to edit it. `/` also opens Search and
explore outside community tabs; inside one it searches that community.

Home uses Reddit's personalized Best feed, including recommendations. All subs
groups the top non-stickied posts from each joined community. Press `r` there to
refresh subscriptions and posts. A failed refresh keeps the last known community list.

Selecting a person in Following shows their submitted posts across communities in
Card or Compact view. User feeds support New, Hot, Top and Controversial, with the
same pagination, media and post details as community feeds. All subs contains
communities only. `O` opens the user profile; sidebar `o` does too. In the list,
`o` opens the selected post. In a user feed or its sidebar entry, `S` follows or
unfollows the person, and `B` opens profile tools with that action first. The header
shows Following, Not following or unknown status with its shortcut. In post details,
`S` and `B` use the post's actual community or profile.

Follow/unfollow uses the same confirmation as community join/leave. After a change,
redtab refreshes Following automatically. Unfollowed feeds move to Opened with their
posts and reading position intact. `x` closes opened tabs; followed users and joined
communities stay pinned, with a hint to use `S` to remove the membership.

`V` switches Card/Compact view in Home, community/user feeds, search and Bookmarks;
the choice survives restarts. All subs keeps its community islands. At 158 columns,
or 128 with the sidebar hidden, an open post appears beside the list and follows
its selection. In narrower terminals it replaces the list.

Subreddit badges count unread posts redtab has fetched, across feeds and searches.
Opening a loaded, revealed post clears that post. Moving through the sidebar,
changing sort and failed loads leave the count alone. `?` means no retained listing
history; a blank badge means no known unread posts or unavailable community access.
Counts follow your filters and NSFW setting, survive restarts, and use local history
rather than fetching every community in the background. Reading history
expires after 90 days without another observation. Previously opened
posts keep their read marks; old whole-visit marks no longer hide unopened posts.

`B` opens community or profile actions from the sidebar, a list or a post. **Mark community
read** clears all fetched unread posts for that community, including posts hidden
by filters. New posts remain unread. This changes local reading history only.
`O` opens the focused community or profile in your configured browser. In the sidebar, `o` also opens
that destination; in lists and detail it opens the selected post/comment, falling
back to the community when a feed is empty.

`Space` opens the selected post's community. `S` opens Join/Leave or Follow/Unfollow; `Enter` confirms
the Reddit account change. Leaving moves an open community tab to **Opened** and
keeps its reading position. Failed changes are not retried automatically; `r` in
All subs checks membership again.

## Read posts and media

Open a post with `Enter`. Comments fold and expand, and deeper replies load on
demand. `f` opens comment sorting or search within loaded comments. `v` reveals
spoilers; `{`/`}` browse gallery images. Captions, crosspost attribution and
available poll-option counts appear in the post body.

Main images and videos fit the pane's width and at most 60% of its usable height,
without cropping.
Previews keep their space while loading and reuse cached media when you return.
Large images and galleries use a suitably sized Reddit preview. If none is
available and the original exceeds inline limits, `o` opens the post on Reddit.
Images fall back to half-block colors without a graphics protocol. Compact
thumbnails and community icons need Kitty, iTerm2 or Sixel graphics; Ghostty and
Kitty also work in tmux. `i` toggles previews. `NO_COLOR` is supported.

With `ffmpeg`, the selected card autoplays video/GIFs after navigation settles,
initially muted, including while the sidebar has focus. An open detail pane takes
playback instead. Return to the list to use the playback controls. `P` plays/pauses,
`a` toggles sound, and `,`/`.` seek five seconds. Scrolling within the same post
keeps pause and position. Turn off **Autoplay videos** in F2 for manual playback; it saves to the current Reddit account.
Play overlays mark paused/waiting media; links show an arrow and domain, and
galleries show an image count.

Public YouTube videos and Shorts use the same inline controls, autoplay setting
and muted card playback. Install a current `yt-dlp` and a supported JavaScript
runtime (Deno is recommended; Node.js is also enabled). Some installations also need
the extractor's [EJS components](https://github.com/yt-dlp/yt-dlp/wiki/EJS).
The YouTube badge and play overlay identify these posts; their poster stays visible
while the stream resolves. `P` retries failures; `m` opens the original YouTube page
in your browser and pauses inline playback. Live, scheduled and login-required
videos use this browser fallback. Redtab does not import YouTube cookies.

`m` opens original images in your viewer, Reddit videos in your video player and links
in your browser. Keep redtab open during external video playback. Use `o` to open the
selected post/comment on Reddit, `y` to copy its link, and `b` for a local bookmark.
Feeds, posts and still previews reuse cached data and refresh in the background.

## Reddit actions

- `:` opens saved/hidden/voted content, profiles, notifications, search, discovery
  and custom feeds. It also creates text/link posts and private custom feeds.
- `;` opens actions for the selected post or comment: vote, save, reply, report,
  hide, follow post notifications, or edit/delete your own content.
- `B` opens community information, rules, moderators and wiki.

In a custom feed, `a` adds a community, `d` removes one, and `x` deletes the feed
after confirmation. Reddit saves and local bookmarks are separate.

Composers use `Tab` to switch fields. `Enter` adds a line in multiline fields;
`Ctrl+s` submits. Failed submissions keep the draft. Check Reddit before resubmitting
an uncertain write: redtab waits for acknowledgment and never retries writes
automatically. Deleting content or a custom feed requires confirmation.

Items marked **open Reddit ↗** use the browser's own signed-in account. This
covers chat/message archives, uploads, poll voting, blocks, community
muting/favorites, flair, awards, advanced moderation, and comment/media search.
`Ctrl+o` opens the corresponding page from Reddit tools. Community posting
requirements, such as mandatory flair, may also require finishing in the browser;
draft text is not transferred.

## Keyboard shortcuts

Press `?` or `F1` for context-aware help. Use `F1` while typing so `?` remains text.
Help scrolls with `j`/`k`; `Esc` returns to the view beneath it.
`Ctrl+` means Control; `Shift+Tab` selects the previous tab or field.

| Context | Keys | Action |
| --- | --- | --- |
| Tabs | `Tab` / `Shift+Tab`, `]` / `[` | Next / previous tab |
| Tabs | `1`–`9`, `0` | Numbered tab / last tab |
| Tabs | `p`, `x`, `\` | Jump/open subreddit or u/name, close opened tab, toggle sidebar |
| Sidebar | `h`, then `j`/`k` or `g`/`G` | Focus sidebar, move or select first/last tab; `l`, `Enter` or `Esc` returns |
| List | `j`/`k`, `g`/`G` | Select next/previous post, first/last post |
| List | `Enter`, `l` | Open selected post or resume retained detail |
| List | `V` | Switch Card/Compact |
| List | `Space`, `S` | Open community/profile; join/leave community or follow/unfollow user |
| Home/subreddit | `s`, `t`, `L` | Sort, time range, load next page |
| Subreddit | `/`, `Esc` | Search, clear search |
| Cards | `Page Down/Up`, `Ctrl+d/u`, `Ctrl+e/y` | Scroll a page, half page, line across card boundaries |
| Compact / All subs | `Page Down/Up`, `Ctrl+d/u` | Move ten posts |
| Collection popup | `j/k`, `Ctrl+d/u`, `Ctrl+e/y`, `Page Down/Up` | Select items; scroll descriptions down/up five lines, one line or ten lines |
| All subs | `J`/`K` | Next/previous community group |
| Post | `j`/`k`, `g`/`G` | Select next/previous comment, top/bottom of post |
| Post | `l`, `c`, `Enter` | Select the current comment, fold its thread or load replies |
| Post | `n`/`N`, `f` | Next/previous top-level comment, comment tools |
| Post | `Space`, `Page Down/Up`, `Ctrl+d/u`, `Ctrl+e/y` | Scroll a page, half page or line; `Space` pages down |
| Post | `v`, `{`/`}` | Reveal/hide spoilers, previous/next gallery image |
| Post | `h` | Focus list in split view; go back one level in narrow view |
| Post | `Esc`, `q` | Close post |
| Video/GIF | `P`, `a`, `,`/`.` | Play/pause, sound, seek back/forward five seconds |
| Content | `o`, `y`, `m`, `b` | Open on Reddit, copy link, open media, local bookmark |
| Tools | `:`, `;`, `B`, `A`, `F2` | Reddit tools, content actions, community, accounts, settings |
| General | `i`, `r` | Toggle previews (saved when writable; F2 reviews managed saves), reload |
| General | `Ctrl+L`, `F3` | Open Reddit URL, search and explore |
| Other views | `/` | Search and explore |
| General | `q`, `Ctrl+c` | Quit; `q` closes an open post first |
