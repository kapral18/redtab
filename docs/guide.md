# User guide

[redtab](../README.md)

## Install

Homebrew (macOS and Linux) installs the release build for your platform:

```sh
brew install kapral18/tap/redtab
```

To build from crates.io instead, you need Rust 1.90+, CMake, a C/C++ compiler and
libclang. On macOS, install the Command Line Tools and CMake. On Debian/Ubuntu, the
build packages are `build-essential cmake clang libclang-dev pkg-config`.

```sh
cargo install redtab --locked
```

On first launch, press `Enter` on **Sign in in a new login window** (needs Chrome,
Chromium, Brave or Edge), sign in there and confirm your username. See
[Accounts](accounts.md) for importing from a browser profile instead.

Optional programs:

| Program | Used for |
| --- | --- |
| `ffmpeg` | Inline video and GIF playback (8.1 or newer seeks YouTube faster) |
| `yt-dlp` (2026.08.19 or newer) and Deno 2.3+ or Node.js 22+ | Inline public and unlisted YouTube videos and Shorts |
| `ffplay` | Sound during inline playback |
| `mpv` | Opening videos with `O` |
| `secret-tool` | Saved sessions on Linux; included in Debian/Ubuntu's `libsecret-tools` |

### Prebuilt binaries

[Releases](https://github.com/kapral18/redtab/releases) hold one
`redtab-<tag>-<target>.tar.gz` per target, for example
`redtab-v0.1.0-aarch64-apple-darwin.tar.gz`. Targets: `aarch64-apple-darwin`,
`x86_64-apple-darwin`, `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu`.
Linux builds need glibc 2.35 or newer. `SHA256SUMS` lists every archive, and each archive has a
GitHub build attestation. Verify before running:

```sh
sha256sum -c SHA256SUMS --ignore-missing           # Linux
shasum -a 256 -c SHA256SUMS --ignore-missing       # macOS
gh attestation verify redtab-<tag>-<target>.tar.gz --repo kapral18/redtab
```

The archive holds `redtab`, its `LICENSE`, `README.md` and
`THIRD_PARTY_LICENSES`. macOS builds are not notarized. If macOS refuses to run a
binary downloaded with a browser, run:

```sh
xattr -d com.apple.quarantine redtab-<tag>-<target>/redtab
```

## Documentation

- [Using redtab](usage.md): feeds, comments, media and posting.
- [Keyboard reference](keybindings.md): Vim navigation and mnemonic action groups.
- [Accounts](accounts.md): sign-in, switching, reconnecting and data privacy.
- [Settings](configuration.md): the settings dialog, TOML file and save conflicts.
- [Development](development.md): build checks and the [UI reference](ui.md).

## Command line

```sh
redtab                   # personalized Home feed
redtab -s neovim         # start on r/neovim
redtab -c ~/redtab.toml  # use another config file
redtab --check           # live connection diagnostics
```

| Flag | Behavior |
| --- | --- |
| `-c`, `--config PATH` | Use this config file |
| `-s`, `--subreddit NAME` | Open this community at startup |
| `--signin` | [Connect a browser session](accounts.md#sign-in-from-the-command-line) or import cookies from stdin |
| `--browser NAME` | Browser for `--signin` |
| `--profile NAME` | Browser profile; requires `--signin --browser` |
| `--check` | Print config, Home status, subscription count and state path; exit |
| `-h`, `--help` | Print help and exit |
| `-V`, `--version` | Print version and exit |

`--check` contacts Reddit and exits with status 1 unless Home reports up and subscriptions load.
`--signin` cannot be combined with `--check`, `--config` or `--subreddit`.
