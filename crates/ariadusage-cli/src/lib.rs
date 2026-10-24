// Ported from CodexBar Sources/CodexBarCLI/CLIDiagnoseCommand.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCLI/CLICacheCommand.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCLI/CLICookieCommand.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCLI/CLISessionsCommand.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCLI/CLIHelp.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

pub mod launch_agent;

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ariadusage_core::config::{
    Config, CookieSource, ProviderConfig, ProviderEntry, TokenAccountMeta,
};
use ariadusage_core::pipeline::{
    ClassifiedError, DiagnosticAuthSummary, DiagnosticBatchExport, DiagnosticExport, FetchError,
    FetchInteraction, FetchRuntime, redact_diagnostic_text,
};
use ariadusage_core::providers::{ProviderDescriptor, SourceMode, first_party_order};
use ariadusage_engine::brokers::browser::{Browser, BrowserGate, DEFAULT_IMPORT_ORDER};
use ariadusage_engine::brokers::cookie_cache::CookieCache;
use ariadusage_engine::config_store::ConfigStore;
use ariadusage_engine::daemon::build_default_runtime_with_options;
use ariadusage_engine::paths;
use ariadusage_engine::runtime::EngineRuntime;
use ariadusage_engine::sessions::{
    AgentSession, AgentSessionProvider, LocalAgentSessionScanner, SessionFocusResult,
    SessionWindowFocuser,
};
use ariadusage_protocol::ids::ProviderId;
use ariadusage_protocol::usage::ProviderErrorKind;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

/// Output format selected via `--format`, `--json`, or `--json-only`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Text,
    Json,
}

impl OutputFormat {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "text" => Some(Self::Text),
            "json" => Some(Self::Json),
            _ => None,
        }
    }
}

/// Resolves the active configuration file path, honoring `ARIADUSAGE_CONFIG_PATH` and
/// `CODEXBAR_CONFIG` environment overrides for isolated CLI tests.
pub fn resolve_cli_config_path() -> Result<PathBuf, paths::PathError> {
    for key in ["ARIADUSAGE_CONFIG_PATH", "CODEXBAR_CONFIG"] {
        if let Some(val) = std::env::var_os(key) {
            let trimmed = val.to_string_lossy().trim().to_owned();
            if !trimmed.is_empty() {
                return Ok(PathBuf::from(trimmed));
            }
        }
    }
    paths::config_path()
}

/// Builds an in-process [`EngineRuntime`] configured for CLI commands when no daemon socket is
/// active or when a command executes directly in the CLI process.
pub fn build_cli_runtime() -> Result<Arc<EngineRuntime>, String> {
    let cfg_path = resolve_cli_config_path().map_err(|e| e.to_string())?;
    let disable_keyring = std::env::var_os("ARIADUSAGE_DISABLE_KEYRING").as_deref()
        == Some(std::ffi::OsStr::new("1"));
    build_default_runtime_with_options(ConfigStore::new(cfg_path), disable_keyring)
        .map_err(|e| e.to_string())
}

/// Sends an IPC v1 `runAction` request to a running daemon socket if reachable.
#[cfg(unix)]
pub fn try_daemon_run_action(
    socket_override: Option<&Path>,
    action_id: &str,
    confirm: Option<bool>,
) -> Result<Option<serde_json::Value>, String> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    use ariadusage_protocol::ids::ActionId;
    use ariadusage_protocol::ipc::{
        ClientInfo, ClientMessage, PROTOCOL_V1, ServerMessage, parse_server_message,
    };

    let sock = match socket_override {
        Some(p) => {
            paths::validate_socket_path_len(p).map_err(|_| "socket path exceeds sun_path limit")?;
            p.to_path_buf()
        }
        None => paths::socket_path().map_err(|e| e.to_string())?,
    };

    let mut stream = UnixStream::connect(&sock).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;

    let hello = ClientMessage::Hello {
        protocols: vec![PROTOCOL_V1.to_string()],
        client: ClientInfo::new("ariadusage-cli", env!("CARGO_PKG_VERSION")),
        id: None,
    };
    let run_action = ClientMessage::RunAction {
        id: ActionId::new(action_id).map_err(|e| e.to_string())?,
        confirm,
    };

    for msg in [&hello, &run_action] {
        let mut line = serde_json::to_vec(msg).map_err(|e| e.to_string())?;
        line.push(b'\n');
        stream.write_all(&line).map_err(|e| e.to_string())?;
    }
    stream.flush().map_err(|e| e.to_string())?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    while reader.read_line(&mut line).map_err(|e| e.to_string())? > 0 {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if let Ok(msg) = parse_server_message(trimmed) {
            match msg {
                ServerMessage::Response {
                    ok: Some(true),
                    payload,
                    ..
                } => {
                    return Ok(payload);
                }
                ServerMessage::Response {
                    ok: Some(false),
                    error: Some(error),
                    ..
                } => {
                    return Err(format!("daemon error: {:?}", error.code));
                }
                _ => {}
            }
        }
        line.clear();
    }
    Err("daemon closed connection before replying".to_owned())
}

// ============================================================================
// 1. Cache Clear (`CLICacheCommand.swift`)
// ============================================================================

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CacheClearOptions {
    pub cookies: bool,
    pub cost: bool,
    pub all: bool,
    pub provider: Option<String>,
    pub format: Option<OutputFormat>,
    pub json_shortcut: bool,
    pub json_only: bool,
    pub pretty: bool,
    pub verbose: bool,
    pub json_output: bool,
    pub log_level: Option<String>,
}

impl CacheClearOptions {
    pub fn parse_from_slice(args: &[&str]) -> Result<Self, String> {
        let mut opts = Self::default();
        let mut idx = 0;
        while idx < args.len() {
            match args[idx] {
                "--cookies" => opts.cookies = true,
                "--cost" => opts.cost = true,
                "--all" => opts.all = true,
                "--json" => opts.json_shortcut = true,
                "--json-only" => opts.json_only = true,
                "--pretty" => opts.pretty = true,
                "-v" | "--verbose" => opts.verbose = true,
                "--json-output" => opts.json_output = true,
                "--provider" => {
                    idx += 1;
                    let val = args
                        .get(idx)
                        .ok_or_else(|| "Missing value for --provider".to_owned())?;
                    opts.provider = Some((*val).to_owned());
                }
                "--format" => {
                    idx += 1;
                    let val = args
                        .get(idx)
                        .ok_or_else(|| "Missing value for --format".to_owned())?;
                    let fmt =
                        OutputFormat::parse(val).ok_or_else(|| format!("Invalid format: {val}"))?;
                    opts.format = Some(fmt);
                }
                "--log-level" => {
                    idx += 1;
                    let val = args
                        .get(idx)
                        .ok_or_else(|| "Missing value for --log-level".to_owned())?;
                    opts.log_level = Some((*val).to_owned());
                }
                other => return Err(format!("Unknown argument: {other}")),
            }
            idx += 1;
        }
        Ok(opts)
    }

    pub fn effective_format(&self) -> OutputFormat {
        if let Some(fmt) = self.format {
            return fmt;
        }
        if self.json_shortcut || self.json_only {
            OutputFormat::Json
        } else {
            OutputFormat::Text
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheClearResult {
    pub cache: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    pub cleared: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Ported from `CLIHelp.cacheHelp(version:)` (`CLIHelp.swift:345-371`).
pub fn cache_help(version: &str) -> String {
    format!(
        "CodexBar {version}\n\n\
         Usage:\n  \
         codexbar cache clear <--cookies|--cost|--all>\n                      \
         [--provider <name>]\n                      \
         [--format text|json]\n                      \
         [--json]\n                      \
         [--json-only]\n                      \
         [--json-output] [--log-level <trace|verbose|debug|info|warning|error|critical>]\n                      \
         [-v|--verbose]\n                      \
         [--pretty]\n\n\
         Description:\n  \
         Clear cached data. Use --cookies to clear browser cookie caches (stored in Keychain),\n  \
         --cost to clear cost usage scan caches, or --all for both.\n  \
         Optionally specify --provider with --cookies to clear cookies for a single provider only.\n\n\
         Examples:\n  \
         codexbar cache clear --cookies\n  \
         codexbar cache clear --cookies --provider claude\n  \
         codexbar cache clear --cost\n  \
         codexbar cache clear --all\n  \
         codexbar cache clear --all --format json --pretty"
    )
}

/// Ported from `CodexBarCLI.cacheClearProviderScopeError(rawProvider:clearCost:)` (`CLICacheCommand.swift:91-94`).
pub fn cache_clear_provider_scope_error(
    raw_provider: Option<&str>,
    clear_cost: bool,
) -> Option<String> {
    if raw_provider.is_some() && clear_cost {
        Some(
            "--provider only scopes cookie caches. Use --cookies --provider <name>, or omit --provider."
                .to_owned(),
        )
    } else {
        None
    }
}

pub fn execute_cache_clear(
    opts: &CacheClearOptions,
    socket_override: Option<&Path>,
) -> Result<(Vec<CacheClearResult>, String), (u8, String)> {
    let clear_cookies = opts.cookies || opts.all;
    let clear_cost = opts.cost || opts.all;

    if !clear_cookies && !clear_cost {
        return Err((2, "Specify --cookies, --cost, or --all.".to_owned()));
    }
    if let Some(err) = cache_clear_provider_scope_error(opts.provider.as_deref(), clear_cost) {
        return Err((2, err));
    }

    let mut results = Vec::new();
    let runtime = build_cli_runtime().ok();

    if clear_cookies {
        if let Some(raw_provider) = opts.provider.as_deref() {
            let normalized = raw_provider.trim().to_ascii_lowercase();
            let Some(descriptor) = first_party_order()
                .iter()
                .find(|d| d.cli_name == normalized || d.id.as_str() == normalized)
            else {
                return Err((2, format!("Unknown provider: {raw_provider}")));
            };
            #[cfg(unix)]
            let daemon_cleared = {
                let action = format!("clearCookieCache.{}", descriptor.id.as_str());
                try_daemon_run_action(socket_override, &action, None)
                    .ok()
                    .flatten()
                    .and_then(|v| v.get("cleared").and_then(|n| n.as_u64()))
                    .map(|n| n as usize)
            };
            #[cfg(not(unix))]
            let daemon_cleared: Option<usize> = {
                let _ = socket_override;
                None
            };

            let cleared = match daemon_cleared {
                Some(n) => n,
                None => runtime
                    .as_ref()
                    .map(|rt| {
                        rt.cookie_cache()
                            .clear_all_scopes_detailed(&descriptor.id)
                            .cleared_count
                    })
                    .unwrap_or(0),
            };
            results.push(CacheClearResult {
                cache: "cookies".to_owned(),
                provider: Some(descriptor.id.as_str().to_owned()),
                cleared,
                error: None,
            });
        } else {
            #[cfg(unix)]
            let daemon_cleared = try_daemon_run_action(socket_override, "clearCookieCache", None)
                .ok()
                .flatten()
                .and_then(|v| v.get("cleared").and_then(|n| n.as_u64()))
                .map(|n| n as usize);
            #[cfg(not(unix))]
            let daemon_cleared: Option<usize> = None;

            let cleared = match daemon_cleared {
                Some(n) => n,
                None => runtime
                    .as_ref()
                    .map(|rt| rt.cookie_cache().clear_all_detailed().cleared_count)
                    .unwrap_or(0),
            };
            results.push(CacheClearResult {
                cache: "cookies".to_owned(),
                provider: None,
                cleared,
                error: None,
            });
        }
    }

    if clear_cost {
        #[cfg(unix)]
        let daemon_cleared = try_daemon_run_action(socket_override, "clearCostCache", None)
            .ok()
            .flatten()
            .map(|v| {
                let n = v.get("cleared").and_then(|n| n.as_u64()).unwrap_or(0) as usize;
                let err = v
                    .get("error")
                    .and_then(|e| e.as_str())
                    .map(ToOwned::to_owned);
                (usize::from(n > 0), err)
            });
        #[cfg(not(unix))]
        let daemon_cleared: Option<(usize, Option<String>)> = None;

        let (cleared, cost_error) = match daemon_cleared {
            Some(pair) => pair,
            None => match runtime.as_ref() {
                Some(rt) => {
                    let (removed, err) = rt.clear_cost_cache();
                    (usize::from(removed > 0), err)
                }
                None => (0, None),
            },
        };
        results.push(CacheClearResult {
            cache: "cost".to_owned(),
            provider: None,
            cleared,
            error: cost_error,
        });
    }

    let rendered = match opts.effective_format() {
        OutputFormat::Text => results
            .iter()
            .map(|result| {
                let scope = result.provider.as_deref().unwrap_or("all providers");
                if let Some(error) = &result.error {
                    format!("{}: failed to clear ({scope}) - {error}", result.cache)
                } else if result.cleared > 0 {
                    format!("{}: cleared ({scope})", result.cache)
                } else {
                    format!("{}: nothing to clear ({scope})", result.cache)
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        OutputFormat::Json => {
            if opts.pretty {
                serde_json::to_string_pretty(&results).unwrap_or_else(|_| "[]".to_owned())
            } else {
                serde_json::to_string(&results).unwrap_or_else(|_| "[]".to_owned())
            }
        }
    };

    if results.iter().any(|r| r.error.is_some()) {
        Err((1, rendered))
    } else {
        Ok((results, rendered))
    }
}

// ============================================================================
// 2. Diagnose (`CLIDiagnoseCommand.swift`)
// ============================================================================

/// Ported from `CLIHelp.diagnoseHelp(version:)` (`CLIHelp.swift:409-431`).
pub fn diagnose_help(version: &str) -> String {
    format!(
        "CodexBar {version}\n\n\
         Usage:\n  \
         codexbar diagnose --provider <name|all> --format json\n                   \
         [--json-output] [--log-level <trace|verbose|debug|info|warning|error|critical>]\n                   \
         [-v|--verbose]\n                   \
         [--redact] [--output <path>]\n                   \
         [--pretty]\n\n\
         Description:\n  \
         Run provider diagnostic fetches and print a safe JSON export for issue reporting.\n  \
         The export is redacted and omits raw API tokens, cookies, auth headers, emails,\n  \
         account IDs, org IDs, raw responses, and billing-history records.\n\n\
         Examples:\n  \
         codexbar diagnose --provider minimax --format json --redact --output diagnostic.json\n  \
         codexbar diagnose --provider minimax --format json --pretty\n  \
         codexbar diagnose --provider claude --format json --pretty\n  \
         codexbar diagnose --provider all --format json"
    )
}

/// Ported from `CodexBarCLI.writeDiagnosticExport(_:to:)` (`CLIDiagnoseCommand.swift:94-103`).
pub fn write_diagnostic_export(json_string: &str, path: &str) -> io::Result<()> {
    let target = Path::new(path);
    if let Some(parent) = target.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(target, json_string)
}

/// Ported from `CodexBarCLI.diagnosticAuthSummary` & `ProviderCredentialAdapter.diagnosticAuthSummary`
/// (`CLIDiagnoseCommand.swift:163-176`, `ProviderCredentialAdapter.swift:171-191`).
pub fn diagnostic_auth_summary(
    provider: &str,
    account: Option<&TokenAccountMeta>,
    config_has_api_key: bool,
    config_has_secret_key: bool,
    config_has_cookie_header: bool,
    environment: &BTreeMap<String, String>,
    manual_cookie_header: Option<&str>,
) -> DiagnosticAuthSummary {
    let mut modes = Vec::new();
    if account.is_some() {
        modes.push("tokenAccount".to_owned());
    }
    if config_has_api_key || config_has_secret_key {
        modes.push("api".to_owned());
    }
    let detected_env_modes: Vec<&'static str> = match provider {
        "claude" => {
            let has_admin = environment
                .get("ANTHROPIC_ADMIN_API_KEY")
                .or_else(|| environment.get("ANTHROPIC_ADMIN_KEY"))
                .is_some_and(|v| !v.trim().is_empty());
            if has_admin { vec!["api"] } else { Vec::new() }
        }
        "codex" | "antigravity" => Vec::new(),
        "minimax" => {
            let has_api_token = environment
                .get("MINIMAX_API_TOKEN")
                .or_else(|| environment.get("MINIMAX_API_KEY"))
                .is_some_and(|v| !v.trim().is_empty());
            if has_api_token {
                vec!["apiToken"]
            } else if manual_cookie_header.is_some_and(|h| !h.trim().is_empty()) {
                vec!["cookie"]
            } else {
                Vec::new()
            }
        }
        "bedrock" => {
            let has_access = environment
                .get("AWS_ACCESS_KEY_ID")
                .is_some_and(|v| !v.trim().is_empty());
            let has_secret = environment
                .get("AWS_SECRET_ACCESS_KEY")
                .is_some_and(|v| !v.trim().is_empty());
            if has_access && has_secret {
                vec!["api"]
            } else {
                Vec::new()
            }
        }
        _ => {
            // Generic API key adapter fallback for test coverage
            let has_env_key = environment.iter().any(|(k, v)| {
                (k.ends_with("_API_KEY") || k.ends_with("_TOKEN")) && !v.trim().is_empty()
            });
            if has_env_key { vec!["api"] } else { Vec::new() }
        }
    };

    if provider == "bedrock" && modes.contains(&"api".to_owned()) && detected_env_modes.is_empty() {
        modes.retain(|m| m != "api");
    }

    for mode in detected_env_modes {
        if !modes.iter().any(|m| m == mode) {
            modes.push(mode.to_owned());
        }
    }
    if config_has_cookie_header && !modes.iter().any(|m| m == "web") {
        modes.push("web".to_owned());
    }
    DiagnosticAuthSummary::new(!modes.is_empty(), modes)
}

#[derive(Debug, Clone, Default)]
pub struct DiagnoseOptions {
    pub provider: Option<String>,
    pub format: Option<String>,
    pub redact: bool,
    pub output: Option<String>,
    pub pretty: bool,
    pub verbose: bool,
    pub json_output: bool,
    pub log_level: Option<String>,
    pub socket: Option<PathBuf>,
}

pub fn execute_diagnose(opts: &DiagnoseOptions) -> Result<String, (u8, String)> {
    let fmt = opts.format.as_deref().unwrap_or("json");
    if !fmt.eq_ignore_ascii_case("json") {
        return Err((
            2,
            "Error: only JSON format is supported for diagnose".to_owned(),
        ));
    }

    let runtime = build_cli_runtime().map_err(|e| (16, format!("Error: {e}")))?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| (16, format!("Error: {e}")))?;

    let mut exports: Vec<DiagnosticExport> = Vec::new();
    match opts.provider.as_deref() {
        Some(raw) if raw.eq_ignore_ascii_case("all") => {
            #[cfg(unix)]
            let from_daemon = try_daemon_run_action(opts.socket.as_deref(), "diagnose", None)
                .ok()
                .flatten()
                .and_then(|val| serde_json::from_value::<DiagnosticBatchExport>(val).ok());
            #[cfg(not(unix))]
            let from_daemon: Option<DiagnosticBatchExport> = None;

            let batch = match from_daemon {
                Some(b) => b,
                None => rt.block_on(runtime.diagnose_all_providers(true)),
            };
            let encoded = if opts.pretty {
                serde_json::to_string_pretty(&batch)
            } else {
                serde_json::to_string(&batch)
            }
            .map_err(|e| (16, format!("Error encoding diagnostic: {e}")))?;
            let redacted = redact_diagnostic_text(&encoded);
            if let Some(path) = opts.output.as_deref().filter(|p| !p.is_empty()) {
                write_diagnostic_export(&redacted, path)
                    .map_err(|e| (16, format!("Error encoding diagnostic: {e}")))?;
            }
            return Ok(redacted);
        }
        Some(raw) => {
            let normalized = raw.trim().to_ascii_lowercase();
            let Ok(pid) = ProviderId::new(&normalized) else {
                return Err((2, format!("Error: unknown provider '{raw}'")));
            };
            if !first_party_order().iter().any(|d| d.id == pid) {
                return Err((2, format!("Error: unknown provider '{raw}'")));
            }
            #[cfg(unix)]
            let from_daemon = {
                let action = format!("diagnose.{}", pid.as_str());
                try_daemon_run_action(opts.socket.as_deref(), &action, None)
                    .ok()
                    .flatten()
                    .and_then(|val| serde_json::from_value::<DiagnosticExport>(val).ok())
            };
            #[cfg(not(unix))]
            let from_daemon: Option<DiagnosticExport> = None;

            let export = match from_daemon {
                Some(exp) => exp,
                None => rt
                    .block_on(runtime.diagnose_provider(&pid, true))
                    .ok_or_else(|| (2, format!("Error: unknown provider '{raw}'")))?,
            };
            exports.push(export);
        }
        None => {
            let cfg_path = resolve_cli_config_path().map_err(|e| (16, format!("Error: {e}")))?;
            let cfg = ConfigStore::new(cfg_path)
                .load_effective()
                .unwrap_or_else(|_| Config::new(Config::CURRENT_VERSION, Vec::new()));
            let mut selected_ids: Vec<ProviderId> = Vec::new();
            for entry in &cfg.providers {
                if let ProviderEntry::Typed(p) = entry
                    && p.enabled.unwrap_or(false)
                    && first_party_order().iter().any(|d| d.id == p.id)
                {
                    selected_ids.push(p.id.clone());
                }
            }
            if selected_ids.is_empty() {
                selected_ids.push(ProviderId::new("codex").expect("valid id"));
            }
            for pid in selected_ids {
                if let Some(exp) = rt.block_on(runtime.diagnose_provider(&pid, true)) {
                    exports.push(exp);
                }
            }
        }
    }

    let encoded = if exports.len() == 1 {
        let single = &exports[0];
        if opts.pretty {
            serde_json::to_string_pretty(single)
        } else {
            serde_json::to_string(single)
        }
    } else {
        let batch = DiagnosticBatchExport::new(Timestamp::now(), exports);
        if opts.pretty {
            serde_json::to_string_pretty(&batch)
        } else {
            serde_json::to_string(&batch)
        }
    }
    .map_err(|e| (16, format!("Error encoding diagnostic: {e}")))?;

    let redacted = redact_diagnostic_text(&encoded);
    if let Some(path) = opts.output.as_deref().filter(|p| !p.is_empty()) {
        write_diagnostic_export(&redacted, path)
            .map_err(|e| (16, format!("Error encoding diagnostic: {e}")))?;
    }
    Ok(redacted)
}

/// Runs `ariadusage usage --provider <name> [--verbose]`, logging safe strategy attempt categories
/// on terminal failure without exposing raw error descriptions (`CLIEntryTests.swift:865`).
pub fn execute_usage(
    provider: Option<&str>,
    verbose: bool,
    pretty: bool,
) -> Result<String, (u8, String)> {
    let raw = provider.unwrap_or("codex");
    let normalized = raw.trim().to_ascii_lowercase();
    let Ok(pid) = ProviderId::new(&normalized) else {
        return Err((2, format!("Error: unknown provider '{raw}'")));
    };
    if !first_party_order().iter().any(|d| d.id == pid) {
        return Err((2, format!("Error: unknown provider '{raw}'")));
    }

    let runtime = build_cli_runtime().map_err(|e| (16, format!("Error: {e}")))?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| (16, format!("Error: {e}")))?;

    let Some((_spec, _inputs, outcome)) = rt.block_on(runtime.fetch_provider_for_cli(
        &pid,
        FetchRuntime::Cli,
        FetchInteraction::UserInitiated,
    )) else {
        return Err((2, format!("Error: unknown provider '{raw}'")));
    };

    match &outcome.result {
        Ok(fetched) => {
            let rendered = if pretty {
                serde_json::to_string_pretty(&fetched.usage)
            } else {
                serde_json::to_string(&fetched.usage)
            }
            .unwrap_or_else(|_| "{}".to_owned());
            Ok(rendered)
        }
        Err(_err) => {
            if verbose {
                let summary = outcome
                    .attempts
                    .iter()
                    .map(|attempt| {
                        let diag = ariadusage_core::pipeline::DiagnosticFetchAttempt::from_attempt(
                            attempt,
                        );
                        let strategy = diag.strategy_id.as_deref().unwrap_or("unknown");
                        match diag.error_category.as_deref() {
                            Some(cat) => {
                                format!("{strategy} ({}): {}: {cat}", diag.kind, diag.outcome)
                            }
                            None => format!("{strategy} ({}): {}", diag.kind, diag.outcome),
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err((1, format!("Provider fetch failed: {summary}")));
            }
            Err((1, "Provider fetch failed".to_owned()))
        }
    }
}

// ============================================================================
// 3. Cookie Refresh (`CLICookieCommand.swift`)
// ============================================================================

/// Ported from `CLIHelp.cookieHelp(version:)` (`CLIHelp.swift:433-457`).
pub fn cookie_help(version: &str) -> String {
    format!(
        "CodexBar {version}\n\n\
         Usage:\n  \
         codexbar cookie refresh <--provider <name>|--all>\n                         \
         [--allow-keychain-prompt]\n                         \
         [--format text|json]\n                         \
         [--json]\n                         \
         [--json-only]\n                         \
         [--pretty]\n\n\
         Description:\n  \
         Re-import browser cookies using each provider's configured browser order.\n  \
         Providers that may decrypt Chromium cookies fail before clearing the cache\n  \
         unless --allow-keychain-prompt explicitly acknowledges a possible macOS\n  \
         Keychain prompt. A prior denial keeps its six-hour cooldown unless that\n  \
         explicit interactive retry flag is supplied. Cookie values are never shown.\n\n\
         Examples:\n  \
         codexbar cookie refresh --provider opencodego --allow-keychain-prompt\n  \
         codexbar cookie refresh --all --allow-keychain-prompt\n  \
         codexbar cookie refresh --provider opencodego --allow-keychain-prompt --format json --pretty"
    )
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CookieRefreshOptions {
    pub verbose: bool,
    pub json_output: bool,
    pub json_shortcut: bool,
    pub json_only: bool,
    pub pretty: bool,
    pub format: Option<OutputFormat>,
    pub all: bool,
    pub provider: Option<String>,
    pub allow_keychain_prompt: bool,
}

impl CookieRefreshOptions {
    pub fn parse_from_slice(args: &[&str]) -> Result<Self, String> {
        let mut opts = Self::default();
        let mut idx = 0;
        while idx < args.len() {
            match args[idx] {
                "-v" | "--verbose" => opts.verbose = true,
                "--json-output" => opts.json_output = true,
                "--json" => opts.json_shortcut = true,
                "--json-only" => opts.json_only = true,
                "--pretty" => opts.pretty = true,
                "--all" => opts.all = true,
                "--allow-keychain-prompt" => opts.allow_keychain_prompt = true,
                "--provider" => {
                    idx += 1;
                    let val = args
                        .get(idx)
                        .ok_or_else(|| "Missing value for --provider".to_owned())?;
                    opts.provider = Some((*val).to_owned());
                }
                "--format" => {
                    idx += 1;
                    let val = args
                        .get(idx)
                        .ok_or_else(|| "Missing value for --format".to_owned())?;
                    let fmt =
                        OutputFormat::parse(val).ok_or_else(|| format!("Invalid format: {val}"))?;
                    opts.format = Some(fmt);
                }
                other => return Err(format!("Unknown argument: {other}")),
            }
            idx += 1;
        }
        Ok(opts)
    }

    pub fn effective_format(&self) -> OutputFormat {
        if let Some(fmt) = self.format {
            return fmt;
        }
        if self.json_shortcut || self.json_only {
            OutputFormat::Json
        } else {
            OutputFormat::Text
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CookieRefreshStatus {
    Refreshed,
    Skipped,
    Blocked,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CookieRefreshResult {
    pub provider: String,
    pub status: CookieRefreshStatus,
    pub message: String,
}

impl CookieRefreshResult {
    pub fn new(
        provider: impl Into<String>,
        status: CookieRefreshStatus,
        message: impl Into<String>,
    ) -> Self {
        Self {
            provider: provider.into(),
            status,
            message: message.into(),
        }
    }

    pub fn is_failure(&self) -> bool {
        matches!(
            self.status,
            CookieRefreshStatus::Blocked | CookieRefreshStatus::Failed
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CookieRefreshCommandError {
    NoSupportedProviders,
    UnknownProvider(String),
    UnsupportedProvider(String),
}

impl std::fmt::Display for CookieRefreshCommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSupportedProviders => {
                f.write_str("No providers support browser cookie refresh on this platform.")
            }
            Self::UnknownProvider(raw) => write!(f, "Unknown provider: {raw}"),
            Self::UnsupportedProvider(raw) => {
                write!(f, "{raw} does not support browser cookie refresh.")
            }
        }
    }
}

impl std::error::Error for CookieRefreshCommandError {}

pub const KEYCHAIN_PROMPT_ACKNOWLEDGEMENT_HINT: &str = "Browser cookie decryption may open a macOS Keychain prompt. Retry interactively with --allow-keychain-prompt to acknowledge it.";
pub const BROWSER_SESSION_FAILURE_HINT: &str =
    "No browser session cookie was refreshed. Sign in in a configured browser and retry.";
pub const BROWSER_COOKIE_ACCESS_FAILURE_HINT: &str = "No browser session cookie was refreshed. Sign in in a configured browser and retry. If Keychain access was declined, CodexBar keeps the six-hour denial cooldown; use --allow-keychain-prompt only for an explicit interactive retry.";

/// Ported from `CodexBarCLI.cookieRefreshTargets` (`CLICookieCommand.swift:82-104`).
pub fn cookie_refresh_targets(
    raw_provider: Option<&str>,
    refresh_all: bool,
    descriptors: &[ProviderDescriptor],
) -> Result<Vec<ProviderDescriptor>, CookieRefreshCommandError> {
    let supported: Vec<ProviderDescriptor> = descriptors
        .iter()
        .filter(|d| d.supports_browser_cookies && d.source_modes.contains(&SourceMode::Web))
        .cloned()
        .collect();
    if refresh_all {
        if supported.is_empty() {
            return Err(CookieRefreshCommandError::NoSupportedProviders);
        }
        return Ok(supported);
    }
    let Some(raw) = raw_provider else {
        return Err(CookieRefreshCommandError::UnknownProvider(String::new()));
    };
    let lower = raw.to_ascii_lowercase();
    let Some(known) = descriptors
        .iter()
        .find(|d| d.cli_name == lower || d.id.as_str() == lower)
    else {
        return Err(CookieRefreshCommandError::UnknownProvider(raw.to_owned()));
    };
    let Some(target) = supported.into_iter().find(|d| d.id == known.id) else {
        return Err(CookieRefreshCommandError::UnsupportedProvider(
            raw.to_owned(),
        ));
    };
    Ok(vec![target])
}

/// Ported from `CodexBarCLI.cookieRefreshBrowserOrder` & `ProviderBrowserProfile.selected`
/// (`CLICookieCommand.swift:145-155`, `ProviderBrowserProfile.swift:15-19`).
pub fn cookie_refresh_browser_order(
    descriptor: &ProviderDescriptor,
    config: Option<&Config>,
) -> Vec<Browser> {
    if !descriptor.supports_browser_cookies {
        return Vec::new();
    }
    let browsers: Vec<Browser> = match descriptor.selected_profile_browsers {
        Some(selected) => selected
            .iter()
            .filter_map(|id| Browser::from_raw_id(id))
            .collect(),
        None => DEFAULT_IMPORT_ORDER.to_vec(),
    };
    let Some(selected_browsers) = descriptor.selected_profile_browsers else {
        return browsers;
    };
    let provider_cfg = config.and_then(|c| c.provider_config(descriptor.id.as_str()));
    if provider_cfg.is_some_and(|p| p.has_browser_id_extension() && p.browser_id().is_none()) {
        return Vec::new();
    }
    let Some(browser_id) = provider_cfg
        .and_then(ProviderConfig::browser_id)
        .or_else(|| selected_browsers.first().map(|s| (*s).to_owned()))
    else {
        return Vec::new();
    };
    if !selected_browsers.contains(&browser_id.as_str()) {
        return Vec::new();
    }
    browsers
        .into_iter()
        .filter(|b| b.raw_id() == browser_id)
        .collect()
}

/// Returns true when any browser in `browsers` requires Keychain prompt acknowledgement
/// (i.e. is a Chromium-family browser).
pub fn requires_keychain_prompt_acknowledgement(browsers: &[Browser]) -> bool {
    browsers.iter().any(|b| b.is_chromium())
}

/// Ported from `CodexBarCLI.cookieRefreshSkipResult` (`CLICookieCommand.swift:279-297`).
pub fn cookie_refresh_skip_result(
    descriptor: &ProviderDescriptor,
    config: &Config,
) -> Option<CookieRefreshResult> {
    let cookie_source = config
        .provider_config(descriptor.id.as_str())
        .and_then(|p| p.cookie_source)
        .unwrap_or(CookieSource::Auto);
    match cookie_source {
        CookieSource::Manual => Some(CookieRefreshResult::new(
            descriptor.cli_name,
            CookieRefreshStatus::Skipped,
            "Browser refresh skipped because this provider uses a manual cookie.",
        )),
        CookieSource::Off => Some(CookieRefreshResult::new(
            descriptor.cli_name,
            CookieRefreshStatus::Skipped,
            "Browser refresh skipped because browser cookies are disabled for this provider.",
        )),
        CookieSource::Auto => None,
    }
}

/// Ported from `CodexBarCLI.performCookieRefreshes` (`CLICookieCommand.swift:106-143`).
pub async fn perform_cookie_refreshes<P, O, Fut>(
    targets: &[ProviderDescriptor],
    allow_keychain_prompt: bool,
    config: Option<&Config>,
    mut preflight: P,
    mut operation: O,
) -> Vec<CookieRefreshResult>
where
    P: FnMut(&ProviderDescriptor) -> Option<CookieRefreshResult>,
    O: FnMut(ProviderDescriptor, FetchInteraction, bool) -> Fut,
    Fut: Future<Output = CookieRefreshResult>,
{
    let mut results = Vec::with_capacity(targets.len());
    for descriptor in targets {
        if let Some(skipped) = preflight(descriptor) {
            results.push(skipped);
            continue;
        }
        let browsers = cookie_refresh_browser_order(descriptor, config);
        let needs_acknowledgement = requires_keychain_prompt_acknowledgement(&browsers);
        if needs_acknowledgement && !allow_keychain_prompt {
            results.push(CookieRefreshResult::new(
                descriptor.cli_name,
                CookieRefreshStatus::Blocked,
                KEYCHAIN_PROMPT_ACKNOWLEDGEMENT_HINT,
            ));
            continue;
        }
        let result = operation(
            descriptor.clone(),
            FetchInteraction::UserInitiated,
            allow_keychain_prompt,
        )
        .await;
        results.push(result);
    }
    results
}

/// Ported from `CodexBarCLI.withCookieRefreshCacheSuppressed` (`CLICookieCommand.swift:252-277`).
pub async fn with_cookie_refresh_cache_suppressed<F, Fut>(
    cache: &CookieCache,
    provider: &ProviderId,
    provider_name: &str,
    operation: F,
) -> CookieRefreshResult
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = CookieRefreshResult>,
{
    let Some(gate) = cache.begin_refresh_read_suppression(provider) else {
        return CookieRefreshResult::new(
            provider_name,
            CookieRefreshStatus::Failed,
            "Cookie cache could not be read safely; no browser import was attempted.",
        );
    };
    let result = operation().await;
    if result.is_failure() {
        cache.end_refresh_read_suppression(&gate);
        return result;
    }
    let commit = cache.commit_refresh_read_suppression(&gate);
    cache.end_refresh_read_suppression(&gate);
    if commit.committed_count == 0
        || commit.committed_count != commit.staged_count
        || commit.failed_count != 0
    {
        return CookieRefreshResult::new(
            provider_name,
            CookieRefreshStatus::Failed,
            "Browser cookie validation succeeded, but the refreshed session could not be saved.",
        );
    }
    result
}

/// Ported from `CodexBarCLI.cookieRefreshFailure` (`CLICookieCommand.swift:157-190`).
pub fn cookie_refresh_failure(
    descriptor: &ProviderDescriptor,
    classified_error: Option<&ClassifiedError>,
    config: Option<&Config>,
    keychain_disabled: bool,
    browser_gate: Option<(&BrowserGate, i64)>,
) -> CookieRefreshResult {
    let prompt_capable_browsers: Vec<Browser> = cookie_refresh_browser_order(descriptor, config)
        .into_iter()
        .filter(|b| requires_keychain_prompt_acknowledgement(&[*b]))
        .collect();

    let message = if let Some(browser) = prompt_capable_browsers.first()
        && keychain_disabled
    {
        format!(
            "{} cookie decryption is disabled in CodexBar; enable Keychain access and refresh.",
            browser.display_name()
        )
    } else if let Some((gate, now_seconds)) = browser_gate
        && let Some(browser) = prompt_capable_browsers
            .iter()
            .find(|b| gate.has_active_denial(**b, now_seconds))
    {
        format!(
            "{} cookie decryption was declined in Keychain; rerun with --allow-keychain-prompt to request Keychain access again.",
            browser.display_name()
        )
    } else {
        match classified_error.map(|e| e.kind) {
            Some(ProviderErrorKind::AuthenticationExpired) => {
                "The provider rejected the browser session. Sign in again and retry.".to_owned()
            }
            Some(ProviderErrorKind::PermissionDenied) => {
                "Permission was denied while refreshing the browser session. Check access and retry."
                    .to_owned()
            }
            Some(ProviderErrorKind::RateLimited) => {
                "The provider rate limited the refresh. Wait and retry.".to_owned()
            }
            Some(ProviderErrorKind::ProviderUnavailable) => {
                "The provider is temporarily unavailable. Retry later.".to_owned()
            }
            Some(ProviderErrorKind::ParseFailure) => {
                "The provider response could not be read. Check for a CodexBar update.".to_owned()
            }
            Some(ProviderErrorKind::NetworkFailure) => {
                "The provider could not be reached. Check your connection and retry.".to_owned()
            }
            Some(ProviderErrorKind::ApiFailure) => {
                "The provider could not validate the browser session. Retry later.".to_owned()
            }
            Some(ProviderErrorKind::MissingCredential)
            | Some(ProviderErrorKind::Unknown)
            | None => {
                if prompt_capable_browsers.is_empty() {
                    BROWSER_SESSION_FAILURE_HINT.to_owned()
                } else {
                    BROWSER_COOKIE_ACCESS_FAILURE_HINT.to_owned()
                }
            }
        }
    };
    CookieRefreshResult::new(descriptor.cli_name, CookieRefreshStatus::Failed, message)
}

/// Ported from `CodexBarCLI.cookieRefreshText` (`CLICookieCommand.swift:192-202`).
pub fn cookie_refresh_text(results: &[CookieRefreshResult]) -> String {
    results
        .iter()
        .map(|result| {
            let marker = match result.status {
                CookieRefreshStatus::Refreshed => "✅",
                CookieRefreshStatus::Skipped => "↷",
                CookieRefreshStatus::Blocked => "⚠️",
                CookieRefreshStatus::Failed => "❌",
            };
            format!("{}: {marker} {}", result.provider, result.message)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn execute_cookie_refresh(opts: &CookieRefreshOptions) -> Result<String, (u8, String)> {
    if opts.provider.is_some() == opts.all {
        return Err((
            2,
            "Specify exactly one of --provider <name> or --all.".to_owned(),
        ));
    }
    let targets = cookie_refresh_targets(opts.provider.as_deref(), opts.all, first_party_order())
        .map_err(|e| (2, e.to_string()))?;

    let cfg_path = resolve_cli_config_path().map_err(|e| (16, e.to_string()))?;
    let cfg = ConfigStore::new(cfg_path)
        .load_effective()
        .unwrap_or_else(|_| Config::new(Config::CURRENT_VERSION, Vec::new()));
    let runtime = build_cli_runtime().map_err(|e| (16, e))?;
    let keychain_disabled = std::env::var_os("ARIADUSAGE_DISABLE_KEYRING").as_deref()
        == Some(std::ffi::OsStr::new("1"));

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| (16, e.to_string()))?;

    let results = rt.block_on(perform_cookie_refreshes(
        &targets,
        opts.allow_keychain_prompt,
        Some(&cfg),
        |descriptor| cookie_refresh_skip_result(descriptor, &cfg),
        |descriptor, interaction, _acknowledged| {
            let runtime = Arc::clone(&runtime);
            let cfg_clone = cfg.clone();
            async move {
                with_cookie_refresh_cache_suppressed(
                    runtime.cookie_cache(),
                    &descriptor.id,
                    descriptor.cli_name,
                    || async {
                        let Some((_spec, _inputs, outcome)) = runtime
                            .fetch_provider_for_cli(&descriptor.id, FetchRuntime::Cli, interaction)
                            .await
                        else {
                            return cookie_refresh_failure(
                                &descriptor,
                                None,
                                Some(&cfg_clone),
                                keychain_disabled,
                                None,
                            );
                        };
                        match outcome.result {
                            Ok(_) => CookieRefreshResult::new(
                                descriptor.cli_name,
                                CookieRefreshStatus::Refreshed,
                                "Browser cookie refreshed.",
                            ),
                            Err(FetchError::Classified(ref c)) => cookie_refresh_failure(
                                &descriptor,
                                Some(c),
                                Some(&cfg_clone),
                                keychain_disabled,
                                None,
                            ),
                            Err(_) => cookie_refresh_failure(
                                &descriptor,
                                None,
                                Some(&cfg_clone),
                                keychain_disabled,
                                None,
                            ),
                        }
                    },
                )
                .await
            }
        },
    ));

    let rendered = match opts.effective_format() {
        OutputFormat::Text => {
            if opts.json_only {
                String::new()
            } else {
                cookie_refresh_text(&results)
            }
        }
        OutputFormat::Json => {
            if opts.pretty {
                serde_json::to_string_pretty(&results).unwrap_or_else(|_| "[]".to_owned())
            } else {
                serde_json::to_string(&results).unwrap_or_else(|_| "[]".to_owned())
            }
        }
    };

    if results.iter().any(CookieRefreshResult::is_failure) {
        Err((1, rendered))
    } else {
        Ok(rendered)
    }
}

// ============================================================================
// 4. Sessions (`CLISessionsCommand.swift`)
// ============================================================================

/// Ported from `CLIHelp.sessionsHelp(version:)` (`CLIHelp.swift:169-190`).
pub fn sessions_help(version: &str) -> String {
    format!(
        "CodexBar {version}\n\n\
         Usage:\n  \
         codexbar sessions [--json|--json-v2] [--pretty]\n  \
         codexbar sessions focus <id>\n\n\
         Description:\n  \
         List live local Codex, Claude Code, pi, and OMP agent sessions.\n  \
         --json emits the legacy v1 array with only Codex and Claude providers.\n  \
         --json-v2 emits the complete current array, including Pi-family sessions.\n  \
         JSON uses stable AgentSession field names and ISO-8601 dates.\n  \
         Focus activates the owning terminal or desktop app on macOS.\n\n\
         Examples:\n  \
         codexbar sessions\n  \
         codexbar sessions --json\n  \
         codexbar sessions --json-v2\n  \
         codexbar sessions focus 019f3497-73bf-7df3-a173-4f67d968914a"
    )
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionsOptions {
    pub json_shortcut: bool,
    pub json_v2: bool,
    pub pretty: bool,
}

impl SessionsOptions {
    pub fn parse_from_slice(args: &[&str]) -> Result<Self, String> {
        let mut opts = Self::default();
        for &arg in args {
            match arg {
                "--json" => opts.json_shortcut = true,
                "--json-v2" => opts.json_v2 = true,
                "--pretty" => opts.pretty = true,
                other => return Err(format!("Unknown argument: {other}")),
            }
        }
        Ok(opts)
    }
}

/// Ported from `CodexBarCLI.sessionsJSONProtocolVersion` (`CLISessionsCommand.swift:17-25`).
pub fn sessions_json_protocol_version(options: &SessionsOptions) -> Option<u8> {
    if options.json_v2 {
        Some(2)
    } else if options.json_shortcut {
        Some(1)
    } else {
        None
    }
}

/// Ported from `CodexBarCLI.sessionsForJSON` (`CLISessionsCommand.swift:27-31`).
pub fn sessions_for_json(sessions: &[AgentSession], include_pi_family: bool) -> Vec<AgentSession> {
    if include_pi_family {
        return sessions.to_vec();
    }
    sessions
        .iter()
        .filter(|s| {
            matches!(
                s.provider,
                AgentSessionProvider::Codex | AgentSessionProvider::Claude
            )
        })
        .cloned()
        .collect()
}

/// Ported from `CodexBarCLI.renderSessionsTable` (`CLISessionsCommand.swift:61-99`).
pub fn render_sessions_table(sessions: &[AgentSession], now: Timestamp) -> String {
    if sessions.is_empty() {
        return "No agent sessions found.".to_owned();
    }
    let rows: Vec<[String; 7]> = sessions
        .iter()
        .map(|session| {
            let state = match session.state {
                ariadusage_engine::sessions::AgentSessionState::Active => "active",
                ariadusage_engine::sessions::AgentSessionState::Idle => "idle",
            };
            let provider = session.provider.as_str();
            let dialect = session.dialect.map_or("—", |d| d.as_str());
            let source = session.source.as_str();
            let project = session.project_name.as_deref().unwrap_or("—");
            let age = session_age(session, now);
            [
                state.to_owned(),
                provider.to_owned(),
                dialect.to_owned(),
                source.to_owned(),
                project.to_owned(),
                age,
                session.id.clone(),
            ]
        })
        .collect();

    let headers = [
        "STATE", "PROVIDER", "DIALECT", "SOURCE", "PROJECT", "ACTIVITY", "ID",
    ];
    let mut widths = [0usize; 7];
    for (idx, header) in headers.iter().enumerate() {
        let max_row = rows
            .iter()
            .map(|r| r[idx].chars().count())
            .max()
            .unwrap_or(0);
        widths[idx] = header.chars().count().max(max_row);
    }

    let render_row = |cols: &[&str; 7]| -> String {
        cols.iter()
            .enumerate()
            .map(|(idx, col)| {
                let len = col.chars().count();
                let pad = widths[idx].saturating_sub(len);
                format!("{col}{}", " ".repeat(pad))
            })
            .collect::<Vec<_>>()
            .join("  ")
    };

    let mut lines = Vec::with_capacity(rows.len() + 1);
    lines.push(render_row(&headers));
    for row in &rows {
        let refs = [
            row[0].as_str(),
            row[1].as_str(),
            row[2].as_str(),
            row[3].as_str(),
            row[4].as_str(),
            row[5].as_str(),
            row[6].as_str(),
        ];
        lines.push(render_row(&refs));
    }
    lines.join("\n")
}

fn session_age(session: &AgentSession, now: Timestamp) -> String {
    let Some(date) = session.last_activity_at.or(session.started_at) else {
        return "now".to_owned();
    };
    let seconds = (now.as_second() - date.as_second()).max(0);
    if seconds < 60 {
        return format!("{seconds}s");
    }
    if seconds < 3600 {
        return format!("{}m", seconds / 60);
    }
    if seconds < 86_400 {
        return format!("{}h", seconds / 3600);
    }
    format!("{}d", seconds / 86_400)
}

pub fn execute_sessions_list(opts: &SessionsOptions) -> String {
    let env_map: HashMap<String, String> = std::env::vars().collect();
    let sessions = LocalAgentSessionScanner::default().scan_sync(Timestamp::now(), &env_map, true);
    if let Some(version) = sessions_json_protocol_version(opts) {
        let filtered = sessions_for_json(&sessions, version == 2);
        if opts.pretty {
            serde_json::to_string_pretty(&filtered).unwrap_or_else(|_| "[]".to_owned())
        } else {
            serde_json::to_string(&filtered).unwrap_or_else(|_| "[]".to_owned())
        }
    } else {
        render_sessions_table(&sessions, Timestamp::now())
    }
}

pub fn execute_sessions_focus(session_id: Option<&str>) -> Result<(), (u8, String)> {
    let Some(id) = session_id.map(str::trim).filter(|s| !s.is_empty()) else {
        return Err((1, "Missing session id.".to_owned()));
    };
    let env_map: HashMap<String, String> = std::env::vars().collect();
    let sessions = LocalAgentSessionScanner::default().scan_sync(Timestamp::now(), &env_map, true);
    let Some(session) = sessions.iter().find(|s| s.id == id) else {
        return Err((1, format!("Unknown session: {id}")));
    };
    match SessionWindowFocuser::focus(session) {
        SessionFocusResult::Focused { .. }
        | SessionFocusResult::ActivatedApplicationOnly { .. } => Ok(()),
        SessionFocusResult::Failed { .. } => Err((2, format!("Could not focus session: {id}"))),
        SessionFocusResult::UnsupportedPlatform => {
            Err((2, "Session focus is only available on macOS.".to_owned()))
        }
    }
}
