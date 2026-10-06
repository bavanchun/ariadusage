# Security

Report vulnerabilities through [GitHub private vulnerability reporting](https://github.com/bavanchun/ariadusage/security/advisories).
Do not open a public issue for a security report.

Never attach real tokens, cookies, API keys, credential files, browser profiles
or account data to a report. Describe the problem and reproduce it with
invented values; redact anything real from logs before you share them.

## Scope

Reports are especially welcome for:

- credential handling: reading other tools' credential files, token refresh,
  the Secret Service keyring and any fallback secret file;
- the engine's Unix socket and its clients;
- browser cookie import;
- the `serve` HTTP server and dashboard;
- the Omarchy plugin.

What AriadUsage is designed to read, write and send is described in
[privacy.md](privacy.md); the threat model is in
[ARCHITECTURE.md](../ARCHITECTURE.md#91-threat-model).
