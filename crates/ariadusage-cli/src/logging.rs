// Ported from CodexBar Sources/CodexBarCLI/CLIOptions.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCLI/CLIEntry.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Logging/CodexBarLog.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Logging/JSONStderrLogHandler.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Logging/LogRedactor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Logging/LogCategories.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Logging/ProviderLogging.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Shared CLI logging configuration, redaction, and `stderr` emission (`CLILoggingOptions`,
//! `CodexBarLog`, `JSONStderrLogHandler`, `LogRedactor`, `LogCategories`, `ProviderLogging`).
//!
//! Security invariants:
//! - Every log message and metadata value passes through [`LogRedactor::redact`] before being
//!   written to `stderr` or stored in the in-process ring buffer, ensuring secrets, API/OAuth/JWT
//!   tokens, cookies, auth headers, and email addresses are never logged.
//! - `ariadusage diagnose` routes CLI logging to [`CliLogDestination::Discard`] (`CLIEntry.swift:427`)
//!   so diagnostic exports on `stdout`/`stderr` are never polluted by log lines.
//! - `--json-output` (or `--json-only`) emits machine-readable JSON log lines on `stderr`
//!   matching upstream `JSONStderrLogHandler`.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::sync::{Mutex, OnceLock};

use clap::Args;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

/// Destination for CLI log output (`CodexBarLog.Destination` in `CodexBarLog.swift`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CliLogDestination {
    #[default]
    Stderr,
    Discard,
}

/// CLI log level (`CodexBarLog.Level` in `CodexBarLog.swift`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum CliLogLevel {
    Trace,
    Verbose,
    Debug,
    Info,
    Warning,
    #[default]
    Error,
    Critical,
}

impl CliLogLevel {
    pub const ALL: [Self; 7] = [
        Self::Trace,
        Self::Verbose,
        Self::Debug,
        Self::Info,
        Self::Warning,
        Self::Error,
        Self::Critical,
    ];

    /// Canonical lowercase CLI string (`rawValue` in `CodexBarLog.Level`).
    #[must_use]
    pub const fn raw_value(self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::Verbose => "verbose",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
            Self::Critical => "critical",
        }
    }

    /// Human-readable display label (`displayName` in `CodexBarLog.Level`).
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Trace => "Trace",
            Self::Verbose => "Verbose",
            Self::Debug => "Debug",
            Self::Info => "Info",
            Self::Warning => "Warning",
            Self::Error => "Error",
            Self::Critical => "Critical",
        }
    }

    /// Numeric severity rank (`rank` in `CodexBarLog.Level`).
    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::Trace => 0,
            Self::Verbose => 1,
            Self::Debug => 2,
            Self::Info => 3,
            Self::Warning => 4,
            Self::Error => 5,
            Self::Critical => 6,
        }
    }

    /// Maps to upstream `Logging.Logger.Level.rawValue` (`swiftLogLevel` in `CodexBarLog.swift`,
    /// where `.trace` and `.verbose` both map to `"trace"`).
    #[must_use]
    pub const fn as_swift_log_level(self) -> &'static str {
        match self {
            Self::Trace | Self::Verbose => "trace",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
            Self::Critical => "critical",
        }
    }

    /// Parses a raw log level string (case-insensitive).
    #[must_use]
    pub fn from_raw(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        match trimmed.to_ascii_lowercase().as_str() {
            "trace" => Some(Self::Trace),
            "verbose" => Some(Self::Verbose),
            "debug" => Some(Self::Debug),
            "info" => Some(Self::Info),
            "warning" => Some(Self::Warning),
            "error" => Some(Self::Error),
            "critical" => Some(Self::Critical),
            _ => None,
        }
    }

    /// Parses an optional raw `--log-level` value (`trace|verbose|debug|info|warning|error|critical`).
    #[must_use]
    pub fn parse_level(raw: Option<&str>) -> Option<Self> {
        raw.and_then(Self::from_raw)
    }

    /// Resolves the effective log level from `--verbose` and `--log-level`
    /// (`CodexBarCLI.resolvedLogLevel(verbose:rawLevel:)` in `CLIEntry.swift:418-423`).
    #[must_use]
    pub fn resolved(verbose: bool, raw_level: Option<&str>) -> Self {
        Self::parse_level(raw_level).unwrap_or(if verbose { Self::Debug } else { Self::Error })
    }
}

/// Resolves the effective CLI log level from `--verbose` and `--log-level`
/// (`CodexBarCLI.resolvedLogLevel(verbose:rawLevel:)` in `CLIEntry.swift:418-423`).
#[must_use]
pub fn resolved_log_level(verbose: bool, raw_level: Option<&str>) -> CliLogLevel {
    CliLogLevel::resolved(verbose, raw_level)
}

/// Shared CLI logging flags (`CLILoggingOptions` in `CLIOptions.swift`).
#[derive(Args, Debug, Clone, Default, PartialEq, Eq)]
pub struct CliLoggingOptions {
    /// Enable verbose logging
    #[arg(short = 'v', long = "verbose")]
    pub verbose: bool,

    /// Emit machine-readable logs
    #[arg(long = "json-output")]
    pub json_output: bool,

    /// Set log level (trace|verbose|debug|info|warning|error|critical)
    #[arg(long = "log-level")]
    pub log_level: Option<String>,
}

impl CliLoggingOptions {
    /// Builds a [`CliLoggingConfiguration`] targeting `stderr` (`CLICommonOptions` default).
    #[must_use]
    pub fn configuration(&self, json_only: bool) -> CliLoggingConfiguration {
        CliLoggingConfiguration::from_flags(
            self.verbose,
            self.json_output,
            json_only,
            self.log_level.as_deref(),
        )
    }

    /// Builds a [`CliLoggingConfiguration`] for a command path (`CodexBarCLI.loggingConfiguration`).
    #[must_use]
    pub fn for_command(&self, json_only: bool, path: &[&str]) -> CliLoggingConfiguration {
        CliLoggingConfiguration::for_command(
            self.verbose,
            self.json_output,
            json_only,
            self.log_level.as_deref(),
            path,
        )
    }
}

/// Resolved CLI logging configuration (`CodexBarLog.Configuration` in `CodexBarLog.swift` and
/// `CodexBarCLI.loggingConfiguration` in `CLIEntry.swift:425-433`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CliLoggingConfiguration {
    pub destination: CliLogDestination,
    pub level: CliLogLevel,
    pub json: bool,
}

impl Default for CliLoggingConfiguration {
    fn default() -> Self {
        Self {
            destination: CliLogDestination::Stderr,
            level: CliLogLevel::Error,
            json: false,
        }
    }
}

impl CliLoggingConfiguration {
    #[must_use]
    pub const fn new(destination: CliLogDestination, level: CliLogLevel, json: bool) -> Self {
        Self {
            destination,
            level,
            json,
        }
    }

    /// Builds the CLI logging configuration from CLI flags (`CodexBarCLI.loggingConfiguration`).
    #[must_use]
    pub fn from_flags(
        verbose: bool,
        json_output: bool,
        json_only: bool,
        log_level: Option<&str>,
    ) -> Self {
        Self {
            destination: CliLogDestination::Stderr,
            level: CliLogLevel::resolved(verbose, log_level),
            json: json_output || json_only,
        }
    }

    /// Builds the CLI logging configuration for a specific command path
    /// (`CodexBarCLI.loggingConfiguration` in `CLIEntry.swift:425-433`), routing `"diagnose"`
    /// to [`CliLogDestination::Discard`] and all other commands to [`CliLogDestination::Stderr`].
    #[must_use]
    pub fn for_command(
        verbose: bool,
        json_output: bool,
        json_only: bool,
        log_level: Option<&str>,
        path: &[&str],
    ) -> Self {
        let destination = if path
            .first()
            .is_some_and(|first| first.eq_ignore_ascii_case("diagnose"))
        {
            CliLogDestination::Discard
        } else {
            CliLogDestination::Stderr
        };
        Self {
            destination,
            level: CliLogLevel::resolved(verbose, log_level),
            json: json_output || json_only,
        }
    }
}

/// Log category identifiers (`LogCategories` in `LogCategories.swift`).
pub struct LogCategories;

impl LogCategories {
    pub const SUBSYSTEM: &'static str = "com.steipete.codexbar";
    pub const TERMINAL_PTY: &'static str = "terminal-pty";
    pub const CLAUDE_USAGE: &'static str = "claude-usage";
    pub const CODEX_USAGE: &'static str = "codex-usage";
    pub const CODEX_RPC: &'static str = "codex-rpc";
    pub const GEMINI_FETCH: &'static str = "gemini-fetch";
    pub const TOKEN_COST: &'static str = "token-cost";
    pub const STARTUP: &'static str = "startup";
    pub const PARSE: &'static str = "parse";
    pub const BROWSER_COOKIE_IMPORT: &'static str = "browser-cookie-import";
    pub const ANTIGRAVITY: &'static str = "antigravity";
    pub const CONFIG_STORE: &'static str = "config-store";
    pub const SETTINGS: &'static str = "settings";
    pub const WIDGET: &'static str = "widget";
    pub const ICON_RENDERER: &'static str = "icon-renderer";
    pub const STATUS_ITEM: &'static str = "status-item";
    pub const COST_USAGE: &'static str = "cost-usage";
    pub const KEYCHAIN: &'static str = "keychain";
    pub const KEYCHAIN_PROMPT: &'static str = "keychain-prompt";
    pub const KEYCHAIN_PREFLIGHT: &'static str = "keychain-preflight";
    pub const KEYCHAIN_MIGRATION: &'static str = "keychain-migration";
    pub const LOGIN_RUNNER: &'static str = "login-runner";
    pub const OPENAI_WEB: &'static str = "openai-web";
    pub const CREDITS_PURCHASE: &'static str = "credits-purchase";
    pub const CLI_HELP: &'static str = "cli-help";
    pub const HOOKS: &'static str = "hooks";
    pub const CACHE: &'static str = "cache";
    pub const COOKIE_REFRESH: &'static str = "cookie-refresh";
    pub const DIAGNOSE: &'static str = "diagnose";
    pub const USAGE: &'static str = "usage";

    /// Returns the provider log category (`"provider.<id>"`).
    #[must_use]
    pub fn provider(id: &str) -> String {
        format!("provider.{id}")
    }
}

/// Redacts sensitive tokens, cookies, auth headers, and emails from log text
/// (`LogRedactor` in `LogRedactor.swift` and `ProviderLogging` in `ProviderLogging.swift`).
pub struct LogRedactor;

impl LogRedactor {
    /// Redacts emails, MiniMax tokens (`sk-cp-*`, `sk-api-*`), API/OAuth/JWT tokens,
    /// `Bearer` tokens, `Cookie` headers, `Authorization` headers, and known session cookies.
    #[must_use]
    pub fn redact(text: &str) -> String {
        if text.is_empty() {
            return String::new();
        }
        let mut out = redact_emails(text);
        out = redact_minimax_tokens(&out);
        out = redact_bearer_tokens(&out);
        out = redact_line_header(&out, "cookie");
        out = redact_line_header(&out, "authorization");
        out = redact_cookie_kv_pairs(&out);
        out = redact_other_api_tokens(&out);
        out
    }
}

/// Provider-scoped redaction helper (`ProviderLogging` in `ProviderLogging.swift`).
pub struct ProviderLogging;

impl ProviderLogging {
    #[must_use]
    pub fn redact(text: &str) -> String {
        LogRedactor::redact(text)
    }
}

fn is_email_local_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'%' | b'+' | b'-')
}

fn is_email_domain_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-')
}

/// Redacts `[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}` as `<redacted-email>`.
fn redact_emails(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut last = 0usize;
    let mut i = 0usize;

    while i < bytes.len() {
        if bytes[i] == b'@' && i > last {
            let mut local_start = i;
            while local_start > last && is_email_local_char(bytes[local_start - 1]) {
                local_start -= 1;
            }
            let mut domain_end = i + 1;
            while domain_end < bytes.len() && is_email_domain_char(bytes[domain_end]) {
                domain_end += 1;
            }
            // Trim trailing '.' or '-' that are sentence punctuation rather than domain TLD
            while domain_end > i + 1 && !bytes[domain_end - 1].is_ascii_alphabetic() {
                domain_end -= 1;
            }
            if local_start < i && domain_end > i + 1 {
                let domain = &input[i + 1..domain_end];
                if let Some((host, tld)) = domain.rsplit_once('.')
                    && !host.is_empty()
                    && tld.len() >= 2
                    && tld.bytes().all(|b| b.is_ascii_alphabetic())
                {
                    out.push_str(&input[last..local_start]);
                    out.push_str("<redacted-email>");
                    last = domain_end;
                    i = domain_end;
                    continue;
                }
            }
        }
        i += 1;
    }
    out.push_str(&input[last..]);
    out
}

fn is_word_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn is_token_body_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

fn is_minimax_boundary_after(b: Option<u8>) -> bool {
    match b {
        None => true,
        Some(ch) => {
            ch.is_ascii_whitespace()
                || matches!(ch, b'.' | b';' | b',' | b')' | b']' | b'"' | b'\'')
        }
    }
}

/// Redacts `\bsk-(?:cp|api)-[A-Za-z0-9_-]{6,}(?=[.;,)\]\s]|$)` as `<redacted-minimax-token>`.
fn redact_minimax_tokens(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;

    while i < bytes.len() {
        let at_word_boundary = i == 0 || !is_word_char(bytes[i - 1]);
        if at_word_boundary {
            let rem = &input[i..];
            let prefix_len = if rem.starts_with("sk-cp-") {
                Some(6usize)
            } else if rem.starts_with("sk-api-") {
                Some(7usize)
            } else {
                None
            };
            if let Some(plen) = prefix_len {
                let mut end = i + plen;
                while end < bytes.len() && is_token_body_char(bytes[end]) {
                    end += 1;
                }
                let body_len = end - (i + plen);
                if body_len >= 6 && is_minimax_boundary_after(bytes.get(end).copied()) {
                    out.push_str("<redacted-minimax-token>");
                    i = end;
                    continue;
                }
            }
        }
        let ch = input[i..].chars().next().expect("non-empty slice");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn is_bearer_token_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'+' | b'/' | b'=')
}

/// Redacts `(?i)(Bearer\s+)[A-Za-z0-9._\-+/=]+` as `$1<redacted>`.
fn redact_bearer_tokens(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;

    while i < bytes.len() {
        let at_boundary = i == 0 || !is_word_char(bytes[i - 1]);
        if at_boundary && i + 6 <= bytes.len() && input[i..i + 6].eq_ignore_ascii_case("bearer") {
            let mut ws_end = i + 6;
            while ws_end < bytes.len()
                && bytes[ws_end].is_ascii_whitespace()
                && bytes[ws_end] != b'\n'
                && bytes[ws_end] != b'\r'
            {
                ws_end += 1;
            }
            if ws_end > i + 6 {
                let mut tok_end = ws_end;
                while tok_end < bytes.len() && is_bearer_token_char(bytes[tok_end]) {
                    tok_end += 1;
                }
                if tok_end > ws_end {
                    out.push_str(&input[i..ws_end]);
                    out.push_str("<redacted>");
                    i = tok_end;
                    continue;
                }
            }
        }
        let ch = input[i..].chars().next().expect("non-empty slice");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Redacts `(?i)(<header>\s*:\s*)([^\r\n]+)` as `$1<redacted>`.
fn redact_line_header(input: &str, header_lower: &str) -> String {
    let bytes = input.as_bytes();
    let hlen = header_lower.len();
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;

    while i < bytes.len() {
        let at_boundary = i == 0 || !is_word_char(bytes[i - 1]);
        if at_boundary
            && i + hlen <= bytes.len()
            && input[i..i + hlen].eq_ignore_ascii_case(header_lower)
        {
            let mut p = i + hlen;
            while p < bytes.len() && matches!(bytes[p], b' ' | b'\t') {
                p += 1;
            }
            if p < bytes.len() && bytes[p] == b':' {
                p += 1;
                while p < bytes.len() && matches!(bytes[p], b' ' | b'\t') {
                    p += 1;
                }
                let mut line_end = p;
                while line_end < bytes.len() && !matches!(bytes[line_end], b'\r' | b'\n') {
                    line_end += 1;
                }
                if line_end > p {
                    out.push_str(&input[i..p]);
                    out.push_str("<redacted>");
                    i = line_end;
                    continue;
                }
            }
        }
        let ch = input[i..].chars().next().expect("non-empty slice");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Redacts standalone cookie key-value pairs (`sessionKey=...`, `session_id=...`,
/// `__Secure-next-auth.session-token=...`, `_cfuvid=...`, `cf_clearance=...`, `HERTZ-SESSION=...`).
fn redact_cookie_kv_pairs(input: &str) -> String {
    const KEYS: &[&str] = &[
        "sessionKey=",
        "session_id=",
        "__Secure-next-auth.session-token=",
        "_cfuvid=",
        "cf_clearance=",
        "HERTZ-SESSION=",
    ];
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;

    while i < bytes.len() {
        let rem = &input[i..];
        if let Some(key) = KEYS.iter().copied().find(|k| rem.starts_with(k)) {
            let val_start = i + key.len();
            let mut val_end = val_start;
            while val_end < bytes.len()
                && !bytes[val_end].is_ascii_whitespace()
                && !matches!(bytes[val_end], b';' | b',' | b'"' | b'\'' | b')' | b']')
            {
                val_end += 1;
            }
            if val_end > val_start {
                out.push_str(key);
                out.push_str("<redacted>");
                i = val_end;
                continue;
            }
        }
        let ch = input[i..].chars().next().expect("non-empty slice");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Redacts other API/OAuth/JWT tokens (`sk-ant-*`, `sk-*`, `ya29.*`, `1//*`, `ghp_*`,
/// `github_pat_*`, `eyJ*`) so secrets never leak to `stderr`.
fn redact_other_api_tokens(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;

    while i < bytes.len() {
        let at_boundary = i == 0 || !is_word_char(bytes[i - 1]);
        if at_boundary {
            let rem = &input[i..];
            // JWT: eyJ<base64url>.<base64url>.<base64url>
            if rem.starts_with("eyJ") {
                let mut end = i + 3;
                let mut dots = 0usize;
                while end < bytes.len() && (is_token_body_char(bytes[end]) || bytes[end] == b'.') {
                    if bytes[end] == b'.' {
                        dots += 1;
                    }
                    end += 1;
                }
                while end > i + 3 && bytes[end - 1] == b'.' {
                    end -= 1;
                    dots = dots.saturating_sub(1);
                }
                if dots >= 1 && end - i >= 12 {
                    out.push_str("<redacted-token>");
                    i = end;
                    continue;
                }
            }

            let token_match: Option<(usize, bool)> = if rem.starts_with("sk-ant-") {
                Some((7, false))
            } else if rem.starts_with("ya29.") {
                Some((5, true))
            } else if rem.starts_with("1//") {
                Some((3, true))
            } else if rem.starts_with("ghp_") {
                Some((4, false))
            } else if rem.starts_with("github_pat_") {
                Some((11, false))
            } else if rem.starts_with("sk-")
                && !rem.starts_with("sk-cp-")
                && !rem.starts_with("sk-api-")
            {
                Some((3, false))
            } else {
                None
            };

            if let Some((plen, allow_dot)) = token_match {
                let mut end = i + plen;
                while end < bytes.len()
                    && (is_token_body_char(bytes[end]) || (allow_dot && bytes[end] == b'.'))
                {
                    end += 1;
                }
                if allow_dot {
                    while end > i + plen && bytes[end - 1] == b'.' {
                        end -= 1;
                    }
                }
                if end - (i + plen) >= 6 {
                    out.push_str("<redacted-token>");
                    i = end;
                    continue;
                }
            }
        }
        let ch = input[i..].chars().next().expect("non-empty slice");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Machine-readable JSON log payload written to `stderr` when `--json-output` or `--json-only`
/// is enabled (`JSONStderrLogHandler.LogLine` in `JSONStderrLogHandler.swift`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonLogLine {
    pub timestamp: String,
    pub level: String,
    pub label: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<BTreeMap<String, String>>,
    pub source: String,
    pub file: String,
    pub function: String,
    pub line: u32,
}

#[derive(Debug, Default)]
struct LoggingState {
    config: Option<CliLoggingConfiguration>,
    startup_logged: bool,
    captured: Vec<JsonLogLine>,
}

static LOGGING_STATE: OnceLock<Mutex<LoggingState>> = OnceLock::new();

fn state_cell() -> &'static Mutex<LoggingState> {
    LOGGING_STATE.get_or_init(|| Mutex::new(LoggingState::default()))
}

/// Bootstraps or updates the process-wide CLI logging configuration (`CodexBarLog.bootstrapIfNeeded`).
pub fn bootstrap_cli_logging(config: CliLoggingConfiguration) {
    if let Ok(mut guard) = state_cell().lock() {
        guard.config = Some(config);
    }
}

/// Returns the most recently bootstrapped CLI logging configuration.
#[must_use]
pub fn current_cli_logging_configuration() -> Option<CliLoggingConfiguration> {
    state_cell().lock().ok().and_then(|guard| guard.config)
}

/// Dynamically updates the active CLI log level (`CodexBarLog.setLogLevel`).
pub fn set_cli_log_level(level: CliLogLevel) {
    if let Ok(mut guard) = state_cell().lock() {
        let mut cfg = guard.config.unwrap_or_default();
        cfg.level = level;
        guard.config = Some(cfg);
    }
}

/// Returns the currently active CLI log level (`CodexBarLog.currentLevel`).
#[must_use]
pub fn current_cli_log_level() -> CliLogLevel {
    current_cli_logging_configuration()
        .map(|cfg| cfg.level)
        .unwrap_or(CliLogLevel::Error)
}

/// Returns `true` when a message at `level` will be written to `stderr` under the active
/// [`CliLoggingConfiguration`].
#[must_use]
pub fn will_log_to_stderr(level: CliLogLevel) -> bool {
    let cfg = current_cli_logging_configuration().unwrap_or_default();
    cfg.destination == CliLogDestination::Stderr && level.rank() >= cfg.level.rank()
}

/// Clears the in-process captured log buffer (used by tests to inspect emitted log records).
pub fn clear_captured_log_lines() {
    if let Ok(mut guard) = state_cell().lock() {
        guard.captured.clear();
    }
}

/// Returns a snapshot of log records emitted since the process started or since the last
/// [`clear_captured_log_lines`] call.
#[must_use]
pub fn captured_log_lines() -> Vec<JsonLogLine> {
    state_cell()
        .lock()
        .map(|guard| guard.captured.clone())
        .unwrap_or_default()
}

/// Returns a category-scoped [`CliLogger`] (`CodexBarLog.logger(_:)` in `CodexBarLog.swift`).
#[must_use]
pub fn logger(category: &str) -> CliLogger {
    CliLogger::new(category)
}

/// Logs the process startup state once (`CodexBarLog.logStartupState`).
pub fn log_startup_state(providers: &[String]) {
    let should_log = state_cell()
        .lock()
        .map(|mut guard| {
            if guard.startup_logged {
                false
            } else {
                guard.startup_logged = true;
                true
            }
        })
        .unwrap_or(false);
    if !should_log {
        return;
    }
    let version = env!("CARGO_PKG_VERSION");
    let joined = providers.join(",");
    logger(LogCategories::STARTUP).info(
        &format!("AriadUsage starting version={version} providers={joined}"),
        None,
    );
}

/// Category-scoped CLI logger (`CodexBarLogger` in `CodexBarLog.swift`).
#[derive(Debug, Clone)]
pub struct CliLogger {
    category: String,
}

impl CliLogger {
    #[must_use]
    pub fn new(category: impl Into<String>) -> Self {
        Self {
            category: category.into(),
        }
    }

    #[must_use]
    pub fn category(&self) -> &str {
        &self.category
    }

    #[track_caller]
    pub fn trace(&self, message: &str, metadata: Option<BTreeMap<&str, String>>) {
        self.log(CliLogLevel::Trace, message, metadata);
    }

    #[track_caller]
    pub fn verbose(&self, message: &str, metadata: Option<BTreeMap<&str, String>>) {
        self.log(CliLogLevel::Verbose, message, metadata);
    }

    #[track_caller]
    pub fn debug(&self, message: &str, metadata: Option<BTreeMap<&str, String>>) {
        self.log(CliLogLevel::Debug, message, metadata);
    }

    #[track_caller]
    pub fn info(&self, message: &str, metadata: Option<BTreeMap<&str, String>>) {
        self.log(CliLogLevel::Info, message, metadata);
    }

    #[track_caller]
    pub fn warning(&self, message: &str, metadata: Option<BTreeMap<&str, String>>) {
        self.log(CliLogLevel::Warning, message, metadata);
    }

    #[track_caller]
    pub fn error(&self, message: &str, metadata: Option<BTreeMap<&str, String>>) {
        self.log(CliLogLevel::Error, message, metadata);
    }

    #[track_caller]
    pub fn critical(&self, message: &str, metadata: Option<BTreeMap<&str, String>>) {
        self.log(CliLogLevel::Critical, message, metadata);
    }

    #[track_caller]
    pub fn log(&self, level: CliLogLevel, message: &str, metadata: Option<BTreeMap<&str, String>>) {
        let cfg = current_cli_logging_configuration().unwrap_or_default();
        if level.rank() < cfg.level.rank() {
            return;
        }

        let caller = std::panic::Location::caller();
        let redacted_message = LogRedactor::redact(message);
        let mut redacted_meta = BTreeMap::new();
        redacted_meta.insert("category".to_owned(), LogRedactor::redact(&self.category));
        if let Some(meta) = metadata {
            for (k, v) in meta {
                redacted_meta.insert(k.to_owned(), LogRedactor::redact(&v));
            }
        }

        let record = JsonLogLine {
            timestamp: Timestamp::now().to_string(),
            level: level.as_swift_log_level().to_owned(),
            label: LogCategories::SUBSYSTEM.to_owned(),
            message: redacted_message.clone(),
            metadata: Some(redacted_meta.clone()),
            source: "CodexBar".to_owned(),
            file: caller.file().to_owned(),
            function: self.category.clone(),
            line: caller.line(),
        };

        if let Ok(mut guard) = state_cell().lock() {
            if guard.captured.len() >= 256 {
                guard.captured.remove(0);
            }
            guard.captured.push(record.clone());
        }

        if cfg.destination == CliLogDestination::Discard {
            return;
        }

        let mut stderr = io::stderr().lock();
        if cfg.json {
            if let Ok(mut encoded) = serde_json::to_vec(&record) {
                encoded.push(b'\n');
                let _ = stderr.write_all(&encoded);
            }
        } else {
            let extra_meta: Vec<String> = redacted_meta
                .iter()
                .filter(|(k, _)| k.as_str() != "category")
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            if extra_meta.is_empty() {
                let _ = writeln!(
                    stderr,
                    "[{}] [{}] {}",
                    level.raw_value(),
                    self.category,
                    redacted_message
                );
            } else {
                let _ = writeln!(
                    stderr,
                    "[{}] [{}] {} {}",
                    level.raw_value(),
                    self.category,
                    extra_meta.join(" "),
                    redacted_message
                );
            }
        }
    }
}
