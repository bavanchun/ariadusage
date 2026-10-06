# Git workflow

How changes move into this repository. It applies to people and AI agents alike. [AGENTS.md](../AGENTS.md) holds the short rules. This file explains them and gives the exact steps.

## Model

- Two long-lived branches:
  - `dev` is the integration branch. All work lands here.
  - `main` is the stable branch. It receives `dev` only through a promotion, and urgent fixes through a hotfix (see [Promotion and hotfixes](#promotion-and-hotfixes)). Release tags are cut on `main` only.
- Nothing is pushed directly to `dev` or `main`. Every change reaches them through a pull request, and the single required status check, `CI result`, must be green before it merges. Neither branch has a ruleset bypass.
- Work on a short-lived branch from `dev` named `<type>/<short-kebab-topic>`, for example `feat/protocol-metric-envelope`, `fix/socket-permissions` or `ci/pin-actions`. Delete the branch after it merges.
- GitHub's default branch stays `main`, so always pass the base explicitly: `gh pr create --base dev`.
- Never force-push `main` or `dev`, and never rewrite history that has already been pushed.

## Commit often

Commit at every logical step that leaves the tree working. Do not save everything for one large commit at the end.

A good step is one of these:

- one crate, module or recipe that compiles and passes its tests;
- one decision recorded in a document;
- one test suite added together with the code it covers;
- one ported CodexBar test or fixture set, with its attribution;
- one dependency or toolchain bump, with its lockfile.

Rules for every commit:

- **Atomic.** One purpose per commit. If the message needs "and" to join two unrelated changes, split it.
- **Green.** It builds and passes the checks relevant to what it touched (see [Before you commit](#before-you-commit)). A red intermediate commit breaks `git bisect`.
- **Complete.** Lockfiles go in with their manifests. Generated outputs go in with their sources: JSON Schemas under `schemas/`, `insta` snapshots and `brand/` outputs.
- **Reviewed.** Read `git diff --staged` before every commit.

## Commit messages

Use [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/):

```text
<type>(<scope>): <subject>

<body: why the change was made and what it affects, wrapped at 72 columns>

<footer: BREAKING CHANGE: …, Refs: #123>
```

- **Subject:** imperative mood, lower case, no trailing period, at most 72 characters. Write "add metric envelope", not "added" or "adds".
- **Body:** optional for a trivial change, expected otherwise. Explain the reason and any trade-off. The diff already shows what changed.
- **Never** mention AI tools, agents, plan IDs, phase numbers or audit labels. Describe the behavior or invariant instead.

| Type | Use for |
|---|---|
| `feat` | New user-visible behavior |
| `fix` | A bug fix |
| `docs` | Documentation only |
| `test` | Tests only |
| `refactor` | A code change with no behavior change |
| `perf` | A performance improvement |
| `build` | Cargo, pnpm, `justfile` or toolchain pins |
| `ci` | GitHub Actions workflows |
| `chore` | Repository housekeeping that fits nothing else |

Scopes follow the code layout and the v1 providers: `protocol`, `core`, `engine`, `cli`, `claude`, `codex`, `antigravity`, `cost`, `omarchy`, `brand`, `fixtures`, `schemas`, `deps`, `ci`. Leave the scope out when a change spans the whole repository.

Examples:

```text
feat(protocol): add the metric state envelope
fix(codex): keep last-good usage when the app server times out
test(claude): port the OAuth delegated refresh tests
build(deps): pin secret-service
```

## Before you commit

1. Run the narrowest check for what you touched, then broaden it (`just fmt`, `just lint`, `just test`, `just deny`).
2. Run `just ci`: it runs the complete quality gate (`fmt-check`, `clippy` with warnings denied, `typos`, `nextest`, `cargo-deny`, and `secrets`). Recipes are defined in [justfile](../justfile), and toolchain requirements are pinned in [rust-toolchain.toml](../rust-toolchain.toml).
3. Check `git status` for stray files: editor backups, `target/`, `node_modules/`.

## Staging

Stage explicit paths:

```bash
git add crates/ariadusage-protocol/src/metric.rs crates/ariadusage-protocol/Cargo.toml Cargo.lock
git diff --staged
git commit
```

Never run `git add -A`, `git add .` or `git commit -a` at the repository root. They pick up unrelated files and local run artifacts.

Never commit `.env*` files, credentials, private keys, tokens, cookies, real account data, `node_modules/`, `target/` or a `plans/` directory. `fixtures/` takes only CodexBar's MIT fixtures and invented data, with provenance recorded in `fixtures/manifest.toml`.

## Pushing

- Commits leave the machine through `just push`: it runs `just secrets` (including `scripts/gitleaks-selftest.sh` and a gitleaks scan over committed history) and then pushes the current branch (`git push -u origin HEAD`).
- Push your branch after a group of green commits, and at least at the end of each working session, so CI checks your work early.
- Watch the run with `gh run watch`. A red branch is fixed on that branch before new work is stacked on it.
- To fix an unpushed commit, use `git commit --amend` for the latest one. Once a commit is pushed, fix it with a new commit.
- Commit hooks are never skipped with `--no-verify`.

## Pull requests

- Keep each pull request to a single purpose. Its title follows the commit-message format.
- In the description, state what changed, why, and how it was verified.
- Merge only when `CI result` is green. Rebase onto the base branch instead of merging it in. Merge into `dev` with "Rebase and merge" so each commit stays on its own; squash only when the branch commits are noise.

## Promotion and hotfixes

- **Promotion.** When `dev` is stable (CI green, no known regression) and the maintainer agrees, open a pull request from `dev` into `main` titled `chore: promote dev to main`. Merge it with a merge commit (`gh pr merge --merge`), never squash or rebase, so the promoted commits keep their SHAs and the next promotion stays a clean merge.
- **Hotfix.** Branch `fix/<topic>` from `main`, open the pull request into `main`, and merge it once `CI result` is green. Then bring `main` back into `dev` through a pull request so the fix is not lost at the next promotion. Dependabot security updates always target `main` and follow this flow.
- Promotions, hotfixes into `main` and tags are outward-facing. Agents perform them only on the maintainer's explicit go-ahead for that action.

## Release tags

Two tag namespaces, both cut on `main`:

- **`v*`** releases the engine. The release carries the source tarball that the AUR package `ariadusage` builds from. Bump the version on `dev` through the usual flow, promote, then tag the resulting `main` commit.
- **`omarchy-v*`** releases the Omarchy plugin. The publishing workflow assembles the plugin tree from `integrations/omarchy/`, validates it, and pushes it to `bavanchun/ariadusage-omarchy` only after the maintainer approves the protected environment. The plugin's `manifest.json` version must equal the tag version.

An engine release never moves the plugin repository, and a plugin release never forces an AUR rebuild.

While the plugin is under Omarchy marketplace review, its default branch must not move. Set the repository variable `OMARCHY_FREEZE` to `true` for the whole review: the publishing job then fails before it reads any secret. Unset it after the listing, then publish.

## Working through a plan

Plans are private. They live outside this repository and are never committed, linked or quoted here, in commits, in issues or in pull requests.

A plan run executed wave by wave (one or two phases at a time) uses one branch and one pull request per wave:

1. Before dispatching a wave, the coordinator creates a branch from the current `dev`, named after the wave's topic (for example `feat/protocol-ipc-v1`).
2. The phases of the wave commit on that branch. A second, parallel phase works in its own worktree and is rebased onto the wave branch once accepted.
3. When every phase of the wave is accepted and `just ci` passes on the integrated branch, the coordinator pushes it, opens a pull request into `dev`, and rebase-merges it once `CI result` is green. A red check is repaired on the same branch before the next wave starts.
4. The coordinator deletes the branch, updates its local `dev` (`git pull --ff-only`) and starts the next wave from there.

A plan run never promotes `dev` to `main` and never tags a release on its own.

## AI agents

AGENTS.md says agents commit only when the user asks. Running an authorized plan counts as that request for commits inside the plan's scope. Pushes, pull requests, merges, repository settings and anything else outward-facing still need the user's explicit go-ahead; the user can grant pushes and pull request merges into `dev` for a whole plan run. Agents follow every rule in this file. They stage explicit paths only, and they never put AI or tool references in commit messages.
