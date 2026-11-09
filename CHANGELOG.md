# Changelog

## [0.1.1] - 2026-11-09

### Bug fixes

- Replace implicit state machines with explicit types and release leaked image tickets

## [0.1.0] - 2026-11-07

### Features

- Initial terminal Reddit reader on top of Redlib
- Add inline video, spoilers, thumbnails, and sidebar navigation
- **cache:** Add persistent disk cache with stale-while-revalidate
- Add GIF playback, side-by-side post view, and inline pictures in text
- Preview posts beside the list and hold back loads during quick navigation
- Fetch Reddit directly via guest sessions instead of Redlib
- Add browser cookie extraction via rookie-cookies
- **reddit:** Add account tools, participation actions and comment sorting
- **ui:** Implement full-width media and improve media display logic
- **scheduler:** Prioritize selected reads and release workers from obsolete requests
- Add user following, account-scoped settings and YouTube playback
- **home:** Stream native Home cards without separate post lookups
- **video:** Cache recent video frames and drive playback from ffplay's audio clock
- Implement URL opening and search exploration
- **reddit:** Render rich text bodies and add a text link picker
- **keys:** Replace shortcuts with Space-prefixed mnemonic action groups and structural navigation
- Buffer feed updates behind an r refresh notice and add video still previews
- Add Search tab, direct key actions, and feed update rules
- **search:** Search comments and media through Reddit's rendered search pages
- **auth:** Default to the login window and decrypt macOS Chromium cookies in-process

### Bug fixes

- Clear Rust 1.99 lints

