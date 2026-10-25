# Using redtab

[User guide](guide.md)

## Feeds and tabs

The sidebar contains Home, Bookmarks, All subs, subscriptions and opened subreddit
tabs. Number keys switch tabs; `p` finds a tab or opens a subreddit. `\` hides the sidebar.

Home uses Reddit's personalized Best feed, including recommendations. All subs
groups the top non-stickied posts from each joined community. Press `r` there to
refresh subscriptions and posts. A failed refresh keeps the last known community list.

`V` switches Card/Compact view in Home, subreddit feeds, search and Bookmarks;
the choice survives restarts. All subs keeps its community islands. At 158 columns,
or 128 with the sidebar hidden, an open post appears beside the list and follows
its selection. In narrower terminals it replaces the list.

`Space` opens the selected post's community. `S` opens Join/Leave; `Enter` confirms
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
initially muted. An open detail pane takes playback instead. `P` plays/pauses,
`a` toggles sound, and `,`/`.` seek five seconds. Scrolling within the same post
keeps pause and position. Set `video_autoplay = false` in `[media]` for manual playback.
Play overlays mark paused/waiting media; links show an arrow and domain, and
galleries show an image count.

`m` opens original images in your viewer, videos in your video player and links
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
covers chat/message archives, uploads, poll voting, follows/blocks, community
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
| Tabs | `p`, `x`, `\` | Jump/open subreddit, close opened tab, toggle sidebar |
| Sidebar | `h`, then `j`/`k` or `g`/`G` | Focus sidebar, move or select first/last tab; `l`, `Enter` or `Esc` returns |
| List | `j`/`k`, `g`/`G` | Select next/previous post, first/last post |
| List | `Enter`, `l` | Open selected post or resume retained detail |
| List | `V` | Switch Card/Compact |
| List | `Space`, `S` | Open community, join/leave community |
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
| General | `q`, `Ctrl+c` | Quit; `q` closes an open post first |
