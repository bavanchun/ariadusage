# CLI exit codes

`ariadusage secret set` writes through SecretStore in the CLI process. It reads
the secret from a no-echo terminal prompt or from stdin, and never includes the
setting ID or secret value in its error text.

| Code | Meaning |
|---:|---|
| 0 | Command succeeded (or secret saved) |
| 1 | `hooks` command argument, configuration, or hook rule execution failure |
| 2 | Invalid setting ID or command-line syntax |
| 3 | Secret or consent input could not be read |
| 4 | Secret input exceeded 64 KiB |
| 5 | Empty, invalid UTF-8 or invalid secret |
| 10 | Keyring unavailable |
| 11 | Keyring locked |
| 12 | Keyring unlock was dismissed |
| 13 | Keyring operation timed out |
| 14 | Keyring operation was cancelled |
| 15 | File fallback consent is required or was declined |
| 16 | Configuration, data path, runtime or storage operation failed |
| 17 | Secret operation is unsupported on this platform |
| 18 | Process hardening failed |
| 19 | Daemon, LaunchAgent or status operation is unsupported on this platform |

When the keyring is unavailable, a terminal call asks whether to store the
secret in the private file fallback. The first non-interactive fallback use
must pass `--allow-file-fallback`; later calls honor the saved
`secretFileFallback` consent. File fallback is currently Linux-only.

## `diagnose`, `cache clear`, `cookie refresh`, and `sessions`

| Command | Code | Meaning |
|---|---:|---|
| `diagnose` | 0 | Diagnostic JSON exported (`--redact` is enforced on every export; tokens, cookies, emails, and account identifiers are never emitted) |
| `diagnose` | 1 | Output file could not be written |
| `diagnose` | 2 | Invalid command-line flags, unsupported format, or unknown provider |
| `diagnose` | 18 | Process hardening failed |
| `cache clear` | 0 | Requested cookie and/or cost caches cleared (or nothing to clear) |
| `cache clear` | 1 | Cost cache directory or file removal failed |
| `cache clear` | 2 | Missing `--cookies`, `--cost`, or `--all`, `--provider` combined with `--cost`, or unknown provider |
| `cache clear` | 18 | Process hardening failed |
| `cookie refresh` | 0 | All targeted providers refreshed or skipped |
| `cookie refresh` | 1 | One or more targeted providers were blocked (Keychain prompt acknowledgement required) or failed validation/persistence |
| `cookie refresh` | 2 | Invalid flags, neither or both of `--provider` and `--all`, unknown provider, or provider does not support browser cookie refresh |
| `cookie refresh` | 16 | Configuration, data path, or runtime initialization failed |
| `cookie refresh` | 18 | Process hardening failed |
| `sessions` / `sessions list` | 0 | Session table or JSON array (`--json` v1 / `--json-v2`) printed |
| `sessions` / `sessions list` | 2 | Invalid command-line flags |
| `sessions` / `sessions list` | 18 | Process hardening failed |
| `sessions focus` | 1 | Missing session ID or unknown session ID |
| `sessions focus` | 2 | Session focus is unsupported on this frontend |
| `sessions focus` | 18 | Process hardening failed |

Argument-validation and flag errors across `diagnose`, `cache clear`,
`cookie refresh`, and `sessions` return exit code `2` (matching AriadUsage's
CLI convention for invalid command-line syntax, whereas upstream returns `1`).
