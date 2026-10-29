# Using redtab

[User guide](guide.md)

## Feeds and tabs

The sidebar contains Home, Bookmarks, All subs, Subscriptions, Following and opened
tabs. Subscriptions are communities; Following contains people you follow. `gt`/`gT` switch tabs; `SPC b b` finds a tab or opens a subreddit or `u/name`.
`SPC v b` hides the sidebar.

`SPC o u` opens a Reddit URL inside redtab. Paste a post, comment, community or
profile link, then press `Enter`. Reddit host variants and `redd.it` post links
work; `/s/` share links need the full post URL. Invalid links keep the input for editing.
Post and comment links select their community or profile tab once loaded and keep
the linked comment selected.

In a post or reading dialog, `SPC o l` lists links in the selected comment, post body
or dialog text. Choose a label with `j`/`k` and `Enter`, then open the link in your
browser or copy its URL. Hidden spoiler links stay out of this list. `SPC o r` and `yy`
still open or copy the selected post/comment's Reddit permalink.

`SPC s e` or `F3` opens Search and explore: search posts across Reddit, communities or people,
discover communities, or browse Popular and All. `Enter` opens a result's post or
feed; `SPC c` shows community/profile tools. Post searches support `SPC f s` sort, `SPC f t` time
and automatic loading as you scroll. `Esc` returns to the query to edit it. `/` searches posts across Reddit outside community tabs; inside one it searches
that community. In an open post, `/` finds text or authors in loaded comments.

Home uses Reddit's personalized Best feed, including recommendations. All subs
groups the top non-stickied posts from each joined community. Press `r` there to
refresh subscriptions and posts. A failed refresh keeps the last known community list.
Feeds and collection dialogs load more as you navigate down. If loading fails,
`r` retries it while retaining loaded results.

Selecting a person in Following shows their submitted posts across communities in
Card or Compact view. User feeds support New, Hot, Top and Controversial, with the
same pagination, media and post details as community feeds. All subs contains
communities only. `SPC o c` opens the user profile. In the list,
`SPC o r` opens the selected post on Reddit. In a user feed or its sidebar entry, `SPC a m` follows or
unfollows the person, and `SPC c` opens profile tools with that action first. The header
shows Following, Not following or unknown status with its shortcut. In post details,
`SPC a m` and `SPC c` use the post's actual community or profile.

Follow/unfollow uses the same confirmation as community join/leave. After a change,
redtab refreshes Following automatically. Unfollowed feeds move to Opened with their
posts and reading position intact. `SPC b d` closes opened tabs; followed users and joined
communities stay pinned, with a hint to use `SPC a m` to remove the membership.

`SPC v c` switches Card/Compact view in Home, community/user feeds, search and Bookmarks;
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

`SPC c` opens community or profile actions from the sidebar, a list or a post. **Mark community
read** clears all fetched unread posts for that community, including posts hidden
by filters. New posts remain unread. This changes local reading history only.
`SPC o c` opens the focused community or profile in your configured browser. `SPC o r` opens the selected post/comment in lists and detail, falling back to
the community when a feed is empty.

`SPC g c` opens the current community or profile feed. `SPC a m` opens Join/Leave or Follow/Unfollow; `Enter` confirms
the Reddit account change. Leaving moves an open community tab to **Opened** and
keeps its reading position. Failed changes are not retried automatically; `r` in
All subs checks membership again.

## Read posts and media

Open a post with `Enter`. Comments fold and expand, and deeper replies load on
demand. `SPC f s` opens comment sorting; `/` searches loaded comments. `SPC v s` reveals
spoilers; `[i`/`]i` browse gallery images. Captions, crosspost attribution and
available poll-option counts appear in the post body.

Main images and videos fit the pane's width and at most 60% of its usable height,
without cropping.
Previews keep their space while loading and reuse cached media when you return.
Large images and galleries use a suitably sized Reddit preview. If none is
available and the original exceeds inline limits, `SPC o r` opens the post on Reddit.
Images fall back to half-block colors without a graphics protocol. Compact
thumbnails and community icons need Kitty, iTerm2 or Sixel graphics; Ghostty and
Kitty also work in tmux. `SPC v i` toggles previews. `NO_COLOR` is supported.

With `ffmpeg`, the selected card autoplays video/GIFs after navigation settles,
initially muted, including while the sidebar has focus. An open detail pane takes
playback instead. Return to the list to use the playback controls. `SPC m p` plays/pauses,
`SPC m m` toggles sound, and `SPC m h`/`SPC m l` seek five seconds. Scrolling within the same post
keeps pause and position. Turn off **Autoplay videos** in F2 for manual playback; it saves to the current Reddit account.
Play overlays mark paused/waiting media; links show an arrow and domain, and
galleries show an image count.

Public YouTube videos and Shorts use the same inline controls, autoplay setting
and muted card playback. Install a current `yt-dlp` and a supported JavaScript
runtime (Deno is recommended; Node.js is also enabled). Some installations also need
the extractor's [EJS components](https://github.com/yt-dlp/yt-dlp/wiki/EJS).
The YouTube badge and play overlay identify these posts; their poster stays visible
while the stream resolves. `SPC m p` retries failures; `SPC o m` opens the original YouTube page
in your browser and pauses inline playback. Live, scheduled and login-required
videos use this browser fallback. Redtab does not import YouTube cookies.

`SPC o m` opens original images in your viewer, Reddit videos in your video player and links
in your browser. Keep redtab open during external video playback. Use `SPC o r` to open the
selected post/comment on Reddit, `yy` to copy its link, and `SPC a b` for a local bookmark.
Feeds, posts and still previews reuse cached data and refresh in the background.

## Reddit actions

- `SPC r` opens saved/hidden/voted content, profiles, notifications, search, discovery
  and custom feeds. It also creates text/link posts and private custom feeds.
- `SPC a a` opens actions for the selected post or comment: vote, save, reply, report,
  hide, follow post notifications, or edit/delete your own content.
- `SPC c` opens community information, rules, moderators and wiki.

In a custom feed, `a` adds a community, `d` removes one, and `D` deletes the feed
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

Press `Space` and pause for 500 ms to see mnemonic action groups. `SPC p s` means
press Space, then `p`, then `s`; release each key before the next. `Esc` cancels a pending sequence.
Use `j/k`, `h/l`, `gg/G`, `Ctrl+f/b` and `Ctrl+d/u` to navigate. Space starts action
groups in reading views; spaces remain text in editors.

Use `[[`/`]]` to jump between root comment threads from deep replies.
In details, `[p`/`]p` opens the previous/next post in the loaded parent feed
without leaving details. `[s`/`]s` switches subreddit feeds, or community groups
in All subs. From a post, those keys and `gt`/`gT` follow the selected post in
the destination; a loading or empty destination shows its list.

Press `?` or `F1` for context-aware help. Use `F1` while typing so `?` remains text.
Help and leader menus scroll with paging keys, and `Esc` returns to the covered view.
See the [complete keyboard reference](keybindings.md) for every action and menu.
