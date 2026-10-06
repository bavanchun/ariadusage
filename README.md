# AriadUsage

AriadUsage tracks the usage, quotas and cost of AI coding assistants on Linux.
It is an independent Rust reimplementation of
[CodexBar](https://github.com/steipete/CodexBar): the same ideas and behavior,
written in a different language, with no runtime dependency on CodexBar.

The first release targets [Omarchy](https://omarchy.org). It covers three
providers at full parity with CodexBar: **Claude**, **Codex** and
**Antigravity**. More providers, other Linux desktops, macOS and Windows come in
later releases.

## Status

AriadUsage is in early development. The repository currently holds design
documents only, and there is nothing to install yet.

The planned install route has two parts, neither of which exists yet:

- the `ariadusage` engine (one binary with a CLI and a background daemon),
  packaged on the AUR;
- a thin Omarchy bar-widget plugin, `io.github.bavanchun.ariadusage`, listed on
  the Omarchy plugin marketplace.

## Project documents

- [System architecture](ARCHITECTURE.md): design, roadmap and decision log
- [Privacy](docs/privacy.md): what AriadUsage reads, writes and sends
- [Porting rules](docs/porting.md): how CodexBar behavior is carried over
- [Git workflow](docs/git-workflow.md)
- [Security reporting](docs/SECURITY.md)

## Credits

- [CodexBar](https://github.com/steipete/CodexBar) by Peter Steinberger, MIT.
  AriadUsage ports its behavior, tests and fixtures; see [NOTICE](NOTICE) and
  [LICENSES/CodexBar-MIT.txt](LICENSES/CodexBar-MIT.txt).
- Provider logos will come from [lobe-icons](https://github.com/lobehub/lobe-icons)
  (MIT). Trademarks belong to their owners.

## License

MIT. See [LICENSE](LICENSE).
