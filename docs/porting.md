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
| Error text on wire | Free-form provider exception and diagnostic strings | Fixed static safe description of the error category (`ProviderErrorCategory`) | Free-form provider detail or raw tokens must never reach clients or UI over IPC |
| Rate window duration on wire | Unbounded signed integers or negative durations | Clamped: non-positive (`<= 0`) or overflowing (`> u32::MAX`) durations project to `None` | Wire protocol `RateWindow.windowMinutes` is unsigned `Option<u32>` and represents meaningful positive durations |
| Extreme numeric boundary tests | `TestsLinux/ProviderNumericBoundaryTests.swift:79` constructs resets at `8e23` seconds | Re-expressed as monthly boundary calculation at `jiff::Timestamp::MAX` (year 9999) without overflow | `jiff::Timestamp` cannot represent years beyond 9999; verified safe at the maximum representable boundary |
| Secrets in configuration | API keys, cookie headers, secret keys, plugin secrets, and account tokens stored in plain `config.json` | Kept out of plain config; if present in config, flagged with `secret_in_config`, never used, and redacted in dump and `Debug` | Security boundary: plain config files must never retain credentials |
| Secret presence for validation | Provider validators inspect plaintext secret values directly in config | Validators receive abstract `SecretPresence` flags without reading secret values | Least privilege; validation and config inspection do not require plaintext secrets |
| Supported provider set | Hardcoded enum covering first-party and community providers | Typed first-party providers (`codex`, `claude`, `antigravity`); all others preserved as byte-exact opaque entries | Extensibility: unknown providers survive round-trips without schema lock-in |
| Config file format and serialization | Swift JSONSerialization with custom option formatting | `serde_json` pretty format with sorted keys; trailing newline added by store | Clean diffs, deterministic encoding, and byte stability |
| Application settings section | App-level settings mixed with provider configuration or defaults | Dedicated top-level `settings` object preserved byte-for-byte as raw JSON | Isolates global application settings from provider records without data loss |
| Unknown top-level configuration keys | Silently discarded or unsupported | Preserved byte-for-byte in `extra_top` map on decode and re-encode | Forward compatibility for future schema extensions |
| Hooks configuration section | Shell command hooks parsed and validated by config model | Preserved byte-for-byte as raw JSON until Milestone 7 | Lifecycle hooks execution engine and validation deferred to M7 |
| Config path override | `CODEXBAR_CONFIG` environment variable | `ARIADUSAGE_CONFIG` (trimmed, tilde-expanded, must be absolute after expansion, relative values return an error) | AriadUsage namespace; strict absolute validation |
| Config path resolution | Probes `$HOME/.config/codexbar`, then legacy `$HOME/.codexbar`, with filesystem existence checks | Pure resolver: `XDG_CONFIG_HOME` if absolute (relative ignored) -> `$HOME/.config/ariadusage/config.json`. No legacy or existing-file probing, never any CodexBar path | Clean XDG resolution without filesystem probes or coupling to old paths |
| Locked read-modify-write | Separate read and write with write-only lock | `update` and `try_update` hold lock across load, modify, and write; `try_update` skips on contention | Prevents lost updates during concurrent refresh or CLI changes |
| Parent directory fsync | Fsyncs staged file descriptor only | Fsyncs staged file descriptor AND parent directory after atomic rename | Guarantees rename metadata durability across crashes |
| Config file and lock trust | Follows symlinks, basic file checks | `O_NOFOLLOW` opens, `fstat`, regular file and effective UID ownership checks on both files; the configuration file additionally must not be group- or world-writable, while the lock file is only checked for symlink, regular file and owner; parent directory must not be group- or world-writable or owned by another user | Hardened security: prevents symlink traversal, privilege escalation, or unauthorized file overwrites |
| Typed error classification and transport | Substring matching on localized error messages (`errorCategoryLabel`), `NSURLError` tables, and substring fallbacks | Typed `ProviderErrorKind`, `ProviderErrorCategory`, and `TransportClass`; engine strategies must set `transport` explicitly | Eliminates brittle localized substring matching and platform-specific `NSError` code tables |
| Delayed retry-after opt-in | Delayed retry mechanism present in core; none of the v1 providers at baseline sets classified delay | Typed `retry_after` on `ClassifiedError` is strictly opt-in (default `None`), normalized (finite >= 0 capped at 10 s) | Preserves exact baseline behavior for v1 providers while supporting rate-limited retry where configured |
| Unported diagnostic export tests | `ProviderDiagnosticExportTests.swift:396-424` (substring classifier) and `:472-488` (legacy attempt reader without strategy ID) | Not ported in M1; replaced by typed `DiagnosticError` and `DiagnosticFetchAttempt` tests | AriadUsage does not use substring error classification or legacy un-attributed attempt records |

## Upstream drift

CodexBar keeps changing the three v1 providers. Changes made upstream after the
baseline are reviewed by diffing CodexBar's Claude, Codex and Antigravity
provider directories against the baseline. Anything relevant goes into the
post-v1 backlog; v1 does not chase upstream. Moving the baseline is an
architecture decision and is recorded in the Decision Log.
