# User guide

[redtab](../README.md)

## Install

Build requirements: Rust 1.90+, CMake, a C/C++ compiler and libclang.
On macOS, install the Command Line Tools and CMake. On Debian/Ubuntu, the build
packages are `build-essential cmake clang libclang-dev pkg-config`.

```sh
cargo install --git https://github.com/kapral18/redtab --locked
redtab
```

Select a signed-in browser profile, allow OS access if prompted, and confirm your
Reddit username. See [Accounts](accounts.md) if importing the session fails.

Optional programs:

| Program | Used for |
| --- | --- |
| `ffmpeg` | Inline video and GIF playback (8.1 or newer seeks YouTube faster) |
| `yt-dlp` and Deno or Node.js | Inline public YouTube videos and Shorts |
| `ffplay` | Sound during inline playback |
| `mpv` | Opening videos with `SPC o m` |
| `secret-tool` | Saved sessions on Linux; included in Debian/Ubuntu's `libsecret-tools` |

## Find what you need

- [Using redtab](usage.md): feeds, comments, media and posting.
- [Keyboard reference](keybindings.md): Vim navigation and mnemonic action groups.
- [Accounts](accounts.md): sign-in, switching, reconnecting and data privacy.
- [Settings](configuration.md): the settings dialog, TOML file and save conflicts.
- [Development](development.md): build checks and the [UI reference](ui.md).

## Startup options

```sh
redtab                   # personalized Home feed
redtab -s neovim         # start on r/neovim
redtab -c ~/redtab.toml  # use another config file
redtab --check           # print connection diagnostics
```

`--check` prints configuration, Home status, subscription count and the state-file
path. It exits with status 1 if Home or subscriptions cannot load. Use
`redtab --help` for all options.
