# Porting from CodexBar

AriadUsage reimplements CodexBar's behavior in Rust. This page holds the rules
for carrying that behavior over and the record of every deliberate divergence.

## Baseline

The parity baseline is [steipete/CodexBar](https://github.com/steipete/CodexBar)
v0.79.0, at commit `b49d42eb53bbf79785b344492d594760dd163eac`. v1 matches that
commit for Claude, Codex and Antigravity, and nothing later.

The baseline moved from `6a26b2e9b1b60471970deb6fe663f9e5f284e2ce` to v0.79.0
so that CodexBar's source and the released app the owner runs describe the
same behavior and UI. Code, tests and fixtures ported from the old baseline
keep its hash in their header and manifest entry until they are re-checked
against v0.79.0: diff the upstream files between the two commits, port the
changes, then update the header and the manifest entry in the same change.

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

Never run the `codexbar` CLI to discover behavior, and never read CodexBar's
config, cache, state or Keychain items. Read its code and tests.

For the macOS app's UI and UX, captures of the running CodexBar app built from
the baseline commit are a visual reference next to the source: they show how
the source renders, but where a capture and the source disagree, the source
wins. Captures are made only on the owner's request and stay outside the
repository, because they show real account data.

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
Ported from CodexBar <path> at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
```

Files not yet re-checked against v0.79.0 still carry `at 6a26b2e9b`.

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
| Browser keyring retry allowance | Record reads start without Keychain UI; background reads proceed without UI and skip denial cooldowns; one interactive read per user action may follow, and only a CLI retry run with `--allow-keychain-prompt` (`withExplicitRetry(bypassingCooldown: true)`) passes a browser or Chromium-family denial cooldown | A user-initiated call reads each browser's Secret Service key at most once and may unlock the keyring; a dismissal closes the call to further key reads; only an acknowledged user-initiated retry may make one key read through a cooldown; background reads skip the cooldown and proceed noninteractively (`allow_unlock=false`) | Secret Service has no no-UI read of a locked item to try first, and one unlock covers the whole collection |
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
| PTY terminal-side descriptor | The runner keeps its copy of the PTY terminal side open until the child exits, reads what is pending, then closes it and drains to EOF, because Darwin can discard unread output when the last terminal-side descriptor closes | The Linux broker hands the terminal side to the child at spawn and keeps no copy; Linux keeps unread output readable after the last terminal-side close, and a test covers output written before an exit seen ahead of the first read. The PTY broker is Linux-only today; the keep-open-then-harvest sequence is pending for the M2.2 macOS brokers milestone | `pty-process` consumes the terminal side at spawn, and the harvest is only needed on Darwin |
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
| Config fields without a consumer yet | `shareUsageWithCLI` makes the app write a usage cache the CLI falls back to; `modelRateLimits` feed throughput rows (`ProviderModelRateLimitScope`, `ProviderConfig.modelRateLimits(for:)`, `setModelRateLimits(_:for:)`, and `ProviderRateLimitNumber.parse`/`format`); a token account's `claudeConfigDirectory` assigns Claude history to that account, and Swift `Codable` fails decoding on non-string values | `shareUsageWithCLI`, `modelRateLimits` and `claudeConfigDirectory` are decoded, normalized and written with CodexBar's rules, while non-string token-account fields such as `claudeConfigDirectory` are leniently ignored on decode (`.ok()`) rather than failing the whole config decode; the scoped `modelRateLimits` accessors, `ProviderRateLimitNumber`, their tests in `ProviderConfigModelRateLimitsTests.swift` (scoped-set, sync and compact-number tests) and the Claude parser tests in `ClaudeSubscriptionMetadataTests.swift` are deferred to the throughput, Settings and M3 Claude milestones | The CLI usage cache, throughput rows and Claude history scanning are not ported yet; carrying the fields keeps a saved config from losing them while avoiding unused UI/provider helpers in M1 |
| Detail-row severity decoding | `ProviderDetailSeverity` (`ProviderDetailSections.swift`) is a plain `String, Codable` enum (`normal`, `warning`, `critical`) that fails decoding on unknown strings as well as non-strings (`ProviderDetailSeverityTests.swift:31`) | `DetailRowSeverity` decodes unknown string values as `Unknown` per [docs/protocol.md](protocol.md) §4.3 while still rejecting non-string values; the plugin, menu-card redaction and color tests in `ProviderDetailSeverityTests.swift` and `ProviderDetailSectionsContentTests.swift` ("privacy redaction preserves detail severity") belong to the macOS menu-card and plugin milestones | Forward compatibility on the wire: newer engine severity variants degrade to `unknown` rather than breaking older clients |
| `UsageSnapshot` provider-specific sidecars | `UsageSnapshot` (`UsageFetcher.swift:164, 409`) carries optional `openAIAPIUsage`, `codexResetCredits` and `throughput` (`ProviderThroughputSnapshot`) fields | Not modeled on `ariadusage_core::snapshot::UsageSnapshot` and ignored on decode until a v1 consumer needs them | Keeps the core snapshot focused on fields with an active v1 consumer |
| Managed-account cookie scopes and refresh-read suppression | Managed-account cache scopes and UI refresh-read suppression in `CookieHeaderCache` | Deferred: managed-account scopes deferred to M4; refresh-read suppression deferred to M8 with its UI caller | Keeps M2 scope focused on core cookie cache and header delivery |
| macOS UI toolkit | SwiftUI views, Swift Charts and AppKit menus | AppKit through objc2 with Core Graphics drawing; same layout constants, strings, defaults and interactions | SwiftUI and Swift Charts are not reachable from Rust; fidelity is checked against captures of the running app |
| macOS status icon style | Defaults to the `critters` style (`menuBarHidesCritters ?? false` in `SettingsStore.swift:614`), adding provider decorations (Claude notches and square corners, Codex face, Antigravity twist) | The macOS app shell renders the `bars` style without provider decorations until the `critters` style and its Settings toggle are ported in M2.3 | Stage the menu-bar shell with the shared `bars` geometry first |
| macOS status icon window resolution | `IconRemainingResolver` delegates to per-provider `iconWindows` resolvers (`CodexProviderDescriptor.swift:185-224`, `AntigravityProviderDescriptor.swift:132-139`, `ClaudeProviderDescriptor.swift:247-256`) | `presentation::icon_input` reads the generic `windows.primary` and `windows.secondary` pair (with Claude's reserved secondary lane) until the per-provider icon window resolvers are ported | Stage the shell against the generic Snapshot v1 window slots before provider-specific icon window policies land |
| Usage card pace | The app computes session and extra-window pace from each rate window (`UsagePaceText.sessionPace`, `extraRateWindowPaceDetail`) and gets weekly pace with run-out risk and a speed multiplier | The card shows only the engine's `ProviderSnapshot.pace`, on the weekly row; session and extra rows have no pace line or tip, and the `(N% risk)` and `1.5× headroom` suffixes stay off until the snapshot carries the risk and multiplier. The text builders implement both | Frontends render engine data and never compute pace themselves |
| Usage card titles and accent | Provider descriptors supply window labels, the tertiary label and the brand color in the app | Titles, the tertiary label and the accent come from the snapshot's `presentation` block; a provider without one shows `Session` and `Weekly`, no tertiary row, and the system accent color | Presentation comes from the engine || Usage card extras | Error copy button, last-known usage line, live refresh subtitle, Codex credit limits and hints, cost and token sections | Not shown yet; the subtitle is the error, `Refreshing…`, `Updated …` or `Not fetched yet`, and credits show `N left` on a power-of-ten scale | Scoped to the data the snapshot carries; the rest follows with its data |
| Usage card refresh state | `Refreshing…` follows the store's in-flight refresh | `Refreshing…` shows while a positional window metric is in the loading state | The snapshot's metric state is the only refresh signal a frontend has |
| Credit number format | `NumberFormatter` in `en_US_POSIX`, decimal style, two fraction digits | Pure Rust with the same output: no grouping, ties to even on the shortest decimal form, checked against Foundation's output | The locale is fixed, so the result does not depend on Foundation; date strings still come from Foundation formatters in the user's locale |
| Menu without engine data | Not applicable; the app owns its data | While the engine is disconnected or has sent no snapshot, the card is hidden and a disabled status line says `Waiting for the AriadUsage engine…` or `Refreshing…` | The macOS app is a client of the engine |
| LocalProbe procfs-only discovery | Uses `ps -ax` and `lsof` subprocesses with procfs fallback | Direct procfs inspection (`/proc/<pid>/cmdline`, `fd/`, `net/tcp{,6}`); no `ps` or `lsof` subprocesses | Avoids subprocess execution overhead and external tool dependencies on Linux |
| LocalProbe user and namespace scoping | `ps -ax` lists processes across all users; does not check network namespace | Same-UID and same-network-namespace only (`readlink /proc/<pid>/ns/net == /proc/self/ns/net`); non-matching argv dropped immediately; kept argv redacted in `Debug` | Prevents inspecting other users' processes or connecting to listeners in foreign network namespaces |
| LocalProbe listener ownership recheck | No recheck before loopback request | `listener_still_owned` verifies process identity, UID, namespace, and socket inode ownership in TCP table | Defends against port rebinding attacks prior to token transfer |


## Upstream drift

CodexBar keeps changing the three v1 providers. Changes made upstream after the
baseline are reviewed by diffing CodexBar's Claude, Codex and Antigravity
provider directories against the baseline. Anything relevant goes into the
post-v1 backlog; v1 does not chase upstream. Moving the baseline is an
architecture decision and is recorded in the Decision Log.
