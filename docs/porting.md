# Porting from CodexBar

AriadUsage reimplements CodexBar's behavior in Rust. This page holds the rules
for carrying that behavior over and the record of every deliberate divergence.

## Baseline

The parity baseline is [steipete/CodexBar](https://github.com/steipete/CodexBar)
at commit `6a26b2e9b1b60471970deb6fe663f9e5f284e2ce`. v1 matches that commit
for Claude, Codex and Antigravity, and nothing later.

## What counts as the specification

In order of precedence:

1. A divergence recorded in [Divergences](#divergences) below, or a decision in
   the [ARCHITECTURE.md Decision Log](../ARCHITECTURE.md#14-decision-log).
2. CodexBar's tests at the baseline.
3. CodexBar's source code at the baseline.
4. CodexBar's documentation. It has drifted from the code in places: for
   example, its CLI docs say Codex `auto` tries the web source first, while the
   code resolves `auto` to personal access token, then OAuth, then CLI. Trust
   the code.

Never run `codexbar` to discover behavior, and never read CodexBar's config,
cache or state. Read its code and tests.

## Translation conventions

- **Typed errors.** CodexBar classifies many errors by matching substrings of
  localized messages. Port them as typed error kinds that carry their category
  and retry hint; do not port the substring classifier.
- **Explicit context.** CodexBar passes interaction mode, refresh phase and
  request id through Swift task-local values. Put them, together with the
  runtime (daemon or one-shot CLI) and a cancellation token, on the fetch
  context explicitly.
- **Cancellation is not a failure.** A cancelled fetch never tries the next
  strategy and never replaces last-good data.
- **Required and optional work stay separate.** Never join a required and an
  optional future in a way that loses the required failure; bound optional work
  with a timeout and abort it.
- **Provider ids are validated strings**, not a closed enum, with CodexBar's
  rule (1–64 characters of `a-z`, `0-9` and `-`), because most future providers
  will be plugins.
- **Serde fidelity where a byte-level round trip is tested.** AriadUsage's
  Snapshot v1 is its own contract, but ported config and fixture round-trip
  tests keep CodexBar's encoding rules: omit-when-default fields stay omitted,
  window keys that CodexBar writes as explicit `null` stay explicit, opaque
  provider entries keep their exact bytes, and output keys are sorted.
- **Dates** encode as RFC 3339 UTC at whole seconds and decode with or without
  fractional seconds.

## Attribution

Every ported source or test file starts with a comment in this form:

```text
Ported from CodexBar <path> at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
```

`<path>` is the file's path in the CodexBar repository. A file that combines
several sources lists each path. The full CodexBar license is in
[LICENSES/CodexBar-MIT.txt](../LICENSES/CodexBar-MIT.txt) and is referenced
from [NOTICE](../NOTICE).

## Fixture provenance

Fixtures cannot always carry a comment, so `fixtures/manifest.toml` (from the
core milestone onward) records, for each fixture: its CodexBar source path and
baseline commit, its license, and any change made to it. AriadUsage's own
fixtures are recorded there too. Fixtures contain only CodexBar's fixture data
and invented values, never a real account.

## Divergences

Record a divergence in the same change that introduces it. Add a row here; if
it changes the architecture, also add a Decision Log row.

| Area | CodexBar at the baseline | AriadUsage | Reason |
|---|---|---|---|
| Browser cookies on Linux | Manual cookie header only | Automatic Chromium-family and Firefox import, opt-in per provider and limited to declared domains; manual header kept | Intentional superset approved for Linux |
| macOS-only features | Widgets, iCloud sync, managed Codex accounts, Codex WebView dashboard extras | Bar segment and panel charts; a sync folder; managed Codex accounts ported; WebView extras over a cookie-authenticated API if feasible, otherwise deferred to the macOS phase | Linux has no equivalent of these macOS facilities |
| Windows cookies | Not applicable at the baseline | Firefox cookies or a manual header only; Chrome's App-Bound Encryption is never bypassed | Security and policy boundary |
| Snapshot and IPC | CodexBar's own JSON and placeholder flags | AriadUsage Snapshot v1 with an explicit metric state envelope | Honest data in every client; the contract is semantic, not byte-compatible |
| Configuration | CodexBar's `config.json` | AriadUsage's own XDG config, with no import and no shared file | No two codebases writing one file |
| Secret storage on Linux | Secrets in the config file | Secret Service keyring; a 0600 file only with consent | Keep secrets out of plain config |

## Upstream drift

CodexBar keeps changing the three v1 providers. Changes made upstream after the
baseline are reviewed by diffing CodexBar's Claude, Codex and Antigravity
provider directories against the baseline. Anything relevant goes into the
post-v1 backlog; v1 does not chase upstream. Moving the baseline is an
architecture decision and is recorded in the Decision Log.
