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
| Browser candidate evaluation | The detection iterator can stop after the requested candidate prefix | The broker evaluates every browser explicitly requested by `CookieQuery`, preserves that order, and never probes unlisted browsers | The M2 broker returns one merged candidate list for the provider pipeline |
| Browser partitioned cookies | No Linux import behavior to port | Chromium `top_frame_site_key` and Firefox `partitionKey` rows are dropped and counted | Partition scope is not represented by the provider cookie contract |
| Firefox expiry units | SweetCookieKit assumes millisecond expiry | `moz_cookies.expiry` is interpreted as seconds before schema 16 and milliseconds at schema 16 or later | Follow the Firefox schema version used by the stored database |
| Firefox containers | Container cookie rows are not exposed as independent Linux candidates | Each `userContextId` has its own candidate and safe label | Prevent cookie scopes from different Firefox containers being merged |
| Browser SQLite safety | Foreign database access may use SQLite recovery sidecars | Copy only the database and `-wal`; never copy `-journal`; open the copy with no-follow, trusted-schema off and defensive mode | A crafted hot journal can cause SQLite to delete a named super-journal during rollback |
| Browser profile containment | Browser detection may accept configured profile locations | Canonicalized profile paths must remain inside their discovered browser root; sandboxed absolute Firefox paths are refused | Prevent profile discovery from escaping the selected browser root |
| Browser profile labels | Upstream labels may include account details | Use only profile `name`; ignore account-name and email fields | Keep account identity data out of labels and diagnostics |
| macOS-only features | Widgets, iCloud sync, managed Codex accounts, Codex WebView dashboard extras | Bar segment and panel charts; a sync folder; managed Codex accounts ported; WebView extras over a cookie-authenticated API if feasible, otherwise deferred to the macOS phase | Linux has no equivalent of these macOS facilities |
| Windows cookies | Not applicable at the baseline | Firefox cookies or a manual header only; Chrome's App-Bound Encryption is never bypassed | Security and policy boundary |
| Snapshot and IPC | CodexBar's own JSON and placeholder flags | AriadUsage Snapshot v1 with an explicit metric state envelope | Honest data in every client; the contract is semantic, not byte-compatible |
| Configuration | CodexBar's `config.json` | AriadUsage's own XDG config, with no import and no shared file | No two codebases writing one file |
| Secret storage on Linux | Secrets in the config file | Default login collection through Secret Service; a trust-checked 0600 file only after consent and when the service is unavailable | Keep secrets out of plain config; avoid session collections and prompts during background work |
| Secret file fallback on macOS and Windows | Not applicable to the Linux baseline | Unsupported; Secret Service is unavailable and file fallback plus consent persistence are Linux-only | The current trust policy cannot guarantee a private 0600 file on these targets, so the operation fails closed |
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
| HTTP session lifecycle | Provider requests use ephemeral `URLSession` instances | AriadUsage reuses a main HTTPS client and a separate loopback client; neither has a cookie store | Connection pools can be reused without sharing cookie state |
| Response size | Provider HTTP responses are read without a common size cap | Net stops reading after the configured cap (5 MiB by default) | Bounds memory use for provider-controlled response bodies |
| Credential delivery | Provider request builders attach headers to their request | Net accepts credentials as `SecretString` values only after the request origin matches the caller's declared origins; redirects must remain on the original HTTPS origin | Makes the credential boundary explicit and prevents cross-origin delivery |
| Local requests | Provider HTTP transport policy handles remote requests | A separate client permits only literal `127.0.0.1` or `::1`, disables proxies and follows no redirects | Keeps local service traffic off proxy routes and prevents name resolution or redirect escape |
| Endpoint override private-network HTTP | Override validation is owned by provider-specific readers | The generic validator can allow HTTP for loopback or private-network hosts; Net currently sends only HTTPS or literal loopback requests | Preserve the validation behavior for future consumers without adding a private-network sending client |
| Net retries | CodexBar can retry selected transient idempotent requests | Net has no retry policy | No v1 provider consumes this option; the pipeline's existing delayed retry remains the only retry path |
| Delayed retry-after opt-in | Delayed retry mechanism present in core; none of the v1 providers at baseline sets classified delay | Typed `retry_after` on `ClassifiedError` is strictly opt-in (default `None`), normalized (finite >= 0 capped at 10 s) | Preserves exact baseline behavior for v1 providers while supporting rate-limited retry where configured |
| Child process environment | Some runners preserve the full ambient environment, including `TTYCommandRunnerTests.swift:173` | `ProcessEnv` copies exact allowlisted names, with only the `LC_*` and `XDG_*` families; provider additions are applied before that provider's denylist | Prevents loader, inspector and unrelated environment variables from reaching provider processes |
| Child secrets | CodexBar runners can receive provider environment values | AriadUsage never places a secret in a child environment or argv; callers use stdin or an owner-only file | Avoids exposing credentials through process inspection |
| Subprocess output caps | CodexBar truncates by default and can reject when strict output is requested | AriadUsage captures only up to the configured cap and fails with `OutputTooLarge` | Fail-closed memory and output handling is the accepted divergence |
| Inherited child descriptors | CodexBar closes extra descriptors through platform-specific spawn plumbing | AriadUsage does not close descriptors in the child; Rust opens descriptors close-on-exec and the systemd unit passes only descriptors 0–2 | Avoids an `unsafe` descriptor-closing exception; a test checks that an engine-owned file does not leak |
| Process signalling | CodexBar uses its platform process helpers | Linux signalling opens a pidfd, verifies uid and start time, and uses `pidfd_send_signal`; group signals require a verified live group member | Prevents PID reuse and excludes unverified processes from signalling |
| Process ownership during teardown | CodexBar uses the process tree and a per-launch marker reaper | Descendants and the process group of AriadUsage's own spawn use pid plus start-time identity; the marker scan requires the exact launch marker and excludes the engine, while the output-holder scan requires one of that launch's pipe inodes and excludes the engine and reaped root. Overflow cleanup skips the system-wide holder scan. | Keeps tree cleanup available for environment-cleared children while requiring explicit ownership proof for system-wide targets |
| Process shutdown and launch gate | CodexBar tracks TTY launches and pauses repeated background Codex launches after non-PTY launch failures | AriadUsage fences new process registrations at shutdown, terminates registered targets, and suppresses background launches of the same binary for 30 minutes after a process launch failure. User-initiated launches proceed; PTY infrastructure failures do not set the gate. | Keeps shutdown bounded and avoids repeating a known failing background launch while preserving manual recovery |
| PTY session leadership | CodexBar's PTY runner owns its platform-specific terminal setup | `pty-process` makes each PTY child a session leader with a controlling terminal | The crate provides the required session setup without workspace `unsafe` code |
| PTY signal mask | CodexBar's spawned-process-group tests cover signal-mask reset | `pty-process` does not reset the inherited signal mask; the PTY child inherits a blocked signal mask when the parent enters spawn with one, and a test asserts the observed behavior | The PTY crate does not expose a safe signal-mask reset hook; record the gap rather than add `unsafe` |
| Process test children | CodexBar process tests use platform helpers and scripts | AriadUsage uses a Rust helper binary gated by `test-hooks`, with a release check that excludes it from shipped builds | Tests exercise real child behavior without interpreter scripts or production hooks |
| Unported diagnostic export tests | `ProviderDiagnosticExportTests.swift:396-424` (substring classifier) and `:472-488` (legacy attempt reader without strategy ID) | Not ported in M1; replaced by typed `DiagnosticError` and `DiagnosticFetchAttempt` tests | AriadUsage does not use substring error classification or legacy un-attributed attempt records |
| Mise and asdf shim paths | Well-known directories followed immediately by PATH and login shell search | `~/.local/share/mise/shims` and `~/.asdf/shims` appended after well-known locations and before `command -v` | Probes common Linux version-manager shims when shells do not export them to non-interactive PATH |
| Executable trust verification | Checks basic file executable bits only | Candidates in group- or world-writable directories without sticky bit (`0o1000`) or owned by foreign UIDs are rejected as `Untrusted` | Prevents privilege escalation or binary substitution from attacker-writable or foreign-owned directories |
| Login shell marker string | Uses `__CODEXBAR_PATH__` sentinel marker in captured shell output | Uses `__ARIADUSAGE_PATH__` sentinel marker | AriadUsage namespace isolation; avoids collision or coupling to CodexBar environment markers |
| Credential stat fingerprint | `(mtime, size)` or path-only | `StatFingerprint (path, dev, ino, mtime_ns, size)` | Protects against same-size atomic file replacements within the same millisecond/second timestamp resolution or inode re-use across distinct files while avoiding SHA-256 hashes on unchanged files |
| Foreign credential file reads | Reads any readable file at the path | Requires `st_uid == euid` on opened file descriptor; wrong owner returns `Untrusted` | Prevents unauthorized cross-user credential reads on multi-user Linux systems |
| Delegated refresh success observation | Observes macOS keychain item modification and token changes | Observes the target credential file's `StatFingerprint` within 2 seconds at 0.2, 0.5, and 0.8 seconds | Linux has no keychain item modification observer; credential files reflect token writes from external CLI / OAuth touches |
| Broker state persistence | Transient memory or ad-hoc defaults; macOS keychain for tokens | `$XDG_STATE_HOME/ariadusage/broker-state.json` (0600 mode, digests and timestamps only, lock file, auto-repair 0644, reset on corrupt) | Preserves delegated-refresh cooldowns and quarantine state across engine restarts without persisting secret tokens, credential text, or foreign path strings |
| Cookie domain matching | SweetCookieKit / CodexBar uses string substring / contains check (`host.contains(domain)`) | `domain_matches` requires exact host match or dot-suffix (`.domain`); leading dots stripped | Security divergence: `notclaude.ai` must never match `claude.ai` |
| Cookie cache storage | In-memory `CookieHeaderCache` with conditional mutation and fingerprinting | In-memory `CookieCache` with `ConditionalMutationCoordinator` and SHA-256 fingerprinting | Parity with CodexBar baseline (Q5 resolved) |
| Managed-account cookie scopes and refresh-read suppression | Managed-account cache scopes and UI refresh-read suppression in `CookieHeaderCache` | Deferred: managed-account scopes deferred to M4; refresh-read suppression deferred to M8 with its UI caller | Keeps M2 scope focused on core cookie cache and header delivery |


## Upstream drift

CodexBar keeps changing the three v1 providers. Changes made upstream after the
baseline are reviewed by diffing CodexBar's Claude, Codex and Antigravity
provider directories against the baseline. Anything relevant goes into the
post-v1 backlog; v1 does not chase upstream. Moving the baseline is an
architecture decision and is recorded in the Decision Log.
