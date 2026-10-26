# Settings

[User guide](guide.md)

Press `F2` to browse all settings. `Enter` toggles a boolean or opens an editor;
`Enter` or `Ctrl+s` applies an edited value, and `Esc` cancels the draft.
The screen separates **Reddit settings — u/name**, **Reader settings — u/name**,
and **This device — all accounts**. Reddit preferences save to that account;
reader rules save under its Reddit username in TOML. Device settings are
shared. Switching accounts loads its own rules; signed-out browsing has separate rules.

Reddit values load in the background on connection and when opening F2. Unavailable
values are labelled, not presented as defaults. A save checks account identity,
changes one preference, then reads it back. `r` on a Reddit setting refreshes its
account value; it never replays a failed write. Account switching waits for a pending
save. Writable file changes save automatically; read-only configs keep session
changes or offer a confirmed chezmoi source save.

## Config file

The default is `$XDG_CONFIG_HOME/redtab/config.toml`, usually
`~/.config/redtab/config.toml`. Use `redtab --config PATH` to choose another file.
The first normal launch creates an annotated config with active defaults for device and signed-out reader settings.
`--check` does not create one.

Every key is optional; removing a key restores its default. Saves preserve comments,
permissions and symlinks. Missing keys are added to their table without moving the
file header; existing inline and dotted tables keep their style.
Valid external edits reload within a second. Reloads pause
while a Reddit write is pending; finish it before changing settings.

This example uses macOS's `open` command. Linux defaults to `xdg-open` for the
browser and image viewer.

```toml
all_subs_posts_per_group = 3

[media]
image_preview = true       # also controlled by i

[accounts.example_user.filters]
keywords = ["giveaway"]    # case-insensitive title matches
flairs = ["Meme"]
authors = []

[accounts.example_user.highlight]
keywords = ["lsp", "treesitter"]

[commands]
browser = ["open"]
image = ["open"]
video = ["mpv", "--force-window=immediate"]
```

`all_subs_posts_per_group` controls each **All subs** island, not Home or subreddit
pagination. Home Best follows Reddit's browser ranking; a small starter is combined
with its continuation before fetching post details. Posts appear as complete records
arrive. Ordinary listings request 25 posts per page; filters can reduce the number shown.

Replace `example_user` with your Reddit username without `u/`. Account names match
without regard to case; saves retain the existing table spelling. Duplicate tables
for the same username are rejected. An unidentified connection must reconnect with
`A` before editing reader rules; it does not use signed-out rules. F2 creates the
account tables when you edit reader rules.
Use `[signed_out.filters]` and `[signed_out.highlight]` for signed-out browsing.
There is no global filter inheritance. Removing an account section restores empty
reader rules for that account. `media.image_preview` controls terminal graphics;
account mature-content and autoplay preferences have no TOML keys.

Commands are argument arrays, not shell strings. redtab appends the URL or file
paths; shell expansion does not run.

## Save status and recovery

The Settings footer shows the file path and status:

| Status | Meaning | Recovery |
| --- | --- | --- |
| **Saved** | Changes are written to the file | None needed |
| **Temporary** | Changes are active for this session | Confirm the chezmoi source save when offered; `r` retries saving, `u` reverts to the file |
| **Unsaved** | Saving failed, the file is invalid, or an edit conflicts | Read the error; `r` retries saving, `u` discards session changes and loads the valid file |

Invalid external edits leave the last working values active. redtab preserves
protected file permissions and uses no hidden override file. Concurrent changes to different
settings merge; conflicting changes to the same setting stay unsaved. The save
lock coordinates redtab instances, not arbitrary editors. After a conflict, use
`u` to load the file value before reapplying your choice.

## Chezmoi-managed configs

For a read-only config, F2 checks whether chezmoi manages it. Plain TOML sources
get a **Save through chezmoi?** dialog showing the source, target and changed keys.
`Enter` or `y` updates those source keys and runs a file-scoped `chezmoi apply`;
the resulting values and permissions must match before the status becomes Saved.
`Esc` or `n` keeps the changes for this session. Long confirmations scroll with
`j/k` or Page Up/Down. Configured chezmoi hooks can run during discovery and apply.

Discovery and saving run in the background. Editing and file reloads wait while
the confirmation or save is active. An approved save finishes before redtab exits.
If apply fails, the source may already contain the edit; `r` prepares a fresh
confirmation and retries. Source or target changes while confirming require
another review. Templates, generated/encrypted sources, symlink targets and
read-only sources remain session-only with an explanation. The `i` shortcut keeps
session changes until you open F2 to review saving them.

## Mature content and autoplay

F2 reads these preferences from Reddit. **Show NSFW** updates Reddit's native
mature-content setting for feeds and search. Reddit
may require completing a setting in its native interface; redtab reports that
restriction. Content reloads after a confirmed change. **Autoplay videos** controls
selected videos and GIFs; when off, press `P` to play manually.

Preference failures leave the confirmed runtime policy intact and show an error.
Refresh with `r` before another change if a save cannot be confirmed. Local reader
and device settings remain usable while account preferences load.
