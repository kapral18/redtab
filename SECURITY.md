# Security policy

Report a vulnerability privately through
[GitHub security advisories](https://github.com/kapral18/redtab/security/advisories/new).
Don't open a public issue for it.

Only the latest release gets fixes. Each fix ships as a new version through the
usual release channels: crates.io, Homebrew and GitHub releases.

redtab stores your Reddit web session in the macOS Keychain or Linux Secret
Service. A report about how that session is stored, sent or exposed is in scope;
see [accounts](docs/accounts.md) for how sign-in works.
