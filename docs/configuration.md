# Settings

[User guide](guide.md)

Press `F2` or `SPC p s`.

| Key | Action |
| --- | --- |
| `Enter` | Toggle a switch or edit a value |
| `Enter` or `Ctrl+s` | Apply the edit |
| `Esc` | Cancel the edit |
| `Esc`, `q` or `F2` | Close Settings when not editing |
| `r` | Reload a Reddit setting, or retry a failed save |
| `u` | Revert unsaved changes |

Settings come in three groups:

| Group | Saved to |
| --- | --- |
| **Reddit settings · u/name** | Your Reddit account |
| **Reader settings · u/name** | The config file, under your username |
| **This device · all accounts** | The config file, shared |

## Reddit settings

- **Show NSFW** changes Reddit's mature-content setting. Content reloads afterward.
- **Autoplay videos** plays the selected video automatically. When off, press `p`.

If your account has not passed Reddit's age verification, change **Show NSFW** at
reddit.com/settings/preferences; redtab shows that message instead of saving.

## Config file

Default: `$XDG_CONFIG_HOME/redtab/config.toml`, or
`~/.config/redtab/config.toml` when `XDG_CONFIG_HOME` is unset or not absolute.
Use `redtab --config PATH` for another file.

Startup creates a missing config file with commented defaults. If creation fails,
redtab uses defaults and shows the save error. `--check` creates no config file
and requires an explicitly selected one to exist.

Omitted keys use defaults; unknown keys and invalid values are rejected. Valid
external edits apply within a second. Invalid files prevent startup; invalid
edits while running leave the last working settings active.

```toml
all_subs_posts_per_group = 3   # posts per All subs island

[media]
image_preview = true           # same as zi

[accounts.example_user.filters]
keywords = ["giveaway"]        # hide titles containing these, any case
flairs = ["Meme"]
authors = []

[accounts.example_user.highlight]
keywords = ["lsp", "treesitter"]

[commands]
browser = ["open"]             # Linux default: xdg-open
image = ["open"]
video = ["mpv", "--force-window=immediate"]
```

- Replace `example_user` with your username, without `u/`.
- Filters and highlights apply only to that account and appear in Settings once
  its username is known.

### Configuration reference

Device settings apply to all accounts:

| Key | Default | Accepted values / effect |
| --- | --- | --- |
| `all_subs_posts_per_group` | `3` | Integer 1–10; posts per All subs island |
| `media.image_preview` | `true` | Boolean; inline images, card previews, thumbnails and community icons |
| `commands.browser` | `["open"]` on macOS, `["xdg-open"]` on Linux | Opens URLs |
| `commands.image` | Same default as `commands.browser` | Opens local image files |
| `commands.video` | `["mpv", "--force-window=immediate"]` | External video player |

Commands are string arrays starting with a nonblank executable. redtab appends
the URL or file path; shell expansion, pipes and redirection do not run.
Changing `commands.browser` does not change `commands.image`.

Reader keys belong under `accounts.<username>`, not the file root. Each is a
string array, defaulting to `[]`:

| Account-relative key | Matching rule |
| --- | --- |
| `filters.keywords` | Hide titles containing any phrase, ignoring case |
| `filters.flairs` | Hide exact flair names, ignoring ASCII case |
| `filters.authors` | Hide exact usernames without `u/`, ignoring ASCII case |
| `highlight.keywords` | Emphasize titles containing any phrase, ignoring case |

Change **Show NSFW** and **Autoplay videos** through Reddit settings.
Their keys, `show_nsfw` and `media.video_autoplay`, are rejected in TOML.
Autoplay does not enable sound; `m` toggles it.

## Save status

The Settings footer shows the file and its status:

| Status | Meaning | What to do |
| --- | --- | --- |
| **Saved** | Written to the file | Nothing |
| **Temporary** | Active for this session only | Save through chezmoi when offered; `r` retries, `u` reverts |
| **Unsaved** | Save failed, the file is invalid, or another edit conflicts | Read the error; `r` retries, `u` reloads the file |

If two windows change the same setting, press `u` to reload, then reapply your
change. Reddit saves are separate: `r` retries failures; local file-save status
does not confirm a Reddit save.

## chezmoi

If the config is read-only and managed by chezmoi, redtab offers
**Save through chezmoi?** with the keys it will change.

- `Enter` or `y` updates the source and runs `chezmoi apply` for that file.
- `Esc` or `n` keeps the change for this session.

Templates, encrypted sources and symlinked targets stay session-only.
