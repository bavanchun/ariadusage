# Hooks

AriadUsage can execute user-configured hook commands when provider quota windows or health states transition, or when a refresh completes.

## Configuration

Hooks are configured under the `hooks` object in `config.json`:

```json
{
  "hooks": {
    "enabled": true,
    "rules": [
      {
        "id": "notify-quota-low",
        "enabled": true,
        "event": "quota_low",
        "provider": "codex",
        "command": "/usr/local/bin/my-hook",
        "arguments": ["--channel", "alerts"],
        "timeoutSeconds": 10
      }
    ]
  }
}
```

### Rule fields

| Field | Type | Description |
|---|---|---|
| `id` | `string` | Stable identifier for rate-limiting and diagnostics (auto-generated when omitted). |
| `enabled` | `boolean` | Whether this rule is active (default: `true`). |
| `event` | `string` | Event trigger (`quota_low`, `quota_reached`, `quota_reset`, `provider_unavailable`, `provider_recovered`, `refresh_failed`, or `usage_updated`). |
| `provider` | `string?` | Optional lowercase provider ID (`claude`, `codex`, `antigravity`). Omit or set to `null` to match all providers. |
| `command` | `string` | Absolute path to the executable (`~/...` is expanded before validation). Relative command names without `/` are rejected. |
| `arguments` | `string[]` | Fixed argument vector passed directly to the executable without shell evaluation. |
| `timeoutSeconds` | `number` | Per-invocation timeout in seconds (0.1 to 300 s, default 10; rules outside the range never run). |

## Supported events

| Event | Trigger |
|---|---|
| `quota_low` | A tracked rate window (`session` or `weekly`) crosses into the configured warning threshold (`<= 20%` remaining by default, or the custom threshold). |
| `quota_reached` | A tracked rate window reaches `0%` remaining (`>= 100%` used). |
| `quota_reset` | A tracked rate window resets (either its `resetsAt` timestamp advances by more than 60 seconds, or remaining quota jumps by more than `50%` after being below `50%`). |
| `provider_unavailable` | Provider status transitions into an unhealthy state (`degraded`, `major_outage`, or `unknown`), or changes between unhealthy states after the initial observation. |
| `provider_recovered` | Provider status returns to `operational` (`none`) from a previously observed unhealthy state. |
| `refresh_failed` | A refresh attempt fails after a previously healthy observation, or fails with a different error message (`network`, `unauthorized`, `rate_limited`, `parse_error`, `timeout`, or `unknown`). |
| `usage_updated` | A refresh publishes a valid usage snapshot (`primary` or `secondary` rate window present). Fires on every successful poll without transition-cooldown suppression. |

## Execution and security invariants

Hooks run exclusively through the engine's `Process` broker (`ariadusage-engine::hooks::ProcessBrokerHookExecutor`):

- **No shell interpolation**: `command` must resolve to an `AbsolutePath`, and `arguments` are passed as a fixed `argv` vector. AriadUsage never constructs a shell command string from event data.
- **Bounded stdin payload**: The serialized `HookEvent` JSON document is written to the child process's `stdin` and capped at `4096` bytes (`MAXIMUM_PAYLOAD_BYTES`).
- **Timeouts and process-group teardown**: Each execution runs in its own process group with `timeoutSeconds` enforced (`SIGTERM` followed by `SIGKILL` after grace period) and stdout/stderr capped at 64 KiB.
- **Rate limiting**: Transition events (`quota_low`, `quota_reached`, `quota_reset`, `provider_unavailable`, `provider_recovered`, `refresh_failed`) are rate-limited to once per 600 seconds per `(rule.id, event, provider, account_lane)`. `usage_updated` bypasses the cooldown so live consumers receive every successful poll.

## Environment variables

Only allowlisted general-purpose environment variables (`HOME`, `LANG`, `LC_ALL`, `LC_CTYPE`, `LOGNAME`, `PATH`, `SHELL`, `TERM`, `TMPDIR`, `TZ`, `USER`, `XDG_CACHE_HOME`, `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_RUNTIME_DIR`, `XDG_STATE_HOME`) plus the event's `ARIADUSAGE_*` variables are passed to the child process. Provider API keys, OAuth tokens, cookies, and unallowlisted ambient variables are never forwarded.

| Variable | Present when | Description |
|---|---|---|
| `ARIADUSAGE_EVENT` | Always | Event name (`quota_low`, `quota_reached`, `quota_reset`, `provider_unavailable`, `provider_recovered`, `refresh_failed`, `usage_updated`). |
| `ARIADUSAGE_PROVIDER` | Always | Lowercase provider identifier (`claude`, `codex`, `antigravity`). |
| `ARIADUSAGE_TIMESTAMP` | Always | ISO-8601 UTC timestamp (`YYYY-MM-DDTHH:MM:SSZ`) when the event was emitted. |
| `ARIADUSAGE_ACCOUNT` | When an account label is available and privacy mode is off | Account label (such as the active account email). Omitted when `hidePersonalInfo` is enabled. |
| `ARIADUSAGE_WINDOW` | Quota events (`quota_low`, `quota_reached`, `quota_reset`) | Tracked window name (`session` or `weekly`). |
| `ARIADUSAGE_USAGE_PERCENT` | Quota events and `usage_updated` (when primary window is non-synthetic) | Primary window used fraction in `0.0..=1.0` (for example `0.85` for 85% used). |
| `ARIADUSAGE_WINDOW_MINUTES` | `usage_updated` (when primary window has `windowMinutes`) | Duration of the primary window in minutes (for example `300`). |
| `ARIADUSAGE_USED` | When `used` count is set on the event | Raw used value. |
| `ARIADUSAGE_LIMIT` | When `limit` count is set on the event | Raw limit value. |
| `ARIADUSAGE_RESET_AT` | When primary/event `resetAt` is known | ISO-8601 UTC reset timestamp for the primary or triggered window. |
| `ARIADUSAGE_SECONDARY_USAGE_PERCENT` | `usage_updated` (when secondary window is present) | Secondary window used fraction in `0.0..=1.0`. |
| `ARIADUSAGE_SECONDARY_WINDOW_MINUTES` | `usage_updated` (when secondary window has `windowMinutes`) | Duration of the secondary window in minutes (for example `10080`). |
| `ARIADUSAGE_SECONDARY_RESET_AT` | `usage_updated` (when secondary window has `resetsAt`) | ISO-8601 UTC reset timestamp for the secondary window. |
| `ARIADUSAGE_STATUS` | `provider_unavailable`, `provider_recovered`, `refresh_failed` | Provider status (`operational`, `degraded`, `major_outage`, `unknown`) or refresh error kind (`network`, `unauthorized`, `rate_limited`, `parse_error`, `timeout`, `unknown`). |

## JSON stdin payload

Every hook receives a single JSON object on `stdin` with `camelCase` keys matching `HookEvent`:

```json
{
  "event": "usage_updated",
  "provider": "codex",
  "account": "user@example.com",
  "usagePercent": 0.2,
  "windowMinutes": 300,
  "resetAt": "2026-10-24T18:00:00Z",
  "secondaryUsagePercent": 0.5,
  "secondaryWindowMinutes": 10080,
  "secondaryResetAt": "2026-10-29T18:00:00Z",
  "timestamp": "2026-10-24T14:00:00Z"
}
```

Fields whose values are `null` (`account`, `window`, `usagePercent`, `windowMinutes`, `used`, `limit`, `resetAt`, `secondaryUsagePercent`, `secondaryWindowMinutes`, `secondaryResetAt`, `status`) are omitted from the JSON object.

## Account labels and privacy mode (`hidePersonalInfo`)

- **Default mode (`hidePersonalInfo == false`)**: When the active provider snapshot includes an account email, `hook_account_display_name` populates `HookEvent.account`, which is serialized as `"account"` in the JSON stdin payload and exported as `ARIADUSAGE_ACCOUNT` in the child environment (matching upstream behavior).
- **Privacy mode (`hidePersonalInfo == true`)**: `hook_account_display_name` returns `None`, so `"account"` is omitted from the JSON stdin payload and `ARIADUSAGE_ACCOUNT` is never set in the child environment. Internally, `HookTransitionDetector` and `HookRunner` still compute a private `hook_account_discriminator` so multi-account state tracking and rate-limiting remain isolated per account without exposing the identifier to hook subprocesses.

## CLI commands (`ariadusage hooks`)

The CLI provides five subcommands under `ariadusage hooks` (running `ariadusage hooks` with no subcommand defaults to `list`):

- `ariadusage hooks [list] [--format text|json] [--json] [--json-only] [--pretty]`
  Lists whether hooks are enabled and prints each configured rule (`[on|off] <event> provider=<provider|any>: <executable> [args...]`), or emits the `HooksConfig` JSON object when JSON output is requested.
- `ariadusage hooks enable [--format text|json] [--json] [--json-only] [--pretty]`
  Enables hooks in `config.json` and prints `Hooks: enabled` (or the updated `HooksConfig` JSON).
- `ariadusage hooks disable [--format text|json] [--json] [--json-only] [--pretty]`
  Disables hooks in `config.json` and prints `Hooks: disabled` (or the updated `HooksConfig` JSON).
- `ariadusage hooks test <event> --provider <name> [--format text|json] [--json] [--json-only] [--pretty]`
  Synthesizes a representative `HookEvent` for `<event>` and `<provider>`, matches it against `config.json`, runs every matching rule through the `Process` broker with `ARIADUSAGE_*` environment variables and JSON stdin, and exits `0` when all matching rules succeed or `1` when arguments/config are invalid or any rule fails.
- `ariadusage hooks watch [--interval <seconds>] [--provider <name|both|all>] [--web-timeout <seconds>] [-v|--verbose] [--format text|json] [--json] [--json-only] [--pretty]`
  Continuously polls the selected (or enabled) providers every `--interval` seconds (default `300`, minimum `60`), evaluates `HookTransitionDetector` across polls, dispatches matching rules through `HookRunner` with `HookRateLimiter`, and stops cleanly on `SIGINT`, `SIGTERM`, or `SIGHUP`. Argument flags (`--once` rejection, `--interval`, `--provider`) are validated before reading `config.json`.
