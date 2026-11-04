# Using redtab

[User guide](guide.md)

## Feeds and tabs

The sidebar contains Home, Bookmarks, All subs, Search, Subscriptions, Following and opened
tabs. Home, Bookmarks, All subs and Search stay fixed while the feed sections scroll.
Very short sidebars prioritize the selected tab when all fixed entries cannot fit.
Subscriptions are communities; Following contains people you follow. `gt`/`gT` switch tabs; `SPC b b` finds a tab or opens a subreddit or `u/name`.
`zb` hides the sidebar.

`SPC o u` opens a Reddit URL inside redtab. Paste a post, comment, community or
profile link, then press `Enter`. Reddit host variants and `redd.it` post links
work; `/s/` share links need the full post URL. Invalid links keep the input for editing.
Post and comment links select their community or profile tab once loaded and keep
the linked comment selected.

In a post or reading dialog, `gx` lists links in the selected comment, post body
or dialog text. Choose a label with `j`/`k` and `Enter`, then open the link in your
browser or copy its URL. Hidden spoiler links stay out of this list. `o` and `yy`
still open or copy the selected post/comment's Reddit permalink.

The Search tab searches Reddit as you type. `F3`, `SPC s e`, and `/` outside community
tabs switch to it with the query box focused; `SPC s p`, `SPC s c` and `SPC s u` also
pick Posts, Communities or People. Results update 450 ms after you stop typing. While
typing, `↑`/`↓` (or `C-n`/`C-p`) choose a result. `Enter` runs a pending search at once
and leaves typing for the results, where `Enter` opens the selected one. `Esc` also leaves
the query for list keys and `/` returns to it. `Tab`/`Shift+Tab` switch Posts, Communities and
People from the query or the result list. While following a post through buffers,
Tab continues buffer navigation until another reading action ends that follow.

Posts results are feed cards from all of Reddit: `l`/`Enter` opens a post beside the
list, `SPC f s` cycles relevance/hot/top/new/comments and `SPC f t` the time range.
Communities and People results are rows with community icons and user avatars. Where space permits,
the selected row's hot posts appear beside the rows as a passive preview in your
Card/Compact view; a person's preview starts with their profile summary. `l`/`Enter` opens the community or person's feed as a tab at every
width. Inside that feed, `j`/`k` moves between posts, paging loads more, and
`l`/`Enter` opens the selected post. In a wide view, the first `h`/`Left` focuses
the post list while keeping detail visible. The next closes detail and restores
communities beside posts, keeping focus on posts. From there, `h` returns to the
same Search rows, keeping the query, kind and selection; another `h` focuses the
sidebar. Narrow views close detail before returning to Search rows. Switching tabs
explicitly ends this return path. `F3` returns to typing
in Search. `SPC c` and `SPC m` act
on the selected community or person, and more Search rows load near the end. With
source rows focused, `gc` and `SPC c f` preserve the same return path as Enter;
`o`/`SPC o c` open their Reddit page and `yy` copies its URL. `C-w h/l` focus visible
panes; `Esc`/`Backspace` clear a local filter before returning from a nested feed.
Source rows use ten-item pages, five-item half pages and one-item `C-e/y` motions. With
an empty query the tab lists Explore rows (Popular, All Reddit and discovered
communities) with the same preview. Inside a community tab, `/` searches that community. In an open post,
`/` finds text or authors in loaded comments.

Home uses Reddit's personalized Best feed, including recommendations. All subs
groups the top non-stickied posts from each joined community. Press `r` there to
refresh subscriptions and posts. A failed refresh keeps the last known community list.
Feeds and collection dialogs load more as you navigate down. If loading fails,
`r` retries it while retaining loaded results.

Selecting a person in Following shows their submitted posts across communities in
Card or Compact view, under their profile summary: avatar, post and comment karma, and
description. Their sidebar entry shows the avatar once the profile is known. User feeds support New, Hot, Top and Controversial, with the
same pagination, media and post details as community feeds. All subs contains
communities only. `SPC o c` opens the user profile. In the list,
`o` opens the selected post on Reddit. In a user feed or its sidebar entry, `SPC m` follows or
unfollows the person, and `SPC c` opens profile tools with that action first. The header
shows Following, Not following or unknown status with its shortcut. In post details,
`SPC m` and `SPC c` use the post's actual community or profile.

Follow/unfollow uses the same confirmation as community join/leave. After a change,
redtab refreshes Following automatically. Unfollowed feeds move to Opened with their
posts and reading position intact. `SPC b d` closes opened tabs; followed users and joined
communities stay pinned, with a hint to use `SPC m` to remove the membership.

`zc` switches Card/Compact view in Home, community/user feeds, search and Bookmarks;
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
`SPC o c` opens the focused community or profile in your configured browser. `o` opens the selected post/comment in lists and detail, falling back to
the community when a feed is empty.

`gc` opens the current community or profile feed. `SPC m` opens Join/Leave or Follow/Unfollow; `Enter` confirms
the Reddit account change. Leaving moves an open community tab to **Opened** and
keeps its reading position. Failed changes are not retried automatically; `r` in
All subs checks membership again.

## Read posts and media

Open a post with `Enter`. Comments fold and expand, and deeper replies load on
demand. `SPC f s` opens comment sorting; `/` searches loaded comments. `zs` reveals
spoilers; `[i`/`]i` browse gallery images. Captions, crosspost attribution and
available poll-option counts appear in the post body.

Main images and videos fit the pane's width and at most 60% of its usable height,
without cropping.
Previews keep their space while loading and reuse cached media when you return.
Large images and galleries use a suitably sized Reddit preview. If none is
available and the original exceeds inline limits, `o` opens the post on Reddit.
Images fall back to half-block colors without a graphics protocol. Compact
thumbnails and community icons need Kitty, iTerm2 or Sixel graphics; Ghostty and
Kitty also work in tmux. `zi` toggles previews. `NO_COLOR` is supported.

With `ffmpeg`, the selected card autoplays video/GIFs after navigation settles,
initially muted, including while the sidebar has focus. Visible videos load a still
preview even when unselected or autoplay is off. An open detail pane takes
playback instead. Return to the list to use the playback controls. `p` plays/pauses,
`m` toggles sound, and `H`/`L` seek five seconds. Scrolling within the same post
keeps pause and position. Turn off **Autoplay videos** in F2 for manual playback; it saves to the current Reddit account.
Play overlays mark paused/waiting media; links show an arrow and domain, and
galleries show an image count.

Public YouTube videos and Shorts use the same inline controls, autoplay setting
and muted card playback. Install `yt-dlp` 2026.08.19 or newer and a JavaScript
runtime it supports (Deno 2.3+ is recommended; Node.js 22+ is also enabled). Some installations also need
the extractor's [EJS components](https://github.com/yt-dlp/yt-dlp/wiki/EJS).
The YouTube badge and play overlay identify these posts; their poster stays visible
with a loading indicator while playback starts. `p` retries failures; `O` opens the
original YouTube page in your browser and pauses inline playback. Live, scheduled and login-required
videos use this browser fallback. Redtab does not import YouTube cookies.

`O` opens original images in your viewer, Reddit videos in your video player and links
in your browser. Keep redtab open during external video playback. Use `o` to open the
selected post/comment on Reddit, `yy` to copy its link, and `b` for a local bookmark.
Feeds, posts and still previews reuse cached data and refresh in the background. Recent Reddit collection pages also reopen from memory for five minutes;
stale pages stay readable while refreshing. Cold All subs islands show posts as the
response arrives. Full thread links reuse cached post pages.
The shown feed, or the shown All subs islands, refetch once their posts are five
minutes old, checked every minute while you stay on them. A failed refetch waits for `r`.
All subs prepares one following row of cold islands once the visible islands have
post rows or show an empty/error state. Icon lookups follow the same readiness rule. Visible reads
run independently of bulk cache warming; cached islands and explicit refreshes
wait until visible before fetching updates.
Updates to what is already shown apply as they arrive, with no notice: scores, comment
counts, text, media URLs, ordering and removals. Your selected post or comment stays
selected and in place, except at an untouched top, which stays on the list's first
post. A post you are reading stays even when a background refresh of the shown tab
drops it, until the next refresh after you move off it. Only new posts at the top,
above every post you have already seen in a feed or an All subs island, show **New
posts · r refresh**; posts that enter lower down appear in place. While they wait,
posts the waiting answer drops stay where they are, and an All subs island keeps its
order. Press `r` to apply them and jump to the top. While an open post has focus, `r`
refreshes that post and the list shows only **New posts**. With nothing waiting, `r`
fetches fresh content or retries a failed read; it jumps to the top of the list, or of
the open post, only when that fetch brings something new at the top, and otherwise
keeps your place. Switching into a feed or All subs applies posts already waiting
before entry. Responses that arrive afterward leave new leading posts behind the
notice. With no new leading posts, your position stays. Following the same open
post between views keeps your reading position.

Search previews use the same new-leading-post rule. A post already loaded on a later
feed page is a ranking update, not a new post. Opening a matching Hot feed applies
waiting preview posts before entry; returning reuses its newer snapshot and pending
posts. Other sorts and searches keep their own results. A notice is not an unread
count: unread badges count fetched posts whose revealed details you have not read.
See [Update rules](ui.md#update-rules) for the full rule set.

## Reddit actions

- `SPC r` opens saved/hidden/voted content, profiles, notifications, search, discovery
  and custom feeds. It also creates text/link posts and private custom feeds.
- `u`/`d`, `s` and `c` vote, save and reply directly. `SPC a` opens every action for
  the selected post or comment: vote, save, reply, report,
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
Use `j/k`, `h/l`, `gg/G`, `Ctrl+f/b` and `Ctrl+d/u` to navigate. Common actions are
single keys on the focused item: `u`/`d` vote, `s` save, `c` reply, `b` bookmark,
`o` opens on Reddit, `p`/`m` play/mute. `g` goes places (`gh` Home); `z` toggles view.
Space starts action groups in reading views; spaces remain text in editors.

Use `[[`/`]]` to jump between root comment threads from deep replies.
In details, `[p`/`]p` opens the previous/next post in the loaded parent feed
without leaving details. In Search Explore, Communities and People, `[s`/`]s`
selects the previous/next source row. From a nested feed or its post, it returns
focus to those rows, closing detail even at the ends. Elsewhere these keys switch
subreddit feeds or All subs community groups and follow the destination's selected
post. `gt`/`gT` follows the selected post across tabs; a loading or empty destination
shows its list.

Press `?` or `F1` for context-aware help. Use `F1` while typing so `?` remains text.
Help and leader menus scroll with paging keys, and `Esc` returns to the covered view.
See the [complete keyboard reference](keybindings.md) for every action and menu.
