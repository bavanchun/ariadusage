// Ported from CodexBar Sources/CodexBarCLI/CLIHelp.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

const V1_PROVIDER_HELP_NAMES: &[&str] = &["codex", "claude", "antigravity", "both", "all"];

#[must_use]
pub fn provider_help_list() -> String {
    V1_PROVIDER_HELP_NAMES.join("|")
}

#[must_use]
pub fn usage_help(version: &str) -> String {
    let providers = provider_help_list();
    format!(
        "AriadUsage {version}

Usage:
  ariadusage usage [--format text|json|toon]
                 [--json]
                 [--json-only]
                 [--json-output] [--log-level <trace|verbose|debug|info|warning|error|critical>] [-v|--verbose]
                 [--provider {providers}]
                 [--account <label>] [--account-index <index>] [--all-accounts]
                 [--no-credits] [--no-color] [--pretty] [--status] [--source <auto|web|cli|oauth|api>]
                 [--web-timeout <seconds>] [--antigravity-plan-debug]

Description:
  Print usage from enabled providers as text (default), JSON, or TOON.
  Output format: use --json (or --format json) for JSON on stdout; use --json-output for JSON logs on stderr.
  --format toon emits the same payload as --format json, rendered as TOON (github.com/toon-format/spec)
  for token-cheaper agent consumption.

Global flags:
  -h, --help      Show help
  -V, --version   Show version
  -v, --verbose   Enable verbose logging
  --no-color      Disable ANSI colors in text output
  --log-level <trace|verbose|debug|info|warning|error|critical>
  --json-output   Emit machine-readable logs (JSONL) to stderr
  --web-debug-dump-html  Unsupported (web strategies use JSON APIs; rejected with exit 64)

Examples:
  ariadusage usage
  ariadusage usage --provider claude
  ariadusage usage --provider antigravity
  ariadusage usage --format json --provider all --pretty
  ariadusage usage --provider all --json
  ariadusage usage --status
  ariadusage usage --provider codex --source web --format json --pretty
  ariadusage usage --format toon --provider claude
"
    )
}

#[must_use]
pub fn cards_help(version: &str) -> String {
    let providers = provider_help_list();
    format!(
        "AriadUsage {version}

Usage:
  ariadusage cards [--json-output] [--log-level <trace|verbose|debug|info|warning|error|critical>] [-v|--verbose]
                 [--provider {providers}]
                 [--account <label>] [--account-index <index>] [--all-accounts]
                 [--no-credits] [--no-color] [--status] [--source <auto|web|cli|oauth|api>]
                 [--web-timeout <seconds>] [--antigravity-plan-debug]
                 [--brief]

Description:
  Print a one-shot usage snapshot as a responsive card grid in the terminal.
  Honors enabled providers from config and reuses the same fetch flags as ariadusage usage.
  Failed providers are summarized in a footer instead of error cards.
  Use --brief for a compact table layout (Provider / Usage / Reset).
  Stdout is always the rendered card/table text; --json-output only affects stderr logs.

Global flags:
  -h, --help      Show help
  -V, --version   Show version
  -v, --verbose   Enable verbose logging
  --no-color      Disable ANSI colors in text output
  --log-level <trace|verbose|debug|info|warning|error|critical>
  --json-output   Emit machine-readable logs (JSONL) to stderr
  --web-debug-dump-html  Unsupported (web strategies use JSON APIs; rejected with exit 64)

Examples:
  ariadusage cards
  ariadusage cards --provider codex
  ariadusage cards --provider all --status
  ariadusage cards --brief
  ariadusage cards --no-color
"
    )
}

#[must_use]
pub fn status_help(version: &str) -> String {
    let providers = provider_help_list();
    format!(
        "AriadUsage {version}

Usage:
  ariadusage status [--provider {providers}]
                    [--format text|json]
                    [--json]
                    [--json-only]
                    [--pretty]
                    [--no-color]

Description:
  Print provider status indicators and status page URLs from the running AriadUsage daemon.

Examples:
  ariadusage status
  ariadusage status --provider codex
  ariadusage status --format json --pretty
"
    )
}

#[must_use]
pub fn root_help(version: &str) -> String {
    let providers = provider_help_list();
    format!(
        "AriadUsage {version}

Usage:
  ariadusage [--format text|json|toon]
           [--json]
           [--json-only]
           [--json-output] [--log-level <trace|verbose|debug|info|warning|error|critical>] [-v|--verbose]
           [--provider {providers}]
           [--account <label>] [--account-index <index>] [--all-accounts]
           [--no-credits] [--no-color] [--pretty] [--status] [--source <auto|web|cli|oauth|api>]
           [--web-timeout <seconds>] [--antigravity-plan-debug]
  ariadusage cards [--provider {providers}] [--brief] [--no-color] [--status]
  ariadusage status [--provider {providers}] [--format text|json] [--json] [--json-only] [--pretty] [--no-color]
  ariadusage daemon
  ariadusage secret set <provider> <field>

Global flags:
  -h, --help      Show help
  -V, --version   Show version
  -v, --verbose   Enable verbose logging
  --no-color      Disable ANSI colors in text output
  --log-level <trace|verbose|debug|info|warning|error|critical>
  --json-output   Emit machine-readable logs (JSONL) to stderr
  --web-debug-dump-html  Unsupported (web strategies use JSON APIs; rejected with exit 64)

Examples:
  ariadusage
  ariadusage --format json --provider all --pretty
  ariadusage usage --format json --provider all --pretty
  ariadusage --format toon --provider claude
  ariadusage --provider all --json
  ariadusage --provider antigravity
  ariadusage cards --provider all --status
  ariadusage cards --brief
"
    )
}
