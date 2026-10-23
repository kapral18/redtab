# redtab

Reddit in your terminal, with tabs, full-size media and a keyboard-driven workflow.

- Browse your personalized Home feed, subscriptions and subreddit tabs.
- Read full-width image cards and galleries, play videos and GIFs inline, and open
  discussions side by side.
- Follow threaded comments, search Reddit, vote, save, reply and post.
- Switch accounts and keep your bookmarks, reading history and cached posts close at hand.

Sign in with your browser's existing Reddit session, stored in your OS credential store.
No Reddit API registration is needed.

## Get started

Requires Rust 1.90+, CMake, a C/C++ compiler and libclang.
See the [installation guide](docs/guide.md#install) for platform requirements.

```sh
cargo install --git https://github.com/kapral18/redtab --locked
redtab
```

On first launch, select your signed-in browser profile and confirm your Reddit account.
Press `?` for keyboard help. Add `ffmpeg` for inline video.

## Learn more

[User guide](docs/guide.md) · [Accounts and sign-in](docs/guide.md#usage) ·
[Configuration](docs/guide.md#configure) · [Keyboard shortcuts](docs/guide.md#keys) ·
[Data and privacy](docs/guide.md#data-and-privacy) · [Development](docs/guide.md#development)

## License

[MIT](LICENSE)
