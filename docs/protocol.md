# AriadUsage IPC v1 Protocol Specification

> **Status: DRAFT**
>
> This specification defines the wire contract for IPC v1 between the AriadUsage engine daemon, CLI tools, and desktop user interfaces (the macOS menu-bar app and the Omarchy Quickshell plugin). It remains a draft during initial client and engine implementation and will be frozen before the first frontend release. After freeze, changes within v1 are additive only.

---

## 1. Transport and Connection

### 1.1 Local Unix Domain Socket
AriadUsage IPC v1 uses newline-delimited UTF-8 JSON streaming over a local Unix domain socket.
- **Default Socket Path**: `$XDG_RUNTIME_DIR/ariadusage/engine.sock` (the engine socket lives only in the runtime directory; no fallback).
- **Development/Fixture Socket Path**: `$XDG_RUNTIME_DIR/ariadusage-dev/engine.sock` (the fixture server falls back to a directory under the system temporary directory when `$XDG_RUNTIME_DIR` is unset; development only, never used by the engine).

### 1.2 File System Permissions and Security
The socket exists strictly within user-private space:
- **Parent Directory**: Mode `0700` (`rwx------`), owned by the running user.
- **Socket File**: Mode `0600` (`rw-------`), owned by the running user.
- The engine rejects connection attempts or binds if directory or file permissions are violated.

---

## 2. Framing and Limits

### 2.1 Newline Delimitation
Every message frame is a single valid JSON object encoded in UTF-8, terminated by a single ASCII newline character (`\n`, `0x0A`). Carriage return (`\r`) characters immediately preceding `\n` are stripped.

### 2.2 1 MiB Framing Boundary
To protect clients from memory exhaustion and unbounded buffering (crucial for Quickshell / QML frontends):
- **Producer Framing Cap**: Neither the engine daemon nor client may emit a frame whose content is larger than **1,048,576 bytes (1 MiB)**, excluding the trailing newline. Attempting to encode an oversized outbound frame results in an immediate encoding error and the frame is discarded.
- **Consumer Decoder Cap**: The decoder rejects any inbound line whose content exceeds 1 MiB with a `payloadTooLarge` static error. The connection remains usable for subsequent valid frames.

---

## 3. Protocol Lifecycle and Handshake

### 3.1 Handshake (`hello` / `welcome`)
Immediately upon establishing a Unix stream connection, the client must initiate the protocol handshake by sending a `hello` message.

#### Client Request
```json
{
  "type": "hello",
  "protocols": ["ariadusage-ipc/1"],
  "client": {
    "name": "ariadusage-omarchy",
    "version": "0.1.0"
  },
  "id": "req-1"
}
```

#### Server Response
```json
{
  "type": "welcome",
  "protocol": "ariadusage-ipc/1",
  "engineVersion": "0.1.0",
  "capabilities": ["snapshot", "settings", "notices"],
  "id": "req-1"
}
```

If no compatible protocol version is shared between client and server, the server responds with an error:
```json
{
  "type": "response",
  "id": "req-1",
  "ok": false,
  "error": {
    "code": "invalidRequest",
    "message": "malformed or invalid request payload"
  }
}
```

---

## 4. Message Format and Client Duties

### 4.1 Two-Pass Decoding
Clients and servers must decode inbound frames in two passes:
1. **Raw Envelope Inspection**: Decode the envelope containing the `type` tag and optional `id` (`{"type": string, "id": string?}`).
2. **Typed Payload Decoding**: Decode the specific typed variant corresponding to `type`.

If `type` is unrecognized, the receiver must NOT drop the connection. Instead, the server emits an `unsupportedMessage` response preserving the original request `id`:
```json
{
  "type": "response",
  "id": "req-unknown",
  "ok": false,
  "error": {
    "code": "unsupportedMessage",
    "message": "unsupported message type"
  }
}
```

### 4.2 Static Error Handling
All error messages emitted across the IPC boundary use static, predetermined descriptions. Serde error strings, stack traces, and input payload fragments are strictly prohibited from appearing in error messages.

| Error Code | Static Message | Description |
|---|---|---|
| `unsupportedMessage` | `unsupported message type` | Unknown message type tag |
| `invalidRequest` | `malformed or invalid request payload` | Deserialization or validation failure |
| `payloadTooLarge` | `message frame exceeds maximum allowed size` | Inbound frame exceeded 1 MiB limit |
| `internal` | `internal engine error` | Unhandled engine failure |
| `notFound` | `requested resource not found` | Target provider or setting does not exist |
| `unknown` | `unknown error` | Unclassified error condition |

### 4.3 Forward Compatibility
Clients must:
- Tolerate and ignore unknown JSON fields.
- Treat unknown enum variants (e.g., status indicators, notice levels) as `unknown` fallback variants rather than failing deserialization.

---

## 5. Client Messages (Requests)

### 5.1 `subscribe`
Subscribe to asynchronous update pushes.
```json
{
  "type": "subscribe",
  "topics": ["snapshot", "settings", "notices"],
  "id": "sub-1"
}
```
*Note*: Subscribing to `"snapshot"` triggers an immediate snapshot push to the client without waiting for the next refresh interval.

### 5.2 `getSnapshot`
Request an immediate snapshot push.
```json
{
  "type": "getSnapshot",
  "id": "snap-1"
}
```

### 5.3 `refresh`
Trigger an immediate background refresh cycle for all providers or a specific provider.
```json
{
  "type": "refresh",
  "provider": "claude",
  "id": "ref-1"
}
```

### 5.4 `getSettings`
Retrieve settings descriptors for the given scope (`"app"` or `{"provider": "<id>"}`).
```json
{
  "type": "getSettings",
  "scope": "app",
  "id": "settings-1"
}
```

### 5.5 `setSetting`
Modify a non-secret configuration setting.
```json
{
  "type": "setSetting",
  "id": "sourceMode",
  "value": "cli"
}
```

### 5.6 `runAction`
Trigger an executable provider action (e.g. login, logout).
```json
{
  "type": "runAction",
  "id": "recheckAccount",
  "confirm": true
}
```

### 5.7 `setSecret`
Securely transmit a sensitive credential token to the engine. Sent exclusively by CLI tools or secure terminal prompts; GUI frontends must never send `setSecret`.
```json
{
  "type": "setSecret",
  "id": "providers.claude.apiKey",
  "value": "sample-secret-token-12345"
}
```

---

## 6. Server Messages (Responses & Pushes)

### 6.1 `response`
Sent in direct reply to a client request.
```json
{
  "type": "response",
  "id": "req-1",
  "ok": true,
  "payload": { ... }
}
```

### 6.2 `snapshot`
Pushed upon subscription, on refresh ticks, or following provider state changes.
```json
{
  "type": "snapshot",
  "snapshot": {
    "schemaVersion": 1,
    "generatedAt": "2026-10-06T12:00:00Z",
    "staleAfterSeconds": 15,
    "engine": {
      "version": "0.1.0",
      "refreshing": false
    },
    "providers": [ ... ]
  }
}
```

#### 6.2.1 Extra Named Rate Windows (`NamedWindow`)
In addition to the standard positional rate windows (`primary`, `secondary`, `tertiary`), both provider and account window collections carry an optional `extra` list of named windows (`NamedWindow`). Each entry contains:
- `id`: Unique identifier for the extra window (e.g. `"burst"`, `"daily-limit"`).
- `title`: Human-readable title for UI presentation (e.g. `"1-Hour Burst"`).
- `window`: A `Metric<RateWindow>` envelope representing the rate window's current state and quota.

Because the window is carried inside a `Metric<RateWindow>` envelope, extra windows enforce the exact same honesty and freshness invariants as positional windows:
- **Fresh or Stale Values (`value` / `stale`)**: The `value` object contains `usedPercent`, optional `windowMinutes`, `resetsAt`, and `resetDescription`.
- **Non-Value States (`loading` / `error` / `unknown`)**: The `value` field is strictly omitted. Non-value states must never emit synthetic, placeholder, or default numeric values (such as `0` or `100`).
- JSON schemas enforce this metric invariant at both provider and account hierarchy levels.

#### 6.2.2 Provider Presentation (`ProviderPresentation`)
Each provider snapshot may carry an optional `presentation` object with the window titles and accent color the engine holds in its provider descriptors. Frontends use it to title the positional windows and tint their progress bars instead of keeping their own copy of this metadata:
- `shortDisplayName`: Compact provider name for tight layouts (e.g. `"Anti"` for Antigravity).
- `sessionLabel`: Title of the `primary` window (e.g. `"Session"`, or `"Gemini Models"` for Antigravity).
- `weeklyLabel`: Title of the `secondary` window (e.g. `"Weekly"`, or `"Claude and GPT"` for Antigravity).
- `tertiaryLabel` (optional): Title of the `tertiary` window when the provider has one (e.g. `"Sonnet"` for Claude).
- `accentColor` (optional): Provider accent color as uppercase `#RRGGBB` hex (e.g. `"#CC7C5E"`). Any other format is rejected by the schema and by decoding.

The engine sets `presentation` only for providers it has a descriptor for and omits it otherwise. The field is additive: payloads without it remain valid.

#### 6.2.3 Detail Row Severity
Each row in a provider's `details` sections may carry an optional `severity`: `normal`, `warning` or `critical`. Frontends use it to emphasize the row (for example, tinting its value or progress bar); a row without it renders as before. The field is omitted when the provider sets none, and payloads without it remain valid. Following §4.3, an unrecognized value decodes as `unknown`, which clients render like a row without severity.

#### 6.2.4 Per-Window Pace, Session-Equivalent Forecast, and Subscription Dates
Each `RateWindow` (positional or named) may carry:
- `pace` (optional `Pace`): Per-window pace comparing consumption to elapsed window progress (`stage`, `deltaPercent`, `expectedUsedPercent`, `willLastToReset`, optional `etaSeconds`, optional `speedMultiplierToReset`, optional `runOutProbability`, and pre-formatted `summary`). Pace lives only on `RateWindow` (`windows.primary`, `windows.secondary`, `windows.tertiary`, or `windows.extra[].window`); there is no separate top-level `ProviderSnapshot.pace` or `AccountSnapshot.pace`.
- `sessionEquivalent` (optional `SessionEquivalentForecast`): On weekly windows, estimates how many full primary sessions the remaining weekly headroom can cover at the current session pace (`estimatedSessions`, `windowsUntilReset`, and optional `sampleCount`).

Both `ProviderSnapshot` and `AccountSnapshot` may also carry an optional `subscription` (`Metric<SubscriptionDates>`) envelope containing `renews` and `expires` (`BillingDate` with `at` RFC 3339 timestamp and optional `dateOnly: true`, omitted when `false`, indicating when the timestamp represents a calendar date without a meaningful time-of-day).

#### 6.2.5 Credits, Credit Limit, and Cost (`Credits` / `Cost`)
- `Credits` carries `remaining`, optional `unit`, `balanceIsWorkspace`, `updatedAt`, `balanceReadSucceeded`, `creditsAvailable`, optional `creditLimit` (optional `title`, `used`, `total`, `remaining`, `unit`, `resetsAt`, `resetDescription`), and optional `resetCredits` (`availableCount`, optional `nextExpiresAt`).
- **Limit-reset credits wire home**: `Credits.resetCredits` (`ResetCredits`) is the wire representation for credit-attached reset inventories such as Codex's `codexResetCredits` (`Sources/CodexBarCore/CreditsModels.swift:155-209`). As a deliberate provider exception recorded in [docs/porting.md](porting.md), Claude's `cedar_ember` rate-limit reset credits (`Sources/CodexBarCore/Providers/Claude/ClaudeRateLimitResetCredits.swift:9-54`) stay in `ProviderSnapshot.details` as a `DetailSection` whose `DetailRow` has `label: "Limit Reset Credits"`, because Claude OAuth usage can report reset grants when no `Credits` balance metric exists and projects per-grant UTC expiry lines through `details` across CLI and GUI frontends.
- `Cost` carries `used`, optional `limit`, `currencyCode`, optional `period`, `resetsAt`, `balance`, `nextRegenAmount`, `personalUsed`, `balanceUpdatedAt`, `balanceIsWorkspace`, and `balanceIsUnavailable`. Freshness is carried by the outer `Metric<Cost>.asOf` envelope.

#### 6.2.6 Cost History and Usage Breakdown (`costHistory`)
Both `ProviderSnapshot` and `AccountSnapshot` may carry an optional `costHistory` (`Metric<CostHistory>`) envelope (with freshness timestamp in `Metric.asOf`) backing the card's Cost section and the Usage Breakdown chart submenu:
- Current local calendar day ("Today") counters: `sessionTokens`, `sessionCostUsd`, `sessionRequests`. These mirror upstream `CostUsageTokenSnapshot.sessionTokens` / `sessionCostUSD` / `sessionRequests` (`Sources/CodexBarCore/CostUsageModels.swift`), where `session*` denotes the current local calendar day ("Today") in the token cost scanner and is distinct from the 5-hour quota `primary` ("Session") window.
- Rolling window totals: `last30DaysTokens`, `last30DaysCostUsd`, `last30DaysRequests`, `historyDays`, `historyLabel`, `historyCoverageIsEstablished`, `historyScanIsPartial`, `meteredCostUsd`.
- Currency and provenance: `currencyCode` and `costProvenance` (`listPriceEstimate`, `vendorMetered`, `mixed`, or `unknown`).
- Daily ledger and breakdowns: `daily` (`CostDailyEntry` items with `date` matching `^\d{4}-\d{2}-\d{2}$`, `totalTokens`, `costUsd`, `totalRequests`, `modelsUsed`, and `modelBreakdowns` including optional `standardCostUsd` / `priorityCostUsd` / `standardTokens` / `priorityTokens`), `comparisonWindows` (`CostWindowSummary` for e.g. 7d / 14d / 30d with `days`, optional `costUsd`, `meteredCostUsd`, `totalTokens`, `totalRequests`, `incompleteRequestCount`, `entryCount`, `provenance`), `topModel`, and `hintLines`.

#### 6.2.7 Plan Utilization History (`utilization`)
Both `ProviderSnapshot` and `AccountSnapshot` may carry an optional `utilization` (`Metric<Vec<PlanUtilizationSeries>>`) envelope for the Plan Usage submenu and quota burndown charts. Each series contains `name` (known series names: `"session"`, `"weekly"`, `"monthly"`, `"opus"`, `"antigravityGemini"`, `"antigravityClaudeGpt"`; open string for forward compatibility), `windowMinutes`, optional `title`, and chronological `entries` (`capturedAt`, `usedPercent`, optional `resetsAt`).

#### 6.2.8 Provider Status and Status Page Submenu (`status`)
Both `ProviderSnapshot` and `AccountSnapshot` may carry `status` (`Metric<ProviderStatus>`) with `indicator` (`StatusIndicator`, used by status icons and banners), optional `description`, `updatedAt`, `pageUrl`, and a tree of `components` (`StatusComponent` with `id`, `name`, `status`, `indicator`, optional `updatedAt`, and recursive `children`).

#### 6.2.9 Settings Section Panes and Read-Only Descriptors
In `SettingsPage` responses (`getSettings`):
- `SettingsSection` may carry optional `subtitle` and `pane` (canonical values: `"general"`, `"menuBar"`, `"menu"`, `"providers"`, `"notifications"`, `"advanced"`) so multi-tab settings windows can group sections by tab.
- `SettingDescriptor` may carry `readOnly: true` (omitted when `false`) for informational fields such as detected CLI versions or configuration paths.

#### 6.2.10 UI Surface Mapping (CodexBar `b49d42eb5`)
| UI Surface | Upstream Source (`b49d42eb5`) | Protocol Field(s) |
|---|---|---|
| Card header & subscription dates | `Sources/CodexBar/MenuCardView.swift`, `Sources/CodexBarCore/UsageFetcher.swift:180-183` | `displayName`, `presentation`, `updatedAt`, `identity` (`Metric<Identity>`), `subscription` (`Metric<SubscriptionDates>`) |
| Rate windows, pace & session-equivalent forecast | `Sources/CodexBarCore/UsagePace.swift`, `Sources/CodexBar/UsagePaceText.swift`, `Sources/CodexBar/SessionEquivalentForecast.swift:9-18` | `windows` (`primary`, `secondary`, `tertiary`, `extra`), `RateWindow.pace` (`Pace`), `RateWindow.sessionEquivalent` (`SessionEquivalentForecast`) |
| Credits, credit limit & reset credits | `Sources/CodexBarCore/CreditsModels.swift:45-209`, `Sources/CodexBar/MenuCardView+Costs.swift:150-191`, `Sources/CodexBar/MenuCardView+CodexResetCredits.swift`, `Sources/CodexBarCore/Providers/Claude/ClaudeRateLimitResetCredits.swift:9-54` | `credits` (`Metric<Credits>` with `creditLimit: CreditLimit` and Codex `resetCredits: ResetCredits`); Claude `cedar_ember` reset credits in `details` (`DetailSection` with `DetailRow` label `"Limit Reset Credits"`) |
| Spend cap, cloud balance & detail sections | `Sources/CodexBarCore/ProviderCostSnapshot.swift`, `Sources/CodexBar/MenuCardView+Costs.swift:5-63`, `Sources/CodexBarCore/ProviderDetailSection.swift` | `cost` (`Metric<Cost>`), `details` (`Vec<DetailSection>`) |
| Cost section & Usage Breakdown submenu | `Sources/CodexBarCore/CostUsageModels.swift:27-37, 164-196`, `Sources/CodexBarCore/CostProvenance.swift:4-11`, `Sources/CodexBar/MenuCardView+Costs.swift`, `Sources/CodexBar/CostHistoryChartMenuView.swift` | `costHistory` (`Metric<CostHistory>` with `daily: Vec<CostDailyEntry>`, `comparisonWindows: Vec<CostWindowSummary>`, `topModel`, `costProvenance`, `hintLines`) |
| Plan Usage submenu | `Sources/CodexBar/PlanUtilizationHistoryStore.swift`, `Sources/CodexBar/UsageStore+PlanUtilization.swift`, `Sources/CodexBar/PlanUtilizationHistoryChartMenuView.swift` | `utilization` (`Metric<Vec<PlanUtilizationSeries>>`) |
| Status banner & Status Page submenu | `Sources/CodexBarCore/ProviderStatus.swift:3-64`, `Sources/CodexBar/StatusComponentsMenuView.swift`, `Sources/CodexBar/StatusItemController+Menu.swift` | `status` (`Metric<ProviderStatus>` with `indicator`, `description`, `updatedAt`, `pageUrl`, `components: Vec<StatusComponent>`) |
| Settings window tabs & descriptors | `Sources/CodexBar/PreferencesView.swift`, `Sources/CodexBar/PreferencesGeneralPane.swift`, `Sources/CodexBar/PreferencesProvidersPane.swift`, `Sources/CodexBar/PreferencesAdvancedPane.swift` | `SettingsPage`, `SettingsSection` (`subtitle`, `pane`), `SettingDescriptor` (`readOnly`, `visibleWhen`, `enabledWhen`, `DescriptorKind`) |

### 6.3 `settingsChanged`
Pushed when configuration or secrets change, prompting clients to re-query `getSettings`.
```json
{
  "type": "settingsChanged",
  "scope": "app"
}
```

### 6.4 `notice`
Pushed when transient status alerts or system notices occur.
```json
{
  "type": "notice",
  "level": "warning",
  "message": "Claude rate limit approaching threshold (85% used)"
}
```

---

## 7. Secret Handling Flow

### 7.1 Secret Isolation Principles
1. **Secrets Never Enter GUI/QML**: Secret descriptors (`DescriptorKind::Secret`) provide only metadata: `isSet` (boolean) and optional `source` (e.g. `"Keyring"`, `"Environment"`). No `value` field exists in the descriptor schema.
2. **Terminal Prompt Entry**: When a user clicks to configure a secret in the desktop panel, the UI launches a detached terminal running:
   ```bash
   ariadusage secret set --id <setting-id>
   ```
3. **No-Echo Input**: On interactive TTYs, `ariadusage secret set` prompts with echo disabled using `rpassword`. When piped, it reads exactly one line from standard input. Secrets are never accepted via command-line arguments (`argv`), preventing leaks to `/proc/PID/cmdline`.
4. **Zeroization and Redaction**: Within engine memory, secret values are stored inside `SecretString` types that display `[redacted]` in `Debug` output and overwrite memory buffers with zeros when dropped.

---

## 8. Development and Fixture Server

An in-process Unix domain socket fixture server is provided for UI prototyping and protocol compliance testing without live AI provider accounts.

### 8.1 Running the Fixture Server Example
```bash
cargo run -p ariadusage-protocol --example fixture_server -- --socket "$XDG_RUNTIME_DIR/ariadusage-dev/engine.sock" --step-seconds 5
```

### 8.2 Command-Line Options
- `--socket <path>`: Path to Unix domain socket to bind.
- `--step-seconds <n>`: Interval in seconds between scenario state advancements (default: 5).
- `--misbehave <mode>`: Misbehavior stress testing modes:
  - `oversize-frame`: Emits an unterminated 50 MiB data stream to test client buffer limits.
