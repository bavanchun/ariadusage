# AGENTS.md

Rules for AI agents working in this repository. Design authority is [ARCHITECTURE.md](ARCHITECTURE.md); read the section you are touching before changing structure, stack, contract or boundaries.

## Current state

- The repository contains the Cargo workspace with the `ariadusage` CLI binary skeleton (`crates/ariadusage-cli`), the `ariadusage-protocol` crate with Snapshot v1 and IPC v1 schemas (`schemas/`), the Omarchy plugin prototype (`integrations/omarchy/`), and design-as-code brand assets (`brand/`).
- Build, test, lint, schema, brand and plugin recipes are defined in [justfile](justfile). Toolchain and runtime versions are pinned in [rust-toolchain.toml](rust-toolchain.toml) and [.node-version](.node-version).
- Commands:
  - `just ci`: runs the full local quality gate (`ci-linux`, `brand-ci`, `omarchy-check`, and `secrets`).
  - `just push`: runs `secrets` then pushes the current branch.
  - `just schemas`: regenerates and blesses JSON schemas for Snapshot v1 and IPC v1.
  - `pnpm --dir brand build`: builds brand SVG and PNG assets.
  - `just brand-ci`: runs frozen pnpm audit, brand build, contrast checks and SVG git diff.
  - `just omarchy-dev`: assembles the plugin and syncs it to `~/.config/omarchy/plugins/`.
  - `just omarchy-dev-enable`: snapshots `shell.json` and enables the plugin in Omarchy.
  - `just omarchy-dev-remove`: disables the plugin, cleans backups and restores `shell.json`.
  - `just omarchy-check`: runs `omarchy-plugin-validate`, strict `qmllint -W 0`, and source contract scans.

## Architecture and protocol rules

- Do not add a crate, dependency, service or boundary that contradicts ARCHITECTURE.md. If a change is warranted, update ARCHITECTURE.md, including its "Decision Log" table, in the same change.
- Providers are modules of `ariadusage-engine`, never their own crates. `ariadusage-protocol` and `ariadusage-core` stay free of I/O and `cfg(target_os)` in their default features. The one exception is `ariadusage-protocol`'s dev-only `fixture` feature, which no shipped crate may enable.
- Create a crate only together with its first real code.
- IPC v1 is a draft until frozen before the first plugin release in M9. Changes to protocol types must pass schema drift checks and be blessed with `just schemas`.

## Parity with CodexBar

- The baseline is `steipete/CodexBar` at `6a26b2e9b1b60471970deb6fe663f9e5f284e2ce`. Port behavior from that commit only; upstream changes after it go to the post-v1 backlog.
- CodexBar's code and tests are the specification. Its docs are not: where they disagree with code, follow the code.
- Record every deliberate divergence in [docs/porting.md](docs/porting.md) in the same change.
- Never run `codexbar`, and never read CodexBar's config, cache or state on this machine.

## Security

- Do not read real provider credentials, tokens, cookies, browser profiles or keyrings unless the owner asks for a live smoke test in the current session.
- Never print a token, cookie, key, email or account identifier. Report counts or `[redacted]`.
- Use invented values in tests, fixtures, schema examples and docs. Never commit real account data.
- Never put a secret on argv, in QML, in a log line, in a notification or in a socket push.
- QML never sends `setSecret`. Secrets are typed into `ariadusage secret set`, which the panel may only launch in a terminal.
- Live smoke tests run locally on the owner's request only. Never add one to CI.

## Omarchy plugin (`integrations/omarchy/`)

- Render every external string with `textFormat: Text.PlainText`.
- Call executables by absolute path with an argv array, a timeout and an output cap. Never build a shell command from data.
- Never write anything inside the plugin directory at runtime.
- Never add a symlink, a binary, a `.service` file, `AGENTS.md`, `CLAUDE.md`, or a file whose name matches `install`, `installer`, `setup` or `uninstall` as a word (name settings files `SettingsForm.qml`, not `Setup.qml`).
- The plugin must pass `omarchy-plugin-validate` and `qmllint -W 0 -I "$OMARCHY_PATH/shell" -i "$OMARCHY_PATH/shell/Commons/qmldir" -i "$OMARCHY_PATH/shell/Ui/qmldir"` with exit 0. Plain `qmllint -I` is not a gate.
- Do not interact with the first-party `omarchy.agents` plugin.
- Ask the owner before enabling a plugin or editing anything under `~/.config/omarchy/`.

## Versions

- Use the latest LTS where a channel exists, otherwise the latest stable release, unless the ARCHITECTURE.md Decision Log records an exception. Never alpha, beta or RC builds.
- Verify a version against its registry on the day you pin it. Do not pin from memory or from a research table.
- Pin through lockfiles and commit each lockfile with its manifest. Do not repeat version numbers in docs; point to the manifest.
- Package managers: `cargo` for Rust, `pnpm` for the brand pipeline. Do not use npm or yarn. Root tasks go into `just` recipes.

## Licensing

- Start every ported source or test file with: `Ported from CodexBar <path> at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt`. Record ported fixtures in `fixtures/manifest.toml`.
- Do not add a dependency whose license `deny.toml` does not allow. Never link GPL, LGPL or AGPL code.

## Brand assets

- `brand/svg/`, `brand/png/`, `brand/icon-composer/` and `brand/preview.png` are generated. The build deletes and rewrites them, so never hand-edit them.
- Change `brand/src/*.mjs`, `brand/scripts/*.mjs` or `brand/build.mjs`, then run `pnpm --dir brand build` or `just brand-ci`.
- After a build, inspect `brand/preview.png` and verify the 16 px and 32 px tiles before committing.
- Commit the sources and the regenerated outputs together.
- Keep SVGO's `prefixIds` plugin enabled. Without it, symbol and gradient IDs collide when multiple SVG assets are inlined into a single document.
- Design rules, color tokens and rationale live in [docs/brand/design-direction.md](docs/brand/design-direction.md). Update that document whenever brand design decisions evolve.
- Never use an image-generation model for brand assets; all marks and icons are produced deterministically through design-as-code scripts.

## Docs and language

- Write every repository file in English. Reply to the owner in Vietnamese.
- Root Markdown is limited to `README.md`, `ARCHITECTURE.md`, `AGENTS.md` and `CLAUDE.md` (which only imports this file). Other Markdown goes under `docs/`.
- Never create or commit a `plans/` directory or plan content in this repository. Plans and reports are private and live outside it.
- Never link to a path outside the repository or to a home-directory path.

## Git

- Follow [docs/git-workflow.md](docs/git-workflow.md).
- Every change reaches `dev` through a short-lived `<type>/<topic>` branch and a pull request with `CI result` green. Never push directly to `dev` or `main`.
- Commit only when the owner asks; an authorized plan run counts as that request within the plan's scope.
- Pushes, pull requests, merges, tags and GitHub settings need the owner's explicit go-ahead.
- Stage explicit paths. Never run `git add -A`, `git add .` or `git commit -a`.
- Use conventional commits with no AI, tool, plan or phase references.
