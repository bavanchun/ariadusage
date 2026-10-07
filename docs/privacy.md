# Privacy

This page states what AriadUsage v1 is designed to read, write and send.
AriadUsage is not released yet, so these are **v1 commitments, not a
description of shipped behavior**. Each provider milestone verifies its section
against the implementation before release, and the network destinations are
listed per provider as each one lands.

## No telemetry

AriadUsage sends no telemetry, analytics or crash reports. It talks only to the
services a feature you enabled needs (see [Network](#network)).

## What is read

AriadUsage reads only what an enabled provider and its selected source mode
need. Other tools' credential files are read-only unless a rule in
[Token refresh](#token-refresh) says otherwise.

| Provider | May read |
|---|---|
| Claude | An Admin API key you enter; Claude Code's OAuth credentials file (`~/.claude/.credentials.json`, or under `$CLAUDE_CONFIG_DIR`); the output of the `claude` CLI's `/usage` and `/status` screens; a claude.ai session cookie, pasted by you or imported on opt-in; Claude Code's local project logs (under `~/.claude/projects`) to compute cost |
| Codex | Codex's OAuth credentials (`auth.json` under `$CODEX_HOME`, default `~/.codex`, or the keyring through `codex app-server`); account and rate-limit data from `codex app-server`; the chatgpt.com usage dashboard with a session cookie, pasted by you or imported on opt-in; Codex session logs and its local SQLite log to compute cost |
| Antigravity | The process list, command-line flags and listening ports of the local Antigravity language server, and its local RPC answers; the output of the `agy` CLI; Google OAuth credentials as a fallback |

On Linux, the Process broker reads same-user `/proc` process identity, owner,
environment-marker and descriptor metadata for process ownership and cleanup.
Environment reads are capped at 1 MiB and retain only the exact marker entry.

AriadUsage also resolves the absolute paths of `claude`, `codex` and `agy`,
including installs managed by mise or asdf.

It never reads CodexBar's configuration, cache or state.

## What is written

| Location | Contents |
|---|---|
| `$XDG_CONFIG_HOME/ariadusage/config.json` (default `~/.config/ariadusage/`) | The single settings file; overridden by `ARIADUSAGE_CONFIG` |
| `$XDG_CONFIG_HOME/ariadusage/config.json.lock` | Mode 0600 lock file, owner-only, never unlinked, used for write serialization |
| `$XDG_CONFIG_HOME/ariadusage/.ariadusage-staged-*` | Mode 0700 temporary staging directories beside target, cleaned up immediately after atomic rename |
| `$XDG_STATE_HOME/ariadusage/` (default `~/.local/state/ariadusage/`) | State such as usage history and notification episodes |
| `$XDG_CACHE_HOME/ariadusage/` (default `~/.cache/ariadusage/`) | Rebuildable caches such as cost scan results and pricing data |
| `$XDG_RUNTIME_DIR/ariadusage/engine.sock` | The engine's owner-only socket, removed at logout |

The configuration path can be customized via the `ARIADUSAGE_CONFIG` environment variable
(trimmed, tilde-expanded, must resolve to an absolute path).
Before reading or modifying the configuration or lock file, trust checks enforce that:
- Neither the configuration file nor its lock file is a symlink (`O_NOFOLLOW`);
- Both are regular files owned by the effective UID; the configuration file additionally must not be group- or world-writable, while the lock file is only checked for symlink, regular file and owner;
- The parent directory is owned by the effective UID and is neither group- nor world-writable.
Writes are staged in an owner-only temporary directory (`0700`) beside the destination and written
to a `0600` file with `fchmod` before the first byte, synced to disk, atomically renamed,
and followed by an `fsync` of the parent directory.

Nothing is ever written inside the Omarchy plugin directory. Files that hold
secrets are created with mode 0600 before any bytes are written.

## Secrets

- Secrets you give AriadUsage (API keys, pasted cookies, account tokens) are
  stored in your Secret Service keyring. Only when no keyring is available, and
  only after you agree, they go to a 0600 file instead.
- Background refreshes never unlock the keyring and never show a prompt. A
  locked keyring is reported as locked.
- Secrets are typed into AriadUsage's own prompt in a terminal
  (`ariadusage secret set`). The Omarchy panel can open that terminal for you,
  but the value never passes through the panel. Secrets never appear on a
  command line, in logs, in notifications or in data sent to the panel.
- Secrets found in the configuration file (`apiKey`, `cookieHeader`, `secretKey`,
  `pluginSecrets` values, or token-account `token`) are never used by providers
  or the engine. They are flagged with a `secret_in_config` validation warning,
  and are always redacted to `"[REDACTED]"` in configuration dumps and Rust
  `Debug` representations.

## Token refresh

- Codex and gcloud refresh tokens are never redeemed by AriadUsage; their own
  CLIs refresh them.
- When the Claude CLI owns your Claude login, AriadUsage asks the CLI to refresh
  it by opening `claude` `/status` in a pseudo-terminal, at most once every five
  minutes.
- Only tokens AriadUsage itself owns are refreshed by AriadUsage.

## Browser cookie import

- Off by default. You opt in per provider.
- Only the cookie domains a provider declares (for example claude.ai for Claude
  and chatgpt.com for Codex) are read, from Chromium-family and Firefox
  profiles.
- Imported cookies are used only for that provider's requests and are never
  sent to another host, including on redirects.
- Pasting a cookie header by hand remains available instead of import.
- On Windows (a later phase), AriadUsage never bypasses Chrome's App-Bound
  Encryption.

## Network

AriadUsage connects to:

- the APIs of the providers you enable;
- their public status pages (Statuspage for Claude and Codex, Google Workspace
  status for Antigravity);
- a public model-pricing source, when cost tracking is on;
- the local Antigravity language server on loopback.

Requests that carry credentials use HTTPS, and the broker attaches them only
to a provider's declared origins. Redirects stay on the original HTTPS host and
port. Responses are capped at 5 MiB by default, and AriadUsage does not use a
shared cookie jar. The main client honors environment proxy settings; the
literal-loopback client ignores proxies and follows no redirects. The `serve`
dashboard is off by default and binds to loopback when enabled.

## Removing all data

1. Stop the daemon: `systemctl --user disable --now ariadusage.service`.
2. Remove the plugin: `omarchy plugin remove io.github.bavanchun.ariadusage`.
3. Uninstall the `ariadusage` package.
4. Delete `~/.config/ariadusage/`, `~/.local/state/ariadusage/` and
   `~/.cache/ariadusage/` (or their `$XDG_*` equivalents).
5. Delete AriadUsage's entries from your keyring, for example with Seahorse.

Credential files belonging to Claude Code, Codex, Antigravity or your browsers
are left untouched.
