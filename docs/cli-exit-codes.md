# CLI exit codes

`ariadusage secret set` writes through SecretStore in the CLI process. It reads
the secret from a no-echo terminal prompt or from stdin, and never includes the
setting ID or secret value in its error text.

| Code | Meaning |
|---:|---|
| 0 | Secret saved |
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

When the keyring is unavailable, a terminal call asks whether to store the
secret in the private file fallback. The first non-interactive fallback use
must pass `--allow-file-fallback`; later calls honor the saved
`secretFileFallback` consent. File fallback is currently Linux-only.
