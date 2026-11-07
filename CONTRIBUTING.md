# Contributing

Bug reports and pull requests are welcome.

- Check [development](docs/development.md) for the checks to run, the test rules and
  where things live.
- Give your pull request a [Conventional Commits](https://www.conventionalcommits.org)
  title such as `fix(search): keep the query after a refresh`. Pull requests are
  squash-merged, and the title decides the release: `feat` releases a minor version,
  `fix` and `perf` a patch, anything else no release.
- Keep a pull request to one change, and add a test only when it guards behavior a
  user or caller relies on.
- Tests never contact Reddit. Check anything that needs a real account, terminal or
  browser by hand, and say what you checked in the pull request.

Report security problems privately, as [SECURITY.md](SECURITY.md) describes.
