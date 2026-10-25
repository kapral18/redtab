# Settings

[User guide](guide.md)

Press `F2` to browse all settings. `Enter` toggles a boolean or opens an editor;
`Enter` or `Ctrl+s` applies an edited value, and `Esc` cancels the draft.
Changes take effect immediately. Writable configs save automatically; read-only
configs keep session changes or offer a confirmed chezmoi source save.

## Config file

The default is `$XDG_CONFIG_HOME/redtab/config.toml`, usually
`~/.config/redtab/config.toml`. Use `redtab --config PATH` to choose another file.
The first normal launch creates an annotated config with every default as an active value.
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
show_nsfw = false

[media]
image_preview = true       # also controlled by i
video_autoplay = true      # false: press P to start playback

[filters]
keywords = ["giveaway"]    # case-insensitive title matches
flairs = ["Meme"]
authors = []

[highlight]
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

Media options belong in `[media]`. Move `image_preview` and `video_autoplay`
from the root into that table in older configs; root media keys are rejected.

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

## NSFW content

NSFW content is hidden by default. Enable **Show NSFW** in `F2`, or set
`show_nsfw = true` in the file. Content reloads without a restart. Empty content
lists suggest checking this setting while it is off; other causes can also produce
an empty list.
