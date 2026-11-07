# RedTab

Reddit in your terminal, with tabs, inline media and keyboard controls.

![Animated demo of redtab browsing Reddit in the terminal](docs/assets/terminal.gif)

- Browse Home, subscriptions and subreddit tabs.
- View images and galleries, play videos and GIFs, and read discussions side by side.
- Search Reddit, follow comment threads, vote, save, reply and post.
- Switch accounts, bookmark posts and keep unread counts across restarts.

Sign in with your browser's Reddit session, saved in your OS credential store.
No Reddit API registration is needed.
Reddit can change the browser endpoints redtab uses; updates may be needed when it does.

## Get started

Requires Rust 1.90+, CMake, a C/C++ compiler and libclang.
See [installation](docs/guide.md#install) for platform requirements.

```sh
cargo install --git https://github.com/kapral18/redtab --locked
redtab
```

On first launch, select your signed-in browser profile and confirm your account.
Press `Space` for action groups and `?` for help. Install `ffmpeg` for inline video.

## Documentation

[User guide](docs/guide.md) · [Accounts](docs/accounts.md) ·
[Settings](docs/configuration.md) · [Keyboard shortcuts](docs/keybindings.md) ·
[Privacy](docs/accounts.md#data-and-privacy) · [Development](docs/development.md)

## License

[MIT](LICENSE)
