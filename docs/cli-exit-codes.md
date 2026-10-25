# CLI exit codes

`ariadusage usage` (the default command) and `ariadusage cards` query the running `ariadusage daemon` over IPC v1 (`GetSnapshot`) first and fall back to an in-process `EngineRuntime` fetch when no daemon socket is active and no explicit `--socket` was passed; fetch-affecting flags (`--source`, `--account`, `--account-index`, `--all-accounts`, `--web`, `--web-timeout`, `--antigravity-plan-debug`) force the in-process fetch so they take effect, while `--web-debug-dump-html` is rejected with exit code `64`. Both commands map provider and argument errors using `CLIExitCode.swift` (`0`, `1`, `2`, `3`, `4`, `64`) plus AriadUsage's runtime codes (`16`, `18`, `19`). Each command follows its own upstream exit code for argument errors (`64` for `usage` and `cards`, `2` for the others). `ariadusage status` is an AriadUsage-only command that queries the running `ariadusage daemon` over IPC v1 (`GetSnapshot`).

`ariadusage secret set` writes through SecretStore in the CLI process. It reads
the secret from a no-echo terminal prompt or from stdin, and never includes the
setting ID or secret value in its error text.

| Code | Meaning |
|---:|---|
| 0 | Command succeeded (or secret saved) |
| 1 | `usage` / `cards` unexpected provider failure (`CLIExitCode.unexpectedFailure`), `status` unknown provider, or `hooks`, `config`, or `cost` command validation, configuration, or execution failure |
| 2 | Provider CLI binary not found (`CLIExitCode.providerNotInstalled`), or invalid setting ID, CLI option value, or command-line syntax |
| 3 | Provider output or JSON parse failure (`CLIExitCode.parseError`), or secret / consent input could not be read |
| 4 | Provider fetch or web timeout (`CLIExitCode.timeout`), or secret input exceeded 64 KiB |
| 5 | Empty, invalid UTF-8 or invalid secret |
| 10 | Keyring unavailable |
| 11 | Keyring locked |
| 12 | Keyring unlock was dismissed |
| 13 | Keyring operation timed out |
| 14 | Keyring operation was cancelled |
| 15 | File fallback consent is required or was declined |
| 16 | Configuration, data path, daemon IPC connection/protocol, runtime or storage operation failed |
| 17 | Secret operation is unsupported on this platform |
| 18 | Process hardening failed |
| 19 | Daemon, LaunchAgent or status operation is unsupported on this platform |
| 20 | `config` operation is unsupported (`set-api-key`, `dump --show-secrets`, `preferences`) |
| 21 | `cost` flag or mode is unsupported (`--remote`, `--summary-only`, `--identified-only`, `--identified-end`) |
| 64 | Invalid `usage` / `cards` arguments, conflicting flags, or unknown `--provider` value (`CLIExitCode.invalidArguments` / `EX_USAGE` in `CLIExitCode.swift`) |

When the keyring is unavailable, a terminal call asks whether to store the
secret in the private file fallback. The first non-interactive fallback use
must pass `--allow-file-fallback`; later calls honor the saved
`secretFileFallback` consent. File fallback is currently Linux-only.

## `diagnose`, `cache clear`, and `cookie refresh`

| Command | Code | Meaning |
|---|---:|---|
| `diagnose` | 0 | Diagnostic JSON exported (`--redact` is enforced on every export; tokens, cookies, emails, and account identifiers are never emitted) |
| `diagnose` | 1 | Output file could not be written |
| `diagnose` | 2 | Invalid command-line flags, unsupported format, or unknown provider |
| `diagnose` | 18 | Process hardening failed |
| `cache clear` | 0 | Requested cookie and/or cost caches cleared (or nothing to clear) |
| `cache clear` | 1 | Cost cache directory removal failed |
| `cache clear` | 2 | Missing `--cookies`, `--cost`, or `--all`, `--provider` combined with `--cost`, or unknown provider |
| `cache clear` | 18 | Process hardening failed |
| `cookie refresh` | 0 | All targeted providers refreshed or skipped |
| `cookie refresh` | 1 | One or more targeted providers were blocked (Keychain prompt acknowledgement required) or failed validation/persistence |
| `cookie refresh` | 2 | Invalid flags, neither or both of `--provider` and `--all`, unknown provider, or provider does not support browser cookie refresh |
| `cookie refresh` | 16 | Configuration, data path, or runtime initialization failed |
| `cookie refresh` | 18 | Process hardening failed |

Argument-validation and flag errors across `diagnose`, `cache clear`, and
`cookie refresh` return exit code `2` (matching AriadUsage's CLI convention for
invalid command-line syntax, whereas upstream returns `1`).

## `ariadusage config` and `ariadusage cost`

- `ariadusage config` (`validate`, `dump`, `providers`, `enable`, `disable`, `set-source`) reads and writes through `ConfigStore`. `config dump` always redacts legacy credentials as `[REDACTED]`, and `config dump --show-secrets`, `config set-api-key`, and `config preferences` exit with code `20` without reading, storing, or printing any secret value.
- `ariadusage cost` scans local Claude, Codex, and Antigravity usage history via `ariadusage-engine` and emits human text or JSON (`schemas/cli-cost.v1.json`). Invalid flag values exit with code `2`, providers without local cost support exit with code `1`, and unsupported remote/identified flags (`--remote`, `--summary-only`, `--identified-only`, `--identified-end`) exit with code `21`.
